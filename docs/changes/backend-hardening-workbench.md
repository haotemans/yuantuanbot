# 后端加固 Workbench(Q52+Q53+Q55)

[文档索引](../README.md) · [活跃工作](README.md)

> 工作记录：保留各轮的决定、实施反馈和验证证据。前期“现状”与待办只对应记录当时；继续工作前核对最新反馈和代码，不把旧状态当作当前验收。

## 2026-10-10 远端日志问题修复与隔离验收

用户要求优化解决日志发现的问题。实现契约统一记录在[Decision 与回复可靠性](../reference/decision-reliability.md)，本节记录验证结果，不把开发验收写成生产部署。

- 根因复现：旧提示下同一 provider 有时返回空 content 和未声明的 bash tool_calls；未执行返回的工具指令。新请求显式 tool_choice=none，分类提示明确没有工具接口/空技能目录；异常工具调用、截断和拒绝有独立错误。
- 代码：任务/技能增加 anchor 原话请求检查；Decision 增加窗口前最多 6 条原文；默认人格只在无历史版本时初始化；补充“无记录是未知，不代表没发生”的事实边界；reply_len 接入生成提示；空正文同快照重试一次；普通插话保留全部原回复；SIGTERM/SIGINT 接入，生产/发送 worker 可取消，发送限时排空。
- 本地：最终 `cargo test --workspace` 119 个测试通过（另 2 个 Linux 沙箱测试及 1 个示例文档测试默认忽略）；Clippy 通过，保留原有告警。测试覆盖真实请求结构、原话拒绝、人格不覆盖、插话保留、空正文重试和关闭队列。
- 远端构建位置：`/home/ubuntu/yuantuan-bot-verify-20261010`。使用 Rust 1.92 构建容器，CPU 1 核、内存 1536 MiB、PID 256；生成无调试符号的 dev 验证程序。复用先前沙箱验收编译缓存，未改生产镜像。
- 真实 provider：最终 8 个合成 Decision 样本全部有效，0 重试、0 fallback，耗时 749–1913 ms。分享公告/转述不触发任务，直接总结走 reply，明确请求实际运行代码才 start_task；这里只验证分类，没有执行该任务。
- 首轮正文样本仍出现“没去过隔壁群”的无依据否定，补规则后复验：隔壁群问题回答“没有隔壁群的聊天记录……没法确认”；历史测试问题回答“没有看到任何测试执行记录……没法确认之前到底跑没跑过”。这是小样本结果，不代表生产重试率为零或已杜绝幻觉。
- Linux Docker 最终验收：只读备份 V5 数据库至临时副本，升级 V6；72 人物、1317 消息、25 长期记忆的数量保持，完整性检查为 ok，默认人格初始化。正常启动后/模拟启动回放中收到 SIGTERM 都以 0 退出（0.166s/0.151s），checkpoint 成功。网络 none，未挂生产配置、凭证、Docker socket，也未连接 QQ。
- 生产容器仍维持此前主动停止状态，生产库仍为 V5。本轮没有重启群聊服务、迁移生产库或修改模型绑定。临时验收容器和数据库副本已清理，源码、编译产物与模型验收日志留在独立目录便于复核。

## 整理时的核对事项（2026-10-09）

- 本页保留多轮历史，早期清单与后续实现反馈存在状态滞后；不直接把未勾选项视为当前未实现，也不把一次编译通过视为全部验收。
- 已在本页“Overnight 工程加固”轮次复现并修复窗口 lane 生命周期差距；Q54 的其他缺口继续见[信息流参考](../reference/message-flow-rules.md)。
- 当前 Q54、Q56–Q64 的契约边界集中见[信息流实现参考](../reference/message-flow-rules.md)，避免本页与正式参考重复维护完成状态。
- 本页旧记录中“需要配置 provider”的 401 问题已有[故障复盘](../archive/incidents/new-api-401-report.md)；该修复只验证模型调用，不能替代群聊及插件端到端验收。

## Overnight 工程加固（2026-10-09）

- 起点：`main` / `8b79463`，工作区干净，origin 为目标仓库 SSH 地址。本轮为一个运行时可靠性提交；不部署生产环境。
- 状态：本轮代码与本地验收完成；Git 交付以包含本节的提交及其远端状态为准。整个后端工作仍保留后续事项。
- 正式行为与剩余边界：[信息流参考](../reference/message-flow-rules.md#agent-任务执行与持久化2026-10-09-加固)、[迁移说明](../reference/data-model.md#后续迁移说明2026-10-09)。

### 已复现的问题与修复

1. 同 chat 首个窗口后 sender 残留，第二窗口不触发；瞬时 200 条消息原实现只保留 64 条；销毁聚合器仍触发回调。三个新增测试在旧代码上失败，修复后通过。批次直接收集消息，按固定截止时间及同 chat 回调顺序处理，空闲 lane 回收，计时与回调由聚合器拥有。
2. 中文非法输出截断 panic、工具 panic 让任务长期停在 running；正常任务重复记录两条 finished。三个回归均在旧代码上失败，现通过 JoinSet 观察错误、UTF-8 安全截断及事务收尾修复。
3. TaskCreated 是易丢失通知，原 runner 无并发上限。改为数据库领取、3 槽和有限结果重试；12 个积压任务在重复/溢出事件下只执行 12 次，实测并发峰值 2–3。
4. 收尾 SQL 错误原先被忽略仍发布 TaskFinished。故障注入验证事务回滚、无虚假完成事件、恢复写入后只收尾一次且不重执行。新增创建流水失败时整笔任务回滚验证。
5. V4 在迁移事务外查列/加列，多连接仍存在竞态和部分迁移风险。改成写锁内检查和迁移，新增旧数据保留、4 连接并发、失败回滚和索引查询计划测试。
6. 取消 supervisor 原先只丢弃子组件 JoinHandle，组件仍继续运行。新增释放信号断言，取消现在向被监督组件传递。Agent 同时覆盖取消工具 future、重启中断任务失败收尾与 120 秒工具超时。
7. 其他 LLM HTTP、模型探测和备份错误摘要同步修复 UTF-8 截断；角色诊断去掉密钥片段，并补齐网关三条错误路径、WebUI JSON 错误契约和密钥不回显测试。
8. 全仓 Clippy 首轮发现既有 TCP mock 测试的循环条件变量被遮蔽，触发 `clippy::while_immutable_condition` 错误；改为直接按缓冲区实际长度判断，保留分段读取行为。

### 验证记录

- 起点 `cargo test --workspace`：全部通过；已有测试未覆盖上述故障。
- `cargo check --workspace --all-targets`：通过。
- `cargo test --workspace`：95 个测试通过，1 个既有文档测试保持 ignored；比起点新增 17 个测试，另强化既有监督取消、Agent 终态次数和 WebUI 错误契约断言。
- `cargo clippy --workspace --all-targets --message-format short`：通过（退出码 0）。仍有既有样式/冗余转换等告警；不是零告警验收，未通过放宽 lint 绕过错误。
- `npm run typecheck`（webui-frontend）：通过；API 状态/字段保留，未改前端产物。
- 核心改动文件 rustfmt 检查、Git 空白差异检查和新增内容凭据检查在提交前完成；配置、真实数据库、构建产物未纳入修改。
- 性能证据限于有界调度、避免批次克隆、减少任务事件 SQL 往返和 `EXPLAIN QUERY PLAN` 使用索引/无临时排序；没有运行生产负载基准，不声称具体延迟或吞吐提升百分比。

### 延期与验证边界

- 真实 QQ/NapCat、生产 provider、生产迁移和 Linux 进程信号未在本地测试。此轮不部署，也不使用真实 API 凭据。
- Q54–Q64 的窗口上下文、高优先级、token 预算、发送归属按正式参考继续推进；整个后端 Workbench 不据本轮局部完成而关闭。
- 待进一步处理：Bot 队列满时的串行约束、聚合批次总量上限，以及其他同步 SQLite 调用。已检查任务列表/流水 API 与 Tasks.vue 的字段使用，本轮保留字段和状态集合。

## 历史状态

### 2026-10-10：群聊 @ 与正文间隔

- 用户截图并明确说明：真正 @ 已成功，但昵称与正文粘连。原因是发送端把 OneBot `at` 与文字直接拼接，没有补显示间隔。
- 在实际带 `at` 段的群聊正文前补一个空格；正文已有空白则保留。自动 mention 和 `::at` 共用规则，原消息正文落库保持不变。
- 更新现有 NapCat mock 回复闭环，验证首泡自动 @ 和第三泡 `::at` 的发送段都含间隔、QQ 目标正确、未 @ 的第二泡和入库正文不受影响。
- `cargo test -p yuantuan-adapter-qq --test reply` 两项通过；rustfmt 与 Git 空白检查通过。本轮未更新运行中的后端或在真实 QQ 客户端重发消息。

### 2026-10-10：Bot 上下文与资料来源

- 按用户确认的“人物总结＋话题记忆＋近期对话”实施，并在开工前核对架构、运行时、数据模型、决策台账和回复归属 ADR。通过 [ADR-0008](../adr/0008-grounded-bot-context.md) 修订 Q26/Q27；正式契约和限制集中在 [Bot 上下文](../reference/bot-context.md)。
- 新增带本人原话出处的四字段简档；实时 Decision/夜间归纳共用来源校验，更晚的本人确认覆盖旧资料。旧每日摘要不自动提升为人物简档，旧无来源记忆保留并标注。
- 话题记忆使用当前问题/引用的中英文关键词检索，限定当前对象/会话；窗口入队携带消息上界，worker 一次读事务生成快照，Decision 与 bot_chat 共用。引用按新保存的外部消息编号解析，不猜测本地编号。
- V6 以原有迁移事务增加来源字段、简档表和索引；失败回滚及旧数据保留已验证。面板保留配置键 `roster_mem_per`，标签改为“话题记忆条数”。
- `cargo test --workspace`：113 个测试通过，3 个既有/环境相关测试保持 ignored（2 个 Docker、1 个文档测试）。新增 8 个上下文用例和 1 个迁移用例；强化收发编号、显式资料、归纳资料、真实管线的等待期间新消息隔离断言。
- 随后补充必要信息超预算时不访问 provider 的用例，上下文专项 9 个通过；最终回复闭环两条复验通过。`cargo check --workspace --all-targets`、Clippy（既有告警）、前端类型检查/构建、rustfmt、手写文件 Git 空白和文档链接检查通过；构建产物的第三方字符串尾部空格保留。
- 验证范围为本地 Rust/模拟 LLM/NapCat；没有真实线上模型的幻觉率评估，没有在生产数据库执行 V6，也没有部署服务器。旧窗口高优先级、精确 tokenizer、记忆语义检索/冲突合并等缺口继续保留。

### 2026-10-10：165 服务器 Docker 验收

- 用户授权远端测试后，在 `165.154.182.21` 的独立源码目录构建沙箱镜像，以限额编译容器生成测试程序；生产配置和服务未改动。
- `aba9087` 的两个真实 Docker 集成测试均通过，测试容器/三个工作卷无残留。工具版本、内核限额读数、环境与日志位置统一记录在[开发沙箱验证状态](../reference/agent-sandbox.md#验证状态)。
- 新增 `deploy/sandbox/verify.sh`，支持普通 Docker 服务器在不安装宿主 Rust 的情况下复验；`.sandbox-verify/` 缓存忽略，脚本固定 LF。远端 Bash 语法与 Compose 合并校验通过。
- 仍未验证 rootless daemon 和生产后端容器启动，未部署生产沙箱。原有 `yuantuan` 在连接前已停止（137，非 Docker 标记 OOM），本轮没有恢复它。

### 2026-10-09：Agent Docker 开发工具第一版

- 用户确认 Linux 服务器 + Docker 部署方向；实现和边界统一维护在[开发沙箱参考](../reference/agent-sandbox.md)。未修改当前运行配置，未部署服务器。
- 新增 `sandbox_exec`、独立 Git/Python/Bun 镜像、可选后端 Docker CLI target 和 Compose overlay。运行时按 task_id 隔离命名卷；命令有进程/内存/CPU/输出/时间限制，正常完成与取消均请求清理临时容器。
- Tool 参数 schema 进入 Agent 目录并转发 MCP inputSchema；沙箱命令及有界结果进入任务回放。配置默认关闭，非法沙箱配置写回前拒绝，有效修改标记重启生效。原有 Agent 10 轮/3 并发/120 秒契约保持。
- `cargo test --workspace`：104 个测试通过；真实 Docker 验收需显式运行（本机无 Docker），既有 supervisor 文档测试继续 ignored。新增 Docker 测试覆盖文件保留/隔离、工具版本、输出截断、超时、取消清理和限额配置，不能据测试文件存在宣称已完成 Linux 验收。
- `cargo check --workspace --all-targets` 与 `cargo clippy --workspace --all-targets --message-format short` 通过；Clippy 保留已有告警。核心改动 rustfmt、Git 空白差异检查通过；未改前端文件。
- 后续重点：Linux/rootless 实测、工作卷磁盘配额与保留回收、产物导出/备份、私有仓凭据及面板配置。没有吞吐或冷启动基准，不声称性能提升比例。

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

```text
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

---

## 四轮(2026-10-08):Docker 部署 + G007 重写(Q2 frontier 收口)

### 触发

用户:「用什么部署好呢」→ 侦察两台 VPS:

**panel(23.148.244.64)实测**:

- Debian 12,469MB 总/176MB 可用,磁盘 7.7G 剩 4G,**docker 未装**
- 已跑:new-api(:3000)、xray(:443)、cli-proxy-api、caddy、python×2
- 装 1Panel+Docker 会撑爆

**165(165.154.182.21)实测**:

- Ubuntu 24.04 x86_64,**3.8G 内存/2.8G 可用**,磁盘 77G 剩 52G,2 核
- **docker 已装好**(/usr/bin/docker)
- 已跑:new-api、sub2api、cliproxy、postgres、redis(占用仅 1.0G)
- 端口:80/3000/8080/8317/6379(内)/5432(内)/22 占用;**8085/6199/3001/6099 全空** ✓

### 裁决

**G013 部署形态**:165 当主战场,docker 但不装 1Panel(用户明确反对)

- compose 双容器:yuantuan + napcat(`mlikiowa/napcat-docker:latest`),内网 yuantuan-net
- 端口策略:**8085/3001/6099 只绑 127.0.0.1**(SSH 隧道访问);**6199 仅 expose 给内网 napcat**(不映射到宿主)
- 云安全组:用户**无需开任何入站端口**给 yuantuan(SSH 22 已通;NapCat 同 compose 拨容器内网 6199)

**G014 G007 重写**:容器内禁重启按钮

- 后端 `/api/config/restart` 检测 `/.dockerenv` 存在 → 返回 `409 Conflict` + 提示「宿主机 `docker compose restart`」
- 前端 Platform.vue「保存并重启」按钮在 409 时:
  - `clearTimeout(reloadTimer)` 取消自动刷新
  - 按钮 loading 关掉,显示后端提示文案
- 裸机/Windows dev 场景保持原行为(spawn+exit 链路不变)
- `restart: unless-stopped` 配合容器退出码语义 → 取代 spawn 自拉,docker 守护掌管

**G015「保存并重启」在容器内的语义切换**:由「面板直接触发热重启」改为「面板提示用户在 SSH 跑 docker compose restart」。这是必要的退化——容器内 exit 等效自杀;`docker compose restart` 是 docker 守护做的干净重启。

### MOD-B12 部署文件

新增:

- `Dockerfile` multi-stage:`rust:1.83-bookworm` builder → `debian:bookworm-slim` runtime
  - cargo-chef 风格缓存层(先 copy manifest 跑 cargo fetch)
  - 非 root `yuantuan:10001` 跑
  - 无任何 libssl 依赖(全 workspace 用 rustls)
  - EXPOSE 8085 6199
- `.dockerignore`:target/node_modules/data/.git/docs 全剔
- `deploy/compose.yml`:yuantuan(build→image `yuantuan:local`)+ napcat(mlikiowa 镜像)
- `deploy/yuantuan-config.toml`:listen `0.0.0.0`(容器语义;宿主仅 127.0.0.1:8085 映射)
- `deploy/yuantuan-providers.toml`:模板占位,部署时填实际 key

数据挂载:

- `yuantuan-data` volume → `/app/data`(SQLite + memes + logs)
- `./yuantuan-config.toml` → `/app/config.toml` (ro)
- `./yuantuan-providers.toml` → `/app/providers.toml` (ro)

### MOD-B13 G007 代码改

- `crates/webui/src/config_api.rs::restart` 头部加 `/.dockerenv` 检测返 409
- `webui-frontend/src/views/Platform.vue` 409 catch:`clearTimeout(reloadTimer)` + 提示
- 顺带修了一个 bug:原版 `setTimeout` 没保存 id,catch 里 `clearTimeout` 是裸标识符(no-op)

### 验证命令

- 本地 `cargo build -p yuantuan` ✓
- 本地 `cargo test --workspace` 19 个 test result: ok 零 FAILED ✓(restart API 改动不影响其他测试)
- 本地 `vite build` 544ms ✓
- 165 服务器 `docker compose -f deploy/compose.yml build yuantuan`:
  **第一次失败**:`rust:1.83` cargo 不支持 `edition2024`(hashbrown 0.17.1 要求)→ 改 `rust:1.92-bookworm` 重试

### 踩坑 1: rust 镜像 tag 兜底

**现象**:build 第 9 步 `cargo fetch` 失败 `feature edition2024 is required`
**根因**:workspace 锁定 Cargo.lock 后 hashbrown 之类的 transitive dep 已用 2024 edition,而 `rust:1.83-bookworm` 的 cargo 1.83 不识别
**修复**:Dockerfile FROM rust:1.83 → rust:1.92(跟本机 rustc 1.92.0 对齐)
**教训**:锁镜像 tag 时以「当前本机能编过」为最低版本,不要靠经验猜

### 踩坑 2: 如何避免 ssh 挂死

**现象**:`docker compose build` 在后台 `( ... & )` + 输出重定向 nohup,但 ssh 不返回
**根因**:stdout/stderr 已重定向但 stdin 没关,ssh 等 stdin EOF
**修复**:加 `< /dev/null` 关 stdin;或 `ssh -f`(force background);或 `-n` 不重定向 stdin(跟 `< /dev/null` 等价)

### SSH 隧道使用方式(部署完后用户操作)

```powershell
# 一条隧道同时看 yuantuan + napcat 面板
ssh -L 8085:127.0.0.1:8085 -L 3001:127.0.0.1:3001 -L 6099:127.0.0.1:6099 ubuntu@165.154.182.21

# 本地浏览器:
#   http://127.0.0.1:8085/  → yuantuan 运维面板
#   http://127.0.0.1:3001/  → NapCat WebUI(首次登录拿 token)
#   http://127.0.0.1:6099/  → NapCat HTTP API(备)
```

### 遗留(下轮)

- G007 spawn+exit 链路在 Windows dev 仍在用,**不动**;服务器场景切换为「用户 SSH 触发」
- providers.toml 需要用户填实际 API key 才能跑决策/agent 链路
- NapCat 容器首次启动要扫码登录 QQ(走 NapCat WebUI :3001)
- 决策数据持久化:重启 NapCat 容器需要 QQ 会话保持 → `napcat-qq` volume(SQLite)解决
- yuantuan G007 服务器语义将来的简化:若长期稳定,可考虑把 spawn+exit 链路也删掉,统一提示

## 五轮(2026-10-08):Q54 10s 窗口聚合(管线最后一块落地)

### 触发

用户:「继续修后端」→ frontier 仅剩 Q54 是真正影响成本的管线裁决。

### 核心设计(全部自行裁决,授权内可逆)

**Q005 实现形态**:dispatcher→窗口聚合器→per-chat worker 三段

- 新增 `core::window` 模块,`WindowAggregator` 按 chat 隔离 lane
- **不**在 worker 内 `sleep(10s)`:会饿死 worker / mpsc 满导致同步保底卡死 dispatcher

**Q006 anchor 选择**:固定为触发开窗的第一条(Q54 明确「回复锚点固定为 A」)

- others = 窗口持续期间追加的同 chat 消息(紧凑,不延长窗口)

**Q007 高优先级 @/引用独立窗口本期不做**(下轮跟 Q56-62 上下文窗口控制一起做)

- 理由:Q54 的主收益是降本(N 条消息 → 1 次 LLM),90% 价值已拿到;独立窗口是正确性补丁,跟上下文控制耦合

**Q008 失败处理**:沿用 decide 现有事件分发,不动

**Q009 窗口消息上限**:本模块不设上限,由 context_builder 取最后 30 条那里统一卡

**Q54 关键洞察**:`decide → build_context` 已经从 db 拉最近消息(且 ingest 早把 others 写库),所以 **decide 不需要改**——窗口 fire 后 anchor handle 跑 decide,自然看到窗口期间所有消息。

### MOD-B14 WindowAggregator 模块

`crates/core/src/window.rs`:

- `feed(m, on_fire)` 按 chat 分 lane,每 chat 一个 tokio 协程管固定窗口
- `WindowBatch { chat_id, anchor, others }` 作为 fire 载荷
- `OnFire = Arc<dyn Fn(WindowBatch) -> Future>` 用户自定义消费
- cancel 信号 `Arc<AtomicBool>` 轮询 100ms(不引 tokio-util 依赖)
- 默认 `DEFAULT_WINDOW_SECS = 10` Q54 定稿
- lane 满 64 条瞬时洪峰 → debug log,本 batch 不丢(lane 满等下次 fire 自然重开)
- 3 单测全过:基本 fire / 多 chat 独立窗口 / cancel 停止

### MOD-B15 bot.rs dispatcher 接入

- `spawn_pipeline` 内部加 aggregator 中间层
- /image 等 media 命令绕过聚合,直派 worker(命令要「立刻」响应,延迟 10s 不合理)
- 非命令消息走 aggregator 聚合
- on_fire 回调:others 先 `mark_processed` 防 Q55 雪崩;anchor try_send 给 per-chat worker;channel 满同步 handle 保底
- buffer lanes 在 RwLock<HashMap> 中共享给 on_fire 闭包(否则闭包拿不到 lanes ref)

### MOD-B16 集成测试重写(Q54 语义适配)

原 `decision_pipeline_end_to_end` 断言「3 条消息 → 3 次 DecisionMade」在 Q54 下不再成立。拆成两个:

1. `decision_pipeline_q54_window_aggregation`:
   - 同 chat 3 条紧密连发(每 120ms 一条)
   - 期望 **恰好 1 次 DecisionMade**(且不会冒出第二次)
   - 期望 `messages.processed_at` 全部非空(Q54 others 立即回写)
   - mock LLM 只喂 1 个响应
   - 跑 13s(窗 10s + 收发 + LLM 调用)

2. `decision_pipeline_fallback_retries`:
   - 独立 chat 触发 fallback → retries=1
   - mock LLM 喂 2 个垃圾输出
   - 断言 fallback=true retries=1

3. `wait_until` 超时从 10s → 25s(Q54 单窗本身要 10s)

### 测试状态

- `cargo test --workspace` 19 ok 零 FAILED
- 新增 window.rs 3 单测 + 集成测试 2 个全过
- vite build 468ms ✓

### 验证命令

- `cargo test -p yuantuan-core --lib window` → 3 ok
- `cargo test -p yuantuan-adapter-qq --test pipeline` → 2 ok
- `cargo test --workspace` → 19 ok

### Frontier(下轮)

- Q56-Q62 anchor 上下文窗口控制(state 8000 字符 / window_messages 30 条 / 8192 tokens 硬卡 / bot_chat 40000 字符预算)—— 需要新实现,tokenizer 接入是重头
- @/引用独立高优先级窗口(挂在 Q56-62 实现时一起做)
- Q65 MCP decide(继续 OpenAI 兼容,已 G002 暂缓)
- prefilter 120 行 TODO 口径不一致:文档说「60s 4 个回复回合」代码不是 — 小清理,跟 Q56 一起收口

---
