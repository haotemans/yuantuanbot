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
}

/// 订阅 Event Bus 的 Decision 管线；主动插话与被动回复走同一条管线
pub fn spawn_pipeline(deps: PipelineDeps) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = deps.bus.subscribe();
        if !decision_ready(&deps.llm) {
            warn!("Decision 角色未配置（providers.toml）：配置前消息过 Prefilter 后直接 ignore，管线照常运行");
        }
        info!("Decision 管线已启动");
        loop {
            match rx.recv().await {
                Ok(Event::MessageReceived(m)) => handle(&deps, &m).await,
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

async fn handle(deps: &PipelineDeps, m: &MessageReceivedPayload) {
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
