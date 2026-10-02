//! Meme 管理 API（全部挂 require_session 后）：
//! GET  /api/memes?status=pending|active（缺省全部）
//! POST /api/memes/{id}/approve {"category":"可选新类别"}
//! POST /api/memes/{id}/reject（删文件 + 删行）
//! GET  /api/meme-file/{id}（图片预览流）

use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
}

pub async fn list(State(state): State<AppState>, Query(q): Query<ListQuery>) -> Response {
    let conn = match state.open_db() {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("数据库打开失败: {e}")),
    };
    let (sql, params_vec): (String, Vec<String>) = match q.status.as_deref() {
        Some(s) if s == "pending" || s == "active" => (
            "SELECT id, file, category, status, use_count, added_by FROM meme_library WHERE status = ?1 ORDER BY id DESC"
                .to_string(),
            vec![s.to_string()],
        ),
        _ => (
            "SELECT id, file, category, status, use_count, added_by FROM meme_library ORDER BY id DESC".to_string(),
            vec![],
        ),
    };
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("查询失败: {e}")),
    };
    let rows = if params_vec.is_empty() {
        stmt.query_map([], row_to_json)
    } else {
        stmt.query_map(params![params_vec[0]], row_to_json)
    };
    match rows {
        Ok(it) => {
            let items: Vec<Value> = it.filter_map(|r| r.ok()).collect();
            (StatusCode::OK, Json(json!({ "memes": items }))).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("查询失败: {e}")),
    }
}

fn row_to_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, i64>(0)?,
        "file": r.get::<_, String>(1)?,
        "category": r.get::<_, String>(2)?,
        "status": r.get::<_, String>(3)?,
        "use_count": r.get::<_, i64>(4)?,
        "added_by": r.get::<_, String>(5)?,
    }))
}

#[derive(Deserialize)]
pub struct ApproveBody {
    category: Option<String>,
}

pub async fn approve(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<Json<ApproveBody>>,
) -> Response {
    let conn = match state.open_db() {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("数据库打开失败: {e}")),
    };
    let exists: Option<String> = conn
        .query_row("SELECT status FROM meme_library WHERE id = ?1", params![id], |r| r.get(0))
        .ok();
    let Some(status) = exists else {
        return err(StatusCode::NOT_FOUND, "meme 不存在");
    };
    if status != "pending" {
        return err(StatusCode::CONFLICT, "仅 pending 可审批");
    }
    let new_cat = body
        .and_then(|Json(b)| b.category)
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());
    let res = match &new_cat {
        Some(cat) => conn.execute(
            "UPDATE meme_library SET status = 'active', category = ?1 WHERE id = ?2",
            params![cat, id],
        ),
        None => conn.execute("UPDATE meme_library SET status = 'active' WHERE id = ?1", params![id]),
    };
    match res {
        Ok(_) => {
            tracing::info!(id, category = ?new_cat, "meme 审批通过");
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("更新失败: {e}")),
    }
}

pub async fn reject(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    let conn = match state.open_db() {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("数据库打开失败: {e}")),
    };
    let file: Option<String> = conn
        .query_row("SELECT file FROM meme_library WHERE id = ?1", params![id], |r| r.get(0))
        .ok();
    let Some(file) = file else {
        return err(StatusCode::NOT_FOUND, "meme 不存在");
    };
    // 删文件（失败仅告警，行照删）
    let path = state.memes_dir.join(&file);
    if let Err(e) = std::fs::remove_file(&path) {
        tracing::warn!(id, file = %path.display(), error = %e, "meme 文件删除失败（行照删）");
    }
    match conn.execute("DELETE FROM meme_library WHERE id = ?1", params![id]) {
        Ok(_) => {
            tracing::info!(id, "meme 已拒绝并删除");
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("删除失败: {e}")),
    }
}

pub async fn file(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    let conn = match state.open_db() {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("数据库打开失败: {e}")),
    };
    let file: Option<String> = conn
        .query_row("SELECT file FROM meme_library WHERE id = ?1", params![id], |r| r.get(0))
        .ok();
    let Some(file) = file else {
        return err(StatusCode::NOT_FOUND, "meme 不存在");
    };
    let path = state.memes_dir.join(&file);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let mime = match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => "application/octet-stream",
            };
            ([(header::CONTENT_TYPE, mime)], bytes).into_response()
        }
        Err(e) => err(StatusCode::NOT_FOUND, &format!("文件读取失败: {e}")),
    }
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}
