//! 任务列表与任务流水（tasks / task_events 时序）。

use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use yuantuan_core::db::params;
use yuantuan_core::db::SqliteExt;

#[derive(Deserialize)]
pub struct TasksQuery {
    state: Option<String>,
    limit: Option<i64>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<TasksQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut conn = state
        .open_db()
        .await
        .map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let rows: Vec<Value> = if let Some(s) = q
        .state
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        let mut stmt = conn
            .prepare(
                "SELECT task_id, goal, state, budget_max_calls, used_calls, created_by_pid, chat_id, created_at, finished_at
                 FROM tasks WHERE state = ?1 ORDER BY created_at DESC LIMIT ?2",
            ).await
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        let it = stmt
            .query_map(params![s, limit], task_row)
            .await
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        it.filter_map(|r| r.ok()).collect()
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT task_id, goal, state, budget_max_calls, used_calls, created_by_pid, chat_id, created_at, finished_at
                 FROM tasks ORDER BY created_at DESC LIMIT ?1",
            ).await
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        let it = stmt
            .query_map(params![limit], task_row)
            .await
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        it.filter_map(|r| r.ok()).collect()
    };
    Ok(Json(json!({ "tasks": rows })))
}

fn task_row(r: &sqlx::sqlite::SqliteRow) -> sqlx::Result<Value> {
    Ok(json!({
        "task_id": r.try_get::<String, _>(0)?,
        "goal": r.try_get::<String, _>(1)?,
        "state": r.try_get::<String, _>(2)?,
        "budget_max_calls": r.try_get::<i64, _>(3)?,
        "used_calls": r.try_get::<i64, _>(4)?,
        "created_by_pid": r.try_get::<String, _>(5)?,
        "chat_id": r.try_get::<String, _>(6)?,
        "created_at": r.try_get::<i64, _>(7)?,
        "finished_at": r.try_get::<Option<i64>, _>(8)?,
    }))
}

pub async fn events(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut conn = state
        .open_db()
        .await
        .map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let exists: Option<i64> = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE task_id = ?1",
            params![task_id],
            |r| r.try_get(0),
        )
        .await
        .ok();
    if exists.unwrap_or(0) == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("任务 {task_id} 不存在") })),
        ));
    }
    let mut stmt = conn
        .prepare("SELECT id, seq, kind, payload, ts FROM task_events WHERE task_id = ?1 ORDER BY seq ASC").await
        .map_err(|e| err(&format!("查询失败: {e}")))?;
    let rows = stmt
        .query_map(params![task_id], |r| {
            let payload_raw: Option<String> = r.try_get(3)?;
            Ok(json!({
                "id": r.try_get::<i64, _>(0)?,
                "seq": r.try_get::<i64, _>(1)?,
                "kind": r.try_get::<String, _>(2)?,
                "payload": payload_raw.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null),
                "ts": r.try_get::<i64, _>(4)?,
            }))
        }).await
        .map_err(|e| err(&format!("查询失败: {e}")))?;
    let items: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
    Ok(Json(json!({ "events": items })))
}

fn err(msg: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
}
