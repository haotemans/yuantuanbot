//! 关系网数据：nodes(persons，确保含 self) + edges(relationship_edges 当前全量值)。

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

pub async fn relations(State(state): State<AppState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let conn = state.open_db().map_err(|e| err(&format!("数据库打开失败: {e}")))?;

    let mut nodes: Vec<Value> = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT person_id, COALESCE(display_name, ''), first_seen, last_seen FROM persons")
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "name": r.get::<_, String>(1)?,
                    "first_seen": r.get::<_, i64>(2)?,
                    "last_seen": r.get::<_, i64>(3)?,
                }))
            })
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
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(json!({
                    "from": r.get::<_, String>(0)?,
                    "to": r.get::<_, String>(1)?,
                    "trust": r.get::<_, f64>(2)?,
                    "familiar": r.get::<_, f64>(3)?,
                    "updated_at": r.get::<_, i64>(4)?,
                }))
            })
            .map_err(|e| err(&format!("查询失败: {e}")))?;
        edges.extend(rows.filter_map(|r| r.ok()));
    }

    Ok(Json(json!({ "nodes": nodes, "edges": edges })))
}

fn err(msg: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": msg })))
}
