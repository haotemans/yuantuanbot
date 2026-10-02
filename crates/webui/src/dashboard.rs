//! 仪表盘：今日收发 / 今日 Decision 调用 / 活跃任务 / 运行秒数 / adapter 连接状态 / 当前 mood。

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;

pub async fn dashboard(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.open_db().map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let q = |sql: &str| -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0)
    };
    // 本地零点（localtime→start of day→utc 标准写法）
    const MIDNIGHT: &str = "CAST(strftime('%s','now','localtime','start of day','utc') AS INTEGER)";

    Ok(Json(json!({
        "messages_in_today": q(&format!("SELECT COUNT(*) FROM messages WHERE sender_pid != 'self' AND ts >= {MIDNIGHT}")),
        "messages_out_today": q(&format!("SELECT COUNT(*) FROM messages WHERE sender_pid = 'self' AND ts >= {MIDNIGHT}")),
        "decision_calls_today": q(&format!("SELECT COUNT(*) FROM events WHERE kind = 'DecisionMade' AND ts >= {MIDNIGHT}")),
        "active_tasks": q("SELECT COUNT(*) FROM tasks WHERE state = 'running'"),
        "uptime_secs": state.started.elapsed().as_secs(),
        "adapter_connected": (state.extras.adapter_connected)(),
        "mood": state.extras.mood.get().as_str(),
    })))
}

fn err(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": msg })))
}
