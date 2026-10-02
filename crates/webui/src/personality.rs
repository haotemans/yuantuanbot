//! 人格版本管理（data-model 六章：线性单向历史）。
//! 回滚 = 以旧版内容生成新版本 + note='回滚自 vX'，历史永远单向。

use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn list(State(state): State<AppState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let conn = state.open_db().map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let mut stmt = conn
        .prepare("SELECT version_no, note, created_by, created_at, active, length(content) FROM personality_versions ORDER BY version_no DESC")
        .map_err(|e| err(&format!("查询失败: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "version_no": r.get::<_, i64>(0)?,
                "note": r.get::<_, Option<String>>(1)?,
                "created_by": r.get::<_, String>(2)?,
                "created_at": r.get::<_, i64>(3)?,
                "active": r.get::<_, i64>(4)? == 1,
                "content_len": r.get::<_, i64>(5)?,
            }))
        })
        .map_err(|e| err(&format!("查询失败: {e}")))?;
    let items: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
    Ok(Json(json!({ "versions": items })))
}

pub async fn get_one(State(state): State<AppState>, Path(no): Path<i64>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let conn = state.open_db().map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    match conn.query_row(
        "SELECT version_no, content, note, created_by, created_at, active FROM personality_versions WHERE version_no = ?1",
        params![no],
        |r| {
            Ok(json!({
                "version_no": r.get::<_, i64>(0)?,
                "content": r.get::<_, String>(1)?,
                "note": r.get::<_, Option<String>>(2)?,
                "created_by": r.get::<_, String>(3)?,
                "created_at": r.get::<_, i64>(4)?,
                "active": r.get::<_, i64>(5)? == 1,
            }))
        },
    ) {
        Ok(v) => Ok(Json(v)),
        Err(_) => Err((StatusCode::NOT_FOUND, Json(json!({ "error": format!("版本 {no} 不存在") })))),
    }
}

#[derive(Deserialize)]
pub struct CreateBody {
    content: String,
    note: Option<String>,
}

pub async fn create(State(state): State<AppState>, Json(body): Json<CreateBody>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if body.content.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "content 不能为空" }))));
    }
    let v = insert_version(&state, &body.content, body.note.as_deref().unwrap_or("WebUI 新建"))?;
    Ok(Json(json!({ "ok": true, "version_no": v })))
}

pub async fn rollback(State(state): State<AppState>, Path(no): Path<i64>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let conn = state.open_db().map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let content: Option<String> = conn
        .query_row(
            "SELECT content FROM personality_versions WHERE version_no = ?1",
            params![no],
            |r| r.get(0),
        )
        .ok();
    let Some(content) = content else {
        return Err((StatusCode::NOT_FOUND, Json(json!({ "error": format!("版本 {no} 不存在") }))));
    };
    drop(conn);
    let v = insert_version(&state, &content, &format!("回滚自 v{no}"))?;
    Ok(Json(json!({ "ok": true, "version_no": v, "rolled_back_from": no })))
}

/// 新版本 = max+1；切 active：先清后设（全局唯一 active=1）；单事务
fn insert_version(state: &AppState, content: &str, note: &str) -> Result<i64, (StatusCode, Json<Value>)> {
    let mut conn = state.open_db().map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let tx = conn.transaction().map_err(|e| err(&format!("事务失败: {e}")))?;
    let next: i64 = tx
        .query_row("SELECT COALESCE(MAX(version_no), 0) + 1 FROM personality_versions", [], |r| r.get(0))
        .map_err(|e| err(&format!("查询失败: {e}")))?;
    tx.execute("UPDATE personality_versions SET active = 0", [])
        .map_err(|e| err(&format!("更新失败: {e}")))?;
    tx.execute(
        "INSERT INTO personality_versions(version_no, content, note, created_by, created_at, active) VALUES (?1, ?2, ?3, 'admin', ?4, 1)",
        params![next, content, note, now],
    )
    .map_err(|e| err(&format!("写入失败: {e}")))?;
    tx.commit().map_err(|e| err(&format!("提交失败: {e}")))?;
    tracing::info!(version_no = next, "人格新版本已激活");
    Ok(next)
}

fn err(msg: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": msg })))
}
