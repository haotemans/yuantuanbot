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
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use tokio::task::JoinSet;
use tokio::time::{Duration, Instant};
use tracing::{debug, trace, warn};

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
    /// 消息直接进入批次，避免中转 channel 满时静默丢消息。
    lanes: Arc<Mutex<HashMap<String, VecDeque<PendingWindow>>>>,
    /// 聚合器拥有全部计时/回调任务；drop 时取消，交给 Q55 回放。
    workers: JoinSet<()>,
    cancel: Arc<AtomicBool>,
}

struct PendingWindow {
    deadline: Instant,
    batch: WindowBatch,
    on_fire: OnFire,
}

/// 同 chat 回调按窗口创建顺序 await，不影响其他 chat。
pub type OnFire = Arc<
    dyn Fn(WindowBatch) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

impl WindowAggregator {
    pub fn new(window_secs: u64) -> Self {
        Self {
            window_secs: window_secs.max(1),
            lanes: Arc::new(Mutex::new(HashMap::new())),
            workers: JoinSet::new(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_cancel(window_secs: u64, cancel: Arc<AtomicBool>) -> Self {
        Self {
            window_secs: window_secs.max(1),
            lanes: Arc::new(Mutex::new(HashMap::new())),
            workers: JoinSet::new(),
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
        while let Some(result) = self.workers.try_join_next() {
            if let Err(error) = result {
                warn!(%error, "Q54 窗口任务异常，未处理消息由 Q55 回放");
            }
        }
        if self.cancel.load(AtomicOrdering::Relaxed) {
            return;
        }
        let chat_id = m.chat_id.clone();
        let now = Instant::now();
        let mut lanes = self.lanes.lock().unwrap();
        let needs_worker = !lanes.contains_key(&chat_id);
        let queue = lanes.entry(chat_id.clone()).or_default();
        if let Some(window) = queue.back_mut() {
            if now < window.deadline {
                window.batch.others.push(m);
                return;
            }
        }
        trace!(%chat_id, anchor_msg = m.msg_id, "Q54 开窗");
        queue.push_back(PendingWindow {
            // 从 feed 收到 anchor 计时；调度或上一窗口回调耗时不延长窗口。
            deadline: now + Duration::from_secs(self.window_secs),
            batch: WindowBatch {
                chat_id: chat_id.clone(),
                anchor: m,
                others: Vec::new(),
            },
            on_fire,
        });
        drop(lanes);
        if !needs_worker {
            return;
        }
        let lanes = Arc::clone(&self.lanes);
        let cancel = Arc::clone(&self.cancel);
        self.workers.spawn(async move {
            // 回调 panic/取消也移除 lane，下一条消息能重新开窗。
            let mut cleanup = LaneCleanup { lanes, chat_id, armed: true };
            loop {
                let deadline = {
                    let mut lanes = cleanup.lanes.lock().unwrap();
                    let queue = lanes.get(&cleanup.chat_id).unwrap();
                    match queue.front() {
                        Some(window) => window.deadline,
                        None => {
                            lanes.remove(&cleanup.chat_id);
                            cleanup.armed = false;
                            return;
                        }
                    }
                };
                // 保留 AtomicBool 关闭接口；每窗口只创建一次取消 future。
                let cancelled = async {
                    while !cancel.load(AtomicOrdering::Relaxed) {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                };
                tokio::pin!(cancelled);
                tokio::select! {
                    biased;
                    _ = &mut cancelled => {
                        debug!(chat_id = %cleanup.chat_id, "Q54 窗口取消，消息由 Q55 回放");
                        return;
                    }
                    _ = tokio::time::sleep_until(deadline) => {}
                }
                let window = cleanup.lanes.lock().unwrap()
                    .get_mut(&cleanup.chat_id).unwrap().pop_front().unwrap();
                trace!(chat_id = %cleanup.chat_id, total = 1 + window.batch.others.len(), "Q54 窗口到期");
                tokio::select! {
                    biased;
                    _ = &mut cancelled => return,
                    _ = (window.on_fire)(window.batch) => {}
                }
            }
        });
    }

    pub fn lane_count(&self) -> usize {
        self.lanes.lock().unwrap().len()
    }
}

struct LaneCleanup {
    lanes: Arc<Mutex<HashMap<String, VecDeque<PendingWindow>>>>,
    chat_id: String,
    armed: bool,
}

impl Drop for LaneCleanup {
    fn drop(&mut self) {
        if self.armed {
            self.lanes.lock().unwrap().remove(&self.chat_id);
        }
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

    fn capture_batches() -> (OnFire, Arc<std::sync::Mutex<Vec<WindowBatch>>>) {
        let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = Arc::clone(&batches);
        let callback: OnFire = Arc::new(move |batch| {
            output.lock().unwrap().push(batch);
            Box::pin(async {})
        });
        (callback, batches)
    }

    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn same_chat_opens_consecutive_windows_and_releases_idle_lane() {
        let (on_fire, batches) = capture_batches();
        let mut agg = WindowAggregator::new(5);
        for id in 1..=3 {
            agg.feed(msg("c1", id), on_fire.clone());
            settle().await;
            tokio::time::advance(std::time::Duration::from_secs(6)).await;
            settle().await;
            assert_eq!(batches.lock().unwrap().len(), id as usize);
        }
        assert_eq!(agg.lane_count(), 0, "idle chats must release their lane");
    }

    #[tokio::test(start_paused = true)]
    async fn burst_keeps_every_message_in_order() {
        let (on_fire, batches) = capture_batches();
        let mut agg = WindowAggregator::new(5);
        // No yield between feeds: exceed the old 64-message channel capacity.
        for id in 1..=200 {
            agg.feed(msg("c1", id), on_fire.clone());
        }
        settle().await;
        tokio::time::advance(std::time::Duration::from_secs(6)).await;
        settle().await;
        let batches = batches.lock().unwrap();
        assert_eq!(batches.len(), 1);
        let ids: Vec<_> = batches[0].all().iter().map(|m| m.msg_id).collect();
        assert_eq!(ids, (1..=200).collect::<Vec<_>>());
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_aggregator_does_not_fire_pending_windows() {
        let (on_fire, batches) = capture_batches();
        let mut agg = WindowAggregator::new(5);
        agg.feed(msg("c1", 1), on_fire);
        settle().await;
        drop(agg);
        tokio::time::advance(std::time::Duration::from_secs(6)).await;
        settle().await;
        assert!(batches.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn slow_callback_preserves_deadlines_and_serial_order() {
        let (capture, batches) = capture_batches();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let callback_gate = gate.clone();
        let on_fire: OnFire = Arc::new(move |batch| {
            let id = batch.anchor.msg_id;
            let captured = capture(batch);
            let gate = callback_gate.clone();
            Box::pin(async move {
                captured.await;
                if id == 1 {
                    gate.acquire().await.unwrap().forget();
                }
            })
        });
        let mut agg = WindowAggregator::new(5);
        agg.feed(msg("c1", 1), on_fire.clone());
        tokio::time::advance(Duration::from_secs(6)).await;
        settle().await;
        agg.feed(msg("c1", 2), on_fire.clone());
        tokio::time::advance(Duration::from_secs(6)).await;
        settle().await;
        agg.feed(msg("c1", 3), on_fire);
        assert_eq!(batches.lock().unwrap().len(), 1);
        gate.add_permits(1);
        settle().await;
        assert_eq!(batches.lock().unwrap().len(), 2);
        tokio::time::advance(Duration::from_secs(6)).await;
        settle().await;
        let batches = batches.lock().unwrap();
        assert_eq!(
            batches.iter().map(|b| b.anchor.msg_id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(batches.iter().all(|b| b.others.is_empty()));
        assert_eq!(agg.lane_count(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn callback_panic_releases_lane_for_next_window() {
        let mut agg = WindowAggregator::new(5);
        agg.feed(msg("c1", 1), Arc::new(|_| panic!("callback panic")));
        tokio::time::advance(Duration::from_secs(6)).await;
        settle().await;
        assert_eq!(agg.lane_count(), 0);
        let (on_fire, batches) = capture_batches();
        agg.feed(msg("c1", 2), on_fire);
        tokio::time::advance(Duration::from_secs(6)).await;
        settle().await;
        assert_eq!(batches.lock().unwrap()[0].anchor.msg_id, 2);
    }
}
