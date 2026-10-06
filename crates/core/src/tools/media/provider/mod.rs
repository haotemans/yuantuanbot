//! MediaProvider：协议适配抽象（Q016/Q017）。一家 provider（如 NAI / OpenAI-Image / Gemini）
//! 各自实现 `MediaProvider` trait；上层（MediaImageTool）按 model 映射找到对应 provider 调用。

pub mod nai;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use super::{ImageArtifacts, ImageRequest};

/// provider 端点形态（Q016：NAI 双接口兼容的核心字段）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointStyle {
    /// NAI 私域协议：`POST /v1/nai/generate-image`，payload `{input, action, parameters:{...}}`
    NaiNative,
    /// OpenAI 兼容：`POST /v1/images/generations`，payload `{model, prompt, size, n, response_format}`
    OpenaiCompat,
    /// Gemini 生图协议（`models/{model_id}:generateContent`）——MOD-022h 落地
    Gemini,
    /// xAI 生图协议（`/images/generations` 自有 shape）——MOD-022h 落地
    Xai,
}

/// provider 配置（来自 media_providers 表）
#[derive(Debug, Clone)]
pub struct ProviderCfg {
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub default_endpoint: EndpointStyle,
}

/// MediaProvider trait：协议适配层
#[async_trait]
pub trait MediaProvider: Send + Sync {
    fn name(&self) -> &str;

    /// 当前 provider 支持的 endpoint 列表（NAI 同时支持 NaiNative + OpenaiCompat）
    fn supported_endpoints(&self) -> &[EndpointStyle];

    /// 模型支持的 ratio 集合（该 provider 的所有模型都一样时往上提）
    fn supported_ratios(&self, model_id: &str) -> Vec<String>;

    /// 执行一次生图；成功返回 ImageArtifacts，失败返回 Err（错误消息已脱敏）
    async fn generate(
        &self,
        req: &ImageRequest,
        model_id: &str,
        endpoint: EndpointStyle,
        data_dir: &std::path::Path,
    ) -> Result<ImageArtifacts>;

    /// 把内部错误转换为对外安全的文案（脱 API key / URL；AstrBot base.rs 移植要点）
    fn redact(&self, raw: &str) -> String {
        let mut out = raw.to_string();
        if let Some(k) = &self.cfg_ref().api_key {
            if k.len() >= 4 {
                out = out.replace(k, "***");
            }
        }
        out.chars().take(500).collect()
    }

    /// 取内部 ProviderCfg 引用（redact 用）
    fn cfg_ref(&self) -> &ProviderCfg;

    /// 通用 JSON request（保留给具体适配器使用）
    fn http_client(&self) -> &reqwest::Client;
}

/// serde helper：把 endpoint style 落库可读
pub fn endpoint_style_str(s: EndpointStyle) -> &'static str {
    match s {
        EndpointStyle::NaiNative => "nai_native",
        EndpointStyle::OpenaiCompat => "openai_compat",
        EndpointStyle::Gemini => "gemini",
        EndpointStyle::Xai => "xai",
    }
}

#[allow(dead_code)]
pub(crate) fn _assert_value_send(_v: Value) {}
