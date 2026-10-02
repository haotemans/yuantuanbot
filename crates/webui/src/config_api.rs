//! 配置 API：GET 结构化 JSON（敏感键递归掩码；provider key 走环境变量、原文永不出环境）、
//! POST 整体写回（写前校验 TOML 能解析回原结构）→ 组件级热应用：
//! LLM Provider 整体重建换槽、prefilter 阈值生效；其余组件 TODO（adapter/webui 端口等重启生效）。
//! 热应用完成后发 ConfigReloaded 事件（/ws 实时推送用）。

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

/// 掩码的键名（大小写不敏感）
const MASK_KEYS: [&str; 4] = ["token", "api_key", "password", "secret"];

pub async fn get_config(State(state): State<AppState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let config = read_toml_as_json(&state.extras.config_path)
        .map(masked)
        .unwrap_or_else(|| json!({}));
    let mut providers = read_toml_as_json(&state.extras.providers_path)
        .map(masked_providers)
        .unwrap_or_else(|| json!({}));
    // 追加每个 provider 的 key 就绪状态（只报布尔，不读原文）
    if let Some(map) = read_toml_as_json(&state.extras.providers_path) {
        if let (Some(out), Some(src)) = (providers.get_mut("provider"), map.get("provider")) {
            if let (Some(out_tbl), Some(src_tbl)) = (out.as_object_mut(), src.as_object()) {
                for (name, src_v) in src_tbl {
                    let env_name = src_v.get("api_key_env").and_then(|e| e.as_str()).unwrap_or("");
                    let set = !env_name.is_empty()
                        && std::env::var(env_name).map(|v| !v.is_empty()).unwrap_or(false);
                    if let Some(o) = out_tbl.get_mut(name).and_then(|v| v.as_object_mut()) {
                        o.insert("api_key_present".into(), json!(set));
                    }
                }
            }
        }
    }
    Ok(Json(json!({ "config": config, "providers": providers })))
}

fn read_toml_as_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    serde_json::to_value(v).ok()
}

/// 递归掩码敏感键（token/api_key/password/secret）
fn masked(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .map(|(k, val)| {
                    if MASK_KEYS.iter().any(|mk| k.eq_ignore_ascii_case(mk)) {
                        (k, json!("***"))
                    } else {
                        (k, masked(val))
                    }
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(masked).collect()),
        other => other,
    }
}

/// providers.toml：api_key_env 是「环境变量名」不是密钥本体（仍按敏感键通用规则过一遍）
fn masked_providers(v: Value) -> Value {
    masked(v)
}

#[derive(Deserialize)]
pub struct WriteBody {
    config: Option<Value>,
    providers: Option<Value>,
}

pub async fn post_config(State(state): State<AppState>, Json(body): Json<WriteBody>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if body.config.is_none() && body.providers.is_none() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "config/providers 至少给一个" }))));
    }
    let mut applied: Vec<String> = Vec::new();

    // config.toml 写回：序列化后必须能解析回原结构（prefilter 阈值取自它）
    if let Some(cfg) = &body.config {
        let toml_text = to_toml_text(cfg)?;
        // 回读校验：结构须完整含 [prefilter]（缺失则补采用默认），解析失败即拒
        let back: toml::Value = toml::from_str(&toml_text)
            .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("config TOML 回读解析失败: {e}") }))))?;
        std::fs::write(&state.extras.config_path, &toml_text)
            .map_err(|e| err(&format!("写 config.toml 失败: {e}")))?;
        let pf = back.get("prefilter");
        let mut slot = state.extras.prefilter_slot.write().unwrap();
        if let Some(pf) = pf {
            if let Some(w) = pf.get("window_secs").and_then(|v| v.as_integer()) {
                slot.window_secs = w;
            }
            if let Some(c) = pf.get("self_msg_cap").and_then(|v| v.as_integer()) {
                slot.self_msg_cap = c;
            }
        }
        applied.push("prefilter 阈值热应用".into());
    }

    // providers.toml 写回：重建 LlmGateway 换槽；文件内容须解析+校验（roles 引用存在性）
    if let Some(p) = &body.providers {
        let toml_text = to_toml_text(p)?;
        let tmp = state.extras.providers_path.with_extension("hotcheck.toml");
        std::fs::write(&tmp, &toml_text).map_err(|e| err(&format!("写临时文件失败: {e}")))?;
        let check = yuantuan_core::llm::LlmGateway::load(&tmp);
        let _ = std::fs::remove_file(&tmp);
        let gateway = check.map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("providers.toml 校验失败: {e}") }))))?;
        std::fs::write(&state.extras.providers_path, &toml_text)
            .map_err(|e| err(&format!("写 providers.toml 失败: {e}")))?;
        *state.extras.llm_slot.write().unwrap() = Some(std::sync::Arc::new(gateway));
        applied.push("LLM Provider 重建热应用".into());
    }

    // TODO(热配单)：adapter 重连、webui 端口、consolidation 时刻等其余组件仍重启生效
    state.extras.bus.publish(yuantuan_core::event::Event::ConfigReloaded);
    tracing::info!(applied = ?applied, "配置写回并热应用");
    Ok(Json(json!({ "ok": true, "applied": applied, "note": "adapter/端口/归纳时刻等其余项重启生效（TODO）" })))
}

/// JSON → TOML 文本；写前须保证能反向解析
fn to_toml_text(v: &Value) -> Result<String, (StatusCode, Json<Value>)> {
    let tv: toml::Value = serde_json::from_value(v.clone())
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("JSON 结构无法映射到 TOML: {e}") }))))?;
    let text = toml::to_string_pretty(&tv)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("TOML 序列化失败: {e}") }))))?;
    toml::from_str::<toml::Value>(&text)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("TOML 回读校验失败: {e}") }))))?;
    Ok(text)
}

fn err(msg: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": msg })))
}
