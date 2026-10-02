//! 仪表盘占位数据：全部来自真实 SQL / 文件系统。

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;

pub async fn overview(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.open_db().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("数据库打开失败: {e}") })),
        )
    })?;
    let count = |sql: &str| -> Result<i64, (StatusCode, Json<serde_json::Value>)> {
        conn.query_row(sql, [], |r| r.get(0)).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("查询失败: {e}") })),
            )
        })
    };

    let events_total = count("SELECT COUNT(*) FROM events")?;
    let messages_total = count("SELECT COUNT(*) FROM messages")?;
    // 本地时区零点（'localtime'→'start of day'→'utc' 是实现本地零点的标准写法）
    let messages_today = count(
        "SELECT COUNT(*) FROM messages \
         WHERE ts >= CAST(strftime('%s','now','localtime','start of day','utc') AS INTEGER)",
    )?;
    let db_size_bytes = std::fs::metadata(&state.db_path).map(|m| m.len()).unwrap_or(0);

    Ok(Json(json!({
        "events_total": events_total,
        "messages_total": messages_total,
        "messages_today": messages_today,
        "uptime_secs": state.started.elapsed().as_secs(),
        "db_size_bytes": db_size_bytes,
    })))
}
