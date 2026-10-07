//! MCP（Phase 3）面板后端：
//! GET  /api/mcp/list          列出 cfg.mcp.servers + 每个 server 的运行状态（含 tools 数量）
//! POST /api/mcp/save          保存 cfg.mcp（回写 config.toml [mcp] 段；下次启动生效；本进程不热改）
//! POST /api/mcp/call          调用某个 server 的某个 tool（debug；正常使用走 Decision/Skill → Tool trait）

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use yuantuan_core::mcp::{McpConfig, McpServerConfig};

pub async fn list(State(state): State<AppState>) -> Json<Value> {
    // 磁盘配置（展示用；当前 mcp_manager 只持有"运行时 clients"，不持有原始 cfg）
    let cfg_text = std::fs::read_to_string(&state.extras.config_path).unwrap_or_default();
    let disk_cfg: McpConfig = toml::from_str::<toml::Value>(&cfg_text)
        .ok()
        .and_then(|v| v.get("mcp").cloned())
        .and_then(|v| v.try_into().ok())
        .unwrap_or_default();

    let manager = state.extras.mcp_manager.as_ref();
    let servers: Vec<Value> = disk_cfg
        .servers
        .iter()
        .map(|s| {
            let client = manager.and_then(|m| {
                m.clients.iter().find(|c| c.server_name() == s.name).cloned()
            });
            json!({
                "name": s.name,
                "command": s.command,
                "args": s.args,
                "env_keys": s.env.keys().collect::<Vec<_>>(),
                "enabled": s.enabled,
                "running": client.is_some(),
                "tools": client.map(|c| {
                    c.tools().iter().map(|t| json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema,
                    })).collect::<Vec<_>>()
                }).unwrap_or_default(),
            })
        })
        .collect();
    Json(json!({
        "ok": true,
        "servers": servers,
        "hint": "改完配置必须重启 yuantuan 才会 spawn / 关停 MCP server；本面板不提供热改",
    }))
}

#[derive(Deserialize)]
pub struct SaveBody {
    servers: Vec<McpServerConfig>,
}

pub async fn save(
    State(state): State<AppState>,
    Json(body): Json<SaveBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // 校验：server name 不重复、不为空
    let mut seen = std::collections::HashSet::new();
    for s in &body.servers {
        if s.name.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": "server name 不能为空" })),
            ));
        }
        if !seen.insert(s.name.clone()) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": format!("server name `{}` 重复", s.name) })),
            ));
        }
        if s.command.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": format!("server `{}` command 不能为空", s.name) })),
            ));
        }
    }

    // 整体回写 config.toml 的 [mcp] 段
    let path = &state.extras.config_path;
    let text = std::fs::read_to_string(path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("读 config 失败：{e}") })),
        )
    })?;
    let mut root: toml::Value = toml::from_str(&text).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("config.toml 解析失败：{e}") })),
        )
    })?;
    let mcp_cfg = McpConfig {
        servers: body.servers,
    };
    let mcp_value = toml::Value::try_from(&mcp_cfg).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("序列化 mcp 配置失败：{e}") })),
        )
    })?;
    root.as_table_mut()
        .ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": "config.toml 顶层不是 table" })),
        ))?
        .insert("mcp".into(), mcp_value);
    let new_text = toml::to_string_pretty(&root).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("序列化 config 失败：{e}") })),
        )
    })?;
    // 备份再覆盖写
    let bak = path.with_extension("toml.bak");
    let _ = std::fs::copy(path, &bak);
    std::fs::write(path, new_text).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("写 config 失败：{e}") })),
        )
    })?;
    Ok(Json(json!({
        "ok": true,
        "message": "已写回 config.toml；重启 yuantuan 后 MCP server 才会被 spawn / 关停",
    })))
}

#[derive(Deserialize)]
pub struct CallBody {
    server: String,
    tool: String,
    #[serde(default)]
    args: Value,
}

pub async fn call(
    State(state): State<AppState>,
    Json(body): Json<CallBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let manager = state.extras.mcp_manager.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "ok": false, "error": "McpManager 未装配" })),
    ))?;
    let client = manager
        .clients
        .iter()
        .find(|c| c.server_name() == body.server)
        .cloned()
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": format!("MCP server `{}` 未运行", body.server) })),
        ))?;
    let resp = client
        .call_tool(&body.tool, body.args)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": format!("MCP call 失败：{e}") })),
            )
        })?;
    Ok(Json(json!({ "ok": true, "result": resp })))
}
