//! 回复形态引擎（架构文档十四章机制）：
//! 发送链路 bot_chat → Bubbleizer → per-chat 发送队列（串行异步） → 适配层发送函数。
//! - Bubbleizer：‖ 分泡、::at/::meme 指令行、3 泡封顶（超出并最后泡）、无分隔符单泡、
//!   超过拆句目标时只沿完整句末切分；半句接回，纯控制符不发送
//! - 延时模拟：首泡 300~800ms；后续 clamp(0.6s+字数×40ms±30%, 0.8s, 4s)；整段总预算 8s，超预算不再延时
//! - 回复始终绑定 anchor；普通插话不取消已入队的回答（ADR-0007）
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

/// 解析 bot_chat 输出（拆句目标 500 字，最多 3 泡；完整单句不强拆）
pub fn bubbleize(raw: &str) -> Vec<Bubble> {
    bubbleize_with(raw, 500, 3)
}

/// char_cap 是沿句末拆分的目标字数，bubble_cap 是泡数封顶；不丢失尾部正文。
pub fn bubbleize_with(raw: &str, char_cap: usize, bubble_cap: usize) -> Vec<Bubble> {
    let char_cap = char_cap.max(20);
    let bubble_cap = bubble_cap.max(1);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let segments = control_segments(trimmed);
    let mut bubbles: Vec<Bubble> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for seg in segments {
        let mut text_lines = Vec::new();
        let mut at = false;
        let mut meme: Option<String> = None;
        for line in seg.lines() {
            let l = line.trim();
            let was_fenced = fence.is_some();
            let marker = if l.starts_with("```") {
                Some(('`', l.chars().take_while(|c| *c == '`').count()))
            } else if l.starts_with("~~~") {
                Some(('~', l.chars().take_while(|c| *c == '~').count()))
            } else {
                None
            };
            if let Some((kind, count)) = marker {
                if fence.is_some_and(|(k, n)| k == kind && count >= n)
                    && l[count..].trim().is_empty()
                {
                    fence = None;
                } else if fence.is_none() {
                    fence = marker;
                }
            }
            let protected = was_fenced || fence.is_some();
            if !protected && l.starts_with("::") {
                let rest = &l[2..];
                if let Some(tail) = rest
                    .strip_prefix("at")
                    .filter(|s| !s.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
                {
                    at = true;
                    if !tail.trim().is_empty() {
                        text_lines.push(tail.trim());
                    }
                } else if let Some(cat) = rest.strip_prefix("meme").map(str::trim) {
                    if !cat.is_empty() {
                        meme = Some(cat.to_string());
                    }
                }
                // 只有指令的输出保持空，不再把原控制符作为兜底正文发出。
                continue;
            }
            if !l.is_empty() || protected {
                text_lines.push(if protected { line } else { l });
            }
        }
        let text = text_lines.join("\n");
        if text.is_empty() && meme.is_none() {
            continue;
        }
        if let Some(last) = bubbles.last_mut() {
            if !last.text.is_empty()
                && !text.is_empty()
                && (!complete_boundary(&last.text)
                    || text.starts_with([
                        '，', '。', '！', '？', ',', '.', '!', '?', '”', '」', '）', ')',
                    ]))
            {
                join_fragments(&mut last.text, &text);
                last.at |= at;
                if last.meme.is_none() {
                    last.meme = meme;
                }
                continue;
            }
        }
        bubbles.push(Bubble { text, at, meme });
    }
    // 超过目标时仅沿完整句末拆分，单句/代码/引用保持完整。
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

// 控制符只在代码之外生效；双竖线只接受独立控制行，保留 Rust || 和 Markdown 表格。
fn control_segments(raw: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut ticks = 0usize;
    let mut fence_tilde = false;
    for line in raw.lines() {
        if ticks == 0 && line.trim_start().starts_with("~~~") {
            fence_tilde = !fence_tilde;
        }
        if ticks == 0 && !fence_tilde && matches!(line.trim(), "||" | "｜｜") {
            parts.push(String::new());
            continue;
        }
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '`' && !fence_tilde {
                let mut count = 1;
                while chars.peek() == Some(&'`') {
                    chars.next();
                    count += 1;
                }
                if ticks == 0 {
                    ticks = count;
                } else if ticks == count {
                    ticks = 0;
                }
                parts
                    .last_mut()
                    .unwrap()
                    .extend(std::iter::repeat_n('`', count));
            } else if c == '‖' && ticks == 0 && !fence_tilde {
                parts.push(String::new());
            } else if c == ':' && ticks == 0 && !fence_tilde {
                let mut look = chars.clone();
                let standalone = !parts
                    .last()
                    .unwrap()
                    .ends_with(|p: char| p.is_ascii_alphanumeric() || matches!(p, '_' | ':'));
                if standalone
                    && look.next() == Some(':')
                    && look.next() == Some('a')
                    && look.next() == Some('t')
                    && look
                        .peek()
                        .is_none_or(|p| !p.is_ascii_alphanumeric() && *p != '_')
                {
                    chars = look;
                    // 容忍模型把 ::at 放在句尾/贴着汉字；标记挪到指令行，正文连续保留。
                    parts.last_mut().unwrap().insert_str(0, "::at\n");
                } else {
                    parts.last_mut().unwrap().push(c);
                }
            } else {
                parts.last_mut().unwrap().push(c);
            }
        }
        parts.last_mut().unwrap().push('\n');
    }
    parts
}

fn complete_boundary(text: &str) -> bool {
    let text = text.trim_end();
    if text.ends_with("```") || text.ends_with("~~~") {
        return true;
    }
    // 引号/括号未闭合时宁可少分泡，不在引用或括号内部截断。
    for (open, close) in [
        ('“', '”'),
        ('「', '」'),
        ('『', '』'),
        ('（', '）'),
        ('(', ')'),
        ('[', ']'),
        ('【', '】'),
    ] {
        if text.matches(open).count() != text.matches(close).count() {
            return false;
        }
    }
    if text.matches('"').count() % 2 != 0 {
        return false;
    }
    text.trim_end_matches(['”', '」', '』', '）', ')', ']', '】', '"'])
        .ends_with(['。', '！', '？', '!', '?', '.', '…'])
}

fn join_fragments(left: &mut String, right: &str) {
    // 汉字半句直接接回；英文词之间保留一个空格，避免拼成另一个单词。
    if left.ends_with(|c: char| c.is_ascii_alphanumeric())
        && right.starts_with(|c: char| c.is_ascii_alphanumeric())
    {
        left.push(' ');
    }
    left.push_str(right);
}

fn mechanical_split(b: &Bubble, max: usize) -> Vec<Bubble> {
    // 代码/引用/括号内容保守地保持整块。字数是拆分目标，不能以截断单句实现。
    if b.text.contains([
        '`', '~', '“', '”', '「', '」', '（', '）', '(', ')', '[', ']', '"',
    ]) || b.text.contains("://")
        || b.text.contains("www.")
    {
        return vec![b.clone()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut last_end = 0;
    let mut chars = 0;
    let mut last_chars = 0;
    let mut iter = b.text.char_indices().peekable();
    while let Some((index, c)) = iter.next() {
        chars += 1;
        let terminal = matches!(c, '。' | '！' | '？' | '!' | '?')
            || (c == '.' && iter.peek().is_none_or(|(_, n)| n.is_whitespace()));
        if !terminal
            || iter
                .peek()
                .is_some_and(|(_, n)| matches!(n, '。' | '！' | '？' | '!' | '?'))
        {
            continue;
        }
        if chars > max && last_end > start {
            chunks.push(b.text[start..last_end].trim().to_owned());
            start = last_end;
            chars -= last_chars;
        }
        last_end = index + c.len_utf8();
        last_chars = chars;
        if chars >= max {
            chunks.push(b.text[start..last_end].trim().to_owned());
            start = last_end;
            chars = 0;
            last_chars = 0;
        }
    }
    if chars > max && last_end > start {
        chunks.push(b.text[start..last_end].trim().to_owned());
        start = last_end;
    }
    if start < b.text.len() {
        chunks.push(b.text[start..].trim().to_owned());
    }
    let last = chunks.len().saturating_sub(1);
    chunks
        .into_iter()
        .enumerate()
        .filter(|(_, s)| !s.is_empty())
        .map(|(i, text)| Bubble {
            text,
            at: b.at && i == 0,
            meme: if i == last { b.meme.clone() } else { None },
        })
        .collect()
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
    /// 拆句目标字数（不强拆完整单句、代码或引用）
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
    tx: mpsc::UnboundedSender<EngineCommand>,
    abort: tokio::task::AbortHandle,
}

enum EngineCommand {
    Enqueue(ReplyJob),
    Shutdown(tokio::sync::oneshot::Sender<()>),
}

impl EngineHandle {
    pub fn enqueue(&self, job: ReplyJob) {
        if self.tx.send(EngineCommand::Enqueue(job)).is_err() {
            warn!("回复引擎已停止，任务丢弃");
        }
    }

    /// 关闭入口并等待已入队消息发送完成；超时取消所有发送 worker。
    pub async fn shutdown(&self, timeout: Duration) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self.tx.send(EngineCommand::Shutdown(tx)).is_err() {
            return true;
        }
        if matches!(tokio::time::timeout(timeout, rx).await, Ok(Ok(()))) {
            true
        } else {
            self.abort.abort();
            false
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
        Self {
            db_path,
            bus,
            send,
            self_ids,
            cfg,
            mood,
        }
    }

    /// 派遣器：每个 chat 一个 worker（mpsc 串行）；折叠卡与普通回复同队列（稳定六条②）
    pub fn spawn(self) -> EngineHandle {
        let engine = Arc::new(self);
        let (tx, mut rx) = mpsc::unbounded_channel::<EngineCommand>();
        let dispatcher = tokio::spawn(async move {
            let mut workers: HashMap<String, mpsc::UnboundedSender<ReplyJob>> = HashMap::new();
            let mut tasks = tokio::task::JoinSet::new();
            let mut shutdown_waiters = Vec::new();
            while let Some(command) = rx.recv().await {
                let job = match command {
                    EngineCommand::Enqueue(job) => job,
                    EngineCommand::Shutdown(done) => {
                        rx.close();
                        shutdown_waiters.push(done);
                        continue;
                    }
                };
                let entry = workers.entry(job.chat_id.clone()).or_insert_with(|| {
                    let (wtx, mut wrx) = mpsc::unbounded_channel::<ReplyJob>();
                    let eng = engine.clone();
                    let chat = job.chat_id.clone();
                    tasks.spawn(async move {
                        while let Some(j) = wrx.recv().await {
                            process_job(&eng, j).await;
                        }
                        debug!(chat_id = %chat, "回复队列 worker 退出");
                    });
                    wtx
                });
                let _ = entry.send(job);
            }
            drop(workers);
            while tasks.join_next().await.is_some() {}
            for done in shutdown_waiters {
                let _ = done.send(());
            }
        });
        info!("回复形态引擎已启动");
        EngineHandle {
            tx,
            abort: dispatcher.abort_handle(),
        }
    }
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
            insert_self_message_ex(
                &eng.db_path,
                job,
                "（图片）",
                true,
                data.get("message_id").and_then(|x| x.as_i64()),
            );
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
            // QQ 不会自动在 at 段与正文之间插入间隔，发送端补空格。
            let text = if want_at && !b.text.starts_with(char::is_whitespace) {
                format!(" {}", b.text)
            } else {
                b.text.clone()
            };
            segs.push(json!({"type": "text", "data": {"text": text}}));
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
                insert_self_message_ex(
                    &eng.db_path,
                    &job,
                    &b.text,
                    false,
                    data.get("message_id").and_then(|x| x.as_i64()),
                );
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
            (send)(req.clone())
                .await
                .map_err(|e2| anyhow!("首试 {e1}；重试 {e2}"))
        }
    }
}

fn insert_self_message_ex(
    db_path: &Path,
    job: &ReplyJob,
    text: &str,
    has_image: bool,
    external_msg_id: Option<i64>,
) {
    let Ok(conn) = crate::db::connect(db_path) else {
        return;
    };
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
        "INSERT INTO messages(chat_id, chat_type, sender_pid, nickname, text, mentions, at_me, has_image, ts, external_msg_id)
         VALUES (?1, ?2, 'self', '云团', ?3, '[]', 0, ?4, ?5, ?6)",
        params![job.chat_id, chat_type, text, has_image as i64, now, external_msg_id],
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
    snapshot: &context_builder::ReplySnapshot,
    llm: &llm::LlmGateway,
    mood: &MoodState,
    msg: &MessageReceivedPayload,
    out: &DecisionOutput,
    ctx_cfg: &context_builder::ContextCfg,
    reply_cfg: &ReplyCfg,
) -> Result<()> {
    let length_hint = match out.reply_len {
        crate::decision::ReplyLen::Short => {
            "本次回复长度：short。优先一句到两句，约80字以内，一泡能说明就不分泡。"
        }
        crate::decision::ReplyLen::Medium => {
            "本次回复长度：medium。先直接回答，再补必要说明，通常200字以内。"
        }
        crate::decision::ReplyLen::Long => {
            "本次回复长度：long。按问题需要展开，避免重复和无依据扩写。"
        }
    };
    let reserve = length_hint.chars().count() + 1;
    let budget = context_builder::ContextCfg {
        budget_chars: ctx_cfg.budget_chars.saturating_sub(reserve),
        ..*ctx_cfg
    };
    let mut ctx = context_builder::render_bot_context(snapshot, mood.get(), &budget)?;
    ctx.system.push('\n');
    ctx.system.push_str(length_hint);
    let started = Instant::now();
    let mut raw = llm
        .chat(llm::Role::BotChat, &ctx.system, &ctx.user, false)
        .await
        .context("bot_chat 调用失败")?;
    // 只对空正文重试一次，保留原快照；不重试工具调用、拒绝或网络错误。
    if raw.trim().is_empty() {
        warn!(chat_id=%msg.chat_id, anchor_msg_id=msg.msg_id, "bot_chat 正文为空，使用同一上下文重试一次");
        raw = llm
            .chat(llm::Role::BotChat, &ctx.system, &ctx.user, false)
            .await
            .context("bot_chat 空正文重试失败")?;
    }
    debug!(
        k_used = ctx.k_used,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "bot_chat 生成完成"
    );
    let style_cap = match out.reply_len {
        crate::decision::ReplyLen::Short => 1,
        crate::decision::ReplyLen::Medium => 2,
        crate::decision::ReplyLen::Long => reply_cfg.bubble_cap,
    };
    let bubbles = bubbleize_with(
        &raw,
        reply_cfg.bubble_char_cap,
        reply_cfg.bubble_cap.min(style_cap),
    );
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

    #[tokio::test]
    async fn shutdown_drains_jobs_and_cancels_stalled_senders() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = std::env::temp_dir().join(format!("yt-drain-{}", rand::random::<u64>()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("test.db");
        crate::db::migrate(&mut crate::db::connect(&path).unwrap()).unwrap();
        let cfg = Arc::new(std::sync::RwLock::new(ReplyCfg {
            total_budget_ms: 0,
            ..ReplyCfg::default()
        }));
        let sent = Arc::new(AtomicUsize::new(0));
        let send_count = sent.clone();
        let send: SendFn = Arc::new(move |_| {
            let n = send_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(json!({"message_id":n+1})) })
        });
        let make_job = || ReplyJob {
            chat_id: "123".into(),
            chat_type: ChatType::Group,
            target: 123,
            anchor_msg_id: 1,
            mention: false,
            mention_qq: None,
            kind: JobKind::Bubbles(vec![Bubble {
                text: "正常回答".into(),
                at: false,
                meme: None,
            }]),
        };
        let engine = ReplyEngine::new(
            path.clone(),
            EventBus::default(),
            send,
            SelfMsgIds::default(),
            cfg.clone(),
            MoodState::default(),
        )
        .spawn();
        engine.enqueue(make_job());
        engine.enqueue(make_job());
        assert!(engine.shutdown(Duration::from_secs(2)).await);
        assert_eq!(sent.load(Ordering::SeqCst), 2);
        let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
        struct OnDrop(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for OnDrop {
            fn drop(&mut self) {
                if let Some(tx) = self.0.take() {
                    let _ = tx.send(());
                }
            }
        }
        let sender = Arc::new(std::sync::Mutex::new(Some(dropped_tx)));
        let stalled: SendFn = Arc::new(move |_| {
            let guard = OnDrop(sender.lock().unwrap().take());
            Box::pin(async move {
                let _guard = guard;
                std::future::pending().await
            })
        });
        let engine = ReplyEngine::new(
            path,
            EventBus::default(),
            stalled,
            SelfMsgIds::default(),
            cfg,
            MoodState::default(),
        )
        .spawn();
        engine.enqueue(make_job());
        assert!(!engine.shutdown(Duration::from_millis(100)).await);
        tokio::time::timeout(Duration::from_secs(1), dropped_rx)
            .await
            .unwrap()
            .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_bubbles_by_marker() {
        let bs = bubbleize("哈。‖确实不错。‖我去试试。");
        assert_eq!(bs.len(), 3);
        assert_eq!(bs[0].text, "哈。");
        assert_eq!(bs[2].text, "我去试试。");
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
        let bs = bubbleize("一。‖二。‖三。‖四。‖五。");
        assert_eq!(bs.len(), 3);
        assert_eq!(bs[2].text, "三。\n四。\n五。");
    }

    #[test]
    fn no_marker_is_single_bubble() {
        let bs = bubbleize("一句话说完");
        assert_eq!(
            vec![Bubble {
                text: "一句话说完".into(),
                at: false,
                meme: None
            }],
            bs
        );
    }

    #[test]
    fn oversized_bubble_mechanically_split_at_punct() {
        let chunk = "好。".repeat(300); // 600 字
        let bs = bubbleize(&chunk);
        assert!(bs.len() >= 2);
        assert!(bs.iter().all(|b| b.text.chars().count() <= 500));
        // 长单句不能为字数目标被拦腰切开。
        let hard = "啊".repeat(700);
        let bs2 = bubbleize(&hard);
        assert_eq!(bs2.len(), 1);
        assert_eq!(bs2[0].text, hard);
    }

    #[test]
    fn control_only_output_is_empty_instead_of_leaking_markers() {
        let bs = bubbleize("‖‖‖");
        assert!(bs.is_empty());
        assert!(bubbleize("::at\n::unknown\n||\n｜｜").is_empty());
        assert!(bubbleize("").is_empty());
    }

    #[test]
    fn multiline_segment_kept_in_one_bubble() {
        let bs = bubbleize("第一行\n第二行。‖另一个泡");
        assert_eq!(bs.len(), 2);
        assert_eq!(bs[0].text, "第一行\n第二行。");
    }

    #[test]
    fn broken_clauses_and_detached_punctuation_are_rejoined() {
        for (raw, expected) in [
            ("这个库的‖性能很好。", "这个库的性能很好。"),
            (
                "如果明天下雨，‖我们就留在家里。",
                "如果明天下雨，我们就留在家里。",
            ),
            ("他说：“可以。‖明天再来。”", "他说：“可以。明天再来。”"),
            ("确实不错‖。", "确实不错。"),
            ("hello‖world.", "hello world."),
        ] {
            let bs = bubbleize(raw);
            assert_eq!(bs.len(), 1, "{raw}");
            assert_eq!(bs[0].text, expected);
        }
        let bs = bubbleize("::at你好。\n||\n再见。");
        assert_eq!(bs.len(), 2);
        assert!(bs[0].at);
        assert_eq!(bs[0].text, "你好。");
        let bs = bubbleize("你好。 ::at‖再见。");
        assert_eq!(bs.len(), 2);
        assert!(bs[0].at);
        assert_eq!(bs[0].text, "你好。");
    }

    #[test]
    fn code_urls_and_long_single_sentences_are_preserved() {
        let code = "```rust\n    let f = || \"a‖b\";\n::at\n```";
        let bs = bubbleize_with(code, 20, 3);
        assert_eq!(bs.len(), 1);
        assert_eq!(bs[0].text, code);
        assert!(!bs[0].at);
        let nested = "````text\n```\n\n::at\na‖b\n```\n````";
        assert_eq!(bubbleize_with(nested, 20, 3)[0].text, nested);
        assert_eq!(
            bubbleize("请用 `a‖b` 和 `||`。")[0].text,
            "请用 `a‖b` 和 `||`。"
        );
        let sentence = "地址是 https://example.com/a.b?version=1.2，后面这一整句不应该按逗号切开。";
        let bs = bubbleize_with(sentence, 20, 3);
        assert_eq!(bs.len(), 1);
        assert_eq!(bs[0].text, sentence);
    }
}
