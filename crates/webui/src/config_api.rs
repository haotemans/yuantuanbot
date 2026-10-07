//! 配置 API：GET 结构化 JSON（敏感键递归掩码；provider key 走环境变量、原文永不出环境）、
//! POST 整体写回（写前校验 TOML 能解析回原结构）→ 逐节热应用：
//! providers→LLM Provider 整体重建换槽；prefilter 阈值+成本闸 / reply 回复形态 / context 预算 /
//! meme 偷表情包开关 → 各共享槽换值；consolidation → 取消旧定时器按新配置重建；
//! napcat/webui/data/log 为槽外组件 → requires_restart 名单。响应 {ok, applied, requires_restart}。
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

/// G-hotfix：面板回填的 "***" 值从磁盘原文件还原真实值
/// 只处理「值恰为 ***」的叶子字段;递归 table
fn unmask_in_place(incoming: &mut Value, orig: &Value) {
    match (incoming, orig) {
        (Value::Object(new_map), Value::Object(orig_map)) => {
            for (k, v) in new_map.iter_mut() {
                if v.as_str() == Some("***") {
                    if let Some(real) = orig_map.get(k) {
                        *v = real.clone();
                    }
                } else if let Some(orig_v) = orig_map.get(k) {
                    unmask_in_place(v, orig_v);
                }
            }
        }
        (Value::Array(new_arr), Value::Array(orig_arr)) => {
            for (i, v) in new_arr.iter_mut().enumerate() {
                if let Some(orig_v) = orig_arr.get(i) {
                    unmask_in_place(v, orig_v);
                }
            }
        }
        _ => {}
    }
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
    let mut requires_restart: Vec<String> = Vec::new();

    // config.toml 写回：序列化后必须能解析回原结构；随后逐节热应用（仅对文件中实际存在的节）
    if let Some(cfg) = &body.config {
        // G-hotfix：面板 GET /config 返回的是***掩码后的 config,前端原样回填会把真 token 覆写成 "***"。
        // 规则:任何节里 value == "***" 的字段,从磁盘原文件里还原真值再写回;否则视为用户清空(显式清空应传空字符串)。
        let mut cfg_clean = cfg.clone();
        let orig = read_toml_as_json(&state.extras.config_path).unwrap_or_else(|| json!({}));
        unmask_in_place(&mut cfg_clean, &orig);
        let toml_text = to_toml_text(&cfg_clean)?;
        let back: toml::Value = toml::from_str(&toml_text)
            .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("config TOML 回读解析失败: {e}") }))))?;
        std::fs::write(&state.extras.config_path, &toml_text)
            .map_err(|e| err(&format!("写 config.toml 失败: {e}")))?;

        // [prefilter] → 阈值 + 成本闸换槽（管线每消息读槽）
        if let Some(pf) = back.get("prefilter") {
            let d = yuantuan_core::prefilter::Config::default();
            let mut slot = state.extras.prefilter_slot.write().unwrap();
            slot.window_secs = pf.get("window_secs").and_then(|v| v.as_integer()).unwrap_or(d.window_secs);
            slot.self_msg_cap = pf.get("self_msg_cap").and_then(|v| v.as_integer()).unwrap_or(d.self_msg_cap);
            slot.decision_cost_per_min = pf.get("decision_cost_per_min").and_then(|v| v.as_integer()).unwrap_or(d.decision_cost_per_min);
            applied.push("prefilter 节流阈值+成本闸热应用".into());
        }

        // [reply] → 回复形态参数换槽（引擎每泡读槽）
        if let Some(rp) = back.get("reply") {
            let d = yuantuan_core::reply_engine::ReplyCfg::default();
            let mut slot = state.extras.reply_slot.write().unwrap();
            slot.first_delay_min_ms = as_u64(rp, "first_delay_min_ms").unwrap_or(d.first_delay_min_ms);
            slot.first_delay_max_ms = as_u64(rp, "first_delay_max_ms").unwrap_or(d.first_delay_max_ms);
            slot.base_delay_ms = as_u64(rp, "base_delay_ms").unwrap_or(d.base_delay_ms);
            slot.per_char_ms = as_u64(rp, "per_char_ms").unwrap_or(d.per_char_ms);
            slot.jitter_ratio = rp.get("jitter_ratio").and_then(|v| v.as_float()).unwrap_or(d.jitter_ratio);
            slot.min_delay_ms = as_u64(rp, "min_delay_ms").unwrap_or(d.min_delay_ms);
            slot.max_delay_ms = as_u64(rp, "max_delay_ms").unwrap_or(d.max_delay_ms);
            slot.total_budget_ms = as_u64(rp, "total_budget_ms").unwrap_or(d.total_budget_ms);
            slot.bubble_cap = as_u64(rp, "bubble_cap").map(|n| n as usize).unwrap_or(d.bubble_cap);
            slot.bubble_char_cap = as_u64(rp, "bubble_char_cap").map(|n| n as usize).unwrap_or(d.bubble_char_cap);
            applied.push("reply 回复形态参数热应用".into());
        }

        // [context] → 上下文预算换槽（组 Bot Context 每消息读槽）
        if let Some(cx) = back.get("context") {
            let d = yuantuan_core::context_builder::ContextCfg::default();
            let mut slot = state.extras.ctx_slot.write().unwrap();
            slot.budget_chars = as_u64(cx, "budget_chars").map(|n| n as usize).unwrap_or(d.budget_chars);
            slot.k_init = as_u64(cx, "k").map(|n| n as usize).unwrap_or(d.k_init);
            slot.roster_mem_per = as_u64(cx, "roster_mem_per").map(|n| n as usize).unwrap_or(d.roster_mem_per);
            applied.push("context 上下文预算热应用".into());
        }

        // [meme] → 偷表情包开关换槽（监听每事件读槽）
        if let Some(mm) = back.get("meme") {
            if let Some(on) = mm.get("steal_enabled").and_then(|v| v.as_bool()) {
                *state.extras.steal_slot.write().unwrap() = on;
                applied.push("meme 偷表情包开关热应用".into());
            }
        }

        // [pipeline] → Q52 单 chat 容量 + SelfMsgIds 容量热应用（G008 裁决）
        if let Some(pp) = back.get("pipeline") {
            let mut applied_any = false;
            if let Some(cap) = pp.get("per_chat_queue_cap").and_then(|v| v.as_integer()) {
                *state.extras.per_chat_cap.write().unwrap() = (cap.max(1)) as usize;
                applied.push(format!("pipeline.per_chat_queue_cap 热应用为 {cap}(仅影响新 chat worker)"));
                applied_any = true;
            }
            if let Some(cap) = pp.get("self_msg_ids_cap").and_then(|v| v.as_integer()) {
                state.extras.self_ids.set_cap((cap.max(1)) as usize);
                applied.push(format!("pipeline.self_msg_ids_cap 热应用为 {cap}"));
                applied_any = true;
            }
            if let Some(cpm) = pp.get("decision_cost_per_min_init").and_then(|v| v.as_integer()) {
                if let Some(gw) = state.extras.llm_slot.read().unwrap().as_ref() {
                    gw.set_cost_per_min((cpm.max(1)) as usize);
                    applied.push(format!("pipeline.decision_cost_per_min_init 热应用为 {cpm}"));
                    applied_any = true;
                }
            }
            if !applied_any {
                applied.push("pipeline 段存在但无可热应用字段".into());
            }
        }

        // [consolidation] → 取消旧定时器，按新配置重建（句柄注册表在 Extras）
        if let Some(cs) = back.get("consolidation") {
            let enabled = cs.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            let daily_time = cs
                .get("daily_time")
                .and_then(|v| v.as_str())
                .unwrap_or("03:00")
                .to_string();
            let run_on_startup = cs.get("run_on_startup").and_then(|v| v.as_bool()).unwrap_or(false);
            if let Some(old) = state.extras.consolidation.lock().unwrap().take() {
                old.abort();
            }
            if enabled {
                let h = yuantuan_core::consolidation::spawn_scheduler(
                    yuantuan_core::consolidation::ConsolidationDeps {
                        db_path: state.db_path.clone(),
                        llm: state.extras.llm_slot.read().unwrap().clone(),
                        bus: state.extras.bus.clone(),
                        self_qq: state.extras.self_qq.clone(),
                        cfg: yuantuan_core::consolidation::ConsolidationCfg { enabled, daily_time, run_on_startup },
                    },
                );
                *state.extras.consolidation.lock().unwrap() = Some(h);
                applied.push("consolidation 归纳定时器重建热应用".into());
            } else {
                applied.push("consolidation 归纳已停用（定时器已取消，热应用）".into());
            }
        }

        // napcat：token 可热应用（写共享槽，ws_handler 每次连接 read），listen_addr 仍需重启
        if let Some(nap) = back.get("napcat") {
            let mut hot = false;
            if let Some(tok) = nap.get("token").and_then(|v| v.as_str()) {
                if let Some(slot) = &state.extras.napcat_token_slot {
                    *slot.write().unwrap() = tok.to_string();
                    hot = true;
                }
            }
            if hot {
                applied.push("napcat token 热应用（下次 NapCat 拨入即生效）".into());
            }
            if nap.get("listen_addr").is_some() || nap.get("enabled").is_some() {
                requires_restart.push("napcat listen_addr/enabled（重启生效；token 为热应用）".into());
            }
        }
        if back.get("webui").is_some() {
            requires_restart.push("webui 监听地址（重启生效）".into());
        }
        if back.get("data").is_some() {
            requires_restart.push("data 目录（重启生效）".into());
        }
        if back.get("log").is_some() {
            requires_restart.push("log 级别（重启生效）".into());
        }

        // backup：全部字段热应用（直接写共享槽；下次定时循环 read 即生效）
        if let Some(bk_cfg) = back.get("backup") {
            let enabled = bk_cfg.get("enabled").and_then(|v| v.as_bool());
            let repo_url = bk_cfg.get("repo_url").and_then(|v| v.as_str());
            let pat_env = bk_cfg.get("pat_env").and_then(|v| v.as_str());
            let daily_time = bk_cfg.get("daily_time").and_then(|v| v.as_str());
            let keep_days = bk_cfg.get("keep_days").and_then(|v| v.as_integer()).map(|n| n as i64);
            let mut slot = state.extras.backup_cfg.write().unwrap();
            if let Some(v) = enabled { slot.enabled = v; }
            if let Some(v) = repo_url { slot.repo_url = v.to_string(); }
            if let Some(v) = pat_env { slot.pat_env = v.to_string(); }
            if let Some(v) = daily_time { slot.daily_time = v.to_string(); }
            if let Some(v) = keep_days { slot.keep_days = v; }
            applied.push("backup 配置热应用（下次定时循环即生效）".into());
        }
    }

    // providers.toml 写回：重建 LlmGateway 换槽；文件内容须解析+校验（roles 引用存在性）
    if let Some(p) = &body.providers {
        let toml_text = to_toml_text(p)?;
        let tmp = state.extras.providers_path.with_extension("hotcheck.toml");
        std::fs::write(&tmp, &toml_text).map_err(|e| err(&format!("写临时文件失败: {e}")))?;
        let check = yuantuan_core::llm::LlmGateway::load(&tmp);
        let _ = std::fs::remove_file(&tmp);
        let mut gateway = check.map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("providers.toml 校验失败: {e}") }))))?;
        std::fs::write(&state.extras.providers_path, &toml_text)
            .map_err(|e| err(&format!("写 providers.toml 失败: {e}")))?;
        // 热重建也要带 usage sink，否则换槽后 token 统计断掉
        let db = state.db_path.clone();
        gateway.set_usage_sink(std::sync::Arc::new(move |rec: yuantuan_core::llm::LlmUsageRecord| {
            let db = db.clone();
            tokio::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    let conn = yuantuan_core::db::connect(&db).ok()?;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    conn.execute(
                        "INSERT INTO llm_usage(ts, role, model, prompt_tokens, completion_tokens, total_tokens)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            now, rec.role.to_string(), rec.model,
                            rec.usage.prompt_tokens as i64,
                            rec.usage.completion_tokens as i64,
                            rec.usage.total_tokens as i64,
                        ],
                    ).ok()
                }).await;
            });
        }));
        *state.extras.llm_slot.write().unwrap() = Some(std::sync::Arc::new(gateway));
        applied.push("LLM Provider 重建热应用".into());
    }

    state.extras.bus.publish(yuantuan_core::event::Event::ConfigReloaded);
    tracing::info!(applied = ?applied, requires_restart = ?requires_restart, "配置写回并热应用");
    Ok(Json(json!({ "ok": true, "applied": applied, "requires_restart": requires_restart })))
}

fn as_u64(tbl: &toml::Value, key: &str) -> Option<u64> {
    tbl.get(key).and_then(|v| v.as_integer()).and_then(|n| u64::try_from(n).ok())
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

/// G007：面板「保存并重启」——spawn 新 yuantuan 进程接管（继承环境变量与 cwd），自己 exit(0)。
/// 时序：先 202 响应 → 500ms 后 spawn 新进程 → 再 200ms 自己 exit；前端在此期间看到短暂连接中断属预期。
/// 新进程起来后 NapCat 会自动重连，面板刷新即可。
pub async fn restart(State(_state): State<AppState>) -> (StatusCode, Json<Value>) {
    // 容器内不自拉 — `spawn+exit(0)` 后容器主进程终止,docker restart policy 会拉起新容器,
    // 但若用户需要重启,正确做法是宿主机 `docker compose restart`,而非容器内自杀。
    // 返回 409 + 提示,前端按钮禁用并显示引导文案。
    if std::path::Path::new("/.dockerenv").exists() {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "error": "当前运行在容器内,无内重启语义。请在宿主机执行: cd deploy && docker compose restart"
            })),
        );
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return err(&format!("无法获取当前可执行路径: {e}"));
        }
    };
    tokio::spawn(async move {
        // 先让 HTTP 响应发出去
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        tracing::info!(new_exe = %exe.display(), "面板触发重启：spawn 新进程");
        // 新进程通过 YUANTUAN_DELAY_START_MS 延迟启动，给本进程留出释放端口的时间窗
        // Stdio 全 null：继承旧进程的 stdout/stderr 会在旧进程退出后变成 broken pipe,
        // 新进程第一次 println/tracing write 即 panic/exit(观察到的真实故障)。
        // 代价:重启后终端不再看到新进程输出;诊断走 data/logs/ 文件或手动重启。
        match std::process::Command::new(&exe)
            .env("YUANTUAN_DELAY_START_MS", "1200")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                tracing::info!(pid = child.id(), "新进程已拉起（延迟 1200ms 启动），本进程即将退出");
            }
            Err(e) => {
                tracing::error!(error = %e, "新进程拉起失败，本进程不退出（保持运行）");
                return;
            }
        }
        std::process::exit(0);
    });
    (StatusCode::ACCEPTED, Json(json!({ "ok": true, "msg": "重启中，500ms 后新进程接管" })))
}
