//! 记忆浏览：long_memories / summaries 列表查询。

use crate::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use rusqlite::ToSql;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct MemQuery {
    owner_type: Option<String>,
    owner_id: Option<String>,
    limit: Option<i64>,
}

/// binds: 已编号 ?1..?n；limit 追加为最后一个占位 ?(n+1)
fn collect_rows(
    stmt: &mut rusqlite::Statement,
    bind_refs: &[&dyn ToSql],
    row_fn: fn(&rusqlite::Row) -> rusqlite::Result<Value>,
) -> rusqlite::Result<Vec<Value>> {
    let rows = stmt.query_map(bind_refs, row_fn)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub async fn memories(State(state): State<AppState>, Query(q): Query<MemQuery>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let conn = state.open_db().map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let mut sql = String::from(
        "SELECT id, owner_type, owner_id, content, source, created_at, updated_at FROM long_memories WHERE 1=1",
    );
    let mut binds: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(t) = q.owner_type.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        binds.push(Box::new(t));
        sql += &format!(" AND owner_type = ?{}", binds.len());
    }
    if let Some(id) = q.owner_id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        binds.push(Box::new(id));
        sql += &format!(" AND owner_id = ?{}", binds.len());
    }
    sql += &format!(" ORDER BY updated_at DESC LIMIT ?{}", binds.len() + 1);
    binds.push(Box::new(limit));
    let refs: Vec<&dyn ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql).map_err(|e| err(&format!("查询失败: {e}")))?;
    let items = collect_rows(&mut stmt, &refs, mem_row).map_err(|e| err(&format!("查询失败: {e}")))?;
    Ok(Json(json!({ "memories": items })))
}

pub async fn summaries(State(state): State<AppState>, Query(q): Query<MemQuery>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let conn = state.open_db().map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let mut sql = String::from(
        "SELECT id, owner_type, owner_id, period, date, summary, msg_id_start, msg_id_end, created_at FROM summaries WHERE 1=1",
    );
    let mut binds: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(t) = q.owner_type.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        binds.push(Box::new(t));
        sql += &format!(" AND owner_type = ?{}", binds.len());
    }
    if let Some(id) = q.owner_id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        binds.push(Box::new(id));
        sql += &format!(" AND owner_id = ?{}", binds.len());
    }
    sql += &format!(" ORDER BY date DESC, id DESC LIMIT ?{}", binds.len() + 1);
    binds.push(Box::new(limit));
    let refs: Vec<&dyn ToSql> = binds.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql).map_err(|e| err(&format!("查询失败: {e}")))?;
    let items = collect_rows(&mut stmt, &refs, sum_row).map_err(|e| err(&format!("查询失败: {e}")))?;
    Ok(Json(json!({ "summaries": items })))
}

fn mem_row(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, i64>(0)?,
        "owner_type": r.get::<_, String>(1)?,
        "owner_id": r.get::<_, String>(2)?,
        "content": r.get::<_, String>(3)?,
        "source": r.get::<_, String>(4)?,
        "created_at": r.get::<_, i64>(5)?,
        "updated_at": r.get::<_, i64>(6)?,
    }))
}

fn sum_row(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, i64>(0)?,
        "owner_type": r.get::<_, String>(1)?,
        "owner_id": r.get::<_, String>(2)?,
        "period": r.get::<_, String>(3)?,
        "date": r.get::<_, String>(4)?,
        "summary": r.get::<_, String>(5)?,
        "msg_id_start": r.get::<_, Option<i64>>(6)?,
        "msg_id_end": r.get::<_, Option<i64>>(7)?,
        "created_at": r.get::<_, i64>(8)?,
    }))
}

fn err(msg: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": msg })))
}
