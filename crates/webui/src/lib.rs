//! yuantuan-webui：axum API + WS 推送 + 内嵌占位前端。
//! 定稿依据：docs/runtime-design.md 第四章、docs/architecture-v0.1.md 第十五章安全小节。

mod auth;
mod config_api;
mod dashboard;
mod events_api;
mod memes;
mod memories;
mod overview;
mod personality;
mod relations;
mod state;
mod static_files;
mod tasks_api;
mod ws;

use anyhow::{Context, Result};
use axum::{middleware, routing::{get, post}, Router};
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::trace::TraceLayer;

pub use state::{AppState, Extras};

/// 组装路由并启动 HTTP 服务（阻塞至进程结束）
pub async fn serve(db_path: PathBuf, host: &str, port: u16, extras: Extras) -> Result<()> {
    let state = AppState::new(db_path, extras);

    // /api/* 中除 login 外一律过 token 校验
    let protected = Router::new()
        .route("/api/overview", get(overview::overview))
        .route("/api/dashboard", get(dashboard::dashboard))
        .route("/api/events", get(events_api::list))
        .route("/api/memes", get(memes::list))
        .route("/api/memes/{id}/approve", post(memes::approve))
        .route("/api/memes/{id}/reject", post(memes::reject))
        .route("/api/meme-file/{id}", get(memes::file))
        .route("/api/memories", get(memories::memories))
        .route("/api/summaries", get(memories::summaries))
        .route("/api/relations", get(relations::relations))
        .route("/api/config", get(config_api::get_config).post(config_api::post_config))
        .route("/api/personality/versions", get(personality::list).post(personality::create))
        .route("/api/personality/versions/{no}", get(personality::get_one))
        .route("/api/personality/rollback/{no}", post(personality::rollback))
        .route("/api/tasks", get(tasks_api::list))
        .route("/api/tasks/{id}/events", get(tasks_api::events))
        .route("/api/kb/status", get(kb_status))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_session,
        ));
    let api = Router::new()
        .route("/api/auth/login", post(auth::login))
        .merge(protected);

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/ws", get(ws::handler)) // token 走 query param（浏览器 WS 无自定义头），handler 内校验
        .merge(api)
        .fallback(static_files::serve)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("非法监听地址: {host}:{port}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("WebUI 监听失败: {addr}"))?;
    tracing::info!(%addr, "WebUI 开始监听");
    axum::serve(listener, app).await.context("WebUI 服务异常退出")
}

async fn healthz() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn kb_status() -> axum::Json<serde_json::Value> {
    // 知识库为二期可插拔功能（架构十五章扩展模型），默认关闭
    axum::Json(serde_json::json!({ "enabled": false, "docs": 0 }))
}
