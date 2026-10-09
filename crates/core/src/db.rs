//! SQLite 共享入口：连接（WAL + 外键 + busy_timeout）与迁移（user_version，幂等）。
//! V0.1 表设计基线见 docs/reference/data-model.md；实际表结构以本文件的后续迁移为准。

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// 打开数据库连接并统一 PRAGMA。
/// WAL 是读路径与多连接写入（adapter / tracer / webui 各自持连接）的基础；
/// busy_timeout 兜底单写者队列落地前的偶发 BUSY。
pub fn connect(path: &Path) -> Result<Connection> {
    let conn =
        Connection::open(path).with_context(|| format!("打开数据库失败: {}", path.display()))?;
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

/// V2：媒体生成（Q007/Q009/Q013/Q014/Q016/Q017）—— 4 张表 + chats 增列
const V2_SQL: &str = r#"
-- 媒体 provider 表（NAI / OpenAI-Image / Gemini ...）
CREATE TABLE IF NOT EXISTS media_providers (
  name             TEXT PRIMARY KEY,
  base_url         TEXT NOT NULL,
  api_key_env      TEXT NOT NULL DEFAULT '',
  default_endpoint TEXT NOT NULL DEFAULT 'nai_native',  -- nai_native | openai_compat | gemini | xai
  enabled          INTEGER NOT NULL DEFAULT 1,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL
);

-- 媒体模型表（一个 provider 下多个模型；命令 -m 入参目标）
CREATE TABLE IF NOT EXISTS media_models (
  alias            TEXT PRIMARY KEY,             -- 用户输入 ID（如 "nai4.5"）
  provider         TEXT NOT NULL REFERENCES media_providers(name),
  model_id         TEXT NOT NULL,                -- 实际请求 model id（如 "nai-diffusion-4-5-full"）
  endpoint_style   TEXT,                          -- 覆盖 provider.default_endpoint；NULL 跟 provider 默认
  prompt_style     TEXT NOT NULL DEFAULT 'natural', -- nai | anima | a1111 | natural（Q010）
  daily_quota      INTEGER NOT NULL DEFAULT 0,    -- 0 = 无上限；>0 = 每日配额
  cost_per_result  INTEGER NOT NULL DEFAULT 0,    -- 单价（0 = 免费；>0 = 计费）
  permission       TEXT NOT NULL DEFAULT 'everyone', -- admin_only | everyone（Q009）
  ratios           TEXT NOT NULL DEFAULT '',      -- 逗号分隔支持的比例（"1:1,2:3,3:2"）；空 = 用 provider 默认
  default_quality  TEXT,                          -- 默认质量档位（Seedream/Seedance 用）
  enabled          INTEGER NOT NULL DEFAULT 1,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_mm_provider ON media_models(provider);

-- 媒体任务表（审计 + /tasks 命令 + 任务可视化页数据源）
CREATE TABLE IF NOT EXISTS media_tasks (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  chat_id        TEXT NOT NULL,
  chat_type      TEXT NOT NULL,
  sender_pid     TEXT NOT NULL REFERENCES persons(person_id),
  model_alias    TEXT NOT NULL,
  provider       TEXT NOT NULL,
  endpoint_style TEXT NOT NULL,
  raw_prompt     TEXT NOT NULL,
  prompt         TEXT NOT NULL,            -- 优化后的最终 prompt
  ratio          TEXT NOT NULL,
  count          INTEGER NOT NULL,
  seed           INTEGER,
  cost           INTEGER NOT NULL DEFAULT 0,
  state          TEXT NOT NULL,             -- queued | running | success | failed | canceled
  error          TEXT,
  artifacts      TEXT,                      -- JSON 数组：落盘绝对路径
  created_at     INTEGER NOT NULL,
  finished_at    INTEGER
);
CREATE INDEX IF NOT EXISTS idx_mt_chat_ts   ON media_tasks(chat_id, created_at);
CREATE INDEX IF NOT EXISTS idx_mt_sender_ts ON media_tasks(sender_pid, created_at);
CREATE INDEX IF NOT EXISTS idx_mt_state     ON media_tasks(state);

-- 用户余额表（Q009）；余额为 0 则计费模型不可用
CREATE TABLE IF NOT EXISTS media_credits (
  person_id  TEXT PRIMARY KEY REFERENCES persons(person_id),
  balance    INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL
);

-- Q014：chat 上次成功任务的 seed（缺省 --seed 时复用，可复现）
-- chats 表原本不存在；现以 state_kv 为通道："last_media_seed:{chat_id}" → int
-- 选 state_kv 而非新表，因为键值型写多读少，且不需 schema 变更
"#;

/// V3：LLM token 用量统计（仪表盘"今日 token"卡数据源）
const V3_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS llm_usage (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,                 -- unix 秒
    role TEXT NOT NULL,                  -- decision / bot_chat / agent_exec / optimizer / ...
    model TEXT NOT NULL,
    prompt_tokens INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_llm_usage_ts ON llm_usage(ts);
CREATE INDEX IF NOT EXISTS idx_llm_usage_role_ts ON llm_usage(role, ts);
"#;

/// V4：Q55 消息流水持久化——pipeline 消费标记；重启扫 NULL 行回放进管线。
/// ALTER 和列存在性检查必须在同一个 IMMEDIATE 迁移事务内。
const V4_SQL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_msg_pending ON messages(processed_at) WHERE processed_at IS NULL;
"#;

/// 迁移列表按版本升序；每步一个事务，成功后推进 user_version
const MIGRATIONS: [(&str, &str); 6] = [
    ("V0.1 基线：14 张表（data-model.md）", V1_SQL),
    (
        "V0.2 媒体生成：media_providers/models/tasks/credits 4 张表",
        V2_SQL,
    ),
    ("V0.3 LLM 用量：llm_usage 表（仪表盘 token 统计）", V3_SQL),
    (
        "V0.4 消息消费标记（Q55 恢复消费）：messages.processed_at",
        V4_SQL,
    ),
    (
        "V0.5 任务调度与列表索引",
        r#"
        CREATE INDEX IF NOT EXISTS idx_tasks_state_created ON tasks(state, created_at, task_id);
        CREATE INDEX IF NOT EXISTS idx_tasks_created ON tasks(created_at);
    "#,
    ),
    (
        "V0.6 回复来源、人物简档与记忆证据",
        r#"
        CREATE INDEX IF NOT EXISTS idx_msg_external ON messages(chat_id, chat_type, external_msg_id);
        CREATE INDEX IF NOT EXISTS idx_msg_chat_id ON messages(chat_id, msg_id);
        CREATE TABLE IF NOT EXISTS person_profile_facts (
            person_id TEXT NOT NULL REFERENCES persons(person_id),
            field TEXT NOT NULL,
            content TEXT NOT NULL,
            source_msg_id INTEGER NOT NULL,
            evidence_quote TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(person_id, field)
        );
    "#,
    ),
];

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
        // 先持有写锁再检查版本和表结构，避免多个连接同时检查缺列后重复 ALTER，
        // 也避免使用启动时的旧版本覆盖另一个连接刚提交的 user_version。
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("开启迁移事务失败")?;
        let locked_version: u32 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if locked_version >= version {
            continue;
        }
        if version == 4 {
            add_column_if_missing(&tx, "messages", "processed_at", "INTEGER")?;
        }
        if version == 6 {
            add_column_if_missing(&tx, "messages", "external_msg_id", "INTEGER")?;
            add_column_if_missing(&tx, "long_memories", "source_chat_id", "TEXT")?;
            add_column_if_missing(&tx, "long_memories", "source_msg_id", "INTEGER")?;
            add_column_if_missing(&tx, "long_memories", "source_end_msg_id", "INTEGER")?;
        }
        tx.execute_batch(sql)
            .with_context(|| format!("执行迁移 v{version} 失败"))?;
        tx.pragma_update(None, "user_version", version)
            .context("推进 user_version 失败")?;
        tx.commit()
            .with_context(|| format!("提交迁移 v{version} 失败"))?;
        tracing::info!(user_version = version, desc, "迁移已应用");
    }
    Ok(())
}

/// 在调用方持有迁移写事务的前提下幂等加列。
fn add_column_if_missing(conn: &Connection, table: &str, column: &str, ty: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if cols.iter().any(|c| c == column) {
        return Ok(());
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"))
        .with_context(|| format!("ALTER {table} ADD {column} 失败"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn v3_database(conn: &Connection) {
        conn.execute_batch(V1_SQL).unwrap();
        conn.execute_batch(V2_SQL).unwrap();
        conn.execute_batch(V3_SQL).unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
        conn.execute_batch("INSERT INTO persons VALUES ('p_test', 'test', 1, 1, NULL);
            INSERT INTO messages(chat_id,chat_type,sender_pid,text,ts) VALUES ('g_test','group','p_test','旧消息',1);
            INSERT INTO tasks(task_id,goal,state,budget_max_calls,created_by_pid,chat_id,created_at)
            VALUES ('t_test','旧任务','running',10,'p_test','g_test',1);").unwrap();
    }

    #[test]
    fn migration_preserves_v3_rows_and_indexes_scheduler_and_lists() {
        let mut conn = Connection::open_in_memory().unwrap();
        v3_database(&conn);
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let row: (String, Option<i64>) = conn
            .query_row("SELECT text, processed_at FROM messages", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(row, ("旧消息".into(), None));
        assert_eq!(
            conn.query_row("SELECT goal FROM tasks", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "旧任务"
        );
        for (query, index) in [
            ("SELECT task_id, chat_id, goal, used_calls FROM tasks WHERE state='running' ORDER BY created_at,task_id LIMIT 6", "idx_tasks_state_created"),
            ("SELECT * FROM tasks ORDER BY created_at DESC LIMIT 50", "idx_tasks_created"),
        ] {
            let plan = conn.prepare(&format!("EXPLAIN QUERY PLAN {query}")).unwrap()
                .query_map([], |r| r.get::<_, String>(3)).unwrap()
                .collect::<rusqlite::Result<Vec<_>>>().unwrap().join("\n");
            assert!(plan.contains(index), "{plan}");
            assert!(!plan.contains("TEMP B-TREE"), "{plan}");
        }
    }

    #[test]
    fn concurrent_migrations_serialize_schema_changes() {
        let path = std::env::temp_dir().join(format!(
            "yuantuan-migrate-{}-{}.db",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut conn = connect(&path).unwrap();
        v3_database(&conn);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut conn = connect(&path).unwrap();
                    barrier.wait();
                    migrate(&mut conn).unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        migrate(&mut conn).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, usize>(0))
                .unwrap(),
            MIGRATIONS.len()
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(conn);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn v6_preserves_legacy_memories_without_fabricating_sources_and_rolls_back_failure() {
        let mut conn = Connection::open_in_memory().unwrap();
        v3_database(&conn);
        add_column_if_missing(&conn, "messages", "processed_at", "INTEGER").unwrap();
        conn.execute_batch(V4_SQL).unwrap();
        conn.execute_batch(MIGRATIONS[4].1).unwrap();
        conn.pragma_update(None, "user_version", 5).unwrap();
        conn.execute_batch("INSERT INTO long_memories(owner_type,owner_id,content,source,created_at,updated_at) VALUES ('person','p_test','旧记忆','explicit',1,1); CREATE TABLE idx_msg_external(dummy INTEGER);").unwrap();
        assert!(migrate(&mut conn).is_err());
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert!(conn
            .prepare("SELECT external_msg_id FROM messages")
            .is_err());
        conn.execute_batch("DROP TABLE idx_msg_external").unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let row: (String, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT content,source_msg_id,source_chat_id FROM long_memories",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, ("旧记忆".into(), None, None));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM person_profile_facts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn failed_v4_migration_rolls_back_added_column_and_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        v3_database(&conn);
        // Index/table name collision fails after V4's ALTER TABLE.
        conn.execute_batch("CREATE TABLE idx_msg_pending(dummy INTEGER)")
            .unwrap();
        assert!(migrate(&mut conn).is_err());
        let columns = conn
            .prepare("PRAGMA table_info(messages)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(!columns.iter().any(|name| name == "processed_at"));
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        conn.execute_batch("DROP TABLE idx_msg_pending").unwrap();
        migrate(&mut conn).unwrap();
    }
}
