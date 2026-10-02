//! yuantuan-webui：axum API + 内嵌占位前端。
//! 定稿依据：docs/runtime-design.md 第四章、docs/architecture-v0.1.md 第十五章安全小节。

mod auth;
mod overview;
mod state;
mod static_files;

use anyhow::{Context, Result};
use axum::{middleware, routing::{get, post}, Router};
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::trace::TraceLayer;

pub use state::AppState;

/// 组装路由并启动 HTTP 服务（阻塞至进程结束）
pub async fn serve(db_path: PathBuf, host: &str, port: u16) -> Result<()> {
    let state = AppState::new(db_path);

    // /api/* 中除 login 外一律过 token 校验
    let protected = Router::new()
        .route("/api/overview", get(overview::overview))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_session,
        ));
    let api = Router::new()
        .route("/api/auth/login", post(auth::login))
        .merge(protected);

    let app = Router::new()
        .route("/healthz", get(healthz))
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
