//! 反向 WS 服务器：yuantuan 起 axum 监听 `GET /ws`，NapCat 作为 Websockets客户端连进来。
//! 与 AstrBot 的 aiocqhttp 同形态：协议端主动连出，bot 框架起服务器接。
//! 鉴权：`Authorization: Bearer <token>`（NapCat 卡片里 Token 字段）；token 为空则不校验。

use anyhow::{Context, Result};
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::path::PathBuf;
use tracing::{debug, info, warn};
use yuantuan_core::event::EventBus;
use yuantuan_core::prefilter::SelfMsgIds;

use crate::client::{
    handle_frame, new_pending, next_echo, AdapterHandle, NapcatConfig, NapcatSender,
};

#[derive(Clone)]
struct ServerState {
    bus: EventBus,
    db_path: PathBuf,
    cfg: NapcatConfig,
    handle: AdapterHandle,
    self_ids: SelfMsgIds,
}

/// 启动反向 WS 服务器（永不返回），返回供 core 取发送端/自身号/连接状态的句柄
pub fn spawn(
    bus: EventBus,
    db_path: PathBuf,
    cfg: NapcatConfig,
    self_ids: SelfMsgIds,
) -> AdapterHandle {
    let handle = AdapterHandle::default();
    let state = ServerState {
        bus,
        db_path,
        cfg: cfg.clone(),
        handle: handle.clone(),
        self_ids,
    };
    tokio::spawn(async move {
        if let Err(e) = serve(state).await {
            warn!(error = %e, "反向 WS 服务器异常退出");
        }
    });
    handle
}

async fn serve(state: ServerState) -> Result<()> {
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind(&state.cfg.listen_addr)
        .await
        .with_context(|| format!("反向 WS 监听失败：{}", state.cfg.listen_addr))?;
    info!(listen_addr = %state.cfg.listen_addr, "反向 WS 服务器已监听，等待 NapCat Websockets客户端连入");
    axum::serve(listener, app)
        .await
        .context("反向 WS 服务异常退出")
}

async fn ws_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    if !state.handle.is_receiving() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    // 每次连接都读最新 token，热应用立即生效
    let expected_token = state.cfg.token.read().unwrap().clone();
    if !expected_token.is_empty() {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let expected = format!("Bearer {}", expected_token);
        if auth != expected {
            warn!(got_auth = ?headers.get("authorization"), "反向 WS 鉴权失败：token 不匹配");
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(ws.on_upgrade(move |socket| session(socket, state)))
}

/// 单个 NapCat 连接的全生命周期：握手 → 主循环 → 掉线清理
/// 新连接到达时旧 sender 自动被替换（handle.current 是新增赋值语义）
async fn session(socket: WebSocket, state: ServerState) {
    info!("NapCat 反向 WS 已连接，开始 get_login_info 握手");
    let (write, mut read) = socket.split();
    let write = std::sync::Arc::new(tokio::sync::Mutex::new(write));
    let pending = new_pending();
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));

    // 1. 握手：get_login_info 拿自身 QQ 号（at_me 判定）
    let echo = next_echo(&counter);
    if let Err(e) = send_action_axum(&write, "get_login_info", serde_json::json!({}), &echo).await {
        warn!(error = %e, "发送 get_login_info 失败，断开");
        return;
    }
    let self_qq = loop {
        let frame = match tokio::time::timeout(crate::client::ECHO_TIMEOUT, read.next()).await {
            Ok(Some(Ok(f))) => f,
            Ok(Some(Err(e))) => {
                warn!(error = %e, "握手期读帧失败");
                return;
            }
            Ok(None) => {
                warn!("get_login_info 响应前连接被关闭");
                return;
            }
            Err(_) => {
                warn!("get_login_info 回执超时");
                return;
            }
        };
        let AxumWsMessage::Text(text) = frame else {
            continue;
        };
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if v.get("echo").and_then(|e| e.as_str()) == Some(echo.as_str()) {
            match v.pointer("/data/user_id").and_then(|u| u.as_u64()) {
                Some(uid) => break uid,
                None => {
                    warn!("get_login_info 响应缺少 data.user_id");
                    return;
                }
            }
        }
        // 握手期混入的帧照常处理；此时自身号未知，at_me 退化
        if state.handle.is_receiving() {
            let _ =
                handle_frame(&v, &state.bus, &state.db_path, 0, &pending, &state.self_ids).await;
        }
    };
    state.handle.store_self_qq(self_qq);
    state.handle.store_connected(true);
    info!(self_qq, "get_login_info 完成");

    // 2. 装配 sender，让 core 侧能拿发送能力
    let sender = NapcatSender::new_axum(write.clone(), pending.clone(), counter);
    state.handle.store_sender(sender).await;

    // 3. 主循环：与 client.rs 相同的帧分发
    loop {
        let Some(frame) = read.next().await else {
            state.handle.on_disconnect().await;
            info!(self_qq, "NapCat WS 连接被对端关闭，等待下一次连入");
            return;
        };
        state.handle.touch_active();
        match frame {
            Ok(AxumWsMessage::Text(text)) => {
                let v: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!(error = %e, "忽略非法 JSON 帧");
                        continue;
                    }
                };
                if !state.handle.is_receiving() && v.get("echo").is_none() {
                    continue;
                }
                if let Err(e) = handle_frame(
                    &v,
                    &state.bus,
                    &state.db_path,
                    self_qq,
                    &pending,
                    &state.self_ids,
                )
                .await
                {
                    warn!(error = %e, "帧处理失败");
                }
            }
            Ok(AxumWsMessage::Close(_)) => {
                state.handle.on_disconnect().await;
                info!(self_qq, "收到 Close 帧，等待下一次连入");
                return;
            }
            Ok(_) => {}
            Err(e) => {
                state.handle.on_disconnect().await;
                warn!(error = %e, self_qq, "WS 帧错误，等待下一次连入");
                return;
            }
        }
    }
}

/// axum WS 发 action 帧（与 client.rs::send_action 等效，但走 axum Message）
pub async fn send_action_axum(
    write: &tokio::sync::Mutex<futures_util::stream::SplitSink<WebSocket, AxumWsMessage>>,
    action: &str,
    params: Value,
    echo: &str,
) -> Result<()> {
    let frame = serde_json::json!({ "action": action, "params": params, "echo": echo }).to_string();
    let mut w = write.lock().await;
    w.send(AxumWsMessage::Text(frame.into()))
        .await
        .with_context(|| format!("发送 action 失败: {action}"))
}
