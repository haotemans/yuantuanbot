//! Bot 系统：聊天管线——订阅 MessageReceived → Prefilter →（放行）→ 成本闸（llm.chat 内）→
//! 组上下文 → Decision → 副作用 + 事件；action=reply 再接 bot_chat → Bubbleizer → 发送队列。
//! 铁律：core 不依赖具体协议端；self_qq 以共享原子单元注入，发送以 SendFn 闭包注入。

use crate::decision::{self, DecisionAction};
use crate::event::{BubbleSentPayload, Event, EventBus, MessageReceivedPayload};
use crate::llm::{LlmGateway, Role};
use crate::meme;
use crate::prefilter::{SelfMsgIds, Verdict};
use crate::reply_engine::{self, EngineHandle};
use crate::state::MoodState;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

pub struct PipelineDeps {
    pub bus: EventBus,
    pub db_path: PathBuf,
    /// providers.toml 未配置 Decision 角色时可为 None 或角色缺失：管线不崩，直接 ignore
    pub llm: Option<Arc<LlmGateway>>,
    pub self_qq: Arc<AtomicU64>,
    pub self_ids: SelfMsgIds,
    pub mood: MoodState,
    pub prefilter: crate::prefilter::Config,
    /// 回复引擎；None 时 reply/send_meme 只记事件跳过（如 adapter 未接入）
    pub reply: Option<EngineHandle>,
    /// data/memes 根目录（meme 抽图）
    pub memes_dir: PathBuf,
}

/// 订阅 Event Bus 的 Decision 管线；主动插话与被动回复走同一条管线
pub fn spawn_pipeline(deps: PipelineDeps) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = deps.bus.subscribe();
        let decision_ready = deps
            .llm
            .as_ref()
            .map(|g| g.role(Role::Decision).is_some())
            .unwrap_or(false);
        if !decision_ready {
            warn!("Decision 角色未配置（providers.toml）：消息过 Prefilter 后直接 ignore，管线照常运行");
        }
        info!("Decision 管线已启动");
        loop {
            match rx.recv().await {
                Ok(Event::MessageReceived(m)) => handle(&deps, decision_ready, &m).await,
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

/// reply_engine::route_of 的转发（chat_type/target 解析）
fn reply_engine_route(m: &MessageReceivedPayload) -> anyhow::Result<(reply_engine::ChatType, u64)> {
    crate::reply_engine::route_of(m).map(|(ct, target, _)| (ct, target))
}

async fn handle(deps: &PipelineDeps, decision_ready: bool, m: &MessageReceivedPayload) {
    let q = deps.self_qq.load(Ordering::Relaxed);
    let self_pid = (q != 0).then(|| format!("p_{q}"));
    match crate::prefilter::check(
        &deps.prefilter,
        self_pid.as_deref(),
        &deps.self_ids,
        &deps.db_path,
        m,
    ) {
        Verdict::Drop(rule) => debug!(rule, chat_id = %m.chat_id, "Prefilter 丢弃"),
        Verdict::Pass => {
            let Some(gw) = deps.llm.as_deref().filter(|_| decision_ready) else {
                debug!(chat_id = %m.chat_id, "Decision 角色不可用，本条 ignore");
                return;
            };
            let outcome = decision::decide(&deps.db_path, gw, &deps.bus, &deps.mood, m).await;
            if outcome.output.action == DecisionAction::SendMeme {
                // send_meme：抽图→发送队列（直接发图，与普通回复同队列保序）；库空记事件不吵
                let cat = outcome
                    .output
                    .meme_type
                    .clone()
                    .unwrap_or_else(|| "misc".into());
                match deps.reply.as_ref() {
                    Some(engine) => {
                        let routed = reply_engine_route(m);
                        match routed {
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
                        }
                    }
                    None => debug!(chat_id = %m.chat_id, "回复引擎未接入，send_meme 跳过"),
                }
                return;
            }
            if outcome.output.action != DecisionAction::Reply {
                return; // ignore / start_task(单6) 仅由 DecisionMade 事件记录
            }
            // action=reply → bot_chat → Bubbleizer → 发送队列
            let bot_chat_ready = gw.role(Role::BotChat).is_some();
            match (&deps.reply, bot_chat_ready) {
                (Some(engine), true) => {
                    if let Err(e) = reply_engine::prepare_and_enqueue(
                        engine, &deps.db_path, gw, &deps.mood, m, &outcome.output,
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
