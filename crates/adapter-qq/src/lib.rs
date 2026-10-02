//! yuantuan-adapter-qq：OneBot 11 客户端。
//! 设计依据 docs/runtime-design.md 第六章：正向 WS、段数组映射铁律（core 不见 CQ 码）、
//! 双工 echo 回执 10s 超时、断线接受丢失（指数退避重连 1s→60s）。

mod client;
mod ingest;

pub use client::{send_fn, spawn, AdapterHandle, NapcatConfig, NapcatSender};
