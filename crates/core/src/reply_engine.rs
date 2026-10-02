//! 回复形态引擎（架构文档十四章机制）：
//! 发送链路 bot_chat → Bubbleizer → per-chat 发送队列（串行异步） → 适配层发送函数。
//! - Bubbleizer：‖ 分泡、::at/::meme 指令行、3 泡封顶（超出并最后泡）、无分隔符单泡、
//!   单泡超 500 字按标点机械切、解析失败整段单泡
//! - 延时模拟：首泡 300~800ms；后续 clamp(0.6s+字数×40ms±30%, 0.8s, 4s)；整段总预算 8s，超预算不再延时
//! - 异步作废：每泡发送前核对该 chat 最新外来 msg_id，变了→作废剩余 + ReplyInterrupted 事件
//! - 单泡失败退避重试 1 次，仍失败跳过该泡记 BubbleSent(ok=false)，回复整体不失踪
//! - 发送成功的泡落 messages（sender_pid='self'）保持会话连贯；napcat message_id 记 SelfMsgIds
//!
//! ::meme 说明：指令已解析；实际从库抽图发送属于施工单 7。本单选最不吵的方案——
//! 纯 ::meme 泡跳过不发送（仅 debug 日志），混合泡只发文字部分。TODO(单7)：接 meme_library 抽图发送。
//! 系数/泡顶/节流等全部数值目前走代码默认，TODO(热配单)：入 config。

use crate::decision::DecisionOutput;
use crate::event::{BubbleSentPayload, Event, EventBus, MessageReceivedPayload};
use crate::prefilter::SelfMsgIds;
use crate::state::{MoodState, MoodValue};
use crate::{context_builder, llm, meme};
use anyhow::{anyhow, Context, Result};
use rand::Rng;
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

// ---------- 发送抽象（core 不依赖适配层：由装配侧注入闭包） ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatType {
    Group,
    Private,
}

#[derive(Debug, Clone)]
pub struct SendRequest {
    pub chat_type: ChatType,
    /// 群号或私聊对方 QQ 号
    pub target: u64,
    /// OneBot 段数组
    pub segments: Value,
}

pub type SendFn = Arc<
    dyn Fn(SendRequest) -> Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send>>
        + Send
        + Sync,
>;

// ---------- Bubbleizer ----------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bubble {
    pub text: String,
    pub at: bool,
    pub meme: Option<String>,
}

/// 解析 bot_chat 定格式输出为气泡序列（出厂参数：单泡 500 字上限、3 泡封顶）
pub fn bubbleize(raw: &str) -> Vec<Bubble> {
    bubbleize_with(raw, 500, 3)
}

/// 带参数的 bubbleize：char_cap=单泡字数上限（超出按标点机械切），bubble_cap=泡数封顶
pub fn bubbleize_with(raw: &str, char_cap: usize, bubble_cap: usize) -> Vec<Bubble> {
    let char_cap = char_cap.max(20);
    let bubble_cap = bubble_cap.max(1);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let segments: Vec<&str> = trimmed.split('‖').collect();
    let mut bubbles: Vec<Bubble> = Vec::new();
    for seg in segments {
        let mut text_lines: Vec<&str> = Vec::new();
        let mut at = false;
        let mut meme: Option<String> = None;
        for line in seg.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("::") {
                if rest == "at" {
                    at = true;
                } else if let Some(cat) = rest.strip_prefix("meme").map(str::trim) {
                    if !cat.is_empty() {
                        meme = Some(cat.to_string());
                    }
                }
                // 未知指令行忽略，不进正文
                continue;
            }
            if !l.is_empty() {
                text_lines.push(l);
            }
        }
        let text = text_lines.join("\n");
        if text.is_empty() && meme.is_none() {
            continue;
        }
        bubbles.push(Bubble { text, at, meme });
    }
    // 解析失败（全部段都被滤空）→ 整段单泡兜底
    if bubbles.is_empty() {
        return vec![Bubble {
            text: trimmed.to_string(),
            at: false,
            meme: None,
        }];
    }
    // 单泡超 char_cap 字 → 按标点机械切
    let mut cut: Vec<Bubble> = Vec::new();
    for b in bubbles {
        if b.text.chars().count() > char_cap {
            cut.extend(mechanical_split(&b, char_cap));
        } else {
            cut.push(b);
        }
    }
    // bubble_cap 泡封顶（封顶不是配额）：保留前 bubble_cap 泡，其余并入最后一泡
    if cut.len() > bubble_cap {
        let tail: Vec<Bubble> = cut.split_off(bubble_cap);
        let last = &mut cut[bubble_cap - 1];
        let merged_text = tail
            .iter()
            .map(|b| b.text.as_str())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if !merged_text.is_empty() {
            if !last.text.is_empty() {
                last.text.push('\n');
            }
            last.text.push_str(&merged_text);
        }
        last.at = last.at || tail.iter().any(|b| b.at);
        if last.meme.is_none() {
            last.meme = tail.into_iter().find_map(|b| b.meme);
        }
    }
    cut
}

fn mechanical_split(b: &Bubble, max: usize) -> Vec<Bubble> {
    let chars: Vec<char> = b.text.chars().collect();
    let mut out: Vec<Bubble> = Vec::new();
    let mut start = 0usize;
    let mut first = true;
    while chars.len() - start > max {
        // 在窗口内从后往前找标点断点；找不到就硬切
        let window_end = start + max;
        let cut_at = (start + 1..=window_end)
            .rev()
            .find(|&i| {
                matches!(
                    chars[i - 1],
                    '。' | '！' | '？' | '!' | '?' | '.' | '，' | '；' | '：' | ';' | ':' | ',' | '\n'
                )
            })
            .unwrap_or(window_end);
        let chunk: String = chars[start..cut_at].iter().collect::<String>().trim().to_string();
        if !chunk.is_empty() {
            out.push(Bubble {
                text: chunk,
                at: b.at && first,
                meme: None,
            });
            first = false;
        }
        start = cut_at;
    }
    let rest: String = chars[start..].iter().collect::<String>().trim().to_string();
    if !rest.is_empty() {
        out.push(Bubble {
            text: rest,
            at: b.at && first,
            meme: b.meme.clone(),
        });
    }
    out
}

// ---------- 发送队列（per-chat 串行） ----------

/// 队列任务种类：文字泡序列 or 直接发图（meme）
#[derive(Debug, Clone)]
pub enum JobKind {
    Bubbles(Vec<Bubble>),
    /// 已抽好的 meme 图片绝对路径（直接发图，无打字延时/无作废核对）
    Image(PathBuf),
}

#[derive(Debug, Clone)]
pub struct ReplyJob {
    pub chat_id: String,
    pub chat_type: ChatType,
    pub target: u64,
    /// 触发消息的 msg_id（仅日志观测）
    pub anchor_msg_id: i64,
    pub mention: bool,
    /// @目标 QQ 号（mention 或泡内 ::at 用）
    pub mention_qq: Option<u64>,
    pub kind: JobKind,
}

/// 回复形态参数（热应用槽：ReplyEngine/管线每次读取当前值，WebUI 写回即换槽）
#[derive(Debug, Clone, Copy)]
pub struct ReplyCfg {
    pub first_delay_min_ms: u64,
    pub first_delay_max_ms: u64,
    pub base_delay_ms: u64,
    pub per_char_ms: u64,
    pub jitter_ratio: f64,
    pub min_delay_ms: u64,
    pub max_delay_ms: u64,
    pub total_budget_ms: u64,
    /// 泡数封顶（超出并入最后一泡）
    pub bubble_cap: usize,
    /// 单泡字数上限（超出按标点机械切）
    pub bubble_char_cap: usize,
}

impl Default for ReplyCfg {
    fn default() -> Self {
        Self {
            first_delay_min_ms: 300,
            first_delay_max_ms: 800,
            base_delay_ms: 600,
            per_char_ms: 40,
            jitter_ratio: 0.3,
            min_delay_ms: 800,
            max_delay_ms: 4000,
            total_budget_ms: 8000,
            bubble_cap: 3,
            bubble_char_cap: 500,
        }
    }
}

#[derive(Clone)]
pub struct EngineHandle {
    tx: mpsc::UnboundedSender<ReplyJob>,
}

impl EngineHandle {
    pub fn enqueue(&self, job: ReplyJob) {
        if self.tx.send(job).is_err() {
            warn!("回复引擎已停止，任务丢弃");
        }
    }

    /// 直接发图（meme）：入 per-chat 串行队列，与普通回复同队列保序（稳定六条②）
    pub fn enqueue_image(&self, chat_id: String, chat_type: ChatType, target: u64, image: PathBuf) {
        self.enqueue(ReplyJob {
            chat_id,
            chat_type,
            target,
            anchor_msg_id: 0,
            mention: false,
            mention_qq: None,
            kind: JobKind::Image(image),
        });
    }
}

/// 回复形态参数共享槽（热应用：WebUI 配置写回后换槽，引擎每泡读取当前值）
pub type SharedReplyCfg = Arc<std::sync::RwLock<ReplyCfg>>;

pub struct ReplyEngine {
    db_path: PathBuf,
    bus: EventBus,
    send: SendFn,
    self_ids: SelfMsgIds,
    cfg: SharedReplyCfg,
    mood: MoodState,
}

impl ReplyEngine {
    pub fn new(
        db_path: PathBuf,
        bus: EventBus,
        send: SendFn,
        self_ids: SelfMsgIds,
        cfg: SharedReplyCfg,
        mood: MoodState,
    ) -> Self {
        Self { db_path, bus, send, self_ids, cfg, mood }
    }

    /// 派遣器：每个 chat 一个 worker（mpsc 串行）；折叠卡与普通回复同队列（稳定六条②）
    pub fn spawn(self) -> EngineHandle {
        let engine = Arc::new(self);
        let (tx, mut rx) = mpsc::unbounded_channel::<ReplyJob>();
        tokio::spawn(async move {
            let mut workers: HashMap<String, mpsc::UnboundedSender<ReplyJob>> = HashMap::new();
            while let Some(job) = rx.recv().await {
                let entry = workers.entry(job.chat_id.clone()).or_insert_with(|| {
                    let (wtx, mut wrx) = mpsc::unbounded_channel::<ReplyJob>();
                    let eng = engine.clone();
                    let chat = job.chat_id.clone();
                    tokio::spawn(async move {
                        while let Some(j) = wrx.recv().await {
                            process_job(&eng, j).await;
                        }
                        debug!(chat_id = %chat, "回复队列 worker 退出");
                    });
                    wtx
                });
                let _ = entry.send(job);
            }
        });
        info!("回复形态引擎已启动");
        EngineHandle { tx }
    }
}

/// 本 chat 最新外来消息版本（self 泡不改变上下文版本，防自己作废自己）
fn latest_incoming(db_path: &Path, chat_id: &str) -> i64 {
    crate::db::connect(db_path)
        .ok()
        .and_then(|c| {
            c.query_row(
                "SELECT COALESCE(MAX(msg_id), 0) FROM messages WHERE chat_id = ?1 AND sender_pid != 'self'",
                params![chat_id],
                |r| r.get::<_, i64>(0),
            )
            .ok()
        })
        .unwrap_or(0)
}

async fn process_job(eng: &Arc<ReplyEngine>, job: ReplyJob) {
    match job.kind.clone() {
        JobKind::Bubbles(bubbles) => process_bubbles(eng, &job, &bubbles).await,
        JobKind::Image(path) => process_image(eng, &job, &path, "meme").await,
    }
}

/// 直接发图：无首泡延时、无作废核对，失败退避重试一次；落 self 图片消息保持流水连贯
async fn process_image(eng: &Arc<ReplyEngine>, job: &ReplyJob, path: &Path, note: &str) {
    let req = SendRequest {
        chat_type: job.chat_type,
        target: job.target,
        segments: json!([{"type": "image", "data": {"file": meme::file_url(path)}}]),
    };
    match send_with_retry(&eng.send, &req).await {
        Ok(data) => {
            if let Some(id) = data.get("message_id").and_then(|x| x.as_i64()) {
                eng.self_ids.record(id);
            }
            insert_self_message_ex(&eng.db_path, job, "（图片）", true);
            eng.bus.publish(Event::BubbleSent(BubbleSentPayload {
                chat_id: job.chat_id.clone(),
                bubble_index: 0,
                total: 1,
                ok: true,
                note: Some(note.into()),
            }));
        }
        Err(e) => {
            warn!(chat_id = %job.chat_id, error = %e, "meme 图发送失败");
            eng.bus.publish(Event::BubbleSent(BubbleSentPayload {
                chat_id: job.chat_id.clone(),
                bubble_index: 0,
                total: 1,
                ok: false,
                note: Some(format!("meme 发送失败: {e}")),
            }));
        }
    }
}

/// ::meme 类别缺省时按 mood 映射（happy→开心/angry→生气/down→低落/calm→misc，映射不到→misc）
fn resolve_meme_category(cat: Option<&str>, mood: MoodValue) -> String {
    if let Some(c) = cat.map(str::trim).filter(|c| !c.is_empty()) {
        return c.to_string();
    }
    match mood {
        MoodValue::Happy => "开心",
        MoodValue::Angry => "生气",
        MoodValue::Down => "低落",
        MoodValue::Calm => "misc",
    }
    .to_string()
}

async fn process_bubbles(eng: &Arc<ReplyEngine>, job: &ReplyJob, bubbles: &[Bubble]) {
    let cfg = *eng.cfg.read().unwrap(); // 热应用：每泡取当前参数
    let started = Instant::now();
    let base_version = latest_incoming(&eng.db_path, &job.chat_id);
    let total = bubbles.len();

    for (i, b) in bubbles.iter().enumerate() {
        // 延时模拟打字（超总预算则不再延时）
        if started.elapsed().as_millis() < cfg.total_budget_ms as u128 {
            if i == 0 {
                let ms = rand::rng().random_range(cfg.first_delay_min_ms..=cfg.first_delay_max_ms);
                tokio::time::sleep(Duration::from_millis(ms)).await;
            } else {
                let cs = b.text.chars().count() as f64;
                let raw = cfg.base_delay_ms as f64 + cs * cfg.per_char_ms as f64;
                let jitter = raw * cfg.jitter_ratio * rand::rng().random_range(-1.0..=1.0_f64);
                let d = (raw + jitter).clamp(cfg.min_delay_ms as f64, cfg.max_delay_ms as f64);
                tokio::time::sleep(Duration::from_millis(d as u64)).await;
            }
        }
        // 异步作废：上下文已变动 → 作废剩余泡
        if latest_incoming(&eng.db_path, &job.chat_id) > base_version {
            info!(chat_id = %job.chat_id, sent = i, "上下文变动，作废剩余泡");
            eng.bus.publish(Event::ReplyInterrupted);
            return;
        }
        // 纯 ::meme 泡：按类别（缺省按 mood 映射）抽图直接发图片；混合泡只发文字部分
        if b.text.is_empty() && b.meme.is_some() {
            let memes_root = eng.db_path.parent().unwrap_or(Path::new(".")).join("memes");
            let cat = resolve_meme_category(b.meme.as_deref(), eng.mood.get());
            match meme::pick(&eng.db_path, &memes_root, &cat)
                .or_else(|| meme::pick(&eng.db_path, &memes_root, "misc"))
            {
                Some(path) => {
                    debug!(chat_id = %job.chat_id, category = %cat, "::meme 抽图发送");
                    process_image(eng, job, &path, "meme(::)").await;
                }
                None => {
                    debug!(chat_id = %job.chat_id, category = %cat, "meme 库无该类别图片，跳过");
                    eng.bus.publish(Event::BubbleSent(BubbleSentPayload {
                        chat_id: job.chat_id.clone(),
                        bubble_index: i,
                        total,
                        ok: false,
                        note: Some(format!("meme 库空（类别 {cat}）")),
                    }));
                }
            }
            continue;
        }
        // 组装 OneBot 段数组（首泡按 Decision.mention 或泡内 ::at 加 at 段；私聊不加）
        let mut segs: Vec<Value> = Vec::new();
        let want_at = job.chat_type == ChatType::Group
            && job.mention_qq.is_some()
            && ((i == 0 && job.mention) || b.at);
        if want_at {
            segs.push(json!({"type": "at", "data": {"qq": job.mention_qq.unwrap().to_string()}}));
        }
        if !b.text.is_empty() {
            segs.push(json!({"type": "text", "data": {"text": b.text}}));
        }
        if segs.is_empty() {
            continue;
        }
        let req = SendRequest {
            chat_type: job.chat_type,
            target: job.target,
            segments: json!(segs),
        };
        match send_with_retry(&eng.send, &req).await {
            Ok(data) => {
                if let Some(id) = data.get("message_id").and_then(|x| x.as_i64()) {
                    eng.self_ids.record(id);
                }
                insert_self_message(&eng.db_path, &job, &b.text);
                eng.bus.publish(Event::BubbleSent(BubbleSentPayload {
                    chat_id: job.chat_id.clone(),
                    bubble_index: i,
                    total,
                    ok: true,
                    note: None,
                }));
            }
            Err(e) => {
                warn!(chat_id = %job.chat_id, bubble = i, error = %e, "泡发送失败，跳过该泡");
                eng.bus.publish(Event::BubbleSent(BubbleSentPayload {
                    chat_id: job.chat_id.clone(),
                    bubble_index: i,
                    total,
                    ok: false,
                    note: Some(format!("发送失败跳过: {e}")),
                }));
            }
        }
    }
    debug!(chat_id = %job.chat_id, total, elapsed_ms = started.elapsed().as_millis() as u64, "回复发送完成");
}

/// 单泡发送失败 → 退避重试一次（稳定六条③）；echo 10s 超时在适配层
async fn send_with_retry(send: &SendFn, req: &SendRequest) -> Result<Value> {
    match (send)(req.clone()).await {
        Ok(v) => Ok(v),
        Err(e1) => {
            tokio::time::sleep(Duration::from_millis(300)).await;
            (send)(req.clone()).await.map_err(|e2| anyhow!("首试 {e1}；重试 {e2}"))
        }
    }
}

fn insert_self_message(db_path: &Path, job: &ReplyJob, text: &str) {
    insert_self_message_ex(db_path, job, text, false)
}

fn insert_self_message_ex(db_path: &Path, job: &ReplyJob, text: &str, has_image: bool) {
    let Ok(conn) = crate::db::connect(db_path) else { return };
    let now = now_secs();
    let chat_type = match job.chat_type {
        ChatType::Group => "group",
        ChatType::Private => "private",
    };
    let _ = conn.execute(
        "INSERT INTO persons(person_id, display_name, first_seen, last_seen) VALUES ('self', '云团', ?1, ?1)
         ON CONFLICT(person_id) DO NOTHING",
        params![now],
    );
    if let Err(e) = conn.execute(
        "INSERT INTO messages(chat_id, chat_type, sender_pid, nickname, text, mentions, at_me, has_image, ts)
         VALUES (?1, ?2, 'self', '云团', ?3, '[]', 0, ?4, ?5)",
        params![job.chat_id, chat_type, text, has_image as i64, now],
    ) {
        warn!(error = %e, "self 回复落库失败");
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------- bot_chat 调用与入队 ----------

/// Decision=reply 之后：组装 Bot Context → 调 bot_chat → Bubbleizer → 入队。
/// ctx_cfg/reply_cfg 由调用方从热应用槽读取传入（每消息取当前值）。
pub async fn prepare_and_enqueue(
    engine: &EngineHandle,
    db_path: &Path,
    llm: &llm::LlmGateway,
    mood: &MoodState,
    msg: &MessageReceivedPayload,
    out: &DecisionOutput,
    ctx_cfg: &context_builder::ContextCfg,
    reply_cfg: &ReplyCfg,
) -> Result<()> {
    let ctx = context_builder::build_bot_context(db_path, mood.get(), msg, ctx_cfg);
    let started = Instant::now();
    let raw = llm
        .chat(llm::Role::BotChat, &ctx.system, &ctx.user, false)
        .await
        .context("bot_chat 调用失败")?;
    debug!(k_used = ctx.k_used, elapsed_ms = started.elapsed().as_millis() as u64, "bot_chat 生成完成");
    let bubbles = bubbleize_with(&raw, reply_cfg.bubble_char_cap, reply_cfg.bubble_cap);
    if bubbles.is_empty() {
        return Err(anyhow!("bot_chat 输出为空"));
    }
    let (chat_type, target, mention_qq) = route_of(msg)?;
    engine.enqueue(ReplyJob {
        chat_id: msg.chat_id.clone(),
        chat_type,
        target,
        anchor_msg_id: msg.msg_id,
        mention: out.mention,
        mention_qq,
        kind: JobKind::Bubbles(bubbles),
    });
    Ok(())
}

pub(crate) fn route_of(msg: &MessageReceivedPayload) -> Result<(ChatType, u64, Option<u64>)> {
    let sender_qq = msg
        .sender_pid
        .strip_prefix("p_")
        .and_then(|s| s.parse::<u64>().ok());
    match msg.chat_type.as_str() {
        "group" => {
            let gid = msg
                .chat_id
                .parse::<u64>()
                .with_context(|| format!("群号无法解析: {}", msg.chat_id))?;
            Ok((ChatType::Group, gid, sender_qq))
        }
        "private" => {
            let uid = sender_qq.with_context(|| format!("私聊对象无法解析: {}", msg.sender_pid))?;
            Ok((ChatType::Private, uid, None))
        }
        other => Err(anyhow!("未知 chat_type: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_bubbles_by_marker() {
        let bs = bubbleize("哈‖确实不错‖我去试试");
        assert_eq!(bs.len(), 3);
        assert_eq!(bs[0].text, "哈");
        assert_eq!(bs[2].text, "我去试试");
        assert!(bs.iter().all(|b| !b.at && b.meme.is_none()));
    }

    #[test]
    fn directives_parsed_and_stripped() {
        let bs = bubbleize("::at\n在的‖::meme 开心");
        assert_eq!(bs.len(), 2);
        assert!(bs[0].at);
        assert_eq!(bs[0].text, "在的");
        assert_eq!(bs[1].meme.as_deref(), Some("开心"));
        assert!(bs[1].text.is_empty());
    }

    #[test]
    fn cap_merges_tail_into_third() {
        let bs = bubbleize("一‖二‖三‖四‖五");
        assert_eq!(bs.len(), 3);
        assert_eq!(bs[2].text, "三\n四\n五");
    }

    #[test]
    fn no_marker_is_single_bubble() {
        let bs = bubbleize("一句话说完");
        assert_eq!(vec![Bubble { text: "一句话说完".into(), at: false, meme: None }], bs);
    }

    #[test]
    fn oversized_bubble_mechanically_split_at_punct() {
        let chunk = "好。".repeat(300); // 600 字
        let bs = bubbleize(&chunk);
        assert!(bs.len() >= 2);
        assert!(bs.iter().all(|b| b.text.chars().count() <= 500));
        // 无标点窗口硬切也不超界
        let hard = "啊".repeat(700);
        let bs2 = bubbleize(&hard);
        assert!(bs2.iter().all(|b| b.text.chars().count() <= 500));
    }

    #[test]
    fn parse_failure_falls_back_to_single_bubble() {
        let bs = bubbleize("‖‖‖");
        assert_eq!(bs.len(), 1);
        assert_eq!(bs[0].text, "‖‖‖");
        assert!(bubbleize("").is_empty());
    }

    #[test]
    fn multiline_segment_kept_in_one_bubble() {
        let bs = bubbleize("第一行\n第二行‖另一个泡");
        assert_eq!(bs.len(), 2);
        assert_eq!(bs[0].text, "第一行\n第二行");
    }
}
