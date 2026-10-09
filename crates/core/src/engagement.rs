//! 参与判断：模型解释语境；Runtime 验证来源、控制打扰度和选择回复方式。
use crate::decision::{DecisionAction, DecisionOutput, ReplyLen};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    Bot,
    Other,
    Group,
    Unclear,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Question,
    Request,
    FollowUp,
    Sharing,
    Acknowledgment,
    Banter,
    Stop,
    Correction,
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Continuity {
    NewTopic,
    Continuing,
    Closing,
    Unclear,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Low,
    Medium,
    High,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Novelty {
    New,
    Repeated,
    Unclear,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    Sufficient,
    Missing,
    Conflicting,
    NotNeeded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assessment {
    pub audience: Audience,
    pub intent: Intent,
    pub continuity: Continuity,
    pub confidence: Level,
    pub benefit: Level,
    pub novelty: Novelty,
    pub evidence: Evidence,
    pub evidence_msg_ids: Vec<i64>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyMode {
    #[default]
    Answer,
    Clarify,
    StateUncertainty,
}
impl ReplyMode {
    pub fn instruction(self) -> &'static str {
        match self {
            Self::Answer => "回复方式：直接回答当前问题，回答完就停；参与判断不是新的事实来源。",
            Self::Clarify => "回复方式：只澄清解决当前问题必需的一项缺失信息，不编造前提，不为了延长聊天追问。",
            Self::StateUncertainty => "回复方式：明确说明现有资料无法确认或有冲突，不猜肯定/否定的经历和执行结果；能确认的部分可简短说明。",
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct PolicyTrace {
    pub version: u8,
    pub suggested_action: String,
    pub evidence_valid: Option<bool>,
    pub cooldown_secs: Option<i64>,
    pub notes: Vec<String>,
}

pub struct PolicyInput<'a> {
    pub is_group: bool,
    pub direct: bool,
    pub anchor_id: i64,
    pub visible_ids: &'a BTreeSet<i64>,
    pub linked_reply_id: Option<i64>,
    pub reply_age: Option<i64>,
    pub bot_bubbles_5min: i64,
    pub human_messages_30s: i64,
}

fn suppress(out: &mut DecisionOutput, trace: &mut PolicyTrace, reason: &str) {
    out.action = DecisionAction::Ignore;
    out.mention = false;
    trace.notes.push(reason.into());
}

/// 不从 ignore 强行生成 reply；语义判断也不能取代任务执行的原话入口。
pub fn apply(out: &mut DecisionOutput, input: &PolicyInput<'_>, trace: &mut PolicyTrace) {
    let Some(a) = out.assessment.clone() else {
        return;
    };
    let valid = !a.evidence_msg_ids.is_empty()
        && a.evidence_msg_ids.len() <= 6
        && a.evidence_msg_ids.contains(&input.anchor_id)
        && a.evidence_msg_ids
            .iter()
            .all(|id| input.visible_ids.contains(id));
    trace.evidence_valid = Some(valid);
    if out.action == DecisionAction::Ignore {
        return;
    }
    if !valid {
        suppress(out, trace, "参与判断的消息依据不在本次可见输入中");
        return;
    }
    if a.intent == Intent::Stop || (input.is_group && a.continuity == Continuity::Closing) {
        suppress(out, trace, "对方在结束对话，不再延长交谈");
        return;
    }
    if input.is_group && a.audience == Audience::Other {
        suppress(out, trace, "当前发言面向其他成员");
        return;
    }
    if matches!(
        out.action,
        DecisionAction::StartTask | DecisionAction::InvokeSkill
    ) {
        if a.intent != Intent::Request || a.confidence != Level::High || a.audience != Audience::Bot
        {
            suppress(out, trace, "参与判断不能确认这是对 Bot 的明确执行请求");
        }
        return;
    }
    let linked_followup = input
        .linked_reply_id
        .is_some_and(|id| a.evidence_msg_ids.contains(&id))
        && a.audience == Audience::Bot
        && matches!(
            a.intent,
            Intent::FollowUp | Intent::Acknowledgment | Intent::Question | Intent::Request
        )
        && a.continuity == Continuity::Continuing
        && a.confidence == Level::High;
    if !input.direct && a.intent == Intent::FollowUp && !linked_followup {
        suppress(out, trace, "隐式承接缺少同一人的近期回复关联");
        return;
    }
    // 模型声称 audience=bot 不能单独绕过频率限制；免冷却需要真实承接或协议/明确称呼。
    let directed = input.direct || linked_followup;
    if linked_followup {
        trace.notes.push("依据真实回复归属承接同一人的对话".into());
    }
    if input.is_group && a.intent == Intent::Acknowledgment && !linked_followup {
        suppress(out, trace, "收尾附和不需要礼貌性再回一轮");
        return;
    }
    if input.is_group && !directed {
        if !matches!(a.audience, Audience::Group | Audience::Bot)
            || a.confidence != Level::High
            || a.benefit != Level::High
            || a.novelty != Novelty::New
            || matches!(a.evidence, Evidence::Missing | Evidence::Conflicting)
            || matches!(a.intent, Intent::Acknowledgment | Intent::Banter)
        {
            suppress(out, trace, "主动参与缺少高把握的新帮助，保持安静");
            return;
        }
        let cooldown = if input.bot_bubbles_5min >= 6 || input.human_messages_30s >= 15 {
            300
        } else if input.bot_bubbles_5min <= 2 && input.human_messages_30s <= 4 {
            60
        } else {
            120
        };
        trace.cooldown_secs = Some(cooldown);
        if input.reply_age.is_some_and(|age| age < cooldown) {
            suppress(
                out,
                trace,
                "群聊参与冷却中，按繁忙程度和 Bot 发言量减少插话",
            );
            return;
        }
        out.reply_len = ReplyLen::Short;
        out.mention = false;
        trace.notes.push("允许一次简短、有新帮助的主动参与".into());
    }
    if out.action == DecisionAction::Reply {
        if a.confidence == Level::Low || a.audience == Audience::Unclear {
            out.reply_mode = ReplyMode::Clarify;
            out.reply_len = ReplyLen::Short;
        } else if matches!(a.evidence, Evidence::Missing | Evidence::Conflicting)
            && out.reply_mode != ReplyMode::Clarify
        {
            out.reply_mode = ReplyMode::StateUncertainty;
            out.reply_len = ReplyLen::Short;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn output() -> DecisionOutput {
        serde_json::from_value(json!({"action":"reply","mood":"calm","mention":true,"reply_len":"long","reason":"场景评估",
            "assessment":{"audience":"group","intent":"sharing","continuity":"new_topic","confidence":"high","benefit":"high","novelty":"new","evidence":"not_needed","evidence_msg_ids":[1]}})).unwrap()
    }
    fn input(ids: &BTreeSet<i64>) -> PolicyInput<'_> {
        PolicyInput {
            is_group: true,
            direct: false,
            anchor_id: 1,
            visible_ids: ids,
            linked_reply_id: Some(2),
            reply_age: Some(70),
            bot_bubbles_5min: 0,
            human_messages_30s: 2,
        }
    }
    #[test]
    fn implicit_answer_requires_own_linked_dialogue_and_visible_evidence() {
        let ids = BTreeSet::from([1, 2, 3]);
        let mut ctx = input(&ids);
        ctx.reply_age = Some(0);
        let mut out = output();
        let a = out.assessment.as_mut().unwrap();
        a.audience = Audience::Bot;
        a.intent = Intent::FollowUp;
        a.continuity = Continuity::Continuing;
        a.evidence_msg_ids = vec![1, 2];
        apply(&mut out, &ctx, &mut PolicyTrace::default());
        assert_eq!(
            out.action,
            DecisionAction::Reply,
            "同一个人的无问号简短回答可承接"
        );
        out.action = DecisionAction::Reply;
        ctx.linked_reply_id = Some(3);
        apply(&mut out, &ctx, &mut PolicyTrace::default());
        assert_eq!(
            out.action,
            DecisionAction::Ignore,
            "不能借另一个人的对话归属继续"
        );
        out.action = DecisionAction::Reply;
        ctx.direct = true;
        out.assessment.as_mut().unwrap().evidence_msg_ids.push(999);
        let mut trace = PolicyTrace::default();
        apply(&mut out, &ctx, &mut trace);
        assert_eq!(out.action, DecisionAction::Ignore);
        assert_eq!(trace.evidence_valid, Some(false));
    }
    #[test]
    fn participation_depends_on_help_and_load_instead_of_question_punctuation() {
        let ids = BTreeSet::from([1]);
        let mut ctx = input(&ids);
        let mut out = output();
        let mut trace = PolicyTrace::default();
        apply(&mut out, &ctx, &mut trace);
        assert_eq!(
            out.action,
            DecisionAction::Reply,
            "有新帮助的分享不必带问号"
        );
        assert_eq!(out.reply_len, ReplyLen::Short);
        assert!(!out.mention);
        assert_eq!(trace.cooldown_secs, Some(60));
        for (bubbles, humans, age, expected) in
            [(3, 8, 119, 120), (6, 1, 299, 300), (0, 15, 299, 300)]
        {
            ctx.bot_bubbles_5min = bubbles;
            ctx.human_messages_30s = humans;
            ctx.reply_age = Some(age);
            let mut out = output();
            let mut trace = PolicyTrace::default();
            apply(&mut out, &ctx, &mut trace);
            assert_eq!(out.action, DecisionAction::Ignore);
            assert_eq!(trace.cooldown_secs, Some(expected));
        }
        ctx.reply_age = None;
        let mut out = output();
        out.assessment.as_mut().unwrap().novelty = Novelty::Repeated;
        apply(&mut out, &ctx, &mut PolicyTrace::default());
        assert_eq!(out.action, DecisionAction::Ignore);
        let mut out = output();
        out.assessment.as_mut().unwrap().benefit = Level::Low;
        apply(&mut out, &ctx, &mut PolicyTrace::default());
        assert_eq!(out.action, DecisionAction::Ignore);
    }
    #[test]
    fn closing_and_uncertainty_have_distinct_behaviors() {
        let ids = BTreeSet::from([1]);
        let mut ctx = input(&ids);
        ctx.direct = true;
        ctx.reply_age = Some(0);
        let mut out = output();
        let a = out.assessment.as_mut().unwrap();
        a.audience = Audience::Bot;
        a.intent = Intent::Question;
        a.evidence = Evidence::Missing;
        apply(&mut out, &ctx, &mut PolicyTrace::default());
        assert_eq!(out.action, DecisionAction::Reply);
        assert_eq!(out.reply_mode, ReplyMode::StateUncertainty);
        out.assessment.as_mut().unwrap().confidence = Level::Low;
        apply(&mut out, &ctx, &mut PolicyTrace::default());
        assert_eq!(out.reply_mode, ReplyMode::Clarify);
        out.assessment.as_mut().unwrap().continuity = Continuity::Closing;
        apply(&mut out, &ctx, &mut PolicyTrace::default());
        assert_eq!(out.action, DecisionAction::Ignore);
        out.action = DecisionAction::Ignore;
        out.assessment.as_mut().unwrap().continuity = Continuity::NewTopic;
        apply(&mut out, &ctx, &mut PolicyTrace::default());
        assert_eq!(
            out.action,
            DecisionAction::Ignore,
            "不强制模型忽略的内容得到回复"
        );
    }
}
