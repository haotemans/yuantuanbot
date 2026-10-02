//! 正向 WS 客户端：连接 NapCat、get_login_info 握手、事件分发、action 回执路由。

use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, info, warn};
use yuantuan_core::event::EventBus;
use yuantuan_core::prefilter::SelfMsgIds;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsWrite = futures_util::stream::SplitSink<Ws, Message>;
type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

const ECHO_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_INIT: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct NapcatConfig {
    pub ws_url: String,
    pub token: String,
}

/// 暴露给装配层的句柄：拿当前会话的发送端（断线期为 None），以及自身 QQ 号（登录前为 0）。
/// 发送能力（send_group_msg / send_private_msg）为回复形态引擎预留。
#[derive(Clone, Default)]
pub struct AdapterHandle {
    current: Arc<AsyncMutex<Option<NapcatSender>>>,
    self_qq: Arc<AtomicU64>,
}

impl AdapterHandle {
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
}

/// 启动 adapter 后台任务（指数退避重连，永不返回），返回句柄
pub fn spawn(bus: EventBus, db_path: PathBuf, cfg: NapcatConfig, self_ids: SelfMsgIds) -> AdapterHandle {
    let handle = AdapterHandle::default();
    let task_handle = handle.clone();
    tokio::spawn(async move {
        run(bus, db_path, cfg, task_handle, self_ids).await;
    });
    handle
}

async fn run(bus: EventBus, db_path: PathBuf, cfg: NapcatConfig, handle: AdapterHandle, self_ids: SelfMsgIds) {
    let mut backoff = BACKOFF_INIT;
    loop {
        let started = Instant::now();
        match session(&bus, &db_path, &cfg, &handle, &self_ids).await {
            Ok(()) => info!("NapCat 会话正常结束"),
            Err(e) => warn!(error = %e, "NapCat 连接断开（窗口期消息接受丢失）"),
        }
        *handle.current.lock().await = None;
        // 会话存活超过 30s 视为有效连接，退避重置
        if started.elapsed() > Duration::from_secs(30) {
            backoff = BACKOFF_INIT;
        }
        info!(retry_in_secs = backoff.as_secs(), "退避后重连 NapCat");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

async fn session(
    bus: &EventBus,
    db_path: &PathBuf,
    cfg: &NapcatConfig,
    handle: &AdapterHandle,
    self_ids: &SelfMsgIds,
) -> Result<()> {
    let mut req = cfg
        .ws_url
        .as_str()
        .into_client_request()
        .with_context(|| format!("WS URL 非法: {}", cfg.ws_url))?;
    if !cfg.token.is_empty() {
        let value = format!("Bearer {}", cfg.token)
            .parse()
            .context("access_token 含非法字符")?;
        req.headers_mut().insert("Authorization", value);
    }
    let (ws, resp) = connect_async(req).await.context("连接 NapCat 失败")?;
    info!(url = %cfg.ws_url, status = %resp.status(), "NapCat WS 已连接");
    let (mut write, mut read) = ws.split();
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let counter = Arc::new(AtomicU64::new(1));
    let mut last_active = Instant::now();

    // 握手：get_login_info 拿自身 QQ 号（at_me 判定），10s 超时判失败
    let echo = next_echo(&counter);
    send_action(&mut write, "get_login_info", json!({}), &echo).await?;
    let self_qq = loop {
        let frame = match tokio::time::timeout(ECHO_TIMEOUT, read.next()).await {
            Ok(Some(f)) => f.context("读取 WS 帧失败")?,
            Ok(None) => bail!("get_login_info 响应前连接被关闭"),
            Err(_) => bail!("get_login_info 回执超时（10s）"),
        };
        let Message::Text(text) = frame else { continue };
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if v.get("echo").and_then(|e| e.as_str()) == Some(echo.as_str()) {
            let uid = v
                .pointer("/data/user_id")
                .and_then(|u| u.as_u64())
                .context("get_login_info 响应缺少 data.user_id")?;
            break uid;
        }
        // 握手期间混入的帧照常处理；此时自身号未知，at_me 判定退化为不匹配
        let _ = handle_frame(&v, bus, db_path, 0, &pending, &mut last_active, self_ids);
    };
    handle.self_qq.store(self_qq, Ordering::Relaxed);
    info!(self_qq, "get_login_info 完成");

    let sender = NapcatSender {
        write: Arc::new(AsyncMutex::new(write)),
        pending: pending.clone(),
        counter,
    };
    *handle.current.lock().await = Some(sender);

    // 主循环：meta_event 只更新活性时间戳；message/message_sent 走段数组摄取；response 路由回执
    loop {
        let Some(frame) = read.next().await else {
            bail!("WS 连接被对端关闭");
        };
        match frame {
            Ok(Message::Text(text)) => {
                let v: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!(error = %e, "忽略非法 JSON 帧");
                        continue;
                    }
                };
                if let Err(e) = handle_frame(&v, bus, db_path, self_qq, &pending, &mut last_active, self_ids) {
                    warn!(error = %e, "帧处理失败");
                }
            }
            Ok(Message::Close(_)) => bail!("收到 Close 帧"),
            Ok(_) => {}
            Err(e) => return Err(e.into()),
        }
    }
}

fn handle_frame(
    v: &Value,
    bus: &EventBus,
    db_path: &PathBuf,
    self_qq: u64,
    pending: &Pending,
    last_active: &mut Instant,
    self_ids: &SelfMsgIds,
) -> Result<()> {
    // action 回执一律优先路由（echo 存在即为响应帧）
    if let Some(echo) = v.get("echo").and_then(|e| e.as_str()) {
        if let Some(tx) = pending.lock().unwrap().remove(echo) {
            let _ = tx.send(v.clone());
        }
        return Ok(());
    }
    match v.get("post_type").and_then(|t| t.as_str()) {
        Some("meta_event") => {
            *last_active = Instant::now();
            Ok(())
        }
        // message_sent = 自己发出的消息回报：同样落库（sender_pid='self'），供 R6 节流计数与流水完整
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

fn next_echo(counter: &AtomicU64) -> String {
    format!("yt-{}", counter.fetch_add(1, Ordering::Relaxed))
}

async fn send_action(write: &mut WsWrite, action: &str, params: Value, echo: &str) -> Result<()> {
    let frame = json!({ "action": action, "params": params, "echo": echo }).to_string();
    write
        .send(Message::Text(frame.into()))
        .await
        .with_context(|| format!("发送 action 失败: {action}"))
}

/// 发送端：action + echo，10s 回执超时（回执路由见 handle_frame）
#[derive(Clone)]
pub struct NapcatSender {
    write: Arc<AsyncMutex<WsWrite>>,
    pending: Pending,
    counter: Arc<AtomicU64>,
}

impl NapcatSender {
    async fn action(&self, action: &str, params: Value) -> Result<Value> {
        let echo = next_echo(&self.counter);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(echo.clone(), tx);
        {
            let mut w = self.write.lock().await;
            send_action(&mut w, action, params, &echo).await?;
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
