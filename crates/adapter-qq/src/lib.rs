//! yuantuan-adapter-qq：OneBot 11 反向 WS 服务器（与 AstrBot 的 aiocqhttp 同形态）。
//! yuantuan 起 axum 监听 `GET /ws`（默认 127.0.0.1:6199），NapCat 作为 Websockets客户端连入。
//! 铁律：core 不见 CQ 码（段数组）、echo 回执路由、掉线仅记录等待 NapCat 重连。

mod client;
mod ingest;
mod server;

pub use client::{send_fn, shared_token, AdapterHandle, NapcatConfig, NapcatSender, SharedToken};
pub use server::spawn;
