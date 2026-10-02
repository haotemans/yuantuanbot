//! Context Builder（架构文档十二章）：Bot Context 四层组装——
//! ① system（active 人格 + mood 一句 + 行为准则）
//! ② 在场名册（近 20 条发送者人物卡：person_id + 名片 + 关系分 + ≤3 条记忆）
//! ③ 会话区（本 chat 最新 K=20 条，一行一条，含 self 发言）
//! ④ 预算 ≤40k 字符代理值：先缩 K、再裁记忆条数；名册条数与锚点不动。
//! 模型认人靠名册以 person_id 锚定，绝不用昵称反推身份。

use crate::event::MessageReceivedPayload;
use crate::state::MoodValue;
use rusqlite::params;
use std::path::Path;

/// 输入预算（字符代理值；40k tokens 代理，可配置化在热配施工单）
const BUDGET_CHARS: usize = 40_000;
const K_INIT: usize = 20;
const K_MIN: usize = 5;

const DEFAULT_PERSONA: &str =
    "你是「云团」，一个长期活跃在 QQ 群里的普通群友，性格温和有点小幽默。\
     说话短句、口语化，像真人一样聊天，不当客服、不端架子。\
     （WebUI 尚未配置人格版本，此为内置极简默认人设——请到管理面板「人格」页设置正式人设。）";

const BEHAVIOR_RULES: &str = r#"行为准则：
- 回复用「‖」分隔拆成多条短气泡；一句话能说完就只发一泡，绝不为分泡而分泡
- 可在气泡内用独立指令行：「::at」表示这一泡@对方；「::meme 类别」表示发表情包
- 短句风格，像真人群聊；不发论文式长段，不刷屏
- 绝不暴露自己是程序或提及任何内部机制（person_id、记忆库、规则等名词禁止出现）
- 一律用中文口语回复"#;

pub struct BotContext {
    pub system: String,
    pub user: String,
    /// 实际采用的会话窗口 K（预算压缩后的值，供日志观测）
    pub k_used: usize,
}

/// 组装 Bot Context。anchor 为触发消息（@我/回复我/私聊的那条），单独标注。
pub fn build_bot_context(db_path: &Path, mood: MoodValue, anchor: &MessageReceivedPayload) -> BotContext {
    let conn = crate::db::connect(db_path).ok();
    let persona = conn.as_ref().and_then(|c: &rusqlite::Connection| {
        c.query_row(
            "SELECT content FROM personality_versions WHERE active = 1 ORDER BY version_no DESC LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
    });
    if persona.is_none() {
        tracing::info!("无 active 人格版本，使用内置极简默认人设（请到 WebUI 人格页配置）");
    }
    let system = format!(
        "{}\n\n当前情绪：{}\n\n{}",
        persona.unwrap_or_else(|| DEFAULT_PERSONA.into()),
        mood.as_str(),
        BEHAVIOR_RULES
    );

    let mut k = K_INIT;
    let mut mem_per: usize = 3;
    let mut user = render_user(conn.as_ref(), anchor, k, mem_per);
    // 预算裁决：先缩 K，再裁记忆；名册条数与锚点不动
    while system.chars().count() + user.chars().count() > BUDGET_CHARS {
        if k > K_MIN {
            k = (k - 5).max(K_MIN);
        } else if mem_per > 0 {
            mem_per -= 1;
        } else {
            break;
        }
        user = render_user(conn.as_ref(), anchor, k, mem_per);
    }
    if k < K_INIT || mem_per < 3 {
        tracing::info!(k_used = k, mem_per, "Bot Context 超出预算，已按层压缩");
    }
    BotContext { system, user, k_used: k }
}

fn render_user(
    conn: Option<&rusqlite::Connection>,
    anchor: &MessageReceivedPayload,
    k: usize,
    mem_per: usize,
) -> String {
    let Some(c) = conn else {
        return format!("【会话区】\n（数据库不可用）\n\n【锚点消息】\n{}", anchor.text);
    };

    // 名册：近 20 条（固定名册窗，不随 K 缩）内出现过的非 self 发送者
    let roster_rows: Vec<(String, String)> = c
        .prepare(
            "SELECT DISTINCT sender_pid, nickname FROM (
               SELECT sender_pid, COALESCE(nickname, '') AS nickname, msg_id
               FROM messages WHERE chat_id = ?1 ORDER BY msg_id DESC LIMIT 20
             ) WHERE sender_pid != 'self'",
        )
        .and_then(|mut st| {
            let rows = st.query_map(params![anchor.chat_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<std::result::Result<Vec<(String, String)>, _>>()
        })
        .unwrap_or_default();
    let mut roster = String::from("【在场名册】（person_id 是身份锚，昵称仅人话渲染）");
    if roster_rows.is_empty() {
        roster.push_str("\n（暂无人名册）");
    }
    for (pid, nick) in &roster_rows {
        let (trust, familiar): (f64, f64) = c
            .query_row(
                "SELECT trust, familiar FROM relationship_edges WHERE from_pid = 'self' AND to_pid = ?1",
                params![pid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or((0.5, 0.0));
        let mems: Vec<String> = if mem_per > 0 {
            c.prepare(
                "SELECT content FROM long_memories WHERE owner_type = 'person' AND owner_id = ?1
                 ORDER BY updated_at DESC LIMIT ?2",
            )
            .and_then(|mut st| {
                let rows = st.query_map(params![pid, mem_per as i64], |r| r.get(0))?;
                rows.collect::<std::result::Result<Vec<String>, _>>()
            })
            .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mem_str = if mems.is_empty() {
            String::new()
        } else {
            format!(" 记忆: {}", mems.join("；"))
        };
        roster.push_str(&format!(
            "\n- {pid}「{nick}」 trust={trust:.2} familiar={familiar:.2} 亲密={familiar:.2}{mem_str}"
        ));
    }

    // 会话区：本 chat 最新 K 条，一行一条，含 self 发言
    let conv_rows: Vec<(String, String, String, i64)> = c
        .prepare(
            "SELECT sender_pid, COALESCE(nickname, ''), COALESCE(text, ''), has_image FROM (
               SELECT sender_pid, nickname, text, has_image, msg_id
               FROM messages WHERE chat_id = ?1 ORDER BY msg_id DESC LIMIT ?2
             ) ORDER BY msg_id ASC",
        )
        .and_then(|mut st| {
            let rows = st.query_map(params![anchor.chat_id, k as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            rows.collect::<std::result::Result<Vec<(String, String, String, i64)>, _>>()
        })
        .unwrap_or_default();
    let mut conv = String::from("【会话区】（最新在底部）");
    for (pid, nick, text, has_image) in &conv_rows {
        let who = if pid == "self" { "我".to_string() } else { nick.clone() };
        let img = if *has_image == 1 { "（图片）" } else { "" };
        conv.push_str(&format!("\n{who}: {text}{img}"));
    }

    // 锚点：触发消息 nick 从 messages 行取
    let anchor_nick: String = c
        .query_row(
            "SELECT COALESCE(nickname, '') FROM messages WHERE msg_id = ?1",
            params![anchor.msg_id],
            |r| r.get(0),
        )
        .unwrap_or_default();
    let anchor_block = format!(
        "【锚点消息】（本次回复围绕它）\n{}: {}",
        if anchor.sender_pid == "self" { "我".into() } else { anchor_nick },
        anchor.text
    );

    format!("{roster}\n\n{conv}\n\n{anchor_block}")
}
