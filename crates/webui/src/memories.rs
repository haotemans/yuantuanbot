//! 记忆浏览：long_memories / summaries 列表查询。

use crate::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use sqlx::Row;
use yuantuan_core::db::SqliteExt;

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct MemQuery {
    owner_type: Option<String>,
    owner_id: Option<String>,
    limit: Option<i64>,
}

/// binds: 已编号 ?1..?n；limit 追加为最后一个占位 ?(n+1)
async fn collect_rows(
    stmt: &mut yuantuan_core::db::Statement<'_>,
    bind_refs: sqlx::sqlite::SqliteArguments<'static>,
    row_fn: fn(&sqlx::sqlite::SqliteRow) -> sqlx::Result<Value>,
) -> sqlx::Result<Vec<Value>> {
    let rows = stmt.query_map(bind_refs, row_fn).await?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub async fn memories(
    State(state): State<AppState>,
    Query(q): Query<MemQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut conn = state
        .open_db()
        .await
        .map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let mut sql = String::from(
        "SELECT id, owner_type, owner_id, content, source, created_at, updated_at FROM long_memories WHERE 1=1",
    );
    let mut binds: Vec<yuantuan_core::db::Value> = Vec::new();
    if let Some(t) = q
        .owner_type
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        binds.push((t).into());
        sql += &format!(" AND owner_type = ?{}", binds.len());
    }
    if let Some(id) = q
        .owner_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        binds.push((id).into());
        sql += &format!(" AND owner_id = ?{}", binds.len());
    }
    sql += &format!(" ORDER BY updated_at DESC LIMIT ?{}", binds.len() + 1);
    binds.push((limit).into());
    let mut stmt = conn
        .prepare(&sql)
        .await
        .map_err(|e| err(&format!("查询失败: {e}")))?;
    let items = collect_rows(
        &mut stmt,
        yuantuan_core::db::params_from_iter(binds),
        mem_row,
    )
    .await
    .map_err(|e| err(&format!("查询失败: {e}")))?;
    Ok(Json(json!({ "memories": items })))
}

pub async fn summaries(
    State(state): State<AppState>,
    Query(q): Query<MemQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut conn = state
        .open_db()
        .await
        .map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let mut sql = String::from(
        "SELECT id, owner_type, owner_id, period, date, summary, msg_id_start, msg_id_end, created_at FROM summaries WHERE 1=1",
    );
    let mut binds: Vec<yuantuan_core::db::Value> = Vec::new();
    if let Some(t) = q
        .owner_type
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        binds.push((t).into());
        sql += &format!(" AND owner_type = ?{}", binds.len());
    }
    if let Some(id) = q
        .owner_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        binds.push((id).into());
        sql += &format!(" AND owner_id = ?{}", binds.len());
    }
    sql += &format!(" ORDER BY date DESC, id DESC LIMIT ?{}", binds.len() + 1);
    binds.push((limit).into());
    let mut stmt = conn
        .prepare(&sql)
        .await
        .map_err(|e| err(&format!("查询失败: {e}")))?;
    let items = collect_rows(
        &mut stmt,
        yuantuan_core::db::params_from_iter(binds),
        sum_row,
    )
    .await
    .map_err(|e| err(&format!("查询失败: {e}")))?;
    Ok(Json(json!({ "summaries": items })))
}

fn mem_row(r: &sqlx::sqlite::SqliteRow) -> sqlx::Result<Value> {
    Ok(json!({
        "id": r.try_get::<i64, _>(0)?,
        "owner_type": r.try_get::<String, _>(1)?,
        "owner_id": r.try_get::<String, _>(2)?,
        "content": r.try_get::<String, _>(3)?,
        "source": r.try_get::<String, _>(4)?,
        "created_at": r.try_get::<i64, _>(5)?,
        "updated_at": r.try_get::<i64, _>(6)?,
    }))
}

fn sum_row(r: &sqlx::sqlite::SqliteRow) -> sqlx::Result<Value> {
    Ok(json!({
        "id": r.try_get::<i64, _>(0)?,
        "owner_type": r.try_get::<String, _>(1)?,
        "owner_id": r.try_get::<String, _>(2)?,
        "period": r.try_get::<String, _>(3)?,
        "date": r.try_get::<String, _>(4)?,
        "summary": r.try_get::<String, _>(5)?,
        "msg_id_start": r.try_get::<Option<i64>, _>(6)?,
        "msg_id_end": r.try_get::<Option<i64>, _>(7)?,
        "created_at": r.try_get::<i64, _>(8)?,
    }))
}

fn err(msg: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
}
