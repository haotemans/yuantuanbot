//! POST /api/llm/test {role}：用当前槽位 LLM 配置对该角色的 provider 发一次最短 chat
//! （"ping"，max_tokens 8），返回 {ok, latency_ms, model}。
//! 失败按真实原因精确反馈（角色未绑定 / provider 不存在 / API key 环境变量缺失 / 请求失败），
//! 均为 400 {ok:false,error:中文说明}；请求与响应体一律不入日志（防泄 key 路径上的中间日志）。

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use yuantuan_core::llm::Role;

#[derive(Deserialize)]
pub struct TestBody {
    role: String,
}

pub async fn test(State(state): State<AppState>, Json(body): Json<TestBody>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let role = match Role::parse(&body.role) {
        Some(r) => r,
        None => return Err(bad("role 须为 decision / bot_chat / agent_exec 之一")),
    };

    // 先按 providers.toml 原文精确诊断不可用原因（比 gateway 的"角色缺席"信息量大）
    let text = std::fs::read_to_string(&state.extras.providers_path).unwrap_or_default();
    let doc: toml::Value = toml::from_str(&text).unwrap_or(toml::Value::Table(Default::default()));
    let Some(bind) = doc.get("roles").and_then(|r| r.get(body.role.as_str())) else {
        return Err(bad(&format!("角色 {role} 未在 providers.toml 绑定——先到「模型」页完成角色绑定再测")));
    };
    let provider = bind.get("provider").and_then(|p| p.as_str()).unwrap_or("");
    let prov = doc.get("provider").and_then(|p| p.get(provider));
    let Some(prov) = prov else {
        return Err(bad(&format!("角色 {role} 绑定的 provider「{provider}」在 providers.toml 中不存在")));
    };
    let env_name = prov.get("api_key_env").and_then(|e| e.as_str()).unwrap_or("");
    if !env_name.is_empty() && std::env::var(env_name).map(|v| v.is_empty()).unwrap_or(true) {
        return Err(bad(&format!(
            "provider「{provider}」的 API key 环境变量 {env_name} 未设置或为空——密钥不走面板，配到服务器环境后重试"
        )));
    }

    let gateway = state
        .extras
        .llm_slot
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| bad("LLM gateway 未初始化（providers.toml 缺失或解析失败，详见启动日志）"))?;
    if gateway.role(role).is_none() {
        return Err(bad(&format!("角色 {role} 当前不可用（providers.toml 热加载后未提供该角色，见日志）")));
    }

    match gateway.test_chat(role).await {
        Ok((model, latency_ms)) => Ok(Json(json!({ "ok": true, "latency_ms": latency_ms, "model": model }))),
        Err(e) => Err(bad(&format!("连通测试失败：{e}"))),
    }
}

fn bad(msg: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "ok": false, "error": msg })))
}

/// GET /api/llm/models?provider=<name>：调 provider 的 /v1/models 拿全部可用模型 id，
/// 返回 {ok:true, models:[...]}。provider 不存在 / 请求失败均返回 400 {ok:false,error}。
#[derive(Deserialize)]
pub struct ModelsQuery {
    provider: String,
}

pub async fn list_models(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<ModelsQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let gateway = state
        .extras
        .llm_slot
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| bad("LLM gateway 未初始化（providers.toml 缺失或解析失败）"))?;
    match gateway.fetch_models(&q.provider).await {
        Ok(models) => Ok(Json(json!({ "ok": true, "models": models }))),
        Err(e) => Err(bad(&format!("获取模型列表失败：{e}"))),
    }
}
