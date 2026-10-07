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

/// 迁移列表按版本升序；每步一个事务，成功后推进 user_version
const MIGRATIONS: [(&str, &str); 3] = [
    ("V0.1 基线：14 张表（data-model.md）", V1_SQL),
    ("V0.2 媒体生成：media_providers/models/tasks/credits 4 张表", V2_SQL),
    ("V0.3 LLM 用量：llm_usage 表（仪表盘 token 统计）", V3_SQL),
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
