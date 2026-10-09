//! Skills 系统（Q-S01 裁决：Rust trait，非 Markdown 文件）。
//!
//! Skill = 预定义提示词模板 + 可选 Tool 联动，对标 Anthropic Claude 的 SKILL.md。
//! 与 Tool 的差异：
//! - Tool 是执行型能力（发图、备份、写文件），返回 ToolOutput
//! - Skill 是提示词级别复用（会议纪要、翻译、总结），返回的还是 ToolOutput，但内容经 chat LLM 加工
//!
//! 生命周期：插件 crates 在 register() 里同时返回 Vec<Arc<dyn Tool>> 和 Vec<Arc<dyn Skill>>；
//!           main 装配时把 Skill 注入 SkillRegistry；Decision 看到 skill 描述列表后
//!           可以在 JSON 里返回 action=invoke_skill 主动触发（Q-S02 裁决）。

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::tools::{ToolCtx, ToolOutput};

/// Skill 元数据（编译期常量 + prompt_template 运行期持有）
#[derive(Debug, Clone)]
pub struct SkillDef {
    /// 稳定 ID（snake_case），Decision 用这个名字索引
    pub name: &'static str,
    /// 一句中文：注入 Decision 系统提示，让模型知道何时使用这个 Skill
    pub description: &'static str,
    /// 提示词模板；支持 {user_text} / {context} 槽位，由 Bot 在调用前替换
    pub prompt_template: String,
    /// 可选联动 Tool 名（skill 执行完后可以接着调这些 tool，多用于发图/发文）
    pub tools: Vec<&'static str>,
    /// 可选 role 提示：拼到 chat role 的 system 尾部；默认 None
    pub role_hint: Option<&'static str>,
}

/// Skill trait：插件实现它提供提示词级别的能力复用。
/// 与 Tool 一样是 Send + Sync，Registry 在 tokio 多任务间共享。
#[async_trait]
pub trait Skill: Send + Sync {
    /// 返回 Skill 元数据
    fn def(&self) -> SkillDef;

    /// 执行 Skill：Bot 已把 slots 塞进 prompt_template，这里专注调 chat LLM。
    /// 默认实现是不调 LLM 直接把 slots 塞进模板渲染出来（用于 deterministic 场景）；
    /// 大多数场景插件会 override 使用自己的 LLM 调用。
    async fn invoke(&self, _ctx: &ToolCtx, slots: Value) -> Result<ToolOutput> {
        let def = self.def();
        let rendered = render_template(&def.prompt_template, &slots);
        Ok(ToolOutput {
            summary: format!("skill {} 渲染完成", def.name),
            artifacts: vec![],
            data: serde_json::json!({
                "skill": def.name,
                "rendered": rendered,
                "slots": slots,
            }),
        })
    }
}

/// 模板渲染：把 {key} 槽位替换成 slots[key] 的字符串值；缺失的槽保留原样
pub fn render_template(template: &str, slots: &Value) -> String {
    let mut out = template.to_string();
    if let Some(obj) = slots.as_object() {
        for (k, v) in obj {
            let needle = format!("{{{k}}}");
            let replacement = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out = out.replace(&needle, &replacement);
        }
    }
    out
}

/// Skill 注册表（类似 tools::Registry）
#[derive(Default, Clone)]
pub struct SkillRegistry {
    inner: Arc<RwLock<HashMap<&'static str, Arc<dyn Skill>>>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_arc(&self, skill: Arc<dyn Skill>) {
        let name = skill.def().name;
        self.inner
            .write()
            .expect("skills registry poisoned")
            .insert(name, skill);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Skill>> {
        self.inner
            .read()
            .expect("skills registry poisoned")
            .get(name)
            .cloned()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.inner
            .read()
            .expect("skills registry poisoned")
            .keys()
            .copied()
            .collect()
    }

    /// 列出每个 skill 的 (name, description)，供 Decision 系统提示注入
    pub fn describe_for_decision(&self) -> Vec<(String, String)> {
        self.inner
            .read()
            .expect("skills registry poisoned")
            .values()
            .map(|s| {
                let d = s.def();
                (d.name.to_string(), d.description.to_string())
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.inner
            .read()
            .expect("skills registry poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 便捷构造器：插件用 `SkillImpl::new(def)` 直接得到一个简单 Skill
pub struct SkillImpl {
    def: SkillDef,
}

impl SkillImpl {
    pub fn new(def: SkillDef) -> Self {
        Self { def }
    }
}

#[async_trait]
impl Skill for SkillImpl {
    fn def(&self) -> SkillDef {
        self.def.clone()
    }
}

/// 从 Def 直接构造 Arc<dyn Skill>（插件 register() 里用）
pub fn arc_skill(def: SkillDef) -> Result<Arc<dyn Skill>> {
    if def.name.is_empty() {
        anyhow::bail!("skill name 不能为空");
    }
    if def.description.is_empty() {
        anyhow::bail!("skill description 不能为空");
    }
    Ok(Arc::new(SkillImpl::new(def)))
}

/// 校验 skill 调用的 slots（Decision 可选提供）
pub fn validate_slots(_def: &SkillDef, _slots: &Value) -> Result<()> {
    // 第一版不做强 schema 校验，模板渲染时缺槽保留原样
    Ok(())
}

/// 帮助函数：Skill 联动 Tool 时，Bot 按顺序调多个
pub async fn chain_with_tools(
    _skill_output: ToolOutput,
    _tool_names: &[&'static str],
    _ctx: &ToolCtx,
) -> Result<ToolOutput> {
    // Phase 2 简化：不在 Skill 链路里自动调 Tool，由 Bot 看到 def.tools 后显式调
    // Phase 3 视需要补全自动链
    anyhow::bail!("chain_with_tools 未实现；请由 Bot 显式编排")
}

/// 上下文便捷方法（由 Bot 调用前填槽用）
pub fn build_slots(user_text: &str, context: Option<&str>) -> Value {
    serde_json::json!({
        "user_text": user_text,
        "context": context.unwrap_or(""),
    })
}

/// 便捷：描述列表渲染成 Decision 系统提示片段
pub fn render_skill_catalog(skills: &[(String, String)]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n可用 Skills（当消息语义匹配时可返回 action=invoke_skill 触发）：\n");
    for (name, desc) in skills {
        out.push_str(&format!("- {name}: {desc}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_skill(name: &'static str, desc: &'static str) -> Arc<dyn Skill> {
        arc_skill(SkillDef {
            name,
            description: desc,
            prompt_template: "处理：{user_text}".into(),
            tools: vec![],
            role_hint: None,
        })
        .unwrap()
    }

    #[test]
    fn template_replaces_slots() {
        let s = render_template(
            "你好 {name}，{msg}",
            &json!({ "name": "小明", "msg": "天气不错" }),
        );
        assert_eq!(s, "你好 小明，天气不错");
    }

    #[test]
    fn template_keeps_missing_slot() {
        let s = render_template("你好 {name}", &json!({}));
        assert_eq!(s, "你好 {name}");
    }

    #[test]
    fn registry_insert_and_get() {
        let reg = SkillRegistry::new();
        reg.register_arc(make_skill("meeting_notes", "整理会议纪要"));
        assert!(reg.get("meeting_notes").is_some());
        assert!(reg.get("not_exist").is_none());
        assert_eq!(reg.names().len(), 1);
    }

    #[test]
    fn catalog_renders_list() {
        let skills = vec![
            ("meeting_notes".to_string(), "整理会议纪要".to_string()),
            ("translate".to_string(), "翻译中文到英文".to_string()),
        ];
        let s = render_skill_catalog(&skills);
        assert!(s.contains("meeting_notes"));
        assert!(s.contains("整理会议纪要"));
    }

    #[test]
    fn catalog_empty_when_no_skills() {
        assert_eq!(render_skill_catalog(&[]), "");
    }

    #[tokio::test]
    async fn default_invoke_renders_template() {
        let skill = make_skill("test", "测试");
        let ctx = ToolCtx {
            task_id: None,
            chat_id: "c1".into(),
            chat_type: "group".into(),
            sender_pid: "p1".into(),
            locale: None,
        };
        let out = skill
            .invoke(&ctx, json!({ "user_text": "帮我整理" }))
            .await
            .unwrap();
        assert_eq!(out.data["skill"], "test");
        assert!(out.data["rendered"].as_str().unwrap().contains("帮我整理"));
    }

    #[test]
    fn arc_skill_rejects_empty_name() {
        let r = arc_skill(SkillDef {
            name: "",
            description: "x",
            prompt_template: "".into(),
            tools: vec![],
            role_hint: None,
        });
        assert!(r.is_err());
    }

    #[test]
    fn build_slots_helper() {
        let v = build_slots("hi", Some("ctx"));
        assert_eq!(v["user_text"], "hi");
        assert_eq!(v["context"], "ctx");
    }

    #[test]
    fn validate_slots_accepts_anything_v1() {
        let def = SkillDef {
            name: "x",
            description: "x",
            prompt_template: "".into(),
            tools: vec![],
            role_hint: None,
        };
        assert!(validate_slots(&def, &json!({})).is_ok());
    }
}
