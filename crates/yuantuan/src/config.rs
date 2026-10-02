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
    pub consolidation: ConsolidationSection,
    pub reply: ReplySection,
    pub context: ContextSection,
    pub meme: MemeSection,
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
            consolidation: ConsolidationSection::default(),
            reply: ReplySection::default(),
            context: ContextSection::default(),
            meme: MemeSection::default(),
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
    /// Decision 成本闸（次/分）
    pub decision_cost_per_min: i64,
}

/// [reply] 回复形态参数（架构十四章；映射 yuantuan_core::reply_engine::ReplyCfg）
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ReplySection {
    /// 首泡延时区间（毫秒）
    pub first_delay_min_ms: u64,
    pub first_delay_max_ms: u64,
    /// 打字基线延时（毫秒）
    pub base_delay_ms: u64,
    /// 延时系数：每字毫秒
    pub per_char_ms: u64,
    /// 抖动比例 0~1
    pub jitter_ratio: f64,
    /// 单泡延时下限（毫秒）
    pub min_delay_ms: u64,
    /// 单泡延时上限/泡顶（毫秒）
    pub max_delay_ms: u64,
    /// 延时总预算（毫秒；超出后剩余泡不再延时）
    pub total_budget_ms: u64,
    /// 泡数封顶（超出并入最后一泡）
    pub bubble_cap: usize,
    /// 单泡字数上限（超出按标点机械切）
    pub bubble_char_cap: usize,
}

/// [context] 上下文预算（架构十二章；映射 yuantuan_core::context_builder::ContextCfg）
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ContextSection {
    /// 输入预算（字符代理值，40k tokens 代理）
    pub budget_chars: usize,
    /// 会话窗口 K（条）
    pub k: usize,
    /// 名册每人携带记忆条数
    pub roster_mem_per: usize,
}

impl Default for ReplySection {
    fn default() -> Self {
        let c = yuantuan_core::reply_engine::ReplyCfg::default();
        Self {
            first_delay_min_ms: c.first_delay_min_ms,
            first_delay_max_ms: c.first_delay_max_ms,
            base_delay_ms: c.base_delay_ms,
            per_char_ms: c.per_char_ms,
            jitter_ratio: c.jitter_ratio,
            min_delay_ms: c.min_delay_ms,
            max_delay_ms: c.max_delay_ms,
            total_budget_ms: c.total_budget_ms,
            bubble_cap: c.bubble_cap,
            bubble_char_cap: c.bubble_char_cap,
        }
    }
}

impl Default for ContextSection {
    fn default() -> Self {
        Self {
            budget_chars: 40_000,
            k: 20,
            roster_mem_per: 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ConsolidationSection {
    pub enabled: bool,
    /// 每日执行时刻（本地 HH:MM）
    pub daily_time: String,
    /// 调试用：启动后立即执行一次
    pub run_on_startup: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MemeSection {
    /// 偷表情包：群图片自动入待审区
    pub steal_enabled: bool,
}

impl Default for MemeSection {
    fn default() -> Self {
        Self { steal_enabled: true }
    }
}

impl Default for PrefilterSection {
    fn default() -> Self {
        Self {
            window_secs: 60,
            self_msg_cap: 12,
            decision_cost_per_min: 30,
        }
    }
}

impl Default for ConsolidationSection {
    fn default() -> Self {
        Self {
            enabled: true,
            daily_time: "03:00".into(),
            run_on_startup: false,
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
# 群聊发言节流（架构十三章节流闸；管理面板「运行参数」可热调）
window_secs = 60
self_msg_cap = 12
decision_cost_per_min = 30

[reply]
# 回复形态参数（架构十四章；面板热调即时生效）
first_delay_min_ms = 300
first_delay_max_ms = 800
base_delay_ms = 600
per_char_ms = 40
jitter_ratio = 0.3
min_delay_ms = 800
max_delay_ms = 4000
total_budget_ms = 8000
bubble_cap = 3
bubble_char_cap = 500

[context]
# 上下文预算（架构十二章；budget_chars 为 40k tokens 的字符代理）
budget_chars = 40000
k = 20
roster_mem_per = 3

[consolidation]
# 夜间归纳（架构七章）；daily_time 为本地 HH:MM
enabled = true
daily_time = "03:00"
# 调试用：启动后立即执行一次（生产保持 false）
run_on_startup = false

[meme]
# 偷表情包：群图片自动进 data/memes/_inbox/ 待审（WebUI 审批后入库）
steal_enabled = true

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
