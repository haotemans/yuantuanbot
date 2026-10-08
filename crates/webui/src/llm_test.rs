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
    let env_name = env_name.trim();
    // sk- 直填 vs 环境变量名:resolve_api_key 给出 None 时报错
    if !env_name.is_empty() && yuantuan_core::llm::resolve_api_key(env_name).is_none() {
        return Err(bad(&format!(
            "provider「{provider}」的 API key 未就绪——值为 `{env_name}`,既非 sk- 开头的密钥也不是已设置的环境变量名"
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

/// POST /api/llm/models/probe {base_url, api_key_env}：
/// 探测未保存的 provider——前端新增 provider 时直接调本端点，不必先把 provider 写进 providers.toml。
/// api_key_env 为环境变量名（不回显密钥本身）；空字符串 = 无需鉴权。
#[derive(Deserialize)]
pub struct ProbeBody {
    base_url: String,
    api_key_env: Option<String>,
}

pub async fn probe_models(Json(body): Json<ProbeBody>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let base = body.base_url.trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err(bad("base_url 不能为空"));
    }
    if !base.starts_with("http://") && !base.starts_with("https://") {
        return Err(bad("base_url 必须以 http:// 或 https:// 开头"));
    }
    let api_key = match body.api_key_env.as_deref().unwrap_or("").trim() {
        "" => None,
        v => match yuantuan_core::llm::resolve_api_key(v) {
            Some(k) => Some(k),
            None => return Err(bad(&format!(
                "API key 未就绪——值 `{v}` 既非 sk- 开头的密钥也不是已设置的环境变量名"
            ))),
        },
    };
    let http = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
    {
        Ok(c) => c,
        Err(e) => return Err(bad(&format!("构造 HTTP client 失败：{e}"))),
    };
    let mut req = http.get(format!("{base}/models"));
    if let Some(k) = &api_key {
        req = req.bearer_auth(k);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return Err(bad(&format!("连接失败：{e}"))),
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(bad(&format!("HTTP {status}: {}", &text[..text.floor_char_boundary(200)])));
    }
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Err(bad("响应非 JSON")),
    };
    let arr = match v.get("data").and_then(|d| d.as_array()) {
        Some(a) => a,
        None => return Err(bad("响应缺 data 数组（不是 OpenAI 兼容端点？）")),
    };
    let mut out: Vec<String> = arr
        .iter()
        .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
        .collect();
    out.sort();
    out.dedup();
    Ok(Json(json!({ "ok": true, "models": out })))
}
