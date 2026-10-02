//! Prefilter 规则集（架构文档十三章终稿）：顺序短路 R1-R7。
//! 阈值来自 config.toml [prefilter]（本单读启动配置，热配后续施工单）。

use crate::event::MessageReceivedPayload;
use rusqlite::params;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// 发言节流窗口（秒）
    pub window_secs: i64,
    /// 窗口内 self 消息数硬顶（个）
    pub self_msg_cap: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            window_secs: 60,
            self_msg_cap: 12,
        }
    }
}

/// 自身已发消息的 NapCat message_id 集合（内存环形，容量 512）。
/// R4「回复/引用我的消息」判定原料。TODO：随发送链路落地时在发送侧持久化该映射，
/// 进程重启后重启前自己消息的引用回复将不被 R4 命中（降级为正常节流路径）。
#[derive(Clone, Default)]
pub struct SelfMsgIds {
    inner: Arc<Mutex<VecDeque<i64>>>,
}

impl SelfMsgIds {
    const CAP: usize = 512;

    pub fn record(&self, msg_id: i64) {
        let mut q = self.inner.lock().unwrap();
        if q.len() >= Self::CAP {
            q.pop_front();
        }
        q.push_back(msg_id);
    }

    pub fn contains(&self, msg_id: i64) -> bool {
        self.inner.lock().unwrap().contains(&msg_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Drop(&'static str),
}

/// 顺序短路判定。self_pid 形如 "p_<bot QQ>"，未知时为 None（此时 R1 仅凭 sender_pid == "self"）。
pub fn check(
    cfg: &Config,
    self_pid: Option<&str>,
    self_ids: &SelfMsgIds,
    db_path: &Path,
    msg: &MessageReceivedPayload,
) -> Verdict {
    // R1：发送者是自己（常开不可关，防自嗨死循环）
    if msg.sender_pid == "self" || Some(msg.sender_pid.as_str()) == self_pid {
        return Verdict::Drop("R1:发送者是自己");
    }
    // R2：发送者是其他 bot / 系统 / 匿名消息
    if msg.sender_bot {
        return Verdict::Drop("R2:bot或系统消息");
    }
    // R3：@我 必放行
    if msg.at_me {
        return Verdict::Pass;
    }
    // R4：回复/引用我的消息 必放行
    if let Some(rt) = msg.reply_to {
        if self_ids.contains(rt) {
            return Verdict::Pass;
        }
    }
    // R5：私聊必放行（仍留给 Decision 以获得 mood / memory_write / task 分流）
    if msg.chat_type == "private" {
        return Verdict::Pass;
    }
    // R6：群聊普通消息过节流闸——只负责拦截，过了交给后续规则
    // TODO(doc): 终稿口径为「每 chat 每 60 秒最多 4 个回复回合」；当前以
    // 「60s 内 messages 表中 self 消息数 <= 12」作代理（见施工单 4 任务卡，别过度工程）。
    if msg.chat_type == "group" {
        let cutoff = now_secs() - cfg.window_secs;
        let self_count = crate::db::connect(db_path)
            .ok()
            .and_then(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM messages WHERE chat_id = ?1 AND sender_pid = 'self' AND ts >= ?2",
                    params![msg.chat_id, cutoff],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
            })
            .unwrap_or(0);
        if self_count > cfg.self_msg_cap {
            return Verdict::Drop("R6:发言节流");
        }
    }
    // R7：无文字纯表情/图片（未@我，到此处必然未命中 R3）丢弃；偷表情包入库不受影响
    if msg.text.trim().is_empty() {
        return Verdict::Drop("R7:无文字纯图片或表情");
    }
    Verdict::Pass
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("yt-prefilter-test-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("yuantuan.db");
        let mut conn = crate::db::connect(&db).unwrap();
        crate::db::migrate(&mut conn).unwrap();
        db
    }

    fn base_msg() -> MessageReceivedPayload {
        MessageReceivedPayload {
            msg_id: 1,
            chat_id: "555666".into(),
            chat_type: "group".into(),
            sender_pid: "p_2001".into(),
            text: "随便聊聊".into(),
            at_me: false,
            has_image: false,
            reply_to: None,
            sender_bot: false,
            image_urls: Vec::new(),
            ts: now_secs(),
        }
    }

    #[test]
    fn r1_drops_self() {
        let db = temp_db();
        let mut m = base_msg();
        m.sender_pid = "self".into();
        assert_eq!(check(&Config::default(), None, &SelfMsgIds::default(), &db, &m), Verdict::Drop("R1:发送者是自己"));
        // 平台号命中 self_pid
        let mut m2 = base_msg();
        m2.sender_pid = "p_10001".into();
        assert_eq!(
            check(&Config::default(), Some("p_10001"), &SelfMsgIds::default(), &db, &m2),
            Verdict::Drop("R1:发送者是自己")
        );
    }

    #[test]
    fn r2_drops_bot_sender() {
        let db = temp_db();
        let mut m = base_msg();
        m.sender_bot = true;
        m.at_me = true; // 顺序短路：R2 在 R3 之前
        assert_eq!(check(&Config::default(), None, &SelfMsgIds::default(), &db, &m), Verdict::Drop("R2:bot或系统消息"));
    }

    #[test]
    fn r3_at_me_passes() {
        let db = temp_db();
        let mut m = base_msg();
        m.at_me = true;
        m.text = String::new(); // 即使无文字也必放行（R3 先于 R7）
        assert_eq!(check(&Config::default(), None, &SelfMsgIds::default(), &db, &m), Verdict::Pass);
    }

    #[test]
    fn r4_reply_to_me_passes() {
        let db = temp_db();
        let ids = SelfMsgIds::default();
        ids.record(424242);
        let mut m = base_msg();
        m.reply_to = Some(424242);
        assert_eq!(check(&Config::default(), None, &ids, &db, &m), Verdict::Pass);
        // 未记录的引用不命中 R4，继续后续规则（文本非空 → Pass）
        let mut m2 = base_msg();
        m2.reply_to = Some(999);
        assert_eq!(check(&Config::default(), None, &ids, &db, &m2), Verdict::Pass);
    }

    #[test]
    fn r5_private_passes() {
        let db = temp_db();
        let mut m = base_msg();
        m.chat_type = "private".into();
        m.chat_id = "dm_2001".into();
        m.text = String::new();
        assert_eq!(check(&Config::default(), None, &SelfMsgIds::default(), &db, &m), Verdict::Pass);
    }

    #[test]
    fn r6_throttle_drops_when_over_cap() {
        let db = temp_db();
        let conn = crate::db::connect(&db).unwrap();
        let now = now_secs();
        // 先建 self 档案再插 13 条 self 消息（超 12 顶）→ Drop
        for _ in 0..13 {
            conn.execute(
                "INSERT INTO persons(person_id, display_name, first_seen, last_seen) VALUES ('self','云团',?1,?1)
                 ON CONFLICT(person_id) DO NOTHING",
                params![now],
            ).unwrap();
            conn.execute(
                "INSERT INTO messages(chat_id, chat_type, sender_pid, text, ts) VALUES ('555666','group','self','回',?1)",
                params![now],
            ).unwrap();
        }
        assert_eq!(
            check(&Config::default(), None, &SelfMsgIds::default(), &db, &base_msg()),
            Verdict::Drop("R6:发言节流")
        );
        // 不超顶 → 过闸
        let db2 = temp_db();
        assert_eq!(check(&Config::default(), None, &SelfMsgIds::default(), &db2, &base_msg()), Verdict::Pass);
    }

    #[test]
    fn r7_drops_image_only() {
        let db = temp_db();
        let mut m = base_msg();
        m.text = "   ".into();
        m.has_image = true;
        assert_eq!(check(&Config::default(), None, &SelfMsgIds::default(), &db, &m), Verdict::Drop("R7:无文字纯图片或表情"));
    }
}
