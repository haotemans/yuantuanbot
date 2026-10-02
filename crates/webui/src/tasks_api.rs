//! 任务列表与任务流水（tasks / task_events 时序）。

use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct TasksQuery {
    state: Option<String>,
    limit: Option<i64>,
}

pub async fn list(State(state): State<AppState>, Query(q): Query<TasksQuery>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let conn = state.open_db().map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let rows: Vec<Value> = if let Some(s) = q.state.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        let mut stmt = conn
            .prepare(
                "SELECT task_id, goal, state, budget_max_calls, used_calls, created_by_pid, chat_id, created_at, finished_at
                 FROM tasks WHERE state = ?1 ORDER BY created_at DESC LIMIT ?2",
            )
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        let it = stmt.query_map(params![s, limit], task_row).map_err(|e| err(&format!("查询失败: {e}")))?;
        it.filter_map(|r| r.ok()).collect()
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT task_id, goal, state, budget_max_calls, used_calls, created_by_pid, chat_id, created_at, finished_at
                 FROM tasks ORDER BY created_at DESC LIMIT ?1",
            )
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        let it = stmt.query_map(params![limit], task_row).map_err(|e| err(&format!("查询失败: {e}")))?;
        it.filter_map(|r| r.ok()).collect()
    };
    Ok(Json(json!({ "tasks": rows })))
}

fn task_row(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "task_id": r.get::<_, String>(0)?,
        "goal": r.get::<_, String>(1)?,
        "state": r.get::<_, String>(2)?,
        "budget_max_calls": r.get::<_, i64>(3)?,
        "used_calls": r.get::<_, i64>(4)?,
        "created_by_pid": r.get::<_, String>(5)?,
        "chat_id": r.get::<_, String>(6)?,
        "created_at": r.get::<_, i64>(7)?,
        "finished_at": r.get::<_, Option<i64>>(8)?,
    }))
}

pub async fn events(State(state): State<AppState>, Path(task_id): Path<String>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let conn = state.open_db().map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let exists: Option<i64> = conn
        .query_row("SELECT COUNT(*) FROM tasks WHERE task_id = ?1", params![task_id], |r| r.get(0))
        .ok();
    if exists.unwrap_or(0) == 0 {
        return Err((StatusCode::NOT_FOUND, Json(json!({ "error": format!("任务 {task_id} 不存在") }))));
    }
    let mut stmt = conn
        .prepare("SELECT id, seq, kind, payload, ts FROM task_events WHERE task_id = ?1 ORDER BY seq ASC")
        .map_err(|e| err(&format!("查询失败: {e}")))?;
    let rows = stmt
        .query_map(params![task_id], |r| {
            let payload_raw: Option<String> = r.get(3)?;
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "seq": r.get::<_, i64>(1)?,
                "kind": r.get::<_, String>(2)?,
                "payload": payload_raw.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null),
                "ts": r.get::<_, i64>(4)?,
            }))
        })
        .map_err(|e| err(&format!("查询失败: {e}")))?;
    let items: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
    Ok(Json(json!({ "events": items })))
}

fn err(msg: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": msg })))
}
