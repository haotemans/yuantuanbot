//! 带出处的人物简档与话题记忆；每日摘要仍是时间线索引，不作为简档输入。
use anyhow::{ensure, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileField {
    PreferredName,
    TechnicalPreferences,
    OngoingProjects,
    CommunicationStyle,
}

impl ProfileField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreferredName => "preferred_name",
            Self::TechnicalPreferences => "technical_preferences",
            Self::OngoingProjects => "ongoing_projects",
            Self::CommunicationStyle => "communication_style",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProfileUpdate {
    pub field: ProfileField,
    pub value: String,
    pub evidence_msg_id: i64,
    pub evidence_quote: String,
}

/// 资料必须来自本轮允许范围内的本人原话。只校验来源，不宣称验证语义真实。
pub fn update_profile(
    conn: &Connection,
    person: &str,
    chat: &str,
    start: i64,
    end: i64,
    update: &ProfileUpdate,
) -> Result<bool> {
    ensure!(person != "self", "Bot 自己的回答不能生成人物简档");
    ensure!(
        (start..=end).contains(&update.evidence_msg_id),
        "人物资料出处超出本轮范围"
    );
    let value = update.value.trim();
    let quote = update.evidence_quote.trim();
    ensure!(
        !value.is_empty() && value.chars().count() <= 200,
        "人物资料长度不合法"
    );
    ensure!(
        (4..=240).contains(&quote.chars().count()),
        "人物资料须附 4–240 字原话"
    );
    ensure!(
        !crate::consolidation::is_sensitive(value) && !crate::consolidation::is_sensitive(quote),
        "人物资料含敏感内容"
    );
    let source: Option<(String, i64)> = conn.query_row(
        "SELECT COALESCE(text,''), ts FROM messages WHERE msg_id=?1 AND sender_pid=?2 AND chat_id=?3",
        params![update.evidence_msg_id, person, chat], |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional()?;
    let (text, ts) = source.ok_or_else(|| anyhow::anyhow!("人物资料原作者或会话不匹配"))?;
    ensure!(text.contains(quote), "人物资料引文不在原消息中");
    // 更晚的原话推进出处；相同内容也推进，避免旧归纳覆盖更新的本人确认。
    Ok(conn.execute(
        "INSERT INTO person_profile_facts(person_id, field, content, source_msg_id, evidence_quote, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(person_id,field) DO UPDATE SET content=excluded.content,
           source_msg_id=excluded.source_msg_id, evidence_quote=excluded.evidence_quote, updated_at=excluded.updated_at
         WHERE excluded.source_msg_id > person_profile_facts.source_msg_id",
        params![person, update.field.as_str(), value, update.evidence_msg_id, quote, ts],
    )? > 0)
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileFact {
    pub field: String,
    pub content: String,
    pub source_msg_id: i64,
    pub evidence_quote: String,
    pub updated_at: i64,
}

pub fn profile(conn: &Connection, person: &str, cutoff: i64) -> Result<Vec<ProfileFact>> {
    let mut st = conn.prepare(
        "SELECT p.field,p.content,p.source_msg_id,p.evidence_quote,p.updated_at
         FROM person_profile_facts p JOIN messages m ON m.msg_id=p.source_msg_id
         WHERE p.person_id=?1 AND m.sender_pid=p.person_id AND p.source_msg_id<=?2
           AND instr(m.text,p.evidence_quote)>0 ORDER BY p.field LIMIT 4",
    )?;
    let rows = st
        .query_map(params![person, cutoff], |r| {
            Ok(ProfileFact {
                field: r.get(0)?,
                content: r.get(1)?,
                source_msg_id: r.get(2)?,
                evidence_quote: r.get(3)?,
                updated_at: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
pub struct RecalledMemory {
    pub id: i64,
    pub owner_type: String,
    pub owner_id: String,
    pub content: String,
    pub source: String,
    pub updated_at: i64,
    pub source_chat_id: Option<String>,
    pub source_msg_id: Option<i64>,
    pub source_end_msg_id: Option<i64>,
    pub evidence_quote: Option<String>,
    pub evidence_status: String,
    pub content_truncated: bool,
}

/// 英文词 + 中文相邻双字匹配；只用于检索，不冒充语义向量或事实置信度。
pub fn query_terms(text: &str) -> Vec<String> {
    let lower: String = text
        .chars()
        .take(800)
        .flat_map(char::to_lowercase)
        .collect();
    let mut terms = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |term: String| {
        if term.chars().count() >= 2
            && ![
                "这个", "那个", "怎么", "什么", "一下", "现在", "还是", "我们", "你们", "可以",
                "知道", "之前", "已经", "the", "and", "what", "this", "that", "with",
            ]
            .contains(&term.as_str())
            && seen.insert(term.clone())
        {
            terms.push(term);
        }
    };
    for word in lower.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if !word.is_empty() {
            push(word.to_owned());
        }
    }
    let chars: Vec<char> = lower.chars().collect();
    for pair in chars.windows(2) {
        if pair.iter().all(|c| ('\u{3400}'..='\u{9fff}').contains(c)) {
            push(pair.iter().collect());
        }
    }
    terms.truncate(16);
    terms
}

/// owner 索引限定范围，最多返回 128 个候选，再按相关性/时间排序。
pub fn recall(
    conn: &Connection,
    person: &str,
    chat: &str,
    cutoff: i64,
    query: &str,
    limit: usize,
) -> Result<Vec<RecalledMemory>> {
    let terms = query_terms(query);
    if terms.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let conditions = (0..terms.len())
        .map(|i| format!("instr(lower(content),?{})>0", i + 4))
        .collect::<Vec<_>>()
        .join(" OR ");
    let sql = format!(
        "SELECT id,owner_type,owner_id,substr(content,1,800),source,updated_at,
                source_chat_id,source_msg_id,source_end_msg_id,length(content)>800
         FROM long_memories WHERE ((owner_type='person' AND owner_id=?1) OR (owner_type='chat' AND owner_id=?2))
         AND (source_msg_id IS NULL OR source_msg_id<=?3) AND (source_end_msg_id IS NULL OR source_end_msg_id<=?3)
         AND ({conditions}) ORDER BY updated_at DESC,id DESC LIMIT 128"
    );
    let mut binds: Vec<rusqlite::types::Value> = vec![
        person.to_owned().into(),
        chat.to_owned().into(),
        cutoff.into(),
    ];
    binds.extend(terms.iter().cloned().map(Into::into));
    let mut st = conn.prepare(&sql)?;
    let rows = st
        .query_map(rusqlite::params_from_iter(binds), |r| {
            Ok(RecalledMemory {
                id: r.get(0)?,
                owner_type: r.get(1)?,
                owner_id: r.get(2)?,
                content: r.get(3)?,
                source: r.get(4)?,
                updated_at: r.get(5)?,
                source_chat_id: r.get(6)?,
                source_msg_id: r.get(7)?,
                source_end_msg_id: r.get(8)?,
                evidence_quote: None,
                evidence_status: "legacy_unverified".into(),
                content_truncated: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut ranked: Vec<_> = rows
        .into_iter()
        .filter(|m| !crate::consolidation::is_sensitive(&m.content))
        .map(|m| {
            let content = m.content.to_lowercase();
            let score: usize = terms
                .iter()
                .filter(|t| content.contains(t.as_str()))
                .map(|t| t.chars().count())
                .sum();
            (score, m)
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    ranked.sort_by_key(|(score, m)| std::cmp::Reverse((*score, m.updated_at, m.id)));
    let mut result = Vec::new();
    let mut seen_content = BTreeSet::new();
    for (_, mut m) in ranked {
        if !seen_content.insert((m.owner_type.clone(), m.owner_id.clone(), m.content.clone())) {
            continue;
        }
        if let (Some(source), Some(source_chat)) = (m.source_msg_id, m.source_chat_id.as_deref()) {
            if let Some(end) = m.source_end_msg_id {
                m.evidence_status = "consolidation_window_not_individual_proof".into();
                if end < source {
                    m.evidence_status = "invalid_source_range".into();
                }
            } else {
                let raw: Option<(String,String)> = conn.query_row(
                    "SELECT sender_pid,substr(COALESCE(text,''),1,240) FROM messages WHERE msg_id=?1 AND chat_id=?2",
                    params![source,source_chat], |r| Ok((r.get(0)?,r.get(1)?)),
                ).optional()?;
                match raw {
                    Some((sender, text))
                        if sender != "self"
                            && (m.owner_type != "person" || sender == m.owner_id) =>
                    {
                        m.evidence_status = "source_message_available_not_fact_verification".into();
                        if !crate::consolidation::is_sensitive(&text) {
                            m.evidence_quote = Some(text);
                        }
                    }
                    _ => m.evidence_status = "source_missing_or_author_mismatch".into(),
                }
            }
        }
        result.push(m);
        if result.len() >= limit.min(10) {
            break;
        }
    }
    Ok(result)
}
