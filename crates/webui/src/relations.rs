//! 关系网数据：nodes(persons，确保含 self) + edges(relationship_edges 当前全量值)。

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};
use sqlx::Row;
use yuantuan_core::db::SqliteExt;

pub async fn relations(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut conn = state
        .open_db()
        .await
        .map_err(|e| err(&format!("数据库打开失败: {e}")))?;

    let mut nodes: Vec<Value> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT person_id, COALESCE(display_name, ''), first_seen, last_seen FROM persons",
            )
            .await
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        let rows = stmt
            .query_map(yuantuan_core::db::params![], |r| {
                Ok(json!({
                    "id": r.try_get::<String, _>(0)?,
                    "name": r.try_get::<String, _>(1)?,
                    "first_seen": r.try_get::<i64, _>(2)?,
                    "last_seen": r.try_get::<i64, _>(3)?,
                }))
            })
            .await
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        nodes.extend(rows.filter_map(|r| r.ok()));
    }
    if !nodes.iter().any(|n| n["id"] == "self") {
        nodes.insert(0, json!({ "id": "self", "name": "云团" }));
    }

    let mut edges: Vec<Value> = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT from_pid, to_pid, trust, familiar, updated_at FROM relationship_edges")
            .await
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        let rows = stmt
            .query_map(yuantuan_core::db::params![], |r| {
                Ok(json!({
                    "from": r.try_get::<String, _>(0)?,
                    "to": r.try_get::<String, _>(1)?,
                    "trust": r.try_get::<f64, _>(2)?,
                    "familiar": r.try_get::<f64, _>(3)?,
                    "updated_at": r.try_get::<i64, _>(4)?,
                }))
            })
            .await
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        edges.extend(rows.filter_map(|r| r.ok()));
    }

    Ok(Json(json!({ "nodes": nodes, "edges": edges })))
}

fn err(msg: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
}
