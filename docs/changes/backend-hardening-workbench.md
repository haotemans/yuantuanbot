# 后端加固 Workbench(Q52+Q53+Q55)

## 状态
- status: implementing → verifying
- owner: kimi + hsb
- last-grill: 2026-10-07
- frontier:
  - Q54 10s 窗口聚合（下一轮主线后）
  - 服务器环境 restart 语义重审（用户反馈「服务器上 spawn+exit 不会拉起」→ G007 的适应边界）
  - Q65 MCP decide 服务（暂缓，继续 OpenAI 兼容）
- 2026-10-07 三轮 grill：主线 = 监督树+优雅停机；Q55 补 1h 时间窗

## 已确认裁决（本轮 grill)

### G007 B 类配置（listen_addr/webui host/data dir/MCP servers）生效机制
- decision: 面板「保存并重启」——config 写回后 SIGTERM 自己，同进程 exec 重启（零中断）
- rationale: 比热重绑定 listener/webui 子进程简单；比手动重启用户友好
- affected-code: crates/yuantuan/src/main.rs(SIGTERM handler + exec 重启）, crates/webui/src/config_api.rs（新增 /api/config/restart 端点）, webui-frontend（配置页按钮）
- status: confirmed

### G008 D 类调优常量提升到 config
- decision: PER_CHAT_QUEUE_CAP / CostGate 初始值 / SelfMsgIds CAP 三项入 config.toml `[pipeline]` 段，走共享槽热应用
- rationale: 调优参数不是设计裁决，是实现参数；应可由面板调
- affected-code: crates/yuantuan/src/config.rs, crates/core/src/bot.rs, crates/core/src/prefilter.rs, crates/core/src/llm.rs, 运行参数页面板 + config_api
- status: confirmed

### G009 A 类设计裁决常量保持定死
- decision: Q53 LLM 并发上限 = 4、TASK_BUDGET_MAX_CALLS = 10 保持编译期常量，不暴露
- rationale: runtime-design 已明确「全局 LLM 请求并发上限固定为 4」/「最多 3 个并发任务预算 10 轮」是设计裁决不是调优参数；要改需新一轮 grill
- status: confirmed

## 已确认裁决（一轮 grill)

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

---

## 二轮(2026-10-07):B/C/D 类配置边界审计 + G007/G008/G009

### 触发
用户:「你是不是很多配置都定死了 我修改重启还是6199」。审计发现四类:
- A 类设计裁决定死(Q53=4,TASK budget=10)——G009 保持
- B 类 config 有但需重启,UI 没说清——G007 面板一键重启
- C 类 config 有且热应用——已正常
- D 类代码定死 config 没暴露——G008 提升到 [pipeline]

### MOD-B04 [pipeline] 新段
`per_chat_queue_cap=32 / self_msg_ids_cap=512 / decision_cost_per_min_init=30`,默认 config.toml 模板已补

### MOD-B05 热应用槽贯通
- `SharedPerChatCap` spawn worker 时读(旧 worker 容量定型,新 worker 用新值)
- `SelfMsgIds::with_cap / set_cap`(set_cap 立刻 truncate 老条目)
- main.rs 装配 + PipelineDeps 接线 + `[pipeline]` post_config 热应用

### MOD-B06/B07 面板一键重启
- POST `/api/config/restart` → 202 → 500ms 后 spawn 新进程(带 `YUANTUAN_DELAY_START_MS=1200`)→ exit(0)
- Platform.vue 加「保存并重启」橙色按钮,3.2s 自动刷新页面

### 意外修复 1:Stdio::inherit broken pipe
**现象**:第一次 restart,新进程 spawn 后立刻死,没起 listener
**根因**:`Stdio::inherit()` 继承旧进程的 stdout/stderr 管道 fd,旧进程 exit 后管道 close,新进程第一次 println/tracing write 就 broken pipe panic
**修复**:Stdio 全 null;代价是新进程 stdout 不进旧终端(诊断走 data/logs/ 或手动重启)

### 意外修复 2:GET /config 脱敏回填丢 token(老坑)
**现象**:面板拿到 masked `token: "***"`,原样 POST 回去,后端 `fs::write` 覆写整个 config.toml → 真 token 没了
**修复**:后端 `unmask_in_place`——POST 收到 `***` 值时从磁盘原文件还原真值再写回;前端 Platform.vue 保留掩码字段不删
**验证**:`grep "^token" config.toml` 在保存前后不变

### 浏览器验证(自动)
- 保存并重启按钮存在 ✓
- 改 6199→6299 保存:config.toml 写入 6299 ✓ netstat 6299 LISTENING pid 新 ✓ 6199 释放 ✓
- 重启后 session 失效(内存 session 表,已知)
- Token 卡在「已配置」✓
- 改回 6199 同样工作 ✓

### 验证命令
- `cargo test --workspace` 19 个 test result: ok 零失败
- vite build 497ms
- curl POST /api/config/restart → 202 → 旧 pid → 新 pid 切换


---

## 三轮(2026-10-07 → 10-08):监督树 + 优雅停机 + Q55 时间窗

### 触发
用户:「后端功能没有实现的要实现 实现的要优化 然后框架进一步确认 grill me」→ 主线定位为 Q1 监督树+优雅停机(Q54 暂缓)。理由:面板一键重启每次都是硬退、任一后台组件 panic 静默死。

### 裁决
- **G010 主线 = 监督树 + 优雅停机**:Supervisor + 指数退避包 4 个长活组件;main.rs `tokio::select!` webui serve vs ctrl_c;5s 排空 → WAL checkpoint → exit
- **G011 Q55 回放窗口**:REPLAY_WINDOW_SECS = 3600(1h),窗口外 UPDATE 标记放弃,防历史脏数据雪崩
- **G012 服务器环境 restart 语义重审(frontier)**:用户反馈「服务器上 spawn+exit 不会拉起」→ G007 一刀切重启在 systemd/docker 下失效,本轮不动

### MOD-B08 Q55 1h 时间窗
`crates/core/src/bot.rs::replay_pending`:
- 常量 `REPLAY_WINDOW_SECS: i64 = 3600`
- 回放前先 UPDATE `processed_at = -2` 标记 stale_pending(窗口外 + 未处理)
- SELECT 加 `ts >= ?1` 窗口过滤
- 新测试 `replay_abandons_stale_pending` ✓;`replay_skips_self_and_image_command` 改用当前时间避免被窗口误删 ✓

### MOD-B09 supervisor.rs 新文件
`crates/core/src/supervisor.rs`:
- `Supervisor struct` + `spawn(name, factory) -> JoinHandle<()>`
- 指数退避 1s → 60s 封顶,MAX_RESTARTS = 20
- `CancellationToken` 控制停止
- 测试:`restart_on_panic`(造 panic 验证拉起) + `cancel_stops_restarting`(cancel 后不再重启) ✓

### MOD-B10 PipelineDeps/TaskRunnerDeps Clone + 4 组件包 supervisor
`crates/yuantuan/src/main.rs`:
- `PipelineDeps` / `TaskRunnerDeps` 加 `#[derive(Clone)]`
- 包监督树的 4 个组件:**tracer / pipeline / task_runner / steal_listener**
- **不包**:adapter-qq(自己有重连逻辑)、consolidation/backup(一次性定时器,语义不同)

### MOD-B11 优雅停机
`main.rs` 末尾:
```rust
tokio::select! {
    _ = webui_serve => { ... }
    _ = tokio::signal::ctrl_c() => {
        info!("收到 Ctrl+C,启动优雅停机序列");
        // ① 停止接新(webui serve 自然退出)
        // ② 5s 排空
        // ③ PRAGMA wal_checkpoint(TRUNCATE)
        // ④ exit(0)
    }
}
```

### 测试状态
- `cargo test --workspace` 19 个 "test result: ok" 零失败
- 新增 `supervisor::tests::{restart_on_panic, cancel_stops_restarting}` 全过
- 新增 `bot::tests::replay_abandons_stale_pending` 全过
- doc test `supervisor.rs line 10` ignored(ignore 块规避)

### 启动验证(2026-10-08)
```
INFO supervisor 启动组件 component="tracer" restarts=0
INFO supervisor 启动组件 component="pipeline" restarts=0
INFO supervisor 启动组件 component="task_runner" restarts=0
INFO supervisor 启动组件 component="steal_listener" restarts=0
INFO Decision 管线已启动(Q52 per-chat 并发)
INFO WebUI 开始监听 addr=127.0.0.1:8085
```
4 个组件全部 supervisor 化 ✓

### 浏览器验证(2026-10-08)
- 登录 → 仪表盘 LIVE、mood · calm、CPU 0.0%、内存 23 MB、运行 2m13s ✓
- 适配器离线(预期,NapCat 未拨入) ✓
- 面板 6 卡数据完整 ✓

### 优雅停机实测边界(已知限制)
- Windows 下从外部进程发 ConsoleCtrlEvent 到 yuantuan pid **无效**(Windows API 要求 pid 是 process group id)
- 从 Git Bash / Python 发信号到 yuantuan 也不到(不共享 console group)
- **真实有效的路径**:用户在前台终端跑 `yuantuan.exe`,光标停在那里按 Ctrl+C → tokio::signal::ctrl_c() 触发 → 走优雅序列
- 本轮代码已就绪并编译过,实测触发留给用户前台验证

### Frontier(下轮)
- **Q2(原 G007 副作用)**:服务器环境(systemd/docker)下 `spawn+exit(0)` 不会拉起新进程,一键重启在服务器上会「自杀」。需要 grill:服务器部署形态 + 是否需要额外守护
- Q54 10s 窗口聚合:为本管线设计的最后一块,Supervisor 稳定后启动


