//! SQLite 共享入口：连接（WAL + 外键 + busy_timeout）与迁移（user_version，幂等）。
//! 表结构与 docs/data-model.md 一致，共 14 张表。

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// 打开数据库连接并统一 PRAGMA。
/// WAL 是读路径与多连接写入（adapter / tracer / webui 各自持连接）的基础；
/// busy_timeout 兜底单写者队列落地前的偶发 BUSY。
pub fn connect(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("打开数据库失败: {}", path.display()))?;
    conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0))
        .context("设置 WAL 模式失败")?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .context("启用外键约束失败")?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .context("设置 busy_timeout 失败")?;
    Ok(conn)
}

/// V1 基线：data-model.md 全部 14 张表 + 索引
const V1_SQL: &str = r#"
-- 二、身份与人
CREATE TABLE IF NOT EXISTS persons (
  person_id   TEXT PRIMARY KEY,
  display_name TEXT,
  first_seen  INTEGER NOT NULL,
  last_seen   INTEGER NOT NULL,
  note        TEXT
);

CREATE TABLE IF NOT EXISTS identities (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  person_id     TEXT NOT NULL REFERENCES persons(person_id),
  platform      TEXT NOT NULL,
  platform_uid  TEXT NOT NULL,
  UNIQUE(platform, platform_uid)
);

CREATE TABLE IF NOT EXISTS member_profiles (
  chat_id    TEXT NOT NULL,
  person_id  TEXT NOT NULL REFERENCES persons(person_id),
  card       TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(chat_id, person_id)
);

-- 三、消息流水
CREATE TABLE IF NOT EXISTS messages (
  msg_id      INTEGER PRIMARY KEY AUTOINCREMENT,
  chat_id     TEXT NOT NULL,
  chat_type   TEXT NOT NULL,
  sender_pid  TEXT NOT NULL REFERENCES persons(person_id),
  nickname    TEXT,
  text        TEXT,
  mentions    TEXT,
  reply_to    INTEGER,
  at_me       INTEGER DEFAULT 0,
  has_image   INTEGER DEFAULT 0,
  ts          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_msg_chat_ts   ON messages(chat_id, ts);
CREATE INDEX IF NOT EXISTS idx_msg_sender_ts ON messages(sender_pid, ts);

-- 四、长期记忆与摘要
CREATE TABLE IF NOT EXISTS long_memories (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  owner_type TEXT NOT NULL,
  owner_id   TEXT NOT NULL,
  content    TEXT NOT NULL,
  source     TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_lm_owner ON long_memories(owner_type, owner_id);

CREATE TABLE IF NOT EXISTS summaries (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  owner_type   TEXT NOT NULL,
  owner_id     TEXT NOT NULL,
  period       TEXT NOT NULL,
  date         TEXT NOT NULL,
  summary      TEXT NOT NULL,
  msg_id_start INTEGER,
  msg_id_end   INTEGER,
  created_at   INTEGER NOT NULL,
  UNIQUE(owner_type, owner_id, period, date)
);

-- 五、关系系统
CREATE TABLE IF NOT EXISTS relationship_edges (
  from_pid   TEXT NOT NULL,
  to_pid     TEXT NOT NULL,
  trust      REAL DEFAULT 0.5,
  familiar   REAL DEFAULT 0.0,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(from_pid, to_pid)
);

CREATE TABLE IF NOT EXISTS relationship_events (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  from_pid      TEXT NOT NULL,
  to_pid        TEXT NOT NULL,
  kind          TEXT NOT NULL,
  delta_familiar REAL DEFAULT 0,
  delta_trust    REAL DEFAULT 0,
  evidence      TEXT,
  created_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_re_pair ON relationship_events(from_pid, to_pid);

-- 六、人格版本
CREATE TABLE IF NOT EXISTS personality_versions (
  version_no INTEGER PRIMARY KEY,
  content    TEXT NOT NULL,
  note       TEXT,
  created_by TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  active     INTEGER DEFAULT 0
);

-- 七、Meme 库
CREATE TABLE IF NOT EXISTS meme_library (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  file         TEXT NOT NULL,
  category     TEXT NOT NULL,
  md5          TEXT NOT NULL,
  phash        TEXT,
  embedding    BLOB,
  added_by     TEXT NOT NULL,
  status       TEXT DEFAULT 'active',
  use_count    INTEGER DEFAULT 0,
  last_used_ts INTEGER,
  created_at   INTEGER NOT NULL
);

-- 八、Task 系统
CREATE TABLE IF NOT EXISTS tasks (
  task_id        TEXT PRIMARY KEY,
  goal           TEXT NOT NULL,
  state          TEXT NOT NULL,
  budget_max_calls INTEGER NOT NULL,
  used_calls     INTEGER DEFAULT 0,
  created_by_pid TEXT NOT NULL,
  chat_id        TEXT NOT NULL,
  created_at     INTEGER NOT NULL,
  finished_at    INTEGER
);

CREATE TABLE IF NOT EXISTS task_events (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id TEXT NOT NULL REFERENCES tasks(task_id),
  seq     INTEGER NOT NULL,
  kind    TEXT NOT NULL,
  payload TEXT,
  ts      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_te_task ON task_events(task_id, seq);

-- 九、State 持久化
CREATE TABLE IF NOT EXISTS state_kv (
  key        TEXT PRIMARY KEY,
  value      TEXT,
  updated_at INTEGER NOT NULL
);

-- 十二、事件表
CREATE TABLE IF NOT EXISTS events (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  kind    TEXT NOT NULL,
  payload TEXT,
  ts      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);
"#;

/// 迁移列表按版本升序；每步一个事务，成功后推进 user_version
const MIGRATIONS: [(&str, &str); 1] = [("V0.1 基线：14 张表（data-model.md）", V1_SQL)];

pub fn migrate(conn: &mut Connection) -> Result<()> {
    let current: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("读取 user_version 失败")?;
    if current as usize >= MIGRATIONS.len() {
        tracing::info!(user_version = current, "迁移已是最新，幂等跳过");
        return Ok(());
    }
    for (i, (desc, sql)) in MIGRATIONS.iter().enumerate() {
        let version = (i + 1) as u32;
        if current >= version {
            continue;
        }
        let tx = conn.transaction().context("开启迁移事务失败")?;
        tx.execute_batch(sql)
            .with_context(|| format!("执行迁移 v{version} 失败"))?;
        tx.pragma_update(None, "user_version", version)
            .context("推进 user_version 失败")?;
        tx.commit().with_context(|| format!("提交迁移 v{version} 失败"))?;
        tracing::info!(user_version = version, desc, "迁移已应用");
    }
    Ok(())
}

/// 列出用户表（排除 sqlite 内部表），供启动自检
pub fn list_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let names = stmt
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<Vec<String>, _>>()?;
    Ok(names)
}
