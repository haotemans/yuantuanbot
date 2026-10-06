//! 媒体生成工具体系（Q007/Q012/Q013/Q016）：
//! - `MediaProvider`：协议层抽象（NAI / OpenAI 兼容 / Gemini / ...），每家一个适配器
//! - `MediaImageTool`：实现 `Tool` trait，挂到全局 Registry；命令直派和 Agent 都用同一个实例
//! - 落盘：图片落 `data/artifacts/media/`，发回 chat 走 OneBot image 段 `file:///`（Q012）
//!
//! 不在这里做的事（按裁决分散）：
//! - 命令解析（`-m/-r/--seed` 等）→ `parser.rs`
//! - 提示词优化 → `prompt.rs`
//! - 配额/余额/权限校验 → `quota.rs`
//! - 协议 endpoint 适配 → `provider/*.rs`

pub mod command;
pub mod params;
pub mod parser;
pub mod prompt;
pub mod provider;
pub mod quota;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 一次生图调用所需的全部信息（parser 输出的最终形态）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageRequest {
    /// media_models.alias（WebUI 配置的别名）
    pub model_alias: String,
    /// media_providers.name
    pub provider_name: String,
    /// 已优化后的最终 prompt（Q010 prompt.rs 输出；若优化失败则等于用户原文）
    pub prompt: String,
    /// 用户原始 prompt（trace 与 fallback 用）
    pub raw_prompt: String,
    /// 比例（"1:1"/"16:9"/"2:3"/...）；像素尺寸由 provider 内部映射
    pub ratio: String,
    pub count: u32,
    pub seed: Option<i64>,
    pub negative: Option<String>,
    pub quality: Option<String>,
    /// 用户显式给的种子；缺省时由 quota.rs 回查 chat.last_media_seed 注入（Q014）
    pub seed_from_chat: bool,
}

/// 一次生图调用的产物
#[derive(Debug, Clone)]
pub struct ImageArtifacts {
    /// 落盘绝对路径列表（一张图一个）
    pub paths: Vec<PathBuf>,
    /// 实际使用的 seed（用于回写 chat.last_media_seed）
    pub seed: Option<i64>,
    /// 实际走的 endpoint_style（nai_native / openai_compat / gemini 等）
    pub endpoint_style: String,
    /// 调用耗时（毫秒）
    pub elapsed_ms: u64,
}

/// 图片落盘（Q012）：写到 `data/artifacts/media/{ts_ms}_{idx}.{ext}`，返回绝对路径集
pub fn save_artifacts(
    data_dir: &Path,
    images_b64: &[String],
    ext: &str,
) -> Result<Vec<PathBuf>> {
    use base64::Engine as _;
    let dir = data_dir.join("artifacts").join("media");
    std::fs::create_dir_all(&dir)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut out = Vec::with_capacity(images_b64.len());
    for (idx, b64) in images_b64.iter().enumerate() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| anyhow::anyhow!("base64 解码失败(idx={idx}): {e}"))?;
        let path = dir.join(format!("{ts}_{idx}.{ext}"));
        std::fs::write(&path, bytes)?;
        out.push(path);
    }
    Ok(out)
}

/// 生成 OneBot image 段（Q012）：`[{type:"image", data:{file: file:///abs/path}}, ...]`
pub fn image_segments(paths: &[PathBuf]) -> serde_json::Value {
    let segs: Vec<_> = paths
        .iter()
        .map(|p| {
            serde_json::json!({
                "type": "image",
                "data": { "file": crate::meme::file_url(p) },
            })
        })
        .collect();
    serde_json::Value::Array(segs)
}
