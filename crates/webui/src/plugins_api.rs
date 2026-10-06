//! Plugins 面板后端：
//! GET  /api/plugins/list       扫 plugins/<name>/ 列出每个插件状态
//! POST /api/plugins/toggle     切换 enabled 标记文件
//!
//! 启禁只动标记文件，必须重启后端才生效（编译期加载 + 启动期 register）

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct PluginInfo {
    name: String,
    enabled: bool,
    version: Option<String>,
    description: Option<String>,
    has_data_dir: bool,
}

impl PluginInfo {
    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "enabled": self.enabled,
            "version": self.version,
            "description": self.description,
            "data_dir": if self.has_data_dir {
                Some(format!("data/plugins/{}", self.name))
            } else {
                None
            },
        })
    }
}

/// 从 Cargo.toml 提取 package.version + description
fn read_cargo_meta(plugin_dir: &Path) -> (Option<String>, Option<String>) {
    let path = plugin_dir.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (None, None);
    };
    let Ok(v) = text.parse::<toml::Value>() else {
        return (None, None);
    };
    let pkg = v.get("package");
    let version = pkg
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let description = pkg
        .and_then(|p| p.get("description"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (version, description)
}

fn scan_plugins(plugins_dir: &Path) -> Vec<PluginInfo> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(plugins_dir) else {
        return out;
    };
    for e in rd.flatten() {
        let path = e.path();
        if !path.is_dir() {
            continue;
        }
        let name = match e.file_name().to_str() {
            Some(s) if !s.starts_with('.') => s.to_string(),
            _ => continue,
        };
        if name == "README.md" || name.starts_with('_') {
            continue;
        }
        if !path.join("Cargo.toml").exists() {
            continue;
        }
        let (version, description) = read_cargo_meta(&path);
        let enabled = path.join("enabled").exists();
        let has_data_dir = path.join("data").exists();
        out.push(PluginInfo {
            name,
            enabled,
            version,
            description,
            has_data_dir,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn plugins_dir_from_state(state: &AppState) -> PathBuf {
    // 主目录：从 db_path（data/yuantuan.db）往上到项目根再往 plugins/
    // db_path = <root>/data/yuantuan.db
    // plugins = <root>/plugins
    state
        .db_path
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(std::path::Path::new("."))
        .join("plugins")
}

pub async fn list(State(state): State<AppState>) -> Json<Value> {
    let dir = plugins_dir_from_state(&state);
    let plugins = scan_plugins(&dir);
    let loaded = state.extras.tools_registry.names();
    Json(json!({
        "ok": true,
        "plugins": plugins.iter().map(|p| {
            let mut j = p.to_json();
            // 实际运行时是否被加载进 Registry（与磁盘 enabled 标记可能不一致——重启才能同步）
            if let Some(obj) = j.as_object_mut() {
                let is_loaded = loaded.iter().any(|n| *n == p.name.as_str());
                obj.insert("loaded".into(), json!(is_loaded));
            }
            j
        }).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
pub struct ToggleBody {
    name: String,
    enabled: bool,
}

pub async fn toggle(
    State(state): State<AppState>,
    Json(body): Json<ToggleBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if body.name.is_empty() || body.name.contains('/') || body.name.contains('\\') || body.name.contains("..") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "插件名非法" })),
        ));
    }
    let plugins_dir = plugins_dir_from_state(&state);
    let plugin_dir = plugins_dir.join(&body.name);
    if !plugin_dir.exists() || !plugin_dir.join("Cargo.toml").exists() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": format!("插件 {} 不存在", body.name) })),
        ));
    }
    let marker = plugin_dir.join("enabled");
    if body.enabled {
        std::fs::write(&marker, b"").map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": format!("写 enabled 标记失败：{e}") })),
            )
        })?;
    } else {
        if marker.exists() {
            std::fs::remove_file(&marker).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "ok": false, "error": format!("删除 enabled 标记失败：{e}") })),
                )
            })?;
        }
    }
    Ok(Json(json!({
        "ok": true,
        "name": body.name,
        "enabled": body.enabled,
        "message": "已写标记文件；重启 yuantuan 后端后生效（编译期插件需要重启才能改注册表）",
    })))
}
