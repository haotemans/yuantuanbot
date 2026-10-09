//! Decision trace / 事件流：events 表过滤 + 倒序分页。

use crate::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use yuantuan_core::db::SqliteExt;

#[derive(Deserialize)]
pub struct EventsQuery {
    kind: Option<String>,
    chat_id: Option<String>,
    limit: Option<i64>,
    before_id: Option<i64>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut conn = state
        .open_db()
        .await
        .map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let mut sql = String::from("SELECT id, kind, payload, ts FROM events WHERE 1=1");
    let mut binds: Vec<yuantuan_core::db::Value> = Vec::new();
    if let Some(k) = q.kind.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        binds.push((k.to_string()).into());
        sql += &format!(" AND kind = ?{}", binds.len());
    }
    if let Some(c) = q
        .chat_id
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        // payload JSON 内联匹配（chat_id 字段）
        binds.push((format!("%\"chat_id\":\"{c}\"%")).into());
        sql += &format!(" AND payload LIKE ?{}", binds.len());
    }
    if let Some(b) = q.before_id {
        binds.push((b).into());
        sql += &format!(" AND id < ?{}", binds.len());
    }
    sql += &format!(" ORDER BY id DESC LIMIT ?{}", binds.len() + 1);
    binds.push((limit).into());

    let mut stmt = conn
        .prepare(&sql)
        .await
        .map_err(|e| err(&format!("查询失败: {e}")))?;
    let rows = stmt
        .query_map(yuantuan_core::db::params_from_iter(binds), row_json)
        .await
        .map_err(|e| err(&format!("查询失败: {e}")))?;
    let items: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
    Ok(Json(json!({ "events": items })))
}

fn row_json(r: &sqlx::sqlite::SqliteRow) -> sqlx::Result<Value> {
    let payload_raw: String = r.try_get(2)?;
    let payload: Value = serde_json::from_str(&payload_raw).unwrap_or(Value::Null);
    Ok(json!({
        "id": r.try_get::<i64, _>(0)?,
        "kind": r.try_get::<String, _>(1)?,
        "payload": payload,
        "ts": r.try_get::<i64, _>(3)?,
    }))
}

fn err(msg: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
}
