//! Event System（架构文档十一章）：tokio broadcast 单总线 + 强类型事件枚举。
//! V1 订阅者：Decision 管线（订 MessageReceived）、tracer（全订落表）、WebUI 实时推送（全订）。
//! 落库：全部事件写入 events 表（轮转保留 7 天，表结构见 docs/reference/data-model.md）。

use rusqlite::params;
use serde::Serialize;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

/// MessageReceived 载荷（字段与 messages 表对应 + Prefilter 判定所需标志）
#[derive(Debug, Clone, Serialize)]
pub struct MessageReceivedPayload {
    pub msg_id: i64,
    pub chat_id: String,
    pub chat_type: String,
    pub sender_pid: String,
    pub text: String,
    pub at_me: bool,
    pub has_image: bool,
    /// 被引消息的 NapCat message_id 原值（R4 判定原料）
    pub reply_to: Option<i64>,
    /// sender 带 bot 标记或匿名（R2 判定原料）
    pub sender_bot: bool,
    /// 图片段下载 URL（轻量留在事件层，不落 messages；偷表情包流程消费）
    pub image_urls: Vec<String>,
    pub ts: i64,
}

/// DecisionMade 载荷（trace 页数据源：输入摘要 + 输出 + 耗时 + 重试次数）
#[derive(Debug, Clone, Serialize)]
pub struct DecisionMadePayload {
    pub chat_id: String,
    pub sender_pid: String,
    /// 触发消息正文截断（≤100 字）
    pub text: String,
    pub action: String,
    pub mood: String,
    pub reason: String,
    /// Schema 两次校验均失败后的兜底 ignore
    pub fallback: bool,
    pub retries: u32,
    pub elapsed_ms: u64,
}

/// BubbleSent 载荷（每泡一条：ok=false 时 note 说明原因——发送失败跳过 / 角色未配置等）
#[derive(Debug, Clone, Serialize)]
pub struct BubbleSentPayload {
    pub chat_id: String,
    pub bubble_index: usize,
    pub total: usize,
    pub ok: bool,
    pub note: Option<String>,
}

/// 单 chat 归纳结果（ConsolidationDone 明细行）
#[derive(Debug, Clone, Serialize)]
pub struct ChatConsolidationOutcome {
    pub chat_id: String,
    pub msg_start: i64,
    pub msg_end: i64,
    pub mention_pairs: usize,
    pub facts_written: usize,
    /// ok | llm_failed | no_new_today | skipped_done
    pub status: String,
}

/// ConsolidationDone 载荷（各 chat 统计 + 总耗时，trace 页数据源）
#[derive(Debug, Clone, Serialize)]
pub struct ConsolidationDonePayload {
    pub date: String,
    pub chats: Vec<ChatConsolidationOutcome>,
    pub elapsed_ms: u64,
}

/// 任务生命周期载荷（TaskCreated / TaskFinished 共用；面板 Tasks 页与 runner 都用 task_id 索引）
#[derive(Debug, Clone, Serialize)]
pub struct TaskLifecyclePayload {
    pub task_id: String,
    pub chat_id: String,
    pub goal: String,
    /// TaskFinished 时的最终状态：finished | failed
    pub final_state: Option<String>,
    pub error: Option<String>,
}

/// 事件清单（架构十一章终稿）；本阶段实装 MessageReceived / DecisionMade / BubbleSent /
/// ReplyInterrupted / ConsolidationDone，其余为变体占位
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum Event {
    MessageReceived(MessageReceivedPayload),
    MessageSent,
    BubbleSent(BubbleSentPayload),
    ReplyInterrupted,
    DecisionMade(DecisionMadePayload),
    TaskCreated(TaskLifecyclePayload),
    TaskStepDone,
    TaskFinished(TaskLifecyclePayload),
    MemoryWritten,
    RelationshipEventAppended,
    MoodChanged,
    MemberJoined,
    MemberLeft,
    ConsolidationDone(ConsolidationDonePayload),
    ConfigReloaded,
    PersonalityVersionChanged,
}

impl Event {
    pub fn kind(&self) -> &'static str {
        match self {
            Event::MessageReceived(_) => "MessageReceived",
            Event::MessageSent => "MessageSent",
            Event::BubbleSent(_) => "BubbleSent",
            Event::ReplyInterrupted => "ReplyInterrupted",
            Event::DecisionMade(_) => "DecisionMade",
            Event::TaskCreated(_) => "TaskCreated",
            Event::TaskStepDone => "TaskStepDone",
            Event::TaskFinished(_) => "TaskFinished",
            Event::MemoryWritten => "MemoryWritten",
            Event::RelationshipEventAppended => "RelationshipEventAppended",
            Event::MoodChanged => "MoodChanged",
            Event::MemberJoined => "MemberJoined",
            Event::MemberLeft => "MemberLeft",
            Event::ConsolidationDone(_) => "ConsolidationDone",
            Event::ConfigReloaded => "ConfigReloaded",
            Event::PersonalityVersionChanged => "PersonalityVersionChanged",
        }
    }

    pub fn payload_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }
}

/// tokio broadcast 单总线；无订阅者时事件直接丢弃
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn publish(&self, event: Event) {
        if self.tx.send(event).is_err() {
            // 无订阅者属正常情况（如早期启动阶段），不视为错误
        }
    }
}

/// tracer：全订事件并按 events(kind, payload, ts) 落库。持自有连接（WAL 多读单写）。
pub fn spawn_tracer(bus: &EventBus, db_path: PathBuf) -> tokio::task::JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        let conn = match crate::db::connect(&db_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "tracer 打开数据库失败，退出");
                return;
            }
        };
        tracing::info!("tracer 已启动（全事件落 events 表）");
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let ts = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    if let Err(e) = conn.execute(
                        "INSERT INTO events(kind, payload, ts) VALUES (?1, ?2, ?3)",
                        params![ev.kind(), ev.payload_json(), ts],
                    ) {
                        tracing::warn!(error = %e, "tracer 写入 events 失败");
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "tracer 消费滞后，跳过旧事件");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::info!("tracer 退出：事件总线已关闭");
                    break;
                }
            }
        }
    })
}
