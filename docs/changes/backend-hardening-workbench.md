# 后端加固 Workbench(Q52+Q53+Q55)

## 状态
- status: decided → implementing
- owner: kimi + hsb
- last-grill: 2026-10-07
- frontier:
  - Q54 10s 窗口聚合（下一步，依赖本次 Q52/Q53）
  - 监督树 / 优雅停机 SIGTERM（下一轮）
  - Q65 MCP decide 服务（暂缓，继续 OpenAI 兼容）

## 已确认裁决（本轮 grill)

### G001 dispatcher 范围
- decision: 先 Q52+Q53 底座，不一次上 Q54
- rationale: Q52 chat 隔离 + Q53 全局信号量是 Q54 窗口调度（多窗口异步 + 全局并发槽）的并发底座；先落地能立刻解决「一群堵塞全 bot」；改动可逆
- status: confirmed

### G002 Q65 暂缓
- decision: 继续 OpenAI 兼容 Decision;Q65 MCP decide 服务不启动
- rationale: Intern-Decision-4B 需 GPU 托管（自托管 9.1GB 或魔搭 xGPU 有暂停/冷启动），GPU 成本 vs 延迟取舍未决；外部依赖不阻塞其他后端优化
- status: confirmed

### G003 可靠性优先级
- decision: Q55 消息流水持久化 + 恢复消费，优先于监督树
- rationale: 丢消息是用户可见故障（重启漏回复）；监督树崩溃是小概率事件且 adapter 已有重连；Q55 落地后即便进程崩也能恢复，互补可靠性纵深
- status: confirmed

### G004 Q53 实现位置（自行裁决，授权内可逆）
- decision: `LlmGateway::chat` 内全局 `tokio::Semaphore` 4 许可，三角色共享；位于成本闸 acquire 之后、http 发送之前
- rationale: 信号量语义 = 并发数上限，与成本闸（每分钟调用频率）正交——runtime-design Q53 明确「不能用信号量替代每分钟速率」，两者并存
- affected-code: crates/core/src/llm.rs

### G005 Q55 持久化机制（自行裁决，授权内可逆）
- decision: messages 表加 `processed_at INTEGER NULL`(V4 迁移）;ingest 落库时默认 NULL；管线 handle 正常返回后 UPDATE 回写当前 ts；进程启动时在 spawn_pipeline 之前扫 `processed_at IS NULL AND sender_pid != 'self'` 的消息，按 ts 升序作为回放事件进入管线（跳过 /image 直派，避免重启刷图）
- rationale: messages 表已是权威流水，无需新增队列表；NULL 即未消费的天然标记；回放跳过 /image 因为 media 命令副作用大且用户当时已得到响应或已放弃
- affected-code: crates/core/src/db.rs (V4), crates/adapter-qq/src/ingest.rs （无改动）, crates/core/src/bot.rs, crates/yuantuan/src/main.rs

### G006 Q52 实现形态（自行裁决，授权内可逆）
- decision: pipeline 改成 dispatcher + per-chat worker。dispatcher 订阅 broadcast，按 chat_id 维护 HashMap<chat_id, mpsc::Sender>;message 到来时若无该 chat 通道则 spawn worker(mpsc cap 32 + 单独协程跑 handle)；同 chat 消息串行，不同 chat 并发
- rationale: 与 reply_engine per-chat 队列保序同构；worker 内 handle 不改业务逻辑
- affected-code: crates/core/src/bot.rs spawn_pipeline

## 工程任务
- [ ] MOD-B01: llm.rs 全局 Semaphore(4) + set_concurrency 可调
- [ ] MOD-B02: bot.rs dispatcher + per-chat worker
- [ ] MOD-B03: db.rs V4 messages.processed_at;bot.rs handle 末尾回写；main.rs 启动扫未消费回放
- [ ] TEST-B01: 单测——信号量并发上限、dispatcher 同 chat 保序、回放跳过 self
- [ ] TEST-B02: cargo test --workspace 全过
- [ ] 浏览器验证（面板启动无回归）+ commit + push

## 代码反馈

### 2026-10-07 实现完毕
- **MOD-B01** `llm.rs`:`tokio::Semaphore` 4 许可挂在 `LlmGateway`,`chat()` 与 `test_chat()` 都在成本闸后获取；`DEFAULT_LLM_CONCURRENCY=4` 常量公开。✓
- **MOD-B02** `bot.rs::spawn_pipeline`:dispatcher 形态,`HashMap<chat_id, mpsc::Sender>`;每 chat worker 串行 handle;mpsc cap 32,满了 try_send 失败 → 同步 handle 保底。✓
- **MOD-B03** `db.rs` V4:`messages.processed_at INTEGER NULL` + 部分索引;`bot::replay_pending` 启动时按 ts 扫未处理非 self 消息回放(跳过 /image);`handle` 包装层无条件回写 processed_at(含 Drop 分支);`main.rs` 在 spawn_pipeline 前调 replay。✓
- **TEST**: `bot::tests::{mark_processed_writes_ts, replay_skips_self_and_image_command, q53_concurrency_limit_is_four}` + 既有 53 单测 + workspace 全部 → 全绿。

### 意外发现与修复
**现象**: workspace 测试 `pages_api.rs::ws_pushes_events_and_config_write_applies` 失败 `duplicate column name: processed_at`。
**根因**: `temp_dir()` 用 `SystemTime::now().as_nanos()` 构造目录,Windows 上 SystemTime 精度仅 ~100ns,两个测试并行 start 时取到同值,共用同一 db 目录;migrate 被两个连接并发跑,user_version 推进与 ALTER 之间窗口期 ALTER 撞列。
**修复(双保险)**:
1. `db::migrate` V4 前置 `add_column_if_missing`(PRAGMA table_info 先查后 ALTER)
2. `pages_api.rs::temp_dir` 加进程 id + 原子序号,彻底不撞目录

### 浏览器验证(yuantuan debug 启动)
- 启动日志 `迁移已是最新,幂等跳过 user_version=4` ✓
- 启动日志 `Decision 管线已启动(Q52 per-chat 并发)` ✓
- 无 Q55 回放日志(当前 db 无未处理消息,符合) ✓
- 面板登录、仪表盘 6 卡(CPU 0.7%、内存 24MB)、WS LIVE、适配器离线(NapCat 未连,预期) ✓

### 已知边界(不本次处理)
- `/image` 回放跳过意味着用户崩溃瞬间的 /image 命令永远丢——裁决如此(G005),media 副作用大且通常用户已重发
- `replay_pending` 单线程串行回放,大量待处理时启动阻塞;当前场景(本地 dev)消息量小,够用;正式部署前若要优化可 batch 并发,需重新裁决保序边界
- Q53 的 4 许可固定编译期,未接 [llm] 热应用槽;若需运行时调,新一轮 grill(当前裁决是 runtime-design 已定稿"固定为 4")

