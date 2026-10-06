//! 提示词优化（Q010）：发生图前先经 LLM 改写；按模型 prompt_style 选择风格
//!
//! 当前阶段（MOD-022 起步）：**先实现直通实现**——把原文返回，接入与超时/fallback 契约留着。
//! 真正的 LLM 优化在 MOD-022e 落地命令链路时接通（要依赖 LlmGateway 的 messages API）。

/// 优化请求（含模型风格；优化实现按此切换系统提示词）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptStyle {
    /// NAI 风格："英文动作句 + Danbooru tag + NovelAI 权重语法"
    Nai,
    /// Anima 风格："英文动作句 + Danbooru tag + Anima 权重语法"
    Anima,
    /// SD/A1111 风格："英文动作句 + Danbooru tag + A1111 权重语法"
    A1111,
    /// 结构化自然语言（默认；适用于非 tag 系 provider，如 gpt-image / nanobanana / Seedream）
    Natural,
}

impl Default for PromptStyle {
    fn default() -> Self {
        PromptStyle::Natural
    }
}

/// 优化一个 prompt（MOD-022 起步阶段：直通；后续接 LLM 再实现真正优化）。
/// 返回 `Ok(optimized)`；任何失败必须 fallback 到原文，不能让命令链中断。
pub async fn optimize_prompt(raw: &str, _style: PromptStyle) -> String {
    raw.trim().to_string()
}
