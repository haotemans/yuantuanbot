//! message / message_sent 事件摄取（段数组铁律）：
//! text 段拼正文、at 段→mentions（命中自身号则 at_me）、reply 段→reply_to（记录原值）、
//! image 段→has_image、face 段忽略。@全体成员不进入 mentions。
//! 自己的消息（user_id == 自身号）sender_pid 记 'self'，并把 NapCat message_id 记入 SelfMsgIds（R4 原料）。
//! 同时维护 persons / identities / member_profiles 档案，然后 messages 落库并发布事件。

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;
use yuantuan_core::db::params;
use yuantuan_core::db::SqliteExt;
use yuantuan_core::event::{Event, EventBus, MessageReceivedPayload};
use yuantuan_core::prefilter::SelfMsgIds;

pub async fn ingest_message(
    v: &Value,
    bus: &EventBus,
    db_path: &Path,
    self_qq: u64,
    self_ids: &SelfMsgIds,
) -> Result<()> {
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
    let napcat_msg_id = v.get("message_id").and_then(|i| i.as_i64());
    let is_self = self_qq != 0 && user_id == self_qq;

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
    // R2 原料：sender 带 bot 标记或匿名
    let sender_bot = sender
        .and_then(|s| s.get("anonymous"))
        .map(|a| !a.is_null())
        .unwrap_or(false)
        || sender
            .and_then(|s| s.get("bot"))
            .and_then(|b| b.as_bool())
            .unwrap_or(false);

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
    let mut image_urls: Vec<String> = Vec::new();
    let segments = v
        .get("message")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    for seg in &segments {
        match seg.get("type").and_then(|t| t.as_str()) {
            Some("text") => text.push_str(
                seg.pointer("/data/text")
                    .and_then(|t| t.as_str())
                    .unwrap_or(""),
            ),
            Some("at") => {
                let qq = seg
                    .pointer("/data/qq")
                    .and_then(|q| q.as_str())
                    .unwrap_or("");
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
            Some("image") => {
                has_image = true;
                // data.url → 事件载荷（偷表情包流程消费；不落 messages）
                if let Some(u) = seg.pointer("/data/url").and_then(|u| u.as_str()) {
                    if u.starts_with("http") && !image_urls.iter().any(|x| x == u) {
                        image_urls.push(u.to_string());
                    }
                }
            }
            Some("face") => {} // V1 忽略不存
            Some(other) => debug!(seg_type = other, "忽略段类型"),
            None => {}
        }
    }
    if self_qq == 0 {
        debug!("自身 QQ 号未知（get_login_info 未完成），at_me 判定退化");
    }
    // 自己的消息回报：记录 NapCat message_id，Prefilter R4（回复/引用我的消息）据此判定
    if is_self {
        if let Some(id) = napcat_msg_id {
            self_ids.record(id);
        }
    }

    // 云团 = 特殊 person_id 'self'（data-model 五章）
    let person_id = if is_self {
        "self".to_string()
    } else {
        format!("p_{user_id}")
    };
    let display_name = if is_self { "云团" } else { display };
    let mut conn = yuantuan_core::db::connect(db_path).await?;
    let mut tx = conn.transaction().await.context("开启摄取事务失败")?;
    tx.execute(
        "INSERT INTO persons(person_id, display_name, first_seen, last_seen) VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(person_id) DO UPDATE SET last_seen = excluded.last_seen, display_name = excluded.display_name",
        params![person_id, display_name, ts],
    ).await?;
    tx.execute(
        "INSERT INTO identities(person_id, platform, platform_uid) VALUES (?1, 'qq', ?2)
         ON CONFLICT(platform, platform_uid) DO NOTHING",
        params![person_id, user_id.to_string()],
    )
    .await?;
    if chat_type == "group" && !is_self {
        tx.execute(
            "INSERT INTO member_profiles(chat_id, person_id, card, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(chat_id, person_id) DO UPDATE SET card = excluded.card, updated_at = excluded.updated_at",
            params![chat_id, person_id, display, ts],
        ).await?;
    }
    let mentions_json = serde_json::to_string(&mentions)?;
    tx.execute(
        "INSERT INTO messages(chat_id, chat_type, sender_pid, nickname, text, mentions, reply_to, at_me, has_image, ts, external_msg_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            chat_id,
            chat_type,
            person_id,
            display_name,
            text,
            mentions_json,
            reply_to,
            at_me as i64,
            has_image as i64,
            ts,
            napcat_msg_id
        ],
    ).await?;
    let msg_id = tx.last_insert_rowid().await?;
    tx.commit().await.context("提交摄取事务失败")?;

    bus.publish(Event::MessageReceived(MessageReceivedPayload {
        msg_id,
        chat_id,
        chat_type: chat_type.into(),
        sender_pid: person_id,
        text,
        at_me,
        has_image,
        reply_to,
        sender_bot,
        image_urls,
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
