//! Bot 系统：聊天管线——订阅 MessageReceived → Prefilter →（放行）→ 成本闸（llm.chat 内）→
//! 组上下文 → Decision → 副作用 + 事件；action=reply 再接 bot_chat → Bubbleizer → 发送队列；
//! action=send_meme → 抽图 → 发送队列。
//! 铁律：core 不依赖具体协议端；self_qq 以共享原子单元注入，发送以 SendFn 闭包注入。
//! 热应用（组件级）：llm / prefilter 走共享 RwLock 槽位，每条消息读取当前值，
//! WebUI config 写回后换槽即生效（其余组件 TODO，见 WebUI config_api）。

use crate::decision::{self, DecisionAction};
use crate::event::{BubbleSentPayload, Event, EventBus, MessageReceivedPayload};
use crate::llm::{LlmGateway, Role};
use crate::meme;
use crate::prefilter::{SelfMsgIds, Verdict};
use crate::reply_engine::{self, EngineHandle};
use crate::state::MoodState;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// LLM 共享槽（配置热应用：WebUI 写回 providers.toml 后整体换槽）
pub type SharedLlm = Arc<RwLock<Option<Arc<LlmGateway>>>>;
/// Prefilter 阈值共享槽
pub type SharedPrefilter = Arc<RwLock<crate::prefilter::Config>>;

#[derive(Clone)]
pub struct PipelineDeps {
    pub bus: EventBus,
    pub db_path: PathBuf,
    pub llm: SharedLlm,
    pub self_qq: Arc<AtomicU64>,
    pub self_ids: SelfMsgIds,
    pub mood: MoodState,
    pub prefilter: SharedPrefilter,
    /// 回复引擎；None 时 reply/send_meme 只记事件跳过（如 adapter 未接入）
    pub reply: Option<EngineHandle>,
    /// 回复形态参数热应用槽（bubbleize 泡顶/单泡上限在入队时读取）
    pub reply_cfg: crate::reply_engine::SharedReplyCfg,
    /// 上下文预算热应用槽（组 Bot Context 每消息读取）
    pub ctx_cfg: crate::context_builder::SharedContextCfg,
    /// data/memes 根目录（meme 抽图）
    pub memes_dir: PathBuf,
    /// 媒体命令上下文（MOD-022e /image 直派）；None 时命令忽略走原 Decision 管线
    pub media_ctx: Option<crate::tools::media::command::MediaCtx>,
    /// Skill 注册表（Q-S02 Decision 自-driven 调用 Skill）；None 时 Decision 不提供 invoke_skill 选项
    pub skill_registry: Option<crate::skills::SkillRegistry>,
    /// Q52 per-chat mpsc 容量热应用槽（G008）；新 worker spawn 时读
    pub per_chat_cap: SharedPerChatCap,
}

/// Q52：按 chat 隔离的 dispatcher + worker 拓扑——
/// dispatcher 订阅 broadcast，按 chat_id 派发到 per-chat mpsc；
/// 每 chat 一个 worker 协程串行 handle（保序）；不同 chat 并发，互不堵塞。
/// mpsc 容量来自共享槽（[pipeline].per_chat_queue_cap，默认 32，G008 热应用）；
/// 单 chat 瞬时洪峰超过则降级为同步 handle（保底不丢，仅阻塞 dispatcher 一拍）。
/// 热应用语义：改槽不影响已 spawn 的 worker（容量在 spawn 时定型），只影响新 worker。
pub type SharedPerChatCap = Arc<RwLock<usize>>;

/// Q55：扫描 messages 表中 processed_at IS NULL 的非 self 消息，按 ts 升序回放给 handle。
/// 在 spawn_pipeline 之前调用；返回回放条数。
/// 跳过 /image 命令——media 副作用大且用户当时已得到响应（避免重启刷图）。
/// 只回放最近 REPLAY_WINDOW_SECS 内的未处理；更早的直接标记已处理（防历史脏数据雪崩）。
const REPLAY_WINDOW_SECS: i64 = 3600;

pub async fn replay_pending(deps: &PipelineDeps) -> anyhow::Result<usize> {
    use rusqlite::params;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let cutoff = now - REPLAY_WINDOW_SECS;
    let conn = crate::db::connect(&deps.db_path)?;
    // 先把窗口外的未处理全部标记放弃（战的，不回放）
    let abandoned = conn.execute(
        "UPDATE messages SET processed_at = ?1
         WHERE processed_at IS NULL AND sender_pid != 'self' AND ts < ?2",
        params![now, cutoff],
    )?;
    if abandoned > 0 {
        info!(count = abandoned, cutoff_secs = REPLAY_WINDOW_SECS, "Q55 放弃窗口外未处理消息");
    }
    let mut stmt = conn.prepare(
        "SELECT msg_id, chat_id, chat_type, sender_pid, text, at_me, has_image, reply_to, ts
         FROM messages
         WHERE processed_at IS NULL AND sender_pid != 'self' AND ts >= ?1
         ORDER BY ts ASC, msg_id ASC",
    )?;
    let rows = stmt.query_map(params![cutoff], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?,
            r.get::<_, Option<i64>>(7)?,
            r.get::<_, i64>(8)?,
        ))
    })?;
    let mut count = 0usize;
    for row in rows {
        let (msg_id, chat_id, chat_type, sender_pid, text, at_me, has_image, reply_to, ts) =
            match row {
                Ok(r) => r,
                Err(e) => {
                    debug!(error = %e, "回放读取行失败，跳过");
                    continue;
                }
            };
        let text = text.unwrap_or_default();
        // Q55 裁决：回放跳过 /image 直派命令（media 副作用大）
        if crate::tools::media::command::match_image_command(&text).is_some() {
            mark_processed(&deps.db_path, msg_id);
            debug!(msg_id, "回放跳过 /image 命令（直接标记已处理）");
            continue;
        }
        let payload = MessageReceivedPayload {
            msg_id,
            chat_id,
            chat_type,
            sender_pid,
            text,
            at_me: at_me != 0,
            has_image: has_image != 0,
            reply_to,
            sender_bot: false, // 已落库的非 self 消息不再重判 R2（回放即信任当时 ingest 判定）
            image_urls: Vec::new(), // 不回放偷图流程（image_urls 不落 messages）
            ts,
        };
        handle(deps, &payload).await;
        count += 1;
    }
    if count > 0 {
        info!(count, "Q55 恢复消费完成");
    }
    Ok(count)
}

pub fn spawn_pipeline(deps: PipelineDeps) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = deps.bus.subscribe();
        if !decision_ready(&deps.llm) {
            warn!("Decision 角色未配置（providers.toml）：配置前消息过 Prefilter 后直接 ignore，管线照常运行");
        }
        info!("Decision 管线已启动（Q52 per-chat 并发 + Q54 10s 窗口聚合）");
        let deps = Arc::new(deps);
        let mut lanes: std::collections::HashMap<String, tokio::sync::mpsc::Sender<MessageReceivedPayload>> =
            std::collections::HashMap::new();

        // Q54 窗口聚合器:
        //   固定 10s 窗口,同 chat 普通消息追加,不重复开窗;anchor 固定为第一条;
        //   窗口到期 fire → anchor 进 per-chat worker 触发一次 decide;
        //   others(Q54 窗口内累积的 N 条)在 decide 中通过 build_context 的最近消息
        //   SQL 拉取自然进入上下文(消息早被 ingest 落库在本进程,时机窗口内已写入);
        //   others 需立刻 mark_processed,Q55 重启不再回放(窗口 fire 后 anchor 替身已代表整个 batch)。
        let deps_for_fire = Arc::clone(&deps);
        let lanes_for_fire: Arc<
            RwLock<std::collections::HashMap<String, tokio::sync::mpsc::Sender<MessageReceivedPayload>>>,
        > = Arc::new(RwLock::new(std::collections::HashMap::new()));
        let lanes_for_fire_clone = Arc::clone(&lanes_for_fire);
        let on_fire: crate::window::OnFire = Arc::new(move |batch: crate::window::WindowBatch| {
            let deps = Arc::clone(&deps_for_fire);
            let lanes_clone = Arc::clone(&lanes_for_fire_clone);
            Box::pin(async move {
                // others 立刻回写 processed_at(否则 Q55 重启会把它们当未处理回放,雪崩)
                for om in &batch.others {
                    mark_processed(&deps.db_path, om.msg_id);
                }

                // anchor 进 per-chat worker(保序不丢);worker 拿不到 lane 则同步保底
                let chat_id = batch.anchor.chat_id.clone();
                let tx_opt = lanes_clone.read().unwrap().get(&chat_id).cloned();
                match tx_opt {
                    Some(tx) => {
                        if let Err(e) = tx.try_send(batch.anchor.clone()) {
                            debug!(chat_id = %chat_id, error = %e, "Q54 anchor 入队失败,同步保底");
                            handle(&deps, &batch.anchor).await;
                        }
                    }
                    None => {
                        debug!(chat_id = %chat_id, "Q54 anchor 找不到 lane(不该发生),同步保底");
                        handle(&deps, &batch.anchor).await;
                    }
                }
            })
        });
        let mut aggregator = crate::window::WindowAggregator::new(crate::window::DEFAULT_WINDOW_SECS);

        loop {
            match rx.recv().await {
                Ok(Event::MessageReceived(m)) => {
                    // /image 等命令走原 dispatcher→worker 直派,不进窗口(特效速报)
                    let is_media_cmd = deps.media_ctx.is_some()
                        && crate::tools::media::command::match_image_command(&m.text).is_some();

                    if is_media_cmd {
                        let lane = lanes.entry(m.chat_id.clone()).or_insert_with(|| {
                            let cap = *deps.per_chat_cap.read().unwrap();
                            let (tx, mut lane_rx) =
                                tokio::sync::mpsc::channel::<MessageReceivedPayload>(cap.max(1));
                            let deps2 = Arc::clone(&deps);
                            let chat = m.chat_id.clone();
                            tokio::spawn(async move {
                                while let Some(mm) = lane_rx.recv().await {
                                    handle(&deps2, &mm).await;
                                }
                                debug!(chat_id = %chat, "chat worker 退出(dispatcher 关闭)");
                            });
                            tx
                        });
                        lanes_for_fire.write().unwrap().insert(m.chat_id.clone(), lane.clone());
                        if let Err(e) = lane.try_send(m.clone()) {
                            debug!(chat_id = %m.chat_id, error = %e, "per-chat 队列满,同步处理保底");
                            handle(&deps, &m).await;
                        }
                        continue;
                    }

                    // 非命令路径:对该 chat 首条建立 lane 并 record map
                    let lane = lanes.entry(m.chat_id.clone()).or_insert_with(|| {
                        let cap = *deps.per_chat_cap.read().unwrap();
                        let (tx, mut lane_rx) =
                            tokio::sync::mpsc::channel::<MessageReceivedPayload>(cap.max(1));
                        let deps2 = Arc::clone(&deps);
                        let chat = m.chat_id.clone();
                        tokio::spawn(async move {
                            while let Some(mm) = lane_rx.recv().await {
                                handle(&deps2, &mm).await;
                            }
                            debug!(chat_id = %chat, "chat worker 退出(dispatcher 关闭)");
                        });
                        tx
                    });
                    lanes_for_fire.write().unwrap().insert(m.chat_id.clone(), lane.clone());

                    // 进 Q54 聚合器,窗口到期由 on_fire 把 anchor 推给 worker(见上)
                    aggregator.feed(m, on_fire.clone());
                }
                Ok(_) => {} // 其余事件类型本管线不消费
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "管线消费滞后，跳过旧事件");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    info!("管线退出：事件总线已关闭");
                    break;
                }
            }
        }
    })
}

fn decision_ready(llm: &SharedLlm) -> bool {
    llm.read()
        .unwrap()
        .as_ref()
        .map(|g| g.role(Role::Decision).is_some())
        .unwrap_or(false)
}

/// reply_engine::route_of 的转发（chat_type/target 解析）
fn reply_engine_route(m: &MessageReceivedPayload) -> anyhow::Result<(reply_engine::ChatType, u64)> {
    crate::reply_engine::route_of(m).map(|(ct, target, _)| (ct, target))
}

/// Q55 包装层：处理完毕无条件回写 messages.processed_at（含 Prefilter Drop 分支——
/// 「处理过」不等于「回复过」；重启回放只扫进程崩溃前来不及进 handle 的消息）。
async fn handle(deps: &PipelineDeps, m: &MessageReceivedPayload) {
    handle_inner(deps, m).await;
    mark_processed(&deps.db_path, m.msg_id);
}

fn mark_processed(db_path: &std::path::Path, msg_id: i64) {
    if msg_id <= 0 {
        return; // 回放路径构造的合成消息没有正 msg_id
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let res = crate::db::connect(db_path).and_then(|c| {
        c.execute(
            "UPDATE messages SET processed_at = ?1 WHERE msg_id = ?2",
            rusqlite::params![now, msg_id],
        )
        .map_err(anyhow::Error::from)
    });
    if let Err(e) = res {
        debug!(msg_id, error = %e, "processed_at 回写失败（下轮重启会回放该条，幂等）");
    }
}

async fn handle_inner(deps: &PipelineDeps, m: &MessageReceivedPayload) {
    // Q008=C：/image /画 显式命令直派，先于 Prefilter 与 Decision（Command deterministic path）。
    // 显式命令不参与节流与成本闸，且独立于 LLM 角色可用性——用户在配 LLM 之前也能用 media。
    if let (Some(media_ctx), Some(body)) = (
        deps.media_ctx.as_ref(),
        crate::tools::media::command::match_image_command(&m.text),
    ) {
        crate::tools::media::command::handle_image_command(media_ctx, m, body).await;
        return;
    }

    let q = deps.self_qq.load(Ordering::Relaxed);
    let self_pid = (q != 0).then(|| format!("p_{q}"));
    let pf = *deps.prefilter.read().unwrap(); // 热应用：每条取当前阈值
    match crate::prefilter::check(&pf, self_pid.as_deref(), &deps.self_ids, &deps.db_path, m) {
        Verdict::Drop(rule) => debug!(rule, chat_id = %m.chat_id, "Prefilter 丢弃"),
        Verdict::Pass => {
            let gw = match deps.llm.read().unwrap().clone() {
                Some(g) if g.role(Role::Decision).is_some() => g,
                _ => {
                    debug!(chat_id = %m.chat_id, "Decision 角色不可用，本条 ignore");
                    return;
                }
            };
            // 成本闸热应用：prefilter 槽 → gateway（每条对齐一次，换槽/调参即生效）
            gw.set_cost_per_min(pf.decision_cost_per_min.max(1) as usize);
            let outcome = decision::decide(
                &deps.db_path,
                &gw,
                &deps.bus,
                &deps.mood,
                m,
                deps.skill_registry.as_ref(),
            )
            .await;

            // action=invoke_skill：Skill 执行（Q-S02）。Skill 内部默认渲染模板；
            // 插件可以 override invoke 自己调 chat LLM。Skill 输出经 bot_chat Bubbleizer 发送。
            if outcome.output.action == DecisionAction::InvokeSkill {
                let skill_name = outcome
                    .output
                    .skill_name
                    .clone()
                    .unwrap_or_default();
                let reg = match deps.skill_registry.as_ref() {
                    Some(r) => r,
                    None => {
                        warn!(chat_id = %m.chat_id, skill = %skill_name, "SkillRegistry 未注入，invoke_skill 跳过");
                        return;
                    }
                };
                let skill = match reg.get(&skill_name) {
                    Some(s) => s,
                    None => {
                        warn!(chat_id = %m.chat_id, skill = %skill_name, "invoke_skill 所选 Skill 未注册（decision 已过滤，理论上到不了这）");
                        return;
                    }
                };
                let engine = match deps.reply.as_ref() {
                    Some(e) => e,
                    None => {
                        debug!(chat_id = %m.chat_id, "回复引擎未接入，invoke_skill 跳过");
                        return;
                    }
                };
                let (chat_type, target) = match reply_engine_route(m) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(error = %e, chat_id = %m.chat_id, "invoke_skill 路由失败");
                        return;
                    }
                };
                let ctx = crate::tools::ToolCtx {
                    task_id: None,
                    chat_id: m.chat_id.clone(),
                    chat_type: m.chat_type.clone(),
                    sender_pid: m.sender_pid.clone(),
                    locale: Some("zh-CN".into()),
                };
                let mut slots = outcome
                    .output
                    .skill_slots
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({}));
                // 把消息文本保底塞进 user_text 槽（Skill 模板最常用槽位）
                if slots.get("user_text").is_none() {
                    slots["user_text"] = serde_json::Value::String(m.text.clone());
                }
                // 把 sender 名字等拼进 context 槽
                if slots.get("context").is_none() {
                    slots["context"] = serde_json::json!(format!(
                        "chat_id={} sender={} chat_type={}",
                        m.chat_id, m.sender_pid, m.chat_type
                    ));
                }
                match skill.invoke(&ctx, slots).await {
                    Ok(out) => {
                        info!(
                            chat_id = %m.chat_id,
                            skill = %skill_name,
                            summary = %out.summary,
                            "invoke_skill 执行完成"
                        );
                        // Skill 的 rendered 内容按普通回复发出（走 bot_chat 再走 Bubbleizer）
                        // Phase 2 简化：直接发送 out.data.rendered，不再让 bot_chat 二次包装；
                        // RoleHint 与 persona 化后续由 Bot Chat 自身处理
                        let rendered = out
                            .data
                            .get("rendered")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| out.summary.clone());
                        // Skill 输出包成单 Bubble（无 at/无 meme），走 Bubbles 路径享受打字延时与作废核对
                        engine.enqueue(reply_engine::ReplyJob {
                            chat_id: m.chat_id.clone(),
                            chat_type,
                            target,
                            anchor_msg_id: 0,
                            mention: false,
                            mention_qq: None,
                            kind: reply_engine::JobKind::Bubbles(vec![reply_engine::Bubble {
                                text: rendered,
                                at: false,
                                meme: None,
                            }]),
                        });
                        deps.bus.publish(Event::BubbleSent(BubbleSentPayload {
                            chat_id: m.chat_id.clone(),
                            bubble_index: 0,
                            total: 1,
                            ok: true,
                            note: Some(format!("skill {} 调用完成", skill_name)),
                        }));
                    }
                    Err(e) => {
                        warn!(chat_id = %m.chat_id, skill = %skill_name, error = %e, "invoke_skill 执行失败");
                        deps.bus.publish(Event::BubbleSent(BubbleSentPayload {
                            chat_id: m.chat_id.clone(),
                            bubble_index: 0,
                            total: 0,
                            ok: false,
                            note: Some(format!("skill {skill_name} 失败: {e}")),
                        }));
                    }
                }
                return;
            }
            if outcome.output.action == DecisionAction::SendMeme {
                // send_meme：抽图→发送队列（直接发图，与普通回复同队列保序）；库空记事件不吵
                let cat = outcome
                    .output
                    .meme_type
                    .clone()
                    .unwrap_or_else(|| "misc".into());
                match deps.reply.as_ref() {
                    Some(engine) => match reply_engine_route(m) {
                        Ok((chat_type, target)) => {
                            let picked = meme::pick(&deps.db_path, &deps.memes_dir, &cat)
                                .or_else(|| meme::pick(&deps.db_path, &deps.memes_dir, "misc"));
                            match picked {
                                Some(path) => {
                                    info!(chat_id = %m.chat_id, category = %cat, "send_meme 抽图入队");
                                    engine.enqueue_image(m.chat_id.clone(), chat_type, target, path);
                                }
                                None => {
                                    debug!(chat_id = %m.chat_id, category = %cat, "meme 库空，跳过发图");
                                    deps.bus.publish(Event::BubbleSent(BubbleSentPayload {
                                        chat_id: m.chat_id.clone(),
                                        bubble_index: 0,
                                        total: 0,
                                        ok: false,
                                        note: Some(format!("meme 库空（类别 {cat}）")),
                                    }));
                                }
                            }
                        }
                        Err(e) => warn!(error = %e, "send_meme 路由失败"),
                    },
                    None => debug!(chat_id = %m.chat_id, "回复引擎未接入，send_meme 跳过"),
                }
                return;
            }
            // action=start_task：建 tasks 行 + TaskCreated 事件（agent::create_task 落两笔）；
            // spawn_runner 订阅事件后立即起执行协程。静默：不发群消息（Q-A03）
            if outcome.output.action == DecisionAction::StartTask {
                let goal = outcome
                    .output
                    .task_goal
                    .clone()
                    .unwrap_or_else(|| m.text.chars().take(120).collect());
                if let Err(e) = crate::agent::create_task(
                    &deps.db_path,
                    &deps.bus,
                    &m.chat_id,
                    &m.sender_pid,
                    &goal,
                ) {
                    warn!(chat_id = %m.chat_id, error = %e, "start_task 落地失败");
                } else {
                    info!(chat_id = %m.chat_id, goal = %goal, "start_task 已建仓");
                }
                return;
            }
            if outcome.output.action != DecisionAction::Reply {
                return; // ignore 仅由 DecisionMade 事件记录
            }
            // action=reply → bot_chat → Bubbleizer → 发送队列
            let bot_chat_ready = gw.role(Role::BotChat).is_some();
            match (&deps.reply, bot_chat_ready) {
                (Some(engine), true) => {
                    let ctx_cfg = *deps.ctx_cfg.read().unwrap();
                    let reply_cfg = *deps.reply_cfg.read().unwrap();
                    if let Err(e) = reply_engine::prepare_and_enqueue(
                        engine, &deps.db_path, &gw, &deps.mood, m, &outcome.output, &ctx_cfg, &reply_cfg,
                    )
                    .await
                    {
                        warn!(chat_id = %m.chat_id, error = %e, "bot_chat 回复准备失败，跳过");
                        deps.bus.publish(Event::BubbleSent(BubbleSentPayload {
                            chat_id: m.chat_id.clone(),
                            bubble_index: 0,
                            total: 0,
                            ok: false,
                            note: Some(format!("bot_chat 调用失败: {e}")),
                        }));
                    }
                }
                _ => {
                    debug!(chat_id = %m.chat_id, "bot_chat 角色未配置或回复引擎未接入，reply 跳过");
                    deps.bus.publish(Event::BubbleSent(BubbleSentPayload {
                        chat_id: m.chat_id.clone(),
                        bubble_index: 0,
                        total: 0,
                        ok: false,
                        note: Some("bot_chat 角色未配置或回复引擎未接入，跳过回复".into()),
                    }));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn temp_db() -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128
            + (std::process::id() as u128) << 16;
        let dir = std::env::temp_dir().join(format!("yt-bot-test-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("yuantuan.db");
        let mut conn = crate::db::connect(&db).unwrap();
        crate::db::migrate(&mut conn).unwrap();
        db
    }

    /// Q55：processed_at 回写——任意 msg_id 调用后该行不再 NULL
    #[test]
    fn mark_processed_writes_ts() {
        let db = temp_db();
        let conn = crate::db::connect(&db).unwrap();
        let now = 1000i64;
        conn.execute(
            "INSERT INTO persons(person_id, display_name, first_seen, last_seen) VALUES ('p_1','甲',?1,?1)",
            params![now],
        ).unwrap();
        conn.execute(
            "INSERT INTO messages(chat_id, chat_type, sender_pid, text, ts) VALUES ('c1','group','p_1','hi',?1)",
            params![now],
        ).unwrap();
        let msg_id = conn.last_insert_rowid();
        drop(conn);

        mark_processed(&db, msg_id);

        let conn = crate::db::connect(&db).unwrap();
        let processed: Option<i64> = conn
            .query_row("SELECT processed_at FROM messages WHERE msg_id = ?1", params![msg_id], |r| r.get(0))
            .unwrap();
        assert!(processed.is_some(), "processed_at 应被回写");
    }

    /// Q55：回放扫描——self 消息不进回放集；/image 命令跳过并标记
    #[tokio::test]
    async fn replay_skips_self_and_image_command() {
        let db = temp_db();
        // 用当前时间避免被 1h 回放窗口丢弃(REPLAY_WINDOW_SECS)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
            - 60; // 一分钟前(窗口内)
        {
            let conn = crate::db::connect(&db).unwrap();
            conn.execute(
                "INSERT INTO persons(person_id, display_name, first_seen, last_seen) VALUES ('self','云团',?1,?1),('p_1','甲',?1,?1)",
                params![now],
            ).unwrap();
            // self 消息（应被回放扫描排除）
            conn.execute(
                "INSERT INTO messages(chat_id, chat_type, sender_pid, text, ts) VALUES ('c1','group','self','我自己说的',?1)",
                params![now],
            ).unwrap();
            // /image 直派命令（回放应跳过且直接标记已处理）
            conn.execute(
                "INSERT INTO messages(chat_id, chat_type, sender_pid, text, ts) VALUES ('c1','group','p_1','/image 一只猫',?1)",
                params![now + 1],
            ).unwrap();
        }

        // 构造最简 PipelineDeps：无 LLM、无 reply、无 media_ctx、无 skills
        let bus = crate::event::EventBus::default();
        let llm_slot: SharedLlm = std::sync::Arc::new(std::sync::RwLock::new(None));
        let prefilter_slot: SharedPrefilter = std::sync::Arc::new(std::sync::RwLock::new(
            crate::prefilter::Config::default(),
        ));
        let reply_slot: crate::reply_engine::SharedReplyCfg = std::sync::Arc::new(
            std::sync::RwLock::new(crate::reply_engine::ReplyCfg::default()),
        );
        let ctx_slot: crate::context_builder::SharedContextCfg = std::sync::Arc::new(
            std::sync::RwLock::new(crate::context_builder::ContextCfg::default()),
        );
        let deps = PipelineDeps {
            bus: bus.clone(),
            db_path: db.clone(),
            llm: llm_slot,
            self_qq: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            self_ids: crate::prefilter::SelfMsgIds::default(),
            mood: crate::state::MoodState::default(),
            prefilter: prefilter_slot,
            reply: None,
            reply_cfg: reply_slot,
            ctx_cfg: ctx_slot,
            memes_dir: std::path::PathBuf::from("data/memes"),
            media_ctx: None,
            skill_registry: None,
            per_chat_cap: std::sync::Arc::new(std::sync::RwLock::new(32)),
        };

        let n = replay_pending(&deps).await.unwrap();
        assert_eq!(n, 0, "self 与 /image 都不应回放（回放计数为 0）");

        // /image 那条也应被标记为已处理
        let conn = crate::db::connect(&db).unwrap();
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE processed_at IS NULL AND sender_pid != 'self'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0, "回放后不应剩余未处理非 self 消息");
    }

    /// Q52 信号量上限常量为 4（runtime-design 裁决值）
    #[test]
    fn q53_concurrency_limit_is_four() {
        assert_eq!(crate::llm::DEFAULT_LLM_CONCURRENCY, 4);
    }

    /// Q55+1h 窗口:远古未处理消息被放弃(标记为已处理),不回放
    #[tokio::test]
    async fn replay_abandons_stale_pending() {
        let db = temp_db();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        {
            let conn = crate::db::connect(&db).unwrap();
            conn.execute(
                "INSERT INTO persons(person_id, display_name, first_seen, last_seen) VALUES ('p_1','甲',?1,?1)",
                params![now],
            ).unwrap();
            // 远古:2 小时前的未处理消息
            let stale = now - 7200;
            conn.execute(
                "INSERT INTO messages(chat_id, chat_type, sender_pid, text, ts) VALUES ('c1','group','p_1','远古消息',?1)",
                params![stale],
            ).unwrap();
        }
        let deps = PipelineDeps {
            bus: crate::event::EventBus::default(),
            db_path: db.clone(),
            llm: std::sync::Arc::new(std::sync::RwLock::new(None)),
            self_qq: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            self_ids: crate::prefilter::SelfMsgIds::default(),
            mood: crate::state::MoodState::default(),
            prefilter: std::sync::Arc::new(std::sync::RwLock::new(crate::prefilter::Config::default())),
            reply: None,
            reply_cfg: std::sync::Arc::new(std::sync::RwLock::new(crate::reply_engine::ReplyCfg::default())),
            ctx_cfg: std::sync::Arc::new(std::sync::RwLock::new(crate::context_builder::ContextCfg::default())),
            memes_dir: std::path::PathBuf::from("data/memes"),
            media_ctx: None,
            skill_registry: None,
            per_chat_cap: std::sync::Arc::new(std::sync::RwLock::new(32)),
        };
        let n = replay_pending(&deps).await.unwrap();
        assert_eq!(n, 0, "远古消息应被放弃,不回放");
        let conn = crate::db::connect(&db).unwrap();
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE processed_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0, "远古未处理应被 UPDATE 标记已处理");
    }
}
