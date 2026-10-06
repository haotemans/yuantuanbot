//! /image /画 命令直派（Q008=Q015）：消息前缀识别 → 解析 → 三次校验 → 优化 → 调 provider → 落盘 → 发图
//!
//! 不走 Decision；不进入 bubble 引擎普通回复流（直接 enqueue_image 入 per-chat 串行队列保序）。

use anyhow::{Context, Result};
use rusqlite::params;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

use crate::event::{EventBus, MessageReceivedPayload};
use crate::reply_engine::{ChatType, EngineHandle};
use super::parser::{parse_image_command, render_error};
use super::provider::{EndpointStyle, MediaProvider};
use super::quota::{check_can_generate, Permission};
use super::{ImageRequest, ImageArtifacts};

/// media 命令所需全部上下文（装配侧注入）
#[derive(Clone)]
pub struct MediaCtx {
    pub db_path: PathBuf,
    pub data_dir: PathBuf,
    pub bus: EventBus,
    pub reply_engine: Option<EngineHandle>,
    pub registry: Arc<std::sync::RwLock<std::collections::HashMap<String, Arc<dyn MediaProvider>>>>,
    pub self_pid_admin: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// Q010 提示词优化使用的 LLM gateway 共享槽（bot 管线的 SharedLlm 同构）
    pub llm: Arc<std::sync::RwLock<Option<Arc<crate::llm::LlmGateway>>>>,
}

/// 消息是否触发生图命令；返回 body（去掉 /image 或 /画 前缀的部分）
pub fn match_image_command(text: &str) -> Option<&str> {
    let t = text.trim();
    for prefix in ["/image ", "/image\n", "/画 ", "/画\n"] {
        if let Some(rest) = t.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    // 末尾 /image 或 /画 不带参数
    if t == "/image" || t == "/画" {
        return Some("");
    }
    None
}

/// 命令直派入口
pub async fn handle_image_command(ctx: &MediaCtx, m: &MessageReceivedPayload, body: &str) {
    let chat_id = m.chat_id.clone();
    let sender_pid = m.sender_pid.clone();

    // 第一步：从 db 读出该用户能看到的模型清单 + 每模型支持的 ratio
    let known_models = list_known_models(&ctx.db_path).unwrap_or_default();
    let ratios_for = |alias: &str| supported_ratios_for_model(&ctx.db_path, alias).unwrap_or_default();

    // 第二步：parse（Q015 严格报错）
    let cmd = match parse_image_command(body, &known_models, &ratios_for) {
        Ok(c) => c,
        Err(e) => {
            send_text(ctx, &chat_id, &m.chat_type, &sender_pid, &render_error(&e)).await;
            return;
        }
    };

    // 第三步：加载 model + provider 配置
    let (model_row, provider_row) = match load_model_and_provider(&ctx.db_path, &cmd.model_alias) {
        Ok(v) => v,
        Err(e) => {
            send_text(ctx, &chat_id, &m.chat_type, &sender_pid, &format!("加载模型失败：{e}")).await;
            return;
        }
    };
    if !model_row.enabled {
        send_text(ctx, &chat_id, &m.chat_type, &sender_pid, "该模型已禁用").await;
        return;
    }
    if !provider_row.enabled {
        send_text(ctx, &chat_id, &m.chat_type, &sender_pid, "该模型所属 provider 已禁用").await;
        return;
    }

    // 第四步：三次校验（Q009）
    let is_admin = (ctx.self_pid_admin)(&sender_pid);
    let perm = if model_row.permission == "admin_only" { Permission::AdminOnly } else { Permission::Everyone };
    let daily_used = daily_used_count(&ctx.db_path, &cmd.model_alias).unwrap_or(0);
    let balance = get_balance(&ctx.db_path, &sender_pid).unwrap_or(0);
    let total_cost = model_row.cost_per_result * cmd.count as i64;
    if let Err(e) = check_can_generate(
        is_admin,
        perm,
        daily_used as u32,
        model_row.daily_quota as u32,
        balance,
        total_cost,
    ) {
        send_text(ctx, &chat_id, &m.chat_type, &sender_pid, &e.to_string()).await;
        return;
    }

    // 第五步：seed 缺省回查 chat.last_media_seed（Q014）
    let seed = match cmd.seed {
        Some(s) => Some(s),
        None => load_chat_last_seed(&ctx.db_path, &chat_id),
    };

    // 第六步：提示词优化（Q010；LLM 可用则改写，不可用/失败 fallback 原文）
    let style = prompt_style_from_str(&model_row.prompt_style);
    let optimized = {
        let llm_gw = ctx.llm.read().unwrap().clone();
        super::prompt::optimize_prompt(&cmd.prompt, style, llm_gw.as_deref()).await
    };

    // 第七步：构造 ImageRequest 并调 provider
    let req = ImageRequest {
        model_alias: cmd.model_alias.clone(),
        provider_name: provider_row.name.clone(),
        prompt: optimized.clone(),
        raw_prompt: cmd.prompt.clone(),
        ratio: cmd.ratio.clone(),
        count: cmd.count,
        seed,
        negative: cmd.negative.clone(),
        quality: cmd.quality.clone().or(model_row.default_quality),
        seed_from_chat: cmd.seed.is_none() && seed.is_some(),
    };

    let endpoint = model_row
        .endpoint_style
        .as_deref()
        .map(endpoint_from_str)
        .unwrap_or_else(|| endpoint_from_str(&provider_row.default_endpoint));

    let provider = {
        let reg = ctx.registry.read().expect("media registry poisoned");
        reg.get(&provider_row.name).cloned()
    };
    let Some(provider) = provider else {
        send_text(ctx, &chat_id, &m.chat_type, &sender_pid,
                  &format!("provider \"{}\" 未注册（启动时未装配）", provider_row.name)).await;
        return;
    };

    // 入队执行任务（先做：占位记录到 media_tasks）
    let task_id = record_task_pending(&ctx.db_path, &m, &req, &endpoint, total_cost).ok();
    info!(model = %cmd.model_alias, ratio = %cmd.ratio, count = cmd.count, "media 任务开始");

    // 预备反馈一句话（避免用户干等）
    send_text(ctx, &chat_id, &m.chat_type, &sender_pid,
              &format!("🎨 {} · {} 生成中…", cmd.model_alias, cmd.ratio)).await;

    // 真正调用
    let result = provider.generate(&req, &model_row.model_id, endpoint, &ctx.data_dir).await;
    let finish_ts = now_ts();

    match result {
        Ok(artifacts) => {
            if let Some(tid) = task_id {
                let _ = mark_task_success(&ctx.db_path, tid, &artifacts, finish_ts);
            }
            if let Some(s) = artifacts.seed {
                let _ = save_chat_last_seed(&ctx.db_path, &chat_id, s);
            }
            if total_cost > 0 {
                let _ = deduct_balance(&ctx.db_path, &sender_pid, total_cost);
            }
            // 发图（Q012：OneBot image 段 file:///）
            send_images(ctx, &chat_id, &m.chat_type, &sender_pid, &artifacts).await;
        }
        Err(e) => {
            if let Some(tid) = task_id {
                let _ = mark_task_failed(&ctx.db_path, tid, &e.to_string(), finish_ts);
            }
            warn!(error = %e, model = %cmd.model_alias, "media 任务失败");
            send_text(ctx, &chat_id, &m.chat_type, &sender_pid, &format!("❌ 生成失败：{e}")).await;
        }
    }
}

// ---------- 数据访问 ----------

#[derive(Debug)]
#[allow(dead_code)] // ratios 已用于 supported_ratios_for_model；保留供未来 default 显示
struct ModelRow {
    model_id: String,
    provider: String,
    endpoint_style: Option<String>,
    prompt_style: String,
    daily_quota: i64,
    cost_per_result: i64,
    permission: String,
    enabled: bool,
    default_quality: Option<String>,
    ratios: String,
}

#[derive(Debug)]
struct ProviderRow {
    name: String,
    default_endpoint: String,
    enabled: bool,
}

fn list_known_models(db: &Path) -> Result<Vec<String>> {
    let conn = crate::db::connect(db)?;
    let mut stmt = conn.prepare("SELECT alias FROM media_models WHERE enabled = 1")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn supported_ratios_for_model(db: &Path, alias: &str) -> Result<Vec<String>> {
    let conn = crate::db::connect(db)?;
    let s: Option<String> = conn
        .query_row(
            "SELECT ratios FROM media_models WHERE alias = ?1",
            params![alias],
            |r| r.get(0),
        )
        .ok();
    Ok(match s {
        Some(s) if !s.is_empty() => s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        _ => Vec::new(),
    })
}

fn load_model_and_provider(db: &Path, alias: &str) -> Result<(ModelRow, ProviderRow)> {
    let conn = crate::db::connect(db)?;
    let model = conn
        .query_row(
            "SELECT model_id, provider, endpoint_style, prompt_style, daily_quota,
                    cost_per_result, permission, enabled, default_quality, ratios
             FROM media_models WHERE alias = ?1",
            params![alias],
            |r| {
                Ok(ModelRow {
                    model_id: r.get(0)?,
                    provider: r.get(1)?,
                    endpoint_style: r.get(2)?,
                    prompt_style: r.get(3)?,
                    daily_quota: r.get(4)?,
                    cost_per_result: r.get(5)?,
                    permission: r.get(6)?,
                    enabled: r.get::<_, i64>(7)? != 0,
                    default_quality: r.get(8)?,
                    ratios: r.get(9)?,
                })
            },
        )
        .context("模型未注册")?;
    let provider = conn
        .query_row(
            "SELECT name, default_endpoint, enabled FROM media_providers WHERE name = ?1",
            params![&model.provider],
            |r| {
                Ok(ProviderRow {
                    name: r.get(0)?,
                    default_endpoint: r.get(1)?,
                    enabled: r.get::<_, i64>(2)? != 0,
                })
            },
        )
        .context("provider 不存在")?;
    Ok((model, provider))
}

fn daily_used_count(db: &Path, alias: &str) -> Result<i64> {
    let conn = crate::db::connect(db)?;
    // UTC 当天 0 点（够用于配额粒度；naive 实现避免引入 chrono）
    let sec_now = now_ts();
    let midnight = sec_now - (sec_now % 86400);
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM media_tasks WHERE model_alias = ?1 AND created_at >= ?2 AND state IN ('queued','running','success')",
        params![alias, midnight],
        |r| r.get(0),
    )?;
    Ok(n)
}

fn get_balance(db: &Path, pid: &str) -> Result<i64> {
    let conn = crate::db::connect(db)?;
    let b: Option<i64> = conn
        .query_row(
            "SELECT balance FROM media_credits WHERE person_id = ?1",
            params![pid],
            |r| r.get(0),
        )
        .ok();
    Ok(b.unwrap_or(0))
}

fn deduct_balance(db: &Path, pid: &str, amount: i64) -> Result<()> {
    let conn = crate::db::connect(db)?;
    let now = now_ts();
    conn.execute(
        "UPDATE media_credits SET balance = balance - ?1, updated_at = ?2 WHERE person_id = ?3",
        params![amount, now, pid],
    )?;
    Ok(())
}

fn load_chat_last_seed(db: &Path, chat_id: &str) -> Option<i64> {
    let conn = crate::db::connect(db).ok()?;
    let key = format!("last_media_seed:{chat_id}");
    conn.query_row(
        "SELECT value FROM state_kv WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.parse().ok())
}

fn save_chat_last_seed(db: &Path, chat_id: &str, seed: i64) -> Result<()> {
    let conn = crate::db::connect(db)?;
    let key = format!("last_media_seed:{chat_id}");
    conn.execute(
        "INSERT INTO state_kv(key, value, updated_at) VALUES(?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = ?3",
        params![key, seed.to_string(), now_ts()],
    )?;
    Ok(())
}

fn record_task_pending(
    db: &Path,
    m: &MessageReceivedPayload,
    req: &ImageRequest,
    endpoint: &EndpointStyle,
    cost: i64,
) -> Result<i64> {
    let conn = crate::db::connect(db)?;
    let now = now_ts();
    conn.execute(
        "INSERT INTO media_tasks(chat_id, chat_type, sender_pid, model_alias, provider, endpoint_style,
                                 raw_prompt, prompt, ratio, count, seed, cost, state, created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'queued',?13)",
        params![
            m.chat_id, m.chat_type, m.sender_pid,
            req.model_alias, req.provider_name, super::provider::endpoint_style_str(*endpoint),
            req.raw_prompt, req.prompt, req.ratio, req.count as i64,
            req.seed, cost, now,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn mark_task_success(db: &Path, id: i64, artifacts: &ImageArtifacts, finish_ts: i64) -> Result<()> {
    let conn = crate::db::connect(db)?;
    let art_json = json!(artifacts
        .paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>())
    .to_string();
    conn.execute(
        "UPDATE media_tasks SET state='success', artifacts=?1, finished_at=?2 WHERE id=?3",
        params![art_json, finish_ts, id],
    )?;
    Ok(())
}

fn mark_task_failed(db: &Path, id: i64, err: &str, finish_ts: i64) -> Result<()> {
    let conn = crate::db::connect(db)?;
    conn.execute(
        "UPDATE media_tasks SET state='failed', error=?1, finished_at=?2 WHERE id=?3",
        params![err.chars().take(400).collect::<String>(), finish_ts, id],
    )?;
    Ok(())
}

// ---------- misc helpers ----------

fn prompt_style_from_str(s: &str) -> super::prompt::PromptStyle {
    match s {
        "nai" => super::prompt::PromptStyle::Nai,
        "anima" => super::prompt::PromptStyle::Anima,
        "a1111" => super::prompt::PromptStyle::A1111,
        _ => super::prompt::PromptStyle::Natural,
    }
}

fn endpoint_from_str(s: &str) -> EndpointStyle {
    match s {
        "nai_native" => EndpointStyle::NaiNative,
        "openai_compat" => EndpointStyle::OpenaiCompat,
        "gemini" => EndpointStyle::Gemini,
        "xai" => EndpointStyle::Xai,
        _ => EndpointStyle::NaiNative,
    }
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 把一段文字通过 reply_engine 当作一个泡发出（错误/反馈用；走 Bubble 通道保序）
async fn send_text(ctx: &MediaCtx, chat_id: &str, chat_type: &str, sender: &str, text: &str) {
    let Some(engine) = &ctx.reply_engine else { return };
    let Some((ct, target)) = route_target(chat_id, chat_type, sender) else { return };
    use crate::reply_engine::{Bubble, JobKind, ReplyJob};
    engine.enqueue(ReplyJob {
        chat_id: chat_id.to_string(),
        chat_type: ct,
        target,
        anchor_msg_id: 0,
        mention: false,
        mention_qq: None,
        kind: JobKind::Bubbles(vec![Bubble { text: text.to_string(), at: false, meme: None }]),
    });
}

/// 把落盘的图片通过 reply_engine 当作纯图发出（Q012：OneBot image 段 file:///）
async fn send_images(ctx: &MediaCtx, chat_id: &str, chat_type: &str, sender: &str, artifacts: &ImageArtifacts) {
    let Some(engine) = &ctx.reply_engine else { return };
    let Some((ct, target)) = route_target(chat_id, chat_type, sender) else { return };
    for p in &artifacts.paths {
        engine.enqueue_image(chat_id.to_string(), ct, target, p.clone());
    }
}

/// chat/chat_type/target 解析（与 reply_engine::route_of 对齐）
fn route_target(chat_id: &str, chat_type: &str, sender_pid: &str) -> Option<(ChatType, u64)> {
    match chat_type {
        "group" => chat_id.parse::<u64>().ok().map(|gid| (ChatType::Group, gid)),
        "private" => sender_pid
            .trim_start_matches("p_")
            .parse::<u64>()
            .ok()
            .map(|uid| (ChatType::Private, uid)),
        _ => None,
    }
}
