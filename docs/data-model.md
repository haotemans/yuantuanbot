# 云团数据模型 V0.1

> 对应 architecture-v0.1.md 的落地表结构。变更走治理协议：小改直接改 + 修订记录，大改先 ADR。

---

# 一、存储布局

所有运行数据集中在 `data/` 目录（docker 单卷挂载、备份、迁移都只认它）：

```text
data/
  yuantuan.db        # SQLite（WAL）
  memes/             # 表情包文件实体（元数据在 meme_library 表）
  artifacts/         # 任务产出文件
  archive/           # 任务归档 / 聊天归档
  logs/              # 运行日志与 trace（轮转，设上限）
  backups/           # 备份暂存（推送 GitHub 私有仓前）
```

---

# 二、身份与人

```sql
-- 全局唯一的人
CREATE TABLE persons (
  person_id   TEXT PRIMARY KEY,      -- 内部稳定ID
  display_name TEXT,                 -- 当前称呼（可变，仅展示）
  first_seen  INTEGER NOT NULL,
  last_seen   INTEGER NOT NULL,
  note        TEXT
);

-- 平台账号 ↔ person
CREATE TABLE identities (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  person_id     TEXT NOT NULL REFERENCES persons(person_id),
  platform      TEXT NOT NULL,       -- qq | telegram | ...
  platform_uid  TEXT NOT NULL,       -- QQ号等
  UNIQUE(platform, platform_uid)
);

-- 群内名片（昵称漂移档案；messages 里另冗余发言时名片）
CREATE TABLE member_profiles (
  chat_id    TEXT NOT NULL,
  person_id  TEXT NOT NULL REFERENCES persons(person_id),
  card       TEXT,                   -- 当前群名片
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(chat_id, person_id)
);
```

QQ 号在同平台全局唯一：同一个人出现在多个群，天然收敛为同一 person_id，无需跨群匹配。

---

# 三、消息流水（全量落库）

```sql
CREATE TABLE messages (
  msg_id      INTEGER PRIMARY KEY AUTOINCREMENT,
  chat_id     TEXT NOT NULL,          -- 群号或私聊会话
  chat_type   TEXT NOT NULL,          -- group | private
  sender_pid  TEXT NOT NULL REFERENCES persons(person_id),
  nickname    TEXT,                   -- 发言时名片（可漂移，故冗余）
  text        TEXT,
  mentions    TEXT,                   -- JSON数组：被@的person_id列表（关系统计原料）
  reply_to    INTEGER,
  at_me       INTEGER DEFAULT 0,
  has_image   INTEGER DEFAULT 0,
  ts          INTEGER NOT NULL
);
CREATE INDEX idx_msg_chat_ts   ON messages(chat_id, ts);
CREATE INDEX idx_msg_sender_ts ON messages(sender_pid, ts);
```

外部依赖：500 条窗口、夜间归纳、@统计、摘要索引的全部数据源。

---

# 四、长期记忆与摘要

```sql
-- 长期记忆：人物/群/云团自身 三主体单表
CREATE TABLE long_memories (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  owner_type TEXT NOT NULL,           -- person | chat | self
  owner_id   TEXT NOT NULL,           -- person_id 或 chat_id
  content    TEXT NOT NULL,
  source     TEXT NOT NULL,           -- explicit（Decision实时）| consolidation（夜间归纳）
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_lm_owner ON long_memories(owner_type, owner_id);

-- 摘要：时间线索引。"找群历史/找某人的对话"的入口
CREATE TABLE summaries (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  owner_type   TEXT NOT NULL,         -- chat | person
  owner_id     TEXT NOT NULL,
  period       TEXT NOT NULL,         -- daily
  date         TEXT NOT NULL,         -- 2026-10-02
  summary      TEXT NOT NULL,
  msg_id_start INTEGER,               -- 覆盖的消息区间
  msg_id_end   INTEGER,
  created_at   INTEGER NOT NULL,
  UNIQUE(owner_type, owner_id, period, date)
);
```

规则：

- 每群每日一条摘要（聊天归档索引）；人物摘要按日滚动（"小明最近在聊什么"入口）
- 长期记忆存**提炼事实**，摘要存**时间线索引**，两者各司其职不混淆
- 敏感信息（密码/密钥/证件类）在显式与归纳两个写入处都拒收

---

# 五、关系系统

```sql
-- 单向边：只存当前值
CREATE TABLE relationship_edges (
  from_pid   TEXT NOT NULL,
  to_pid     TEXT NOT NULL,
  trust      REAL DEFAULT 0.5,
  familiar   REAL DEFAULT 0.0,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(from_pid, to_pid)
);

-- 事件流水：可回放、可审计
CREATE TABLE relationship_events (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  from_pid      TEXT NOT NULL,
  to_pid        TEXT NOT NULL,
  kind          TEXT NOT NULL,        -- mention | help | platform | ...
  delta_familiar REAL DEFAULT 0,
  delta_trust    REAL DEFAULT 0,
  evidence      TEXT,                 -- 消息区间 / 摘要引用
  created_at    INTEGER NOT NULL
);
CREATE INDEX idx_re_pair ON relationship_events(from_pid, to_pid);
```

规则：

- A→B 与 B→A 是两条独立边（A 信 B ≠ B 信 A）
- 云团 = 特殊 person_id `'self'`；"云团↔人"亲密度就是 from/to 含 'self' 的边，不做特殊结构
- V1 熟悉度主要由 **@互动统计**驱动（A@B→familiar 增量，按 mentions 字段）；信任分由夜间归纳提炼（"A 帮助 B 解决问题"类事件）
- 夜间归纳只追加 events、重算 edges 当前值——"亲密度怎么涨的"永远讲得清

---

# 六、人格版本

```sql
CREATE TABLE personality_versions (
  version_no INTEGER PRIMARY KEY,
  content    TEXT NOT NULL,           -- 人设提示词全文
  note       TEXT,                    -- 修改说明（"回滚自 v2"）
  created_by TEXT NOT NULL,           -- admin
  created_at INTEGER NOT NULL,
  active     INTEGER DEFAULT 0        -- 全局唯一 active=1
);
```

回滚语义：生成新版本号、内容等于旧版、note 写明来源——历史永远线性单向。

---

# 七、Meme 库

```sql
CREATE TABLE meme_library (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  file         TEXT NOT NULL,          -- data/memes/ 下相对路径
  category     TEXT NOT NULL,          -- 类别标签
  md5          TEXT NOT NULL,
  phash        TEXT,                   -- 感知哈希（感知去重）
  embedding    BLOB,                   -- DINOv3 向量，可选槽位（默认不用）
  added_by     TEXT NOT NULL,          -- admin | steal（偷表情包）
  status       TEXT DEFAULT 'active',  -- pending（待确认）| active
  use_count    INTEGER DEFAULT 0,
  last_used_ts INTEGER,
  created_at   INTEGER NOT NULL
);
```

管线：

- **去重**：md5 精确 + pHash 感知（本地免费底座）；DINOv3（魔搭 API）为可选增强，做导入时相似图聚类建议，不可用自动降级 pHash
- **分类**：导入时可由多模态 LLM 自动建议类别标签，管理员确认入库；模型不支持视觉则纯手动
- **偷表情包**：开关开启后，群聊图片进 `pending` 队列，去重跑同一管线，管理员一键收编
- **抽图**：类别内按"最久未用"加权随机（last_used_ts 升序加权），杜绝三连发

---

# 八、Task 系统

```sql
CREATE TABLE tasks (
  task_id        TEXT PRIMARY KEY,
  goal           TEXT NOT NULL,
  state          TEXT NOT NULL,        -- pending | running | finished | failed | archived
  budget_max_calls INTEGER NOT NULL,   -- 工具循环硬预算
  used_calls     INTEGER DEFAULT 0,
  created_by_pid TEXT NOT NULL,
  chat_id        TEXT NOT NULL,
  created_at     INTEGER NOT NULL,
  finished_at    INTEGER
);

-- 工具调用流水（每步一行，摘要级）
CREATE TABLE task_events (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id TEXT NOT NULL REFERENCES tasks(task_id),
  seq     INTEGER NOT NULL,
  kind    TEXT NOT NULL,               -- tool_call | tool_result | note
  payload TEXT,                        -- 摘要
  ts      INTEGER NOT NULL
);
CREATE INDEX idx_te_task ON task_events(task_id, seq);
```

产出文件实体进 `data/artifacts/`，表里只存路径；归档 = state 转 `archived` + 摘要进 `data/archive/`。

## 完工交接契约（Agent → Bot）

- `result_summary`：≤300 字人话结果摘要
- `artifacts`：产出文件路径列表
- `key_data`：关键数据点（长度、耗时、通过率等）

Bot 只拿这三样组织语言，不接触工具流水。

## 上下文截断规则

LLM 可见的工具结果恒经 Runtime 硬性截断：最近一步结果全量进 Working Memory，更早的压成摘要行；全文只存 task_events。截断由 Runtime 执行，不指望模型自觉（与 Schema 校验同一哲学）。

---

# 九、State 持久化

```sql
CREATE TABLE state_kv (
  key        TEXT PRIMARY KEY,         -- 当前话题等轻量状态
  value      TEXT,
  updated_at INTEGER NOT NULL
);
```

**mood 不入库**：情绪只活在内存，重启即 calm（"睡一觉起来总是平静的"）。

---

# 十、Decision 输入契约（Decision Context）

每字段有界，由 Context Builder 组装；摘要优先于原始消息。

```json
{
  "anchor": {
    "msg_id": 100,
    "sender_pid": "p_001",
    "text": "原始触发消息（超长截断）",
    "at_me": true,
    "reply_to_me": false
  },
  "window_messages": [
    {"msg_id": 100, "sender_pid": "p_001", "text": "A 的原始消息"},
    {"msg_id": 101, "sender_pid": "p_007", "text": "窗口内普通插话"}
  ],
  "message": {
    "text": "原文（超长截断）",
    "chat_type": "group | private",
    "at_me": true,
    "reply_to_me": false,
    "has_image": false,
    "mentions": ["person_id..."]
  },
  "sender": {
    "person_id": "p_001",
    "nickname": "当前名片",
    "trust": 0.8,
    "familiar": 0.7,
    "affinity_self": 0.6
  },
  "scene": {
    "recent_speakers": ["p_001", "p_007"],
    "msgs_since_my_reply": 3,
    "my_replies_last_5min": 1,
    "chat_topic": "当前话题摘要，可空"
  },
  "memory_hints": ["≤5条，关于sender/本群的长期记忆精选"],
  "mood": "calm | happy | angry | down",
  "active_task": null
}
```

`anchor` 是本轮不可变的回复对象；`window_messages` 只提供 10 秒聚合窗口中的上下文。Decision 输出不包含目标 person 或目标 msg_id，Runtime 始终按 anchor 路由回复。B 的独立 @/引用请求创建自己的窗口与 anchor。同 chat 已存在普通等待窗口时，新普通消息加入该窗口；若无等待窗口，则新建窗口；@/引用云团始终另建独立窗口。

窗口裁剪规则：必须保留 anchor、窗口内所有 @/引用云团消息和最后 30 条普通消息；Decision `state` 最多 8,000 字符，并以模型 tokenizer 对完整模板/schema 后的输入做 8,192-token 硬限制。超出的消息仍在 `messages` 表中，不能送入本次 Decision。窗口有创建序号，Runtime 按同 chat 的创建序号入发送队列。bot_chat 的 40,000 字符预算单独计算。

输出 Schema 见 architecture-v0.1.md 第十三章。

---

# 十一、更新语义总表

| 动作 | 时机 | 写入方 |
| --- | --- | --- |
| 消息落库 | 实时 | QQ 适配层收到即写 |
| 显式记忆 | 实时 | Decision.memory_write |
| 夜间归纳 | 固定时间（默认每夜） | 每 chat 取最新 500 条 → summaries + long_memories + relationship_events → 重算 edges |
| 摘要 | 夜间归纳产物 | 每群每日 1 条 + 人物按日滚动 |
| meme 抽图 | Decision=send_meme | 类别内最久未用加权随机 |
| 人格版本 | 管理员 WebUI 操作 | 新版本号 + active 切换 |

---

# 十二、事件表（events）

Event Bus 全量事件的落库副本（由 tracer 订阅写入），Decision trace 页与任务执行可视化页的数据源：

```sql
CREATE TABLE events (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  kind    TEXT NOT NULL,     -- MessageReceived | DecisionMade | TaskStepDone | ...
  payload TEXT,              -- JSON，按事件类型定结构
  ts      INTEGER NOT NULL
);
CREATE INDEX idx_events_ts ON events(ts);
```

轮转保留 7 天（定时任务清理）。

---

# 修订记录

- 2026-10-02 V0.1：数据模型定稿（拷问轮 Q21–Q25）：三主体长期记忆单表、每日摘要索引、@统计驱动熟悉度、人格线性版本链、meme 去重双档 + DINOv3 可选槽位、消息 mentions 字段、mood 免持久化、Decision 输入契约。
- 2026-10-02（Q26–Q30）：Task 章新增完工交接契约（result_summary + artifacts + key_data）与工具结果硬性截断规则。
- 2026-10-02（Q34–Q37）：新增第十二章 events 表（Event Bus 落库副本，7 天轮转），供 trace 与任务可视化页查询。
