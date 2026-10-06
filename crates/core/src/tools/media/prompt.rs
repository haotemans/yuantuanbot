//! 提示词优化（Q010）：发生图前先经 LLM 改写；按模型 prompt_style 选择风格
//!
//! 设计：
//! - LLM 不可用时 fallback 原文（不阻塞命令链）
//! - 风格系统提示词（nai / anima / a1111 / natural）从 AstrBot 移植要点
//! - 优化失败/超时/返回空 → fallback 原文；记 warn 日志

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

fn system_prompt(style: PromptStyle) -> &'static str {
    match style {
        PromptStyle::Nai => {
            "You are an expert prompt optimizer for NovelAI (NAI). \
             Given a user's request (often in Chinese), rewrite it into a high-quality NAI prompt. \
             Format: English action sentence first, then Danbooru tags, optionally with NovelAI weight syntax like {tag} or [tag]. \
             Keep it under 200 tokens. \
             Only output the final prompt, no explanation. \
             If the input is already a good English prompt, return it as-is."
        }
        PromptStyle::Anima => {
            "You are an expert prompt optimizer for Anima image generation. \
             Rewrite user's request (often Chinese) into Anima format: \
             English action sentence first, then Danbooru tags, with Anima weight syntax. \
             Keep it under 200 tokens. Only output the final prompt."
        }
        PromptStyle::A1111 => {
            "You are an expert Stable Diffusion / A1111 prompt optimizer. \
             Rewrite user's request (often Chinese) into A1111 format: \
             English action sentence, then comma-separated tags with A1111 weight syntax (tag:1.2). \
             Keep it under 200 tokens. Only output the final prompt."
        }
        PromptStyle::Natural => {
            "You are an expert image prompt optimizer. \
             Rewrite user's request (often Chinese) into structured natural English description. \
             Be vivid and specific (subject, action, environment, lighting, style). \
             Keep it under 200 words. Only output the final prompt."
        }
    }
}

/// 优化一个 prompt：LLM 可用时按风格改写；任何失败/空响应 fallback 原文（Q010 契约）。
pub async fn optimize_prompt(
    raw: &str,
    style: PromptStyle,
    llm: Option<&crate::llm::LlmGateway>,
) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let Some(gw) = llm else {
        tracing::debug!("media prompt: LLM 不可用，fallback 原文");
        return raw.to_string();
    };
    // 当前用 bot_chat 角色（默认；optimizer_role 配置在 MOD-022g 接入）
    let role = crate::llm::Role::BotChat;
    let system = system_prompt(style);
    let user = format!("User request:\n{raw}");
    match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        gw.chat(role, system, &user, false),
    )
    .await
    {
        Ok(Ok(optimized)) => {
            let t = optimized.trim();
            if t.is_empty() {
                tracing::warn!("media prompt: LLM 返回空，fallback 原文");
                raw.to_string()
            } else {
                tracing::info!(style = ?style, "media prompt 优化完成");
                t.to_string()
            }
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "media prompt: LLM 调用失败，fallback 原文");
            raw.to_string()
        }
        Err(_) => {
            tracing::warn!("media prompt: LLM 超时 30s，fallback 原文");
            raw.to_string()
        }
    }
}
