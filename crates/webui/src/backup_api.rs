//! POST /api/backup/run 立即备份 / GET /api/backup/list 历史列表 /
//! POST /api/backup/restore 写恢复标记 / GET /api/backup/file/{name} 下载
//! 热应用槽：config.toml 写回 [backup] 段时同步更新 SharedBackupCfg

use crate::AppState;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use yuantuan_core::backup as bk;

pub async fn run(State(state): State<AppState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let cfg = state.extras.backup_cfg.read().unwrap().clone();
    let data_dir = state.db_path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
    match bk::run_backup(&cfg, &data_dir).await {
        Ok(a) => Ok(Json(json!({
            "ok": true,
            "file": a.path.file_name().unwrap_or_default().to_string_lossy(),
            "size_bytes": a.size_bytes,
            "elapsed_ms": a.elapsed_ms,
            "pushed": a.pushed,
            "push_error": a.push_error,
        }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("备份失败：{e:#}") })),
        )),
    }
}

pub async fn list(State(state): State<AppState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let data_dir = state.db_path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
    let backups = data_dir.join("backups");
    let mut items: Vec<Value> = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&backups).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("yt-") || !name.ends_with(".tar.gz") {
                continue;
            }
            let meta = match e.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            items.push(json!({
                "name": name,
                "size_bytes": meta.len(),
                "created_at": mtime,
            }));
        }
    }
    items.sort_by_key(|v| v.get("created_at").and_then(|x| x.as_i64()).unwrap_or(0));
    items.reverse();
    Ok(Json(json!({ "ok": true, "backups": items })))
}

#[derive(Deserialize)]
pub struct RestoreBody {
    file: String,
}

pub async fn restore(
    State(state): State<AppState>,
    Json(body): Json<RestoreBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // 防路径穿越：必须形如 yt-XXX.tar.gz
    if !body.file.starts_with("yt-") || !body.file.ends_with(".tar.gz") || body.file.contains('/') || body.file.contains('\\') || body.file.contains("..") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "文件名非法" })),
        ));
    }
    let data_dir = state.db_path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
    let tar = data_dir.join("backups").join(&body.file);
    if !tar.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": "备份文件不存在" })),
        ));
    }
    let marker = data_dir.join(".restore-pending");
    tokio::fs::write(&marker, &body.file).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("写恢复标记失败：{e}") })),
        )
    })?;
    tracing::warn!(file = %body.file, "恢复标记已写入，等待手动重启 yuantuan 完成恢复");
    Ok(Json(json!({
        "ok": true,
        "message": format!("恢复标记已写入。请手动重启 yuantuan；启动时会自动解压 {} 覆盖 data/", body.file),
    })))
}

pub async fn file(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    if !name.starts_with("yt-") || !name.ends_with(".tar.gz") || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "文件名非法" })),
        ));
    }
    let data_dir = state.db_path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
    let tar = data_dir.join("backups").join(&name);
    let bytes = tokio::fs::read(&tar).await.map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": format!("读取失败：{e}") })),
        )
    })?;
    let mut resp = bytes.into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/gzip".parse().unwrap(),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{name}\"").parse().unwrap(),
    );
    Ok(resp)
}
