//! 夜间归纳（架构文档七章 + docs/reference/data-model.md 十一章更新语义）：
//! 调度：config [consolidation]（enabled / daily_time="HH:MM" / run_on_startup 调试项），
//! tokio 定时任务 + AtomicBool 单实例锁（夜间归纳重跑会脏数据，见 runtime-design 三串行）。
//!
//! 执行（每 chat 独立，最新 500 条窗口，当日零新消息的 chat 跳过）：
//! - @关系统计纯 SQL：窗口内 mentions 对计数 → relationship_events(kind='mention',
//!   delta_familiar=0.01×count 单边单次封顶 0.05) → edges 增量累加 clamp(0..1)；
//!   @bot 计入 self→该 user 的 familiar
//! - LLM 提炼（decision 角色）：chat_summary / person_facts / group_facts / rel_events
//! - 写入：summaries 每日 UPSERT、long_memories(consolidation)、rel_events→edges(trust)
//! - 敏感拒收：写入前每条 fact/summary/evidence 过 is_sensitive，命中丢弃记日志
//!
//! 幂等方案（简单可靠）：每 chat 处理“开始”即在 state_kv 记当日完成戳（先行占位，
//! 中途进程崩则该 chat 当日损失一轮，可接受）；同日重复执行整 chat 跳过。
//! summaries 另有 UNIQUE(owner_type,owner_id,period,date) + UPSERT 兜底防重。

use crate::event::{
    ChatConsolidationOutcome, ConsolidationDonePayload, Event, EventBus,
};
use crate::llm::{LlmGateway, Role};
use anyhow::{Context, Result};
use rusqlite::params;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

static RUNNING: AtomicBool = AtomicBool::new(false);

const WINDOW: i64 = 500;
const MENTION_DELTA_PER: f64 = 0.01;
const MENTION_DELTA_CAP: f64 = 0.05;

#[derive(Debug, Clone)]
pub struct ConsolidationCfg {
    pub enabled: bool,
    pub daily_time: String,
    pub run_on_startup: bool,
}

impl Default for ConsolidationCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            daily_time: "03:00".into(),
            run_on_startup: false,
        }
    }
}

pub struct ConsolidationDeps {
    pub db_path: PathBuf,
    /// 无 decision 角色 → LLM 提炼整轮跳过（仅 @统计照旧跑不起作用？不——纯统计独立于 LLM，照常执行）
    pub llm: Option<Arc<LlmGateway>>,
    pub bus: EventBus,
    pub self_qq: Arc<AtomicU64>,
    pub cfg: ConsolidationCfg,
}

// ---------- 调度 ----------

pub fn spawn_scheduler(deps: ConsolidationDeps) -> JoinHandle<()> {
    tokio::spawn(async move {
        let deps = Arc::new(deps);
        if deps.cfg.run_on_startup {
            info!("consolidation run_on_startup 调试项开启：立即执行一次");
            run_once(&deps).await;
        }
        loop {
            let wait = secs_until_next(&deps.db_path, &deps.cfg.daily_time);
            info!(hours = wait / 3600, daily_time = %deps.cfg.daily_time, "夜间归纳下次执行倒计时");
            tokio::time::sleep(Duration::from_secs(wait.max(1))).await;
            run_once(&deps).await;
        }
    })
}

/// 距下一次 daily_time（本地 HH:MM）的秒数；用 SQL 的 localtime 换算避免引时区依赖
fn secs_until_next(db_path: &Path, daily_time: &str) -> u64 {
    let (h, m) = parse_hhmm(daily_time).unwrap_or((3, 0));
    let now = now_secs();
    let midnight: i64 = crate::db::connect(db_path)
        .ok()
        .and_then(|c| {
            c.query_row(
                "SELECT CAST(strftime('%s','now','localtime','start of day','utc') AS INTEGER)",
                [],
                |r| r.get(0),
            )
            .ok()
        })
        .unwrap_or(now - now % 86400);
    let mut target = midnight + (h as i64) * 3600 + (m as i64) * 60;
    if target <= now {
        target += 86400;
    }
    (target - now) as u64
}

fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    (h < 24 && m < 60).then_some((h, m))
}

// ---------- 单轮执行 ----------

pub async fn run_once(deps: &ConsolidationDeps) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        info!("夜间归纳已有实例在跑（单实例锁），本次跳过");
        return;
    }
    let _guard = scopeguard_defer(|| RUNNING.store(false, Ordering::SeqCst));
    let started = Instant::now();
    info!("夜间归纳开始");

    let today = today_str(&deps.db_path);
    let chat_ids = list_chats(&deps.db_path);
    let mut outcomes: Vec<ChatConsolidationOutcome> = Vec::new();
    for chat_id in &chat_ids {
        let outcome = process_chat(deps, chat_id, &today).await;
        outcomes.push(outcome);
    }

    deps.bus.publish(Event::ConsolidationDone(ConsolidationDonePayload {
        date: today.clone(),
        chats: outcomes.clone(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    }));
    info!(
        date = %today,
        chats = outcomes.len(),
        ok = outcomes.iter().filter(|o| o.status == "ok").count(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "夜间归纳完成"
    );
}

fn scopeguard_defer<F: FnOnce()>(f: F) -> impl Drop {
    struct Guard<F: FnOnce()>(Option<F>);
    impl<F: FnOnce()> Drop for Guard<F> {
        fn drop(&mut self) {
            if let Some(f) = self.0.take() {
                f()
            }
        }
    }
    Guard(Some(f))
}

fn stamp_key(chat_id: &str, date: &str) -> String {
    format!("consolidation_done:{}:{}", chat_id, date)
}

async fn process_chat(deps: &ConsolidationDeps, chat_id: &str, today: &str) -> ChatConsolidationOutcome {
    let mut outcome = ChatConsolidationOutcome {
        chat_id: chat_id.to_string(),
        msg_start: 0,
        msg_end: 0,
        mention_pairs: 0,
        facts_written: 0,
        status: "ok".into(),
    };

    let mut conn = match crate::db::connect(&deps.db_path) {
        Ok(c) => c,
        Err(e) => {
            warn!(chat_id, error = %e, "归纳失败：打开数据库");
            outcome.status = "db_error".into();
            return outcome;
        }
    };
    // 幂等闸：同 chat 同日已完成 → 整 chat 跳过
    let stamped: Option<String> = conn
        .query_row(
            "SELECT value FROM state_kv WHERE key = ?1",
            params![stamp_key(chat_id, today)],
            |r| r.get(0),
        )
        .ok();
    if stamped.is_some() {
        outcome.status = "skipped_done".into();
        return outcome;
    }

    // 窗口：最新 500 条（含 self 发言）
    let rows: Vec<MsgRow> = conn
        .prepare(
            "SELECT msg_id, sender_pid, COALESCE(nickname,''), COALESCE(text,''), COALESCE(mentions,'[]'), ts
             FROM (SELECT msg_id, sender_pid, nickname, text, mentions, ts FROM messages
                   WHERE chat_id = ?1 ORDER BY msg_id DESC LIMIT ?2)
             ORDER BY msg_id ASC",
        )
        .unwrap()
        .query_map(params![chat_id, WINDOW], |r| {
            Ok(MsgRow {
                msg_id: r.get(0)?,
                sender_pid: r.get(1)?,
                nickname: r.get(2)?,
                text: r.get(3)?,
                mentions: r.get(4)?,
                ts: r.get(5)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    if rows.is_empty() {
        outcome.status = "no_messages".into();
        return outcome;
    }
    // 当日零新消息的 chat 跳过
    let midnight: i64 = conn
        .query_row(
            "SELECT CAST(strftime('%s','now','localtime','start of day','utc') AS INTEGER)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if !rows.iter().any(|r| r.ts >= midnight) {
        outcome.status = "no_new_today".into();
        return outcome;
    }
    outcome.msg_start = rows.first().unwrap().msg_id;
    outcome.msg_end = rows.last().unwrap().msg_id;

    // 先行占位：同 chat 同日再跑直接 skipped_done（防 mention 双重计数）
    let _ = conn.execute(
        "INSERT INTO state_kv(key, value, updated_at) VALUES (?1, '1', ?2)
         ON CONFLICT(key) DO UPDATE SET value = '1', updated_at = excluded.updated_at",
        params![stamp_key(chat_id, today), now_secs()],
    );

    // —— 通道一：@关系统计（纯 SQL，不经 LLM）——
    let mention_pairs = apply_mention_stats(&mut conn, deps, chat_id, &rows, &outcome);
    outcome.mention_pairs = mention_pairs;

    // —— LLM 提炼（decision 角色；未配置或两轮校验失败 → 本 chat 本轮跳过提炼）——
    let decision_ready = deps
        .llm
        .as_ref()
        .map(|g| g.role(Role::Decision).is_some())
        .unwrap_or(false);
    if !decision_ready {
        warn!(chat_id, "Decision 角色未配置，本 chat 跳过 LLM 提炼（@统计已落库）");
        outcome.status = "llm_unavailable".into();
        return outcome;
    }
    let distill = match distill(deps.llm.as_ref().unwrap(), chat_id, &rows).await {
        Some(d) => d,
        None => {
            warn!(chat_id, "LLM 提炼两轮校验失败，本 chat 本轮跳过");
            outcome.status = "llm_failed".into();
            return outcome;
        }
    };

    // —— 通道二三四：summaries / long_memories / rel_events（单事务）——
    match write_distilled(&mut conn, chat_id, today, &distill, &outcome) {
        Ok(facts) => outcome.facts_written = facts,
        Err(e) => {
            warn!(chat_id, error = %e, "提炼结果落库失败");
            outcome.status = "db_error".into();
        }
    }
    outcome
}

struct MsgRow {
    msg_id: i64,
    sender_pid: String,
    nickname: String,
    text: String,
    mentions: String,
    ts: i64,
}

/// @统计：同对按窗口聚合计数，单边单次 ≤0.05；@bot → 记 self→发送者的边
fn apply_mention_stats(
    conn: &mut rusqlite::Connection,
    deps: &ConsolidationDeps,
    chat_id: &str,
    rows: &[MsgRow],
    outcome: &ChatConsolidationOutcome,
) -> usize {
    let self_pid = {
        let q = deps.self_qq.load(Ordering::Relaxed);
        (q != 0).then(|| format!("p_{q}"))
    };
    let mut pairs: HashMap<(String, String), i64> = HashMap::new();
    for r in rows {
        if r.sender_pid == "self" {
            continue; // 云团@人不产生关系增量（防自嗨）
        }
        let mentions: Vec<String> = serde_json::from_str(&r.mentions).unwrap_or_default();
        for m in mentions {
            if m == r.sender_pid {
                continue;
            }
            // @bot → self→user；普通@ → sender→被@
            let (from, to) = if Some(m.as_str()) == self_pid.as_deref() {
                ("self".to_string(), r.sender_pid.clone())
            } else {
                (r.sender_pid.clone(), m)
            };
            *pairs.entry((from, to)).or_insert(0) += 1;
        }
    }
    let now = now_secs();
    let evidence = format!(
        "chat {} msg {}-{} 窗口@统计",
        chat_id, outcome.msg_start, outcome.msg_end
    );
    let n = pairs.len();
    let tx = match conn.transaction() {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "@统计事务开启失败");
            return 0;
        }
    };
    for ((from, to), count) in &pairs {
        let delta = (MENTION_DELTA_PER * *count as f64).min(MENTION_DELTA_CAP);
        if let Err(e) = tx.execute(
            "INSERT INTO relationship_events(from_pid, to_pid, kind, delta_familiar, delta_trust, evidence, created_at)
             VALUES (?1, ?2, 'mention', ?3, 0, ?4, ?5)",
            params![from, to, delta, evidence, now],
        ) {
            warn!(error = %e, "relationship_events 写入失败");
            continue;
        }
        bump_edge(&tx, from, to, delta, 0.0, now);
    }
    if let Err(e) = tx.commit() {
        warn!(error = %e, "@统计事务提交失败");
        return 0;
    }
    n
}

/// edges 增量累加，clamp(0..1)
fn bump_edge(conn: &rusqlite::Connection, from: &str, to: &str, d_familiar: f64, d_trust: f64, now: i64) {
    let cur: Option<(f64, f64)> = conn
        .query_row(
            "SELECT trust, familiar FROM relationship_edges WHERE from_pid = ?1 AND to_pid = ?2",
            params![from, to],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let (trust, familiar) = cur.unwrap_or((0.5, 0.0));
    let nt = (trust + d_trust).clamp(0.0, 1.0);
    let nf = (familiar + d_familiar).clamp(0.0, 1.0);
    let _ = conn.execute(
        "INSERT INTO relationship_edges(from_pid, to_pid, trust, familiar, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(from_pid, to_pid) DO UPDATE SET trust = excluded.trust, familiar = excluded.familiar, updated_at = excluded.updated_at",
        params![from, to, nt, nf, now],
    );
}

// ---------- LLM 提炼 ----------

#[derive(Debug, Deserialize)]
struct DistillOut {
    chat_summary: String,
    #[serde(default)]
    person_facts: Vec<PersonFact>,
    #[serde(default)]
    group_facts: Vec<String>,
    #[serde(default)]
    rel_events: Vec<RelEvent>,
}

#[derive(Debug, Deserialize)]
struct PersonFact {
    person_id: String,
    fact: String,
}

#[derive(Debug, Deserialize)]
struct RelEvent {
    from: String,
    to: String,
    kind: String,
    delta_trust: f64,
    #[serde(default)]
    evidence: String,
}

const DISTILL_SYSTEM: &str = r#"你是云团夜间归纳的提炼器。输入是一个会话窗口的消息流水（每行：昵称(person_id): 内容），输出严格 JSON，不要任何其他文字。

Schema：
{
  "chat_summary": "本日群摘要一段话（时间线索引用，100 字内）",
  "person_facts": [{"person_id": "p_xxx", "fact": "关于此人的一条提炼事实（陈述句）"}],
  "group_facts": ["关于本群长期值得记住的事实（陈述句；即群话题摘要）"],
  "rel_events": [{"from": "p_a", "to": "p_b", "kind": "help|quarrel|praise|...", "delta_trust": 0.05, "evidence": "一句话依据"}]
}

规则：
- 只提炼长期有意义的事实（偏好、关系、正在进行的事），日常寒暄不进 facts
- rel_events 的 delta_trust 取值 -0.2 ~ 0.2；没有可信依据就留空数组
- 严禁在输出中包含任何敏感信息（密码、密钥、证件号、手机号等）
"#;

async fn distill(gw: &LlmGateway, chat_id: &str, rows: &[MsgRow]) -> Option<DistillOut> {
    let mut user = String::from("消息流水（最新 500 条窗口，按时间序）：\n");
    for r in rows {
        let text: String = r.text.chars().take(80).collect();
        user.push_str(&format!("{}({}): {}\n", r.nickname, r.sender_pid, text));
    }
    let first = gw
        .chat(Role::Decision, DISTILL_SYSTEM, &user, true)
        .await
        .ok()
        .and_then(|c| parse_distill(&c).ok());
    if first.is_some() {
        return first;
    }
    debug!(chat_id, "归纳提炼首轮校验失败，重试一次");
    let retry = format!("{user}\n\n上一次输出未通过 Schema 校验，请严格按 Schema 重新输出，只输出 JSON。");
    gw.chat(Role::Decision, DISTILL_SYSTEM, &retry, true)
        .await
        .ok()
        .and_then(|c| parse_distill(&c).ok())
}

fn parse_distill(content: &str) -> std::result::Result<DistillOut, String> {
    let s = content.trim();
    let s = if let Some(rest) = s.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        rest.strip_suffix("```").map(|b| b.trim()).unwrap_or(s)
    } else {
        s
    };
    serde_json::from_str::<DistillOut>(s).map_err(|e| e.to_string())
}

/// 提炼结果落库（单事务；敏感条目逐条拒收记日志）
fn write_distilled(
    conn: &mut rusqlite::Connection,
    chat_id: &str,
    today: &str,
    d: &DistillOut,
    outcome: &ChatConsolidationOutcome,
) -> Result<usize> {
    let now = now_secs();
    let tx = conn.transaction().context("归纳写入事务开启失败")?;
    let mut facts_written = 0usize;

    // chat 每日摘要（UPSERT 防重）
    if !is_sensitive(&d.chat_summary) {
        tx.execute(
            "INSERT INTO summaries(owner_type, owner_id, period, date, summary, msg_id_start, msg_id_end, created_at)
             VALUES ('chat', ?1, 'daily', ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(owner_type, owner_id, period, date)
             DO UPDATE SET summary = excluded.summary, msg_id_start = excluded.msg_id_start, msg_id_end = excluded.msg_id_end",
            params![chat_id, today, d.chat_summary, outcome.msg_start, outcome.msg_end, now],
        )?;
    } else {
        warn!(chat_id, "chat_summary 命中敏感拒收");
    }

    // 成员当日出席清点（person 摘要有事实也写、无事实写字数兜底，由 person_facts 聚合一句话）
    let speakers: Vec<(String, String, i64)> = tx
        .prepare(
            "SELECT sender_pid, COALESCE(MAX(nickname),''), COUNT(*) FROM messages
             WHERE chat_id = ?1 AND msg_id BETWEEN ?2 AND ?3 AND sender_pid != 'self'
             GROUP BY sender_pid",
        )?
        .query_map(params![chat_id, outcome.msg_start, outcome.msg_end], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<std::result::Result<Vec<(String, String, i64)>, rusqlite::Error>>()?;
    for (pid, _nick, msg_count) in &speakers {
        let facts: Vec<&str> = d
            .person_facts
            .iter()
            .filter(|f| &f.person_id == pid)
            .map(|f| f.fact.as_str())
            .collect();
        let fallback = format!("今日发言 {msg_count} 条。");
        let summary = if facts.is_empty() {
            fallback
        } else {
            let joined = facts.join("；");
            if is_sensitive(&joined) {
                // 敏感拒收滤的是内容不是该人的当日索引——回退到计数保底
                warn!(chat_id, person = pid, "person 聚合摘要命中敏感，回退发言计数");
                fallback
            } else {
                joined
            }
        };
        tx.execute(
            "INSERT INTO summaries(owner_type, owner_id, period, date, summary, msg_id_start, msg_id_end, created_at)
             VALUES ('person', ?1, 'daily', ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(owner_type, owner_id, period, date)
             DO UPDATE SET summary = excluded.summary, msg_id_start = excluded.msg_id_start, msg_id_end = excluded.msg_id_end",
            params![pid, today, summary, outcome.msg_start, outcome.msg_end, now],
        )?;
    }

    // person_facts → long_memories(owner=person, source=consolidation)
    for f in &d.person_facts {
        let fact = f.fact.trim();
        if fact.is_empty() {
            continue;
        }
        if is_sensitive(fact) {
            warn!(chat_id, person = %f.person_id, "person_fact 命中敏感拒收");
            continue;
        }
        tx.execute(
            "INSERT INTO long_memories(owner_type, owner_id, content, source, created_at, updated_at)
             VALUES ('person', ?1, ?2, 'consolidation', ?3, ?3)",
            params![f.person_id, fact, now],
        )?;
        facts_written += 1;
    }
    // group_facts → long_memories(owner=chat)
    for fact in &d.group_facts {
        let fact = fact.trim();
        if fact.is_empty() {
            continue;
        }
        if is_sensitive(fact) {
            warn!(chat_id, "group_fact 命中敏感拒收");
            continue;
        }
        tx.execute(
            "INSERT INTO long_memories(owner_type, owner_id, content, source, created_at, updated_at)
             VALUES ('chat', ?1, ?2, 'consolidation', ?3, ?3)",
            params![chat_id, fact, now],
        )?;
        facts_written += 1;
    }

    // rel_events → relationship_events + edges(trust)
    for ev in &d.rel_events {
        if !ev.delta_trust.is_finite() {
            continue;
        }
        let delta = ev.delta_trust.clamp(-0.2, 0.2);
        if is_sensitive(&ev.evidence) {
            warn!(chat_id, "rel_event evidence 命中敏感拒收");
            continue;
        }
        let evidence = if ev.evidence.is_empty() {
            format!("夜间归纳提炼 msg {}-{}", outcome.msg_start, outcome.msg_end)
        } else {
            ev.evidence.clone()
        };
        tx.execute(
            "INSERT INTO relationship_events(from_pid, to_pid, kind, delta_familiar, delta_trust, evidence, created_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6)",
            params![ev.from, ev.to, ev.kind, delta, evidence, now],
        )?;
        bump_edge(&tx, &ev.from, &ev.to, 0.0, delta, now);
    }

    tx.commit().context("归纳写入事务提交失败")?;
    Ok(facts_written)
}

// ---------- 工具 ----------

fn list_chats(db_path: &Path) -> Vec<String> {
    crate::db::connect(db_path)
        .ok()
        .and_then(|c| {
            c.prepare("SELECT DISTINCT chat_id FROM messages")
                .and_then(|mut st| {
                    let rows = st.query_map([], |r| r.get(0))?;
                    rows.collect::<std::result::Result<Vec<String>, _>>()
                })
                .ok()
        })
        .unwrap_or_default()
}

/// 今日日期串（本地时区，经 SQL 换算）
fn today_str(db_path: &Path) -> String {
    crate::db::connect(db_path)
        .ok()
        .and_then(|c| {
            c.query_row("SELECT strftime('%Y-%m-%d','now','localtime')", [], |r| r.get::<_, String>(0))
                .ok()
        })
        .unwrap_or_else(|| "1970-01-01".into())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 敏感拒收（无 regex 依赖的手工扫描）：
/// 关键词子串（密码/口令/密钥/api_key/api-key/apikey/token/身份证/手机号/验证码/私钥/secret）
/// + 11 位 1 开头连续数字（手机号）+ 18 位连续数字 或 17 位数字+X（身份证）
pub fn is_sensitive(s: &str) -> bool {
    const KEYWORDS: [&str; 12] = [
        "密码", "口令", "密钥", "api_key", "api-key", "apikey", "token", "身份证", "手机号", "验证码",
        "私钥", "secret",
    ];
    let lower = s.to_lowercase();
    if KEYWORDS.iter().any(|k| lower.contains(k)) {
        return true;
    }
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < n && chars[i].is_ascii_digit() {
                i += 1;
            }
            let run = i - start;
            if (run == 11 && chars[start] == '1') || run == 18 {
                return true;
            }
            if run == 17 && i < n && (chars[i] == 'X' || chars[i] == 'x') {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_keywords_hit() {
        assert!(is_sensitive("我的密码是123"));
        assert!(is_sensitive("口令：abc"));
        assert!(is_sensitive("私钥如下"));
        assert!(is_sensitive("api_key=sk-xxx"));
        assert!(is_sensitive("ApiKey: 1"));
        assert!(is_sensitive("带上你的 token"));
        assert!(is_sensitive("SECRET=value"));
    }

    #[test]
    fn sensitive_phone_and_id_hit() {
        assert!(is_sensitive("联系我 13800001111"));
        assert!(is_sensitive("手机号是13800001111"));
        assert!(is_sensitive("证件 110101199003077777"));
        assert!(is_sensitive("ID 11010119900307777X"));
    }

    #[test]
    fn normal_text_passes() {
        assert!(!is_sensitive("他喜欢 Rust 和钓鱼"));
        assert!(!is_sensitive("今天 2026 年聚会"));
        assert!(!is_sensitive("群号 555666"));
        assert!(!is_sensitive("代号是 9527"));
    }
}
