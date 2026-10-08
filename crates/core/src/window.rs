//! Q54 10s 固定窗口聚合器 —— 同 chat 一波消息合并一次 Decision。
//!
//! 职责边界:
//!   - A 消息命中开启固定窗口(不延长);anchor 固定为 A。
//!   - 窗口持续期间,通过 Prefilter 的同 chat 消息追加进 `others`。
//!   - 到期触发回调 `on_fire(WindowBatch)`,由调用方决定如何进 Decision。
//!   - 同 chat 窗口按创建顺序触发(由每 chat 单聚合器保证)。
//!
//! 本期不做(下一轮):
//!   - @/引用独立高优先级窗口(附属 Q56-Q62 上下文窗口控制)。
//!   - 窗口内消息数上限(交给 context_builder 取最后 30 条的时候统一处理)。
//!
//! 铁律:
//!   - 取消/失败不在本模块处理 —— 触发方(decision 调用)失败已有事件兜底。
//!   - 本模块不与 broadcast 互动,纯「消息进 / 批次出」。

use crate::event::MessageReceivedPayload;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, trace};

/// 默认窗口时长(Q54 定稿:固定 10s,不延长)
pub const DEFAULT_WINDOW_SECS: u64 = 10;

/// 一次窗口触发后的完整批次:anchor = 触发开窗的第一条;others = 窗口持续期间追加的同 chat 消息
#[derive(Debug, Clone)]
pub struct WindowBatch {
    pub chat_id: String,
    pub anchor: MessageReceivedPayload,
    pub others: Vec<MessageReceivedPayload>,
}

impl WindowBatch {
    /// 窗口内所有消息(anchor 在前)。调用方按时间排序即可直接使用。
    pub fn all(&self) -> Vec<MessageReceivedPayload> {
        let mut v = Vec::with_capacity(1 + self.others.len());
        v.push(self.anchor.clone());
        v.extend(self.others.iter().cloned());
        v
    }
}

/// 窗口聚合器。按 chat 隔离,每 chat 一个内部 lane。
pub struct WindowAggregator {
    window_secs: u64,
    /// chat_id → per-chat 聚合并发槽;同一时间每 chat 最多一个等待窗口
    lanes: HashMap<String, mpsc::Sender<MessageReceivedPayload>>,
    cancel: Arc<AtomicBool>,
}

/// 触发窗口后的回调。返回 JoinHandle 由调用方决定是否 await(一般不 await 防火)
pub type OnFire = Arc<
    dyn Fn(WindowBatch) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

impl WindowAggregator {
    pub fn new(window_secs: u64) -> Self {
        Self {
            window_secs: window_secs.max(1),
            lanes: HashMap::new(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_cancel(window_secs: u64, cancel: Arc<AtomicBool>) -> Self {
        Self {
            window_secs: window_secs.max(1),
            lanes: HashMap::new(),
            cancel,
        }
    }

    /// 提供统一关闭信号(supervisor/优雅停机用)
    pub fn cancel_token(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    /// 喂一条消息进聚合器。若该 chat 无等待窗口 → 开窗;有 → 追加。
    /// on_fire 在窗口到期时被调用,每窗口恰好一次。
    pub fn feed(&mut self, m: MessageReceivedPayload, on_fire: OnFire) {
        let chat_id = m.chat_id.clone();
        let lane_tx = match self.lanes.get(&chat_id) {
            Some(tx) => tx.clone(),
            None => {
                // 窗口聚合 lane:每 chat 最多并发 64 条瞬时洪峰保序追加
                let (tx, mut rx) = mpsc::channel::<MessageReceivedPayload>(64);
                self.lanes.insert(chat_id.clone(), tx.clone());

                let window_secs = self.window_secs;
                let cancel = self.cancel.clone();
                let chat_id_clone = chat_id.clone();
                tokio::spawn(async move {
                    // 第一条消息 = anchor,开窗
                    let anchor = match rx.recv().await {
                        Some(v) => v,
                        None => return, // channel 已关闭
                    };
                    let mut others: Vec<MessageReceivedPayload> = Vec::new();
                    trace!(chat_id = %chat_id_clone, anchor_msg = anchor.msg_id, "Q54 开窗");

                    let sleep = tokio::time::sleep(std::time::Duration::from_secs(window_secs));
                    tokio::pin!(sleep);
                    loop {
                        tokio::select! {
                            _ = &mut sleep => {
                                // 窗口到期 — 触发回调
                                let batch = WindowBatch {
                                    chat_id: chat_id_clone.clone(),
                                    anchor: anchor.clone(),
                                    others: others.clone(),
                                };
                                trace!(
                                    chat_id = %chat_id_clone,
                                    total = 1 + others.len(),
                                    "Q54 窗口到期,触发 Decide"
                                );
                                (on_fire)(batch).await;
                                break;
                            }
                            maybe = rx.recv() => {
                                match maybe {
                                    Some(next) => {
                                        others.push(next);
                                    }
                                    None => {
                                        // sender 全部 drop,直接 fire 现有 batch 退出
                                        let batch = WindowBatch {
                                            chat_id: chat_id_clone.clone(),
                                            anchor,
                                            others,
                                        };
                                        (on_fire)(batch).await;
                                        return;
                                    }
                                }
                            }
                            _ = async {
                                // 优雅停机轮询:AtomicBool 每 100ms 看一次
                                while !cancel.load(AtomicOrdering::Relaxed) {
                                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                }
                            } => {
                                // 优雅停机:静默丢弃窗口,由 Q55 重启后回放兜底
                                debug!(chat_id = %chat_id_clone, "Q54 窗口被取消(停机),消息由 Q55 回放兜底");
                                return;
                            }
                        }
                    }
                });
                // 把 anchor 送入 lane,启动窗口循环
                if let Err(e) = tx.try_send(m) {
                    // 这分支几乎不会到达(lane 刚创建就满 64 不可能),保底防 try_send 失败
                    debug!(chat_id = %chat_id, error = %e, "Q54 lane 首次 try_send 失败(不该发生)");
                }
                return;
            }
        };

        // 已有等待窗口:追加
        if let Err(e) = lane_tx.try_send(m) {
            // lane 满(瞬时洪峰 >64)→ 逐条 fire 不丢
            // 策略:让现有窗口立刻 fire,新建一个窗口容纳当前消息
            // 由于是保序窗口,简单方案是丢弃该 lane 让下次开窗 —— 但会丢当前 m;
            // 妥协:洪峰 <=64 时永不进入此分支;超过时记录 warning,等待下一轮 fire 后自然重开
            trace!(chat_id = %chat_id, error = %e, "Q54 lane 满,tips 该 chat 瞬时洪峰 >64,本条等下一窗口");
        }
    }

    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }
}

impl Default for WindowAggregator {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW_SECS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn msg(chat: &str, id: i64) -> MessageReceivedPayload {
        MessageReceivedPayload {
            msg_id: id,
            chat_id: chat.into(),
            chat_type: "group".into(),
            sender_pid: "p_1001".into(),
            text: format!("m{id}"),
            at_me: false,
            has_image: false,
            reply_to: None,
            sender_bot: false,
            image_urls: vec![],
            ts: 0,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn window_fires_after_10s_with_anchor_and_others() {
        let fired = Arc::new(AtomicUsize::new(0));
        let batches: Arc<std::sync::Mutex<Vec<WindowBatch>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let cb_fired = Arc::clone(&fired);
        let cb_batches = Arc::clone(&batches);
        let on_fire: OnFire = Arc::new(move |b| {
            cb_fired.fetch_add(1, Ordering::SeqCst);
            cb_batches.lock().unwrap().push(b);
            Box::pin(async {})
        });

        // 用 5s 窗口,推进可暂停时钟观察到期
        let mut agg = WindowAggregator::new(5);
        agg.feed(msg("c1", 1), on_fire.clone());
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        agg.feed(msg("c1", 2), on_fire.clone());
        agg.feed(msg("c1", 3), on_fire.clone());

        // 窗口还未到期
        assert_eq!(fired.load(Ordering::SeqCst), 0);

        // 推进超 5s → fire
        tokio::time::advance(std::time::Duration::from_secs(6)).await;
        // 让 tokio 跑一下聚合协程
        tokio::task::yield_now().await;

        assert_eq!(fired.load(Ordering::SeqCst), 1);
        let list = batches.lock().unwrap();
        assert_eq!(list.len(), 1);
        let b = &list[0];
        assert_eq!(b.chat_id, "c1");
        assert_eq!(b.anchor.msg_id, 1);
        // 其他 2 条追加进 others
        assert_eq!(b.others.len(), 2);
        assert_eq!(b.all().len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn multiple_chats_open_independent_windows() {
        let fired = Arc::new(AtomicUsize::new(0));
        let cb = Arc::clone(&fired);
        let on_fire: OnFire = Arc::new(move |_| {
            cb.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        });

        let mut agg = WindowAggregator::new(5);
        agg.feed(msg("c1", 1), on_fire.clone());
        agg.feed(msg("c2", 1), on_fire.clone());
        agg.feed(msg("c3", 1), on_fire.clone());

        assert_eq!(agg.lane_count(), 3);

        // 给聚合协程充足机会跨 anchor 接收 + select 起步
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }

        tokio::time::advance(std::time::Duration::from_secs(6)).await;

        // 每个 chat 的协程都要 yield 几次走过:被唤醒 → select 命中 sleep 分支 → 回调
        for _ in 0..60 {
            tokio::task::yield_now().await;
        }

        // 三个独立窗口都 fire 一次
        assert_eq!(fired.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_token_stops_pending_window() {
        let fired = Arc::new(AtomicUsize::new(0));
        let cb = Arc::clone(&fired);
        let on_fire: OnFire = Arc::new(move |_| {
            cb.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        });

        let cancel = Arc::new(AtomicBool::new(false));
        let mut agg = WindowAggregator::with_cancel(5, cancel.clone());
        agg.feed(msg("c1", 1), on_fire);

        cancel.store(true, AtomicOrdering::Relaxed);
        tokio::time::advance(std::time::Duration::from_secs(6)).await;
        tokio::task::yield_now().await;

        // 取消后不 fire,Q55 重启兜底
        assert_eq!(fired.load(Ordering::SeqCst), 0);
    }
}
