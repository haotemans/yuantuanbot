//! yuantuan-core：云团全部领域系统（纯逻辑，不碰 Web/网络框架）。
//! 各子系统本单仅占位，见 docs/runtime-design.md 第一章。

pub mod agent;
pub mod backup;
pub mod bot;
pub mod consolidation;
pub mod context_builder;
pub mod db;
pub mod decision;
pub mod event;
pub mod llm;
pub mod meme;
pub mod memory;
pub mod napcat_slot;
pub mod prefilter;
pub mod reply_engine;
pub mod state;
pub mod tools;
