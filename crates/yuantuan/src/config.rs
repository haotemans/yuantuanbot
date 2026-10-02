//! config.toml 加载：缺失时生成带注释的默认模板并回落默认值，任何失败都走 anyhow 不上 panic。

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub data: DataConfig,
    pub napcat: NapcatConfig,
    pub webui: WebuiConfig,
    pub prefilter: PrefilterSection,
    pub log: LogConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DataConfig {
    /// 运行数据根目录（yuantuan.db 与各子目录都在其下）
    pub dir: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NapcatConfig {
    /// 是否接入 NapCat（false 则跳过 adapter-qq）
    pub enabled: bool,
    /// NapCat OneBot 11 正向 WS 地址
    pub ws_url: String,
    pub token: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WebuiConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    pub level: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data: DataConfig::default(),
            napcat: NapcatConfig::default(),
            webui: WebuiConfig::default(),
            prefilter: PrefilterSection::default(),
            log: LogConfig::default(),
        }
    }
}

impl Default for DataConfig {
    fn default() -> Self {
        Self { dir: "data".into() }
    }
}

impl Default for NapcatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ws_url: "ws://127.0.0.1:3001".into(),
            token: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PrefilterSection {
    /// 发言节流窗口（秒）
    pub window_secs: i64,
    /// 窗口内 self 消息数硬顶（个）
    pub self_msg_cap: i64,
}

impl Default for PrefilterSection {
    fn default() -> Self {
        Self {
            window_secs: 60,
            self_msg_cap: 12,
        }
    }
}

impl Default for WebuiConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8085,
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self { level: "info".into() }
    }
}

const DEFAULT_TEMPLATE: &str = r#"# 云团主配置
# 运行数据全部落在 [data].dir 下（见 docs/data-model.md 第一章）

[data]
dir = "data"

[napcat]
# NapCat OneBot 11 正向 WS（见 docs/runtime-design.md 第六章）
# enabled = false 时不接入 NapCat（其余子系统照常运行）
enabled = true
ws_url = "ws://127.0.0.1:3001"
token = ""

[webui]
# 管理面板监听地址（仅本机；公网暴露走反代，见架构文档安全小节）
host = "127.0.0.1"
port = 8085

[prefilter]
# 群聊发言节流（架构十三章节流闸；热配在后续施工单接入）
window_secs = 60
self_msg_cap = 12

[log]
level = "info"
"#;

pub fn load_or_default(path: &str) -> Result<Config> {
    let p = Path::new(path);
    if !p.exists() {
        std::fs::write(p, DEFAULT_TEMPLATE)
            .with_context(|| format!("写入默认配置模板失败: {path}"))?;
        tracing::info!(path, "config.toml 不存在，已生成默认模板，继续使用内置默认值");
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(p)
        .with_context(|| format!("读取配置文件失败: {path}"))?;
    let cfg: Config = toml::from_str(&text)
        .with_context(|| format!("解析配置文件失败: {path}"))?;
    Ok(cfg)
}
