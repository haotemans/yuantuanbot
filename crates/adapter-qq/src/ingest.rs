//! message 事件摄取（段数组铁律）：
//! text 段拼正文、at 段→mentions（命中自身号则 at_me）、reply 段→reply_to（记录原值）、
//! image 段→has_image、face 段忽略。@全体成员不进入 mentions。
//! 同时维护 persons / identities / member_profiles 档案，然后 messages 落库并发布事件。

use anyhow::{bail, Context, Result};
use rusqlite::params;
use serde_json::Value;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;
use yuantuan_core::event::{Event, EventBus, MessageReceivedPayload};

pub fn ingest_message(v: &Value, bus: &EventBus, db_path: &Path, self_qq: u64) -> Result<()> {
    let message_type = v
        .get("message_type")
        .and_then(|t| t.as_str())
        .context("message 事件缺 message_type")?;
    let ts = v
        .get("time")
        .and_then(|t| t.as_i64())
        .unwrap_or_else(now_secs);
    let user_id = v
        .pointer("/sender/user_id")
        .or_else(|| v.get("user_id"))
        .and_then(|u| u.as_u64())
        .context("message 事件缺 sender.user_id")?;

    let sender = v.get("sender");
    let card = sender
        .and_then(|s| s.get("card"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let nickname = sender
        .and_then(|s| s.get("nickname"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    // 群名片取 card，无则 nickname
    let display = if card.is_empty() { nickname } else { card };

    let (chat_type, chat_id) = match message_type {
        "group" => (
            "group",
            v.get("group_id")
                .and_then(|g| g.as_u64())
                .map(|g| g.to_string())
                .context("群消息缺 group_id")?,
        ),
        "private" => ("private", format!("dm_{user_id}")),
        other => bail!("未知 message_type: {other}"),
    };

    let mut text = String::new();
    let mut mentions: Vec<String> = Vec::new();
    let mut at_me = false;
    let mut reply_to: Option<i64> = None;
    let mut has_image = false;
    let segments = v
        .get("message")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    for seg in &segments {
        match seg.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                text.push_str(seg.pointer("/data/text").and_then(|t| t.as_str()).unwrap_or(""))
            }
            Some("at") => {
                let qq = seg.pointer("/data/qq").and_then(|q| q.as_str()).unwrap_or("");
                if qq == "all" {
                    debug!("忽略 @全体成员");
                } else if let Ok(n) = qq.parse::<u64>() {
                    if self_qq != 0 && n == self_qq {
                        at_me = true;
                    }
                    mentions.push(format!("p_{n}"));
                }
            }
            Some("reply") => {
                // 记录被引 message_id 原值（OneBot 段字段为字符串数字）
                reply_to = seg
                    .pointer("/data/id")
                    .and_then(|i| i.as_str())
                    .and_then(|s| s.parse::<i64>().ok());
            }
            Some("image") => has_image = true,
            Some("face") => {} // V1 忽略不存
            Some(other) => debug!(seg_type = other, "忽略段类型"),
            None => {}
        }
    }
    if self_qq == 0 {
        debug!("自身 QQ 号未知（get_login_info 未完成），at_me 判定退化");
    }

    let person_id = format!("p_{user_id}");
    let mut conn = yuantuan_core::db::connect(db_path)?;
    let tx = conn.transaction().context("开启摄取事务失败")?;
    tx.execute(
        "INSERT INTO persons(person_id, display_name, first_seen, last_seen) VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(person_id) DO UPDATE SET last_seen = excluded.last_seen, display_name = excluded.display_name",
        params![person_id, display, ts],
    )?;
    tx.execute(
        "INSERT INTO identities(person_id, platform, platform_uid) VALUES (?1, 'qq', ?2)
         ON CONFLICT(platform, platform_uid) DO NOTHING",
        params![person_id, user_id.to_string()],
    )?;
    if chat_type == "group" {
        tx.execute(
            "INSERT INTO member_profiles(chat_id, person_id, card, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(chat_id, person_id) DO UPDATE SET card = excluded.card, updated_at = excluded.updated_at",
            params![chat_id, person_id, display, ts],
        )?;
    }
    let mentions_json = serde_json::to_string(&mentions)?;
    tx.execute(
        "INSERT INTO messages(chat_id, chat_type, sender_pid, nickname, text, mentions, reply_to, at_me, has_image, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            chat_id,
            chat_type,
            person_id,
            display,
            text,
            mentions_json,
            reply_to,
            at_me as i64,
            has_image as i64,
            ts
        ],
    )?;
    let msg_id = tx.last_insert_rowid();
    tx.commit().context("提交摄取事务失败")?;

    bus.publish(Event::MessageReceived(MessageReceivedPayload {
        msg_id,
        chat_id,
        chat_type: chat_type.into(),
        sender_pid: person_id,
        text,
        at_me,
        has_image,
        ts,
    }));
    Ok(())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
