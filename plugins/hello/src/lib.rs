//! Hello 示范插件：证明 plugins/<name>/ 独立 crate + Tool trait + Skill trait 注册链路走通。
//! 这个插件不做正事，只是骨架。删除前请先读完 plugins/README.md。

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use yuantuan_core::skills::{Skill, SkillDef};
use yuantuan_core::tools::{Tool, ToolCtx, ToolOutput};

pub struct HelloTool;

#[async_trait]
impl Tool for HelloTool {
    fn name(&self) -> &'static str {
        "hello"
    }
    fn description(&self) -> &'static str {
        "打招呼示例插件；验证插件链路"
    }

    async fn call(&self, _ctx: &ToolCtx, args: Value) -> Result<ToolOutput> {
        let who = args.get("who").and_then(|v| v.as_str()).unwrap_or("world");
        Ok(ToolOutput {
            summary: format!("hello, {who}!"),
            artifacts: vec![],
            data: json!({ "greeted": who }),
        })
    }
}

/// 会议纪要示范 Skill。Decision 看到用户说"帮我整理一下这次讨论/会议纪要"时可触发。
/// 实现策略：invoke 用 BotChat LLM（由 main 在装配时塞进 OnceCell）加工；
/// LLM 未就绪时退回模板直接渲染。
///
/// 注意：Skill trait 本身不依赖 LLM（保持 core 纯净）；
/// 插件层通过 `set_llm()` 注入可选 LLM 槽。
use std::sync::OnceLock;
use yuantuan_core::llm::{LlmGateway, Role};

static LLM_SLOT: OnceLock<Arc<LlmGateway>> = OnceLock::new();

/// main 装配完 LlmGateway 后调用，让本插件内所有 Skill 都能调 LLM。
/// 只能调一次；重复调用返回 Err（表示已有值）。
pub fn set_llm(gw: Arc<LlmGateway>) -> std::result::Result<(), Arc<LlmGateway>> {
    LLM_SLOT.set(gw)
}

/// 会议纪要示范 Skill。
pub struct MeetingNotesSkill;

#[async_trait]
impl Skill for MeetingNotesSkill {
    fn def(&self) -> SkillDef {
        SkillDef {
            name: "meeting_notes",
            description: "把一段讨论/会议内容整理成结构化纪要（议题/结论/待办）",
            prompt_template: r#"请把下面的内容整理成会议纪要，分三段：议题、结论、待办。

--------
{user_text}
--------

上下文（可选参考）：{context}
"#
            .into(),
            tools: vec![],
            role_hint: Some("你是会议纪要助手，输出精炼、条理清晰的中文。"),
        }
    }

    // 用 SkillImpl 的默认 invoke：只把 slots 填进模板，直接发给用户。
    // 真实业务场景下，这里应调 BotChat LLM 让它根据 prompt_template 做加工
    // （BotChat 未就绪时退回默认渲染）。
    async fn invoke(&self, ctx: &ToolCtx, slots: Value) -> Result<ToolOutput> {
        llm_assisted_invoke(self.def(), ctx, slots).await
    }
}

/// 共享 helper：若 LLM_SLOT 已装配则调 BotChat 加工，否则退回默认模板渲染。
/// 抽出来给多个 Skill 复用。
async fn llm_assisted_invoke(def: SkillDef, _ctx: &ToolCtx, slots: Value) -> Result<ToolOutput> {
    let rendered_prompt = yuantuan_core::skills::render_template(&def.prompt_template, &slots);
    let sys = def.role_hint.unwrap_or("你是云团的助手，按用户给定模板完成任务。");
    let llm_result = match LLM_SLOT.get() {
        Some(gw) if gw.role(Role::BotChat).is_some() => {
            // 用渲染后的 prompt 作为 user，role_hint 作为 system，调 BotChat
            Some(gw.chat(Role::BotChat, sys, &rendered_prompt, false).await)
        }
        _ => None,
    };
    let (final_text, llm_used) = match llm_result {
        Some(Ok(text)) => (text, true),
        Some(Err(e)) => {
            tracing::warn!(skill = def.name, error = %e, "Skill LLM 调用失败，退回模板渲染");
            (rendered_prompt, false)
        }
        None => (rendered_prompt, false),
    };
    Ok(ToolOutput {
        summary: format!(
            "skill {} {}（{}）",
            def.name,
            if llm_used { "LLM 加工完成" } else { "模板渲染完成" },
            def.description
        ),
        artifacts: vec![],
        data: json!({
            "skill": def.name,
            "rendered": final_text,
            "llm_used": llm_used,
            "slots": slots,
        }),
    })
}

/// 翻译示范 Skill（中文→英文）
pub struct TranslateSkill;

#[async_trait]
impl Skill for TranslateSkill {
    fn def(&self) -> SkillDef {
        SkillDef {
            name: "translate_zh_en",
            description: "把用户给出的中文内容翻译成英文",
            prompt_template: r#"请把以下中文翻译成流畅的英文，只输出英文译文，不要解释：

{user_text}"#
                .into(),
            tools: vec![],
            role_hint: Some("你是专业中英互译译员。"),
        }
    }

    async fn invoke(&self, ctx: &ToolCtx, slots: Value) -> Result<ToolOutput> {
        llm_assisted_invoke(self.def(), ctx, slots).await
    }
}

/// 装配入口：main 启动时调用，把插件的所有 Tool 加到 registry
pub fn register() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(HelloTool)]
}

/// 装配入口：main 启动时调用，把插件的所有 Skill 加到 SkillRegistry
pub fn skills() -> Vec<Arc<dyn Skill>> {
    vec![Arc::new(MeetingNotesSkill), Arc::new(TranslateSkill)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yuantuan_core::decision::{parse_decision, DecisionAction};
    use yuantuan_core::skills::SkillRegistry;

    /// 模拟 main 装配路径：register() 和 skills() 分别进 Registry / SkillRegistry；
    /// 然后走 Decision 裁决链路：LLM 返回 invoke_skill → parse → registry.get → invoke
    #[tokio::test]
    async fn assemble_then_decide_then_invoke() {
        // 1. 装配
        let skill_reg = SkillRegistry::new();
        for s in skills() {
            skill_reg.register_arc(s);
        }
        assert_eq!(skill_reg.names().len(), 2);
        assert!(skill_reg.get("meeting_notes").is_some());
        assert!(skill_reg.get("translate_zh_en").is_some());

        // 2. Decision 返回 invoke_skill（LLM 在 catalog 里看到 meeting_notes 并选中）
        let llm_resp = r#"{
          "action": "invoke_skill", "mood": "calm", "mention": false, "reply_len": "medium",
          "meme_type": null, "task_goal": null, "memory_write": null,
          "skill_name": "meeting_notes",
          "skill_slots": { "user_text": "今天讨论了插件层 Phase 2 + Skills 抽象，结论是走 Rust trait" },
          "reason": "用户要整理讨论内容"
        }"#;
        let out = parse_decision(llm_resp).unwrap();
        assert_eq!(out.action, DecisionAction::InvokeSkill);
        assert_eq!(out.skill_name.as_deref(), Some("meeting_notes"));

        // 3. registry.get + invoke（bot.rs 链路）
        let skill_name = out.skill_name.as_deref().unwrap();
        let s = skill_reg.get(skill_name).expect("skill must be registered");
        let ctx = ToolCtx {
            chat_id: "g_1".into(),
            chat_type: "group".into(),
            sender_pid: "p_10001".into(),
            locale: Some("zh-CN".into()),
        };
        let result = s.invoke(&ctx, out.skill_slots.clone().unwrap()).await.unwrap();
        let rendered = result.data["rendered"].as_str().unwrap();
        assert!(
            rendered.contains("插件层 Phase 2"),
            "会议纪要 skill 应该把 user_text 填进模板，得到包含原文的渲染；got: {rendered}"
        );

        // 4. translate skill 同理
        let translate = skill_reg.get("translate_zh_en").unwrap();
        let r2 = translate
            .invoke(&ctx, json!({ "user_text": "你好世界" }))
            .await
            .unwrap();
        assert!(r2.data["rendered"].as_str().unwrap().contains("你好世界"));
    }

    #[test]
    fn llm_slot_unset_invocation_falls_back_to_template() {
        // 未装配 LlmGateway 时 invoke 直接退回模板渲染（不报错）
        assert!(LLM_SLOT.get().is_none(), "装配前 LLM_SLOT 应为 None");
        let skill = MeetingNotesSkill;
        let def = skill.def();
        assert_eq!(def.name, "meeting_notes");
        assert!(!def.description.is_empty());
        assert!(def.prompt_template.contains("{user_text}"));
        assert!(def.role_hint.is_some());
    }
}
