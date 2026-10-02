//! 静态资源：rust-embed 内嵌 Vue 构建产物（webui-frontend/vite build → static/）。
//! SPA 回退：未知路径回 index.html（客户端路由接管）；产物缺失时兜底 legacy-index.html。
//! 产物入库是刻意的——无 node 的机器也能 cargo build（重新生成方法见根 README）。

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

pub async fn serve(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = Assets::get(path) {
        return ([(header::CONTENT_TYPE, mime_of(path))], file.data).into_response();
    }
    // SPA 回退（/trace、/personality 等前端路由直开）
    if let Some(file) = Assets::get("index.html") {
        return ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], file.data).into_response();
    }
    // 构建产物缺失兜底（无 node 机器仓库快照）
    if let Some(file) = Assets::get("legacy-index.html") {
        return ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], file.data).into_response();
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("mjs") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}
