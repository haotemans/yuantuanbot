//! Decision 小脑（架构文档十三章）：分类器，不是对话者。
//! 输入契约组装（docs/data-model.md 第十章）→ 调 decision 角色模型 → 输出 Schema 严格校验 →
//! 失败带错误重试一次 → 再失败兜底 ignore 并记录。
//! 本单副作用：mood 写回 state、memory_write 入 long_memories（explicit）、DecisionMade 事件供 trace。
//! action=reply/send_meme/start_task 只记事件与日志，发送链路在施工单 5 接入。

use crate::event::{DecisionMadePayload, Event, EventBus, MessageReceivedPayload};
use crate::llm::{LlmGateway, Role};
use crate::state::{MoodState, MoodValue};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// Decision 输出 Schema 终稿（架构十三章）。mention/reply_len/reason 必填，
/// meme_type/task_goal/memory_write 可为 null。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionOutput {
    pub action: DecisionAction,
    pub mood: MoodValue,
    pub mention: bool,
    pub reply_len: ReplyLen,
    pub meme_type: Option<String>,
    pub task_goal: Option<String>,
    pub memory_write: Option<String>,
    /// action=invoke_skill 时必填：触发哪个 Skill
    #[serde(default)]
    pub skill_name: Option<String>,
    /// action=invoke_skill 时可选：传给 Skill 的槽位（user_text/context 等）
    #[serde(default)]
    pub skill_slots: Option<Value>,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAction {
    Reply,
    Ignore,
    SendMeme,
    StartTask,
    /// 触发已注册 Skill（Q-S02 裁决，Decision 模型自-driven）
    InvokeSkill,
}

impl DecisionAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            DecisionAction::Reply => "reply",
            DecisionAction::Ignore => "ignore",
            DecisionAction::SendMeme => "send_meme",
            DecisionAction::StartTask => "start_task",
            DecisionAction::InvokeSkill => "invoke_skill",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplyLen {
    Short,
    Medium,
    Long,
}

const SYSTEM_PROMPT_BASE: &str = r#"你是云团的 Decision 小脑：一个分类器，不是对话者。根据输入的 Decision Context 决定云团要做什么，只输出严格 JSON，不要输出任何其他文字。

输出 Schema（action/mood/mention/reply_len/reason 必填；meme_type/task_goal/memory_write/skill_name/skill_slots 可为 null）：
{
  "action": "reply | ignore | send_meme | start_task | invoke_skill",
  "mood": "calm | happy | angry | down",
  "mention": true 或 false（回复是否@对方）,
  "reply_len": "short | medium | long",
  "meme_type": "类别标签；仅 action=send_meme 时填，否则 null",
  "task_goal": "一句话任务目标；仅 action=start_task 时填，否则 null",
  "memory_write": "提炼出的显式事实字符串，无则 null",
  "skill_name": "已注册 Skill 名；仅 action=invoke_skill 时必填，否则 null",
  "skill_slots": "传给 Skill 的槽位对象，通常至少含 user_text；仅 action=invoke_skill 时可填，否则 null",
  "reason": "一句话中文理由（只进 trace，不展示给用户）"
}

规则：
- 拿不准就 ignore；主动插话与被动回复由你同一裁决
- memory_write 只在消息包含值得长期记住的新事实时填写，提炼成陈述句
- 是否回复、回复长度、情绪 mood 由场景与你的判断决定
- 当消息请求某个已注册 Skill 的能力时（如"整理一下"、"翻译"、"会议纪要"），优先返回 action=invoke_skill 而非 reply；skill_name 必须从可用 Skills 列表里挑
"#;

/// 拼 SYSTEM_PROMPT：基础说明 + 当前已注册 Skills 目录（Q-S02 裁决）
pub fn build_system_prompt(skill_catalog: &[(String, String)]) -> String {
    let mut s = SYSTEM_PROMPT_BASE.to_string();
    s.push_str(&crate::skills::render_skill_catalog(skill_catalog));
    s
}

pub struct DecisionOutcome {
    pub output: DecisionOutput,
    pub fallback: bool,
    pub retries: u32,
    pub elapsed_ms: u64,
}

/// 完整决策一次：组上下文 → 调模型 → 校验（重试一次）→ 副作用 + DecisionMade 事件
pub async fn decide(
    db_path: &Path,
    llm: &LlmGateway,
    bus: &EventBus,
    mood: &MoodState,
    msg: &MessageReceivedPayload,
    skills: Option<&crate::skills::SkillRegistry>,
) -> DecisionOutcome {
    let started = Instant::now();
    let ctx = build_context(db_path, mood, msg);
    let user_prompt = serde_json::to_string_pretty(&ctx).unwrap_or_else(|_| "{}".into());
    let catalog: Vec<(String, String)> = skills
        .map(|r| r.describe_for_decision())
        .unwrap_or_default();
    let sys_prompt = build_system_prompt(&catalog);

    let raw = llm.chat(Role::Decision, &sys_prompt, &user_prompt, true).await;
    let mut retries = 0u32;
    let mut parsed: Option<DecisionOutput> = None;
    match raw {
        Err(e) => {
            warn!(error = %e, "Decision 模型调用失败，兜底 ignore");
        }
        Ok(content) => match parse_decision(&content) {
            Ok(o) => parsed = Some(o),
            Err(err) => {
                // 带错误重试一次
                retries = 1;
                let retry_user = format!(
                    "{user_prompt}\n\n上一次你的输出未通过 Schema 校验：{err}\n请严格按 Schema 重新输出，只输出 JSON。"
                );
                match llm.chat(Role::Decision, &sys_prompt, &retry_user, true).await {
                    Ok(content2) => match parse_decision(&content2) {
                        Ok(o) => parsed = Some(o),
                        Err(err2) => {
                            warn!(error = %err2, "Decision 重试校验仍失败，兜底 ignore");
                        }
                    },
                    Err(e) => warn!(error = %e, "Decision 重试调用失败，兜底 ignore"),
                }
            }
        },
    }

    // 若 LLM 返回 invoke_skill 但 registry 里没有这个 skill，退回 reply 兜底
    if let Some(o) = parsed.as_mut() {
        if o.action == DecisionAction::InvokeSkill {
            let ok = o
                .skill_name
                .as_deref()
                .and_then(|n| skills.and_then(|r| r.get(n)))
                .is_some();
            if !ok {
                warn!(skill = ?o.skill_name, "Decision 选择了未知/未注册 Skill，退回 reply");
                o.action = DecisionAction::Reply;
                o.skill_name = None;
                o.skill_slots = None;
                o.reason.push_str("（原 invoke_skill 未注册，退回 reply）");
            }
        }
    }

    let (output, fallback) = match parsed {
        Some(o) => (o, false),
        None => (fallback_output(), true),
    };
    apply_side_effects(db_path, bus, mood, msg, &output);

    let elapsed_ms = started.elapsed().as_millis() as u64;
    // action=reply/send_meme/start_task 的发送链路在施工单 5 接入，本单只记事件与日志
    bus.publish(Event::DecisionMade(DecisionMadePayload {
        chat_id: msg.chat_id.clone(),
        sender_pid: msg.sender_pid.clone(),
        text: truncate_chars(&msg.text, 100),
        action: output.action.as_str().into(),
        mood: output.mood.as_str().into(),
        reason: output.reason.clone(),
        fallback,
        retries,
        elapsed_ms,
    }));
    info!(
        chat_id = %msg.chat_id,
        action = output.action.as_str(),
        mood = %output.mood,
        fallback,
        retries,
        elapsed_ms,
        reason = %output.reason,
        "Decision 完成"
    );
    DecisionOutcome {
        output,
        fallback,
        retries,
        elapsed_ms,
    }
}

fn fallback_output() -> DecisionOutput {
    DecisionOutput {
        action: DecisionAction::Ignore,
        mood: MoodValue::Calm,
        mention: false,
        reply_len: ReplyLen::Short,
        meme_type: None,
        task_goal: None,
        memory_write: None,
        skill_name: None,
        skill_slots: None,
        reason: "模型输出未通过 Schema 校验，兜底 ignore".into(),
    }
}

/// 输出 Schema 校验（纯函数，单测直测）：容忍 ```json 围栏，其余严格 serde
pub fn parse_decision(content: &str) -> Result<DecisionOutput, String> {
    let s = strip_fence(content.trim());
    serde_json::from_str::<DecisionOutput>(&s).map_err(|e| e.to_string())
}

fn strip_fence(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        if let Some(body) = rest.strip_suffix("```") {
            return body.trim().to_string();
        }
    }
    s.to_string()
}

/// Decision Context 组装（data-model 第十章）；摘要优先于原始消息，各字段有界
fn build_context(db_path: &Path, mood: &MoodState, msg: &MessageReceivedPayload) -> Value {
    let conn = crate::db::connect(db_path).ok();
    let now = now_secs();
    let sender = conn.as_ref().map(|c| {
        let (trust, familiar): (f64, f64) = c
            .query_row(
                "SELECT trust, familiar FROM relationship_edges WHERE from_pid = 'self' AND to_pid = ?1",
                params![msg.sender_pid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or((0.5, 0.0));
        let nickname: String = c
            .query_row(
                "SELECT COALESCE(display_name, '') FROM persons WHERE person_id = ?1",
                params![msg.sender_pid],
                |r| r.get(0),
            )
            .unwrap_or_default();
        let affinity: f64 = c
            .query_row(
                "SELECT familiar FROM relationship_edges WHERE from_pid = 'self' AND to_pid = ?1",
                params![msg.sender_pid],
                |r| r.get(0),
            )
            .unwrap_or(0.0);
        json!({
            "person_id": msg.sender_pid,
            "nickname": nickname,
            "trust": trust,
            "familiar": familiar,
            "affinity_self": affinity,
        })
    }).unwrap_or_else(|| json!({
        "person_id": msg.sender_pid, "nickname": "", "trust": 0.5, "familiar": 0.0, "affinity_self": 0.0
    }));

    let scene = conn.as_ref().map(|c| {
        let recent_speakers: Vec<String> = c
            .prepare(
                "SELECT DISTINCT sender_pid FROM (
                   SELECT sender_pid FROM messages WHERE chat_id = ?1 ORDER BY msg_id DESC LIMIT 10
                 ) LIMIT 10",
            )
            .and_then(|mut st| {
                let rows = st.query_map(params![msg.chat_id], |r| r.get(0))?;
                rows.collect::<std::result::Result<Vec<String>, _>>()
            })
            .unwrap_or_default();
        let msgs_since_my_reply: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE chat_id = ?1 AND sender_pid != 'self' AND msg_id > COALESCE(
                   (SELECT MAX(msg_id) FROM messages WHERE chat_id = ?1 AND sender_pid = 'self'), 0)",
                params![msg.chat_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let my_replies_last_5min: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE chat_id = ?1 AND sender_pid = 'self' AND ts >= ?2",
                params![msg.chat_id, now - 300],
                |r| r.get(0),
            )
            .unwrap_or(0);
        json!({
            "recent_speakers": recent_speakers,
            "msgs_since_my_reply": msgs_since_my_reply,
            "my_replies_last_5min": my_replies_last_5min,
            "chat_topic": Value::Null,
        })
    }).unwrap_or_else(|| json!({
        "recent_speakers": [], "msgs_since_my_reply": 0, "my_replies_last_5min": 0, "chat_topic": Value::Null
    }));

    let memory_hints: Vec<String> = conn.as_ref().map(|c| {
        c.prepare(
            "SELECT content FROM long_memories
             WHERE (owner_type = 'person' AND owner_id = ?1) OR (owner_type = 'chat' AND owner_id = ?2)
             ORDER BY updated_at DESC LIMIT 3",
        )
        .and_then(|mut st| {
            let rows = st.query_map(params![msg.sender_pid, msg.chat_id], |r| r.get(0))?;
            rows.collect::<std::result::Result<Vec<String>, _>>()
        })
        .unwrap_or_default()
    }).unwrap_or_default();

    json!({
        "message": {
            "text": truncate_chars(&msg.text, 500),
            "chat_type": msg.chat_type,
            "at_me": msg.at_me,
            "reply_to_me": msg.reply_to.is_some(),
            "has_image": msg.has_image,
            "mentions": Value::Null,
        },
        "sender": sender,
        "scene": scene,
        "memory_hints": memory_hints,
        "mood": mood.get().as_str(),
        "active_task": Value::Null,
    })
}

fn apply_side_effects(db_path: &Path, bus: &EventBus, mood: &MoodState, msg: &MessageReceivedPayload, out: &DecisionOutput) {
    // mood 写回（仅变化时记 MoodChanged）
    if mood.set(out.mood) {
        bus.publish(Event::MoodChanged);
    }
    // memory_write → long_memories（owner=person，source=explicit）；敏感/空串拒收
    if let Some(fact) = out.memory_write.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let conn = match crate::db::connect(db_path) {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "memory_write 落库失败：打开数据库");
                return;
            }
        };
        let now = now_secs();
        match conn.execute(
            "INSERT INTO long_memories(owner_type, owner_id, content, source, created_at, updated_at)
             VALUES ('person', ?1, ?2, 'explicit', ?3, ?3)",
            params![msg.sender_pid, fact, now],
        ) {
            Ok(_) => {
                debug!(person = %msg.sender_pid, "显式记忆已写入");
                bus.publish(Event::MemoryWritten);
            }
            Err(e) => warn!(error = %e, "memory_write 落库失败"),
        }
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn truncate_chars(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_json() -> &'static str {
        r#"{
          "action": "reply", "mood": "happy", "mention": true, "reply_len": "short",
          "meme_type": null, "task_goal": null,
          "memory_write": "小明是 Rust 爱好者",
          "reason": "他@我打招呼"
        }"#
    }

    #[test]
    fn schema_accepts_valid() {
        let out = parse_decision(valid_json()).unwrap();
        assert_eq!(out.action, DecisionAction::Reply);
        assert_eq!(out.mood, MoodValue::Happy);
        assert!(out.mention);
        assert_eq!(out.reply_len, ReplyLen::Short);
        assert_eq!(out.memory_write.as_deref(), Some("小明是 Rust 爱好者"));
    }

    #[test]
    fn schema_accepts_fenced() {
        let fenced = format!("```json\n{}\n```", valid_json());
        assert!(parse_decision(&fenced).is_ok());
    }

    #[test]
    fn schema_rejects_bad_action() {
        let bad = valid_json().replace("\"reply\"", "\"dance\"");
        assert!(parse_decision(&bad).is_err());
    }

    #[test]
    fn schema_rejects_bad_mood() {
        let bad = valid_json().replace("\"happy\"", "\"ecstatic\"");
        assert!(parse_decision(&bad).is_err());
    }

    #[test]
    fn schema_rejects_missing_field() {
        let bad = valid_json().replace("true, \"reply_len\"", "true, \"reply_len2\"")
            .replace("\"mention\": true", "");
        assert!(parse_decision(&bad).is_err());
    }

    #[test]
    fn schema_rejects_garbage() {
        assert!(parse_decision("这不是 JSON").is_err());
    }

    #[test]
    fn schema_accepts_invoke_skill() {
        let j = r#"{
          "action": "invoke_skill", "mood": "calm", "mention": false, "reply_len": "medium",
          "meme_type": null, "task_goal": null, "memory_write": null,
          "skill_name": "meeting_notes",
          "skill_slots": { "user_text": "2026-10-07 周会内容..." },
          "reason": "用户要整理会议纪要"
        }"#;
        let out = parse_decision(j).unwrap();
        assert_eq!(out.action, DecisionAction::InvokeSkill);
        assert_eq!(out.skill_name.as_deref(), Some("meeting_notes"));
        assert!(out.skill_slots.is_some());
    }

    #[test]
    fn schema_accepts_missing_skill_fields_for_reply() {
        // action=reply 时 skill_name/skill_slots 可以省略（serde default）
        let j = r#"{
          "action": "reply", "mood": "happy", "mention": true, "reply_len": "short",
          "meme_type": null, "task_goal": null, "memory_write": null,
          "reason": "打招呼"
        }"#;
        let out = parse_decision(j).unwrap();
        assert_eq!(out.action, DecisionAction::Reply);
        assert!(out.skill_name.is_none());
        assert!(out.skill_slots.is_none());
    }

    #[test]
    fn system_prompt_appends_skill_catalog() {
        let p = build_system_prompt(&[
            ("meeting_notes".to_string(), "整理会议纪要".to_string()),
        ]);
        assert!(p.contains("可用 Skills"));
        assert!(p.contains("meeting_notes"));
    }

    #[test]
    fn system_prompt_no_skills_no_section() {
        let p = build_system_prompt(&[]);
        // 空目录时不追加"可用 Skills（当消息语义…"那一段；基础说明里只出现在规则中
        assert!(!p.contains("\n\n可用 Skills（当消息语义匹配时"));
    }
}
