//! NAI 适配器（Q016）：双接口兼容（nai_native + openai_compat），共用 base_url + NAI_API_KEY

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::json;

use super::{EndpointStyle, MediaProvider, ProviderCfg};
use crate::tools::media::params::{nai_dimensions, with_quality_tags, NAI_NEGATIVE_PROMPT};
use crate::tools::media::{ImageArtifacts, ImageRequest};

pub struct NaiProvider {
    cfg: ProviderCfg,
    http: reqwest::Client,
}

impl NaiProvider {
    pub fn new(cfg: ProviderCfg) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(310))
            .build()
            .expect("build nai http client");
        Self { cfg, http }
    }

    /// nai_native 形态调用：`POST {base}/v1/nai/generate-image`
    async fn call_nai_native(&self, req: &ImageRequest, model_id: &str) -> Result<Vec<String>> {
        let (w, h) = nai_dimensions(&req.ratio)
            .ok_or_else(|| anyhow::anyhow!("NAI 不支持比例 {}", req.ratio))?;
        // Q018：质量标签注入 prompt 头部（用户已含任一标签则跳过）
        let final_prompt = with_quality_tags(&req.prompt);
        let body = json!({
            "input": final_prompt,
            "model": model_id,
            "action": "generate",
            "parameters": {
                "width": w,
                "height": h,
                "steps": 28,
                "n_samples": req.count,
                "sampler": "k_euler_ancestral",
                "scale": 4.5,
                "seed": req.seed.unwrap_or(0),
                "negative_prompt": req.negative.clone().unwrap_or_else(|| NAI_NEGATIVE_PROMPT.to_string()),
                "noise_schedule": "karras",
                "qualityToggle": true,
                "ucPreset": 0,
                "params_version": 3,
            }
        });
        let mut rb = self
            .http
            .post(format!("{}/v1/nai/generate-image", self.cfg.base_url.trim_end_matches('/')))
            .json(&body);
        if let Some(k) = &self.cfg.api_key {
            rb = rb.bearer_auth(k);
        }
        let started = std::time::Instant::now();
        let resp = rb.send().await.context("NAI 请求失败")?;
        let status = resp.status();
        let text = resp.text().await.context("NAI 读取响应失败")?;
        if !status.is_success() {
            bail!("NAI HTTP {status}: {}", self.redact(&text));
        }
        let v: serde_json::Value = serde_json::from_str(&text).context("NAI 响应非 JSON")?;
        let images = v
            .get("images")
            .and_then(|i| i.as_array())
            .ok_or_else(|| anyhow::anyhow!("NAI 响应缺 images"))?;
        let out: Vec<String> = images
            .iter()
            .filter_map(|s| s.as_str().map(|x| x.to_string()))
            .collect();
        if out.is_empty() {
            bail!("NAI 响应 images 为空: {}", self.redact(&text));
        }
        tracing::info!(elapsed_ms = started.elapsed().as_millis(), "NAI nai_native 完成");
        Ok(out)
    }

    /// openai_compat 形态调用：`POST {base}/v1/images/generations`
    async fn call_openai_compat(&self, req: &ImageRequest, model_id: &str) -> Result<Vec<String>> {
        let (w, h) = nai_dimensions(&req.ratio)
            .ok_or_else(|| anyhow::anyhow!("NAI 不支持比例 {}", req.ratio))?;
        let body = json!({
            "model": model_id,
            "prompt": req.prompt,
            "size": format!("{w}x{h}"),
            "n": req.count,
            "response_format": "b64_json",
        });
        let mut rb = self
            .http
            .post(format!("{}/v1/images/generations", self.cfg.base_url.trim_end_matches('/')))
            .json(&body);
        if let Some(k) = &self.cfg.api_key {
            rb = rb.bearer_auth(k);
        }
        if req.seed.is_some() {
            tracing::warn!("openai_compat 不支持 seed，已忽略用户显式 seed");
        }
        let started = std::time::Instant::now();
        let resp = rb.send().await.context("NAI(openai) 请求失败")?;
        let status = resp.status();
        let text = resp.text().await.context("NAI(openai) 读取响应失败")?;
        if !status.is_success() {
            bail!("NAI(openai) HTTP {status}: {}", self.redact(&text));
        }
        let v: serde_json::Value = serde_json::from_str(&text).context("NAI(openai) 响应非 JSON")?;
        let out: Vec<String> = v
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.get("b64_json").and_then(|s| s.as_str()).map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if out.is_empty() {
            bail!("NAI(openai) 响应 data 为空: {}", self.redact(&text));
        }
        tracing::info!(elapsed_ms = started.elapsed().as_millis(), "NAI openai_compat 完成");
        Ok(out)
    }
}

#[async_trait]
impl MediaProvider for NaiProvider {
    fn name(&self) -> &str {
        &self.cfg.name
    }

    fn supported_endpoints(&self) -> &[EndpointStyle] {
        &[EndpointStyle::NaiNative, EndpointStyle::OpenaiCompat]
    }

    fn supported_ratios(&self, _model_id: &str) -> Vec<String> {
        crate::tools::media::params::NAI_DIMENSIONS
            .iter()
            .map(|(r, _)| r.to_string())
            .collect()
    }

    async fn generate(
        &self,
        req: &ImageRequest,
        model_id: &str,
        endpoint: EndpointStyle,
        data_dir: &std::path::Path,
    ) -> Result<ImageArtifacts> {
        let started = std::time::Instant::now();
        let images_b64 = match endpoint {
            EndpointStyle::NaiNative => self.call_nai_native(req, model_id).await?,
            EndpointStyle::OpenaiCompat => self.call_openai_compat(req, model_id).await?,
            other => bail!("NAI provider 不支持 endpoint {:?}", other),
        };
        let paths = crate::tools::media::save_artifacts(data_dir, &images_b64, "png")?;
        Ok(ImageArtifacts {
            paths,
            seed: req.seed,
            endpoint_style: super::endpoint_style_str(endpoint).to_string(),
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    fn cfg_ref(&self) -> &ProviderCfg {
        &self.cfg
    }

    fn http_client(&self) -> &reqwest::Client {
        &self.http
    }
}
