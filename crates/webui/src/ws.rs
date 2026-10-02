//! /ws 实时推送：握手带 token（query param，浏览器 WS 无法设自定义头），
//! 校验后订阅 EventBus 全量事件转发 JSON 给前端（trace 页 / 仪表盘迷你流数据源）。

use crate::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct WsQuery {
    token: Option<String>,
}

pub async fn handler(
    State(state): State<AppState>,
    Query(q): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    let ok = q
        .token
        .as_deref()
        .map(|t| state.valid_session(t))
        .unwrap_or(false);
    if !ok {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(ws.on_upgrade(move |socket| forward(socket, state)))
}

async fn forward(mut socket: WebSocket, state: AppState) {
    let mut rx = state.extras.bus.subscribe();
    tracing::info!("WebUI /ws 客户端已连接");
    loop {
        tokio::select! {
            ev = rx.recv() => {
                match ev {
                    Ok(event) => {
                        if socket.send(Message::Text(event.payload_json().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
    tracing::info!("WebUI /ws 客户端断开");
}
