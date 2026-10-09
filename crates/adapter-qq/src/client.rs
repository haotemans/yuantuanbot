//! OneBot 11 协议的握手/分发/回执路由公共逻辑（传输无关）。
//! 传输层在 server.rs：反向 WS 服务器，yuantuan 起 axum 监听 :6199/ws，NapCat 主动连入。

use anyhow::{bail, Context, Result};
use futures_util::SinkExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use tracing::debug;
use yuantuan_core::event::EventBus;
use yuantuan_core::prefilter::SelfMsgIds;

pub const ECHO_TIMEOUT: Duration = Duration::from_secs(10);

pub type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

/// Pending 的便捷构造（server.rs 的 `Default::default()` 会用到）
pub fn new_pending() -> Pending {
    Arc::new(Mutex::new(HashMap::new()))
}

/// 共享的 access token（重导出 core 类型；本 crate 内部与外部调用都用这一个）。
/// 仅 token 支持热应用；listen_addr 改动仍需重启进程（端口绑定是一次性的）。
pub use yuantuan_core::napcat_slot::{shared_token, SharedToken};

#[derive(Debug, Clone)]
pub struct NapcatConfig {
    /// 反向 WS 监听地址，例如 "127.0.0.1:6199"。NapCat Websockets客户端 URL 填 `ws://{listen_addr}/ws`
    pub listen_addr: String,
    /// 共享 token：ws_handler 在每次连接进来时 read，热应用侧可 write 立即生效
    pub token: SharedToken,
}

/// 暴露给装配层的句柄：拿当前会话的发送端（断线期为 None），自身 QQ 号（登录前为 0），连接活性。
#[derive(Clone)]
pub struct AdapterHandle {
    current: Arc<AsyncMutex<Option<NapcatSender>>>,
    self_qq: Arc<AtomicU64>,
    connected: Arc<std::sync::atomic::AtomicBool>,
    receiving: Arc<std::sync::atomic::AtomicBool>,
    last_active: Arc<Mutex<Instant>>,
}

impl Default for AdapterHandle {
    fn default() -> Self {
        Self {
            current: Arc::new(AsyncMutex::new(None)),
            self_qq: Arc::new(AtomicU64::new(0)),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            receiving: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            last_active: Arc::new(Mutex::new(Instant::now())),
        }
    }
}

impl AdapterHandle {
    /// 停止接收新业务消息，保留当前连接处理发送回执以便排空。
    pub fn stop_receiving(&self) { self.receiving.store(false, Ordering::Release); }

    pub(crate) fn is_receiving(&self) -> bool { self.receiving.load(Ordering::Acquire) }

    pub async fn sender(&self) -> Option<NapcatSender> {
        self.current.lock().await.clone()
    }

    pub fn self_qq(&self) -> u64 {
        self.self_qq.load(Ordering::Relaxed)
    }

    /// 自身 QQ 号的共享单元（core 侧注入用，遵守 core 不依赖协议端的铁律）
    pub fn self_qq_shared(&self) -> Arc<AtomicU64> {
        self.self_qq.clone()
    }

    /// 连接状态：已连接且最近 30s 内有任何帧活动（心跳/消息/回执都算）
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed) && self.last_active.lock().unwrap().elapsed() <= Duration::from_secs(30)
    }

    // —— server.rs 内部使用的修改器（pub(crate)，装配层不直接接触）——

    pub(crate) async fn store_sender(&self, sender: NapcatSender) {
        *self.current.lock().await = Some(sender);
    }

    pub(crate) fn store_self_qq(&self, qq: u64) {
        self.self_qq.store(qq, Ordering::Relaxed);
    }

    pub(crate) fn store_connected(&self, v: bool) {
        self.connected.store(v, Ordering::Relaxed);
    }

    pub(crate) fn touch_active(&self) {
        *self.last_active.lock().unwrap() = Instant::now();
    }

    pub(crate) async fn on_disconnect(&self) {
        *self.current.lock().await = None;
        self.connected.store(false, Ordering::Relaxed);
    }
}

/// 装配助手：把句柄包成 core 回复引擎的 SendFn（断线期调用返回错误，由引擎退避重试）
pub fn send_fn(handle: AdapterHandle) -> yuantuan_core::reply_engine::SendFn {
    use yuantuan_core::reply_engine::ChatType;
    std::sync::Arc::new(move |req| {
        let h = handle.clone();
        Box::pin(async move {
            let s = h
                .sender()
                .await
                .ok_or_else(|| anyhow::anyhow!("adapter 当前未连接 NapCat"))?;
            match req.chat_type {
                ChatType::Group => s.send_group_msg(req.target, req.segments).await,
                ChatType::Private => s.send_private_msg(req.target, req.segments).await,
            }
        })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = anyhow::Result<serde_json::Value>> + Send>,
            >
    })
}

/// 帧分发（传输无关）：action 回执优先路由，meta_event 跳过，message/message_sent 走 ingest
pub(crate) fn handle_frame(
    v: &Value,
    bus: &EventBus,
    db_path: &Path,
    self_qq: u64,
    pending: &Pending,
    self_ids: &SelfMsgIds,
) -> Result<()> {
    if let Some(echo) = v.get("echo").and_then(|e| e.as_str()) {
        if let Some(tx) = pending.lock().unwrap().remove(echo) {
            let _ = tx.send(v.clone());
        }
        return Ok(());
    }
    match v.get("post_type").and_then(|t| t.as_str()) {
        Some("meta_event") => Ok(()),
        Some("message") | Some("message_sent") => {
            crate::ingest::ingest_message(v, bus, db_path, self_qq, self_ids)
        }
        Some(other) => {
            debug!(post_type = other, "忽略非 message 事件");
            Ok(())
        }
        None => {
            debug!("忽略无法分类的帧");
            Ok(())
        }
    }
}

pub(crate) fn next_echo(counter: &AtomicU64) -> String {
    format!("yt-{}", counter.fetch_add(1, Ordering::Relaxed))
}

/// 发送端：action + echo，10s 回执超时（回执路由见 handle_frame）。
/// 写端类型只在 server.rs 装配时确定，这里保留具体类型（axum 的 SplitSink<WebSocket>）。
#[derive(Clone)]
pub struct NapcatSender {
    write: Arc<AsyncMutex<futures_util::stream::SplitSink<axum::extract::ws::WebSocket, axum::extract::ws::Message>>>,
    pending: Pending,
    counter: Arc<AtomicU64>,
}

impl NapcatSender {
    pub(crate) fn new_axum(
        write: Arc<AsyncMutex<futures_util::stream::SplitSink<axum::extract::ws::WebSocket, axum::extract::ws::Message>>>,
        pending: Pending,
        counter: Arc<AtomicU64>,
    ) -> Self {
        Self { write, pending, counter }
    }

    async fn action(&self, action: &str, params: Value) -> Result<Value> {
        let echo = next_echo(&self.counter);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(echo.clone(), tx);
        {
            let mut w = self.write.lock().await;
            let frame = json!({ "action": action, "params": params, "echo": echo }).to_string();
            w.send(axum::extract::ws::Message::Text(frame.into()))
                .await
                .with_context(|| format!("发送 action 失败: {action}"))?;
        }
        let resp = tokio::time::timeout(ECHO_TIMEOUT, rx)
            .await
            .with_context(|| format!("action {action} 回执超时（10s）"))?
            .context("action 回执通道被丢弃")?;
        let retcode = resp.get("retcode").and_then(|c| c.as_i64()).unwrap_or(-1);
        if retcode != 0 {
            bail!("action {action} 被 NapCat 拒绝: {resp}");
        }
        Ok(resp.get("data").cloned().unwrap_or(Value::Null))
    }

    /// 发送群消息（message 为 OneBot 段数组），回复形态引擎使用
    pub async fn send_group_msg(&self, group_id: u64, message: Value) -> Result<Value> {
        self.action(
            "send_group_msg",
            json!({ "group_id": group_id, "message": message }),
        )
        .await
    }

    /// 发送私聊消息（message 为 OneBot 段数组）
    pub async fn send_private_msg(&self, user_id: u64, message: Value) -> Result<Value> {
        self.action(
            "send_private_msg",
            json!({ "user_id": user_id, "message": message }),
        )
        .await
    }

}
