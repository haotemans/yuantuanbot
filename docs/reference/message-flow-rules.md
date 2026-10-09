# 信息流实现参考

[文档索引](../README.md) · [运行时设计](../design/runtime-design.md) · [后端工作记录](../changes/backend-hardening-workbench.md)

> 文档性质：实现记录与已知差距。2026-10-09 整理时对照 `c6f3f5b` 的相关代码核对裁决编号与窗口边界；本次未重新执行全量运行验收。

> 本文记录消息处理实现及其边界，对照[运行时设计](../design/runtime-design.md)与[决策台账](../decision-log.md)。历史实现描述保留；本次明确核对的裁决及差距见文末，不以局部实现替代完整验收。

2026-10-10 增量：Decision → Runtime 参与策略 → BotChat 的当前契约见[语义参与框架](decision-framework.md)。同一快照新增真实回复承接与任务状态，DecisionMade 保留建议/最终动作和限制原因；成功发送气泡以 V7 `reply_anchor_id` 保存本地归属。它不替代原有窗口、任务执行与发送队列，也不表示已补齐高优先级窗口或精确 token 预算。

## 一、事件总线（Event Bus）职责边界

**形态**：单例 tokio broadcast，容量 1024；`EventBus::default()` 在 main 装配时创建。

**规则**：

1. **广播即发即丢**：无订阅者时事件直接丢弃（tokio broadcast 语义）。这意味着：
   - 观测类事件（DecisionMade / BubbleSent / MoodChanged / ConsolidationDone）**允许丢失**——tracer 落 events 表是观测通道，不是消息队列
   - 生产类事件（MessageReceived / TaskCreated）**必须有订阅者在线**——bot pipeline 和 agent runner 是常驻协程，spawn 顺序在 main 里保证先订阅再发事件

2. **事件 = 不可变事实**：事件一旦发出，内容不可修改；订阅者各自拷贝。任何"修改事件"的需求都是新事件。

3. **事件不携带可变引用**：所有 payload 是 owned data（String / Value / PathBuf），不借用外部状态。

4. **tracer 是旁路**：tracer 订阅全事件落 events 表，仅用于 trace/回放/观测，**不是**其他组件的数据源。组件间直接依赖 broadcast，不经过 events 表。

## 二、消息入口（adapter-qq → core）

**链路**：

```text
NapCat WS → adapter-qq::ingest → parse 消息段 → Event::MessageReceived → broadcast
```

**规则**：

5. **MessageReceived 是唯一入口**：所有外部消息（群聊/私聊/@/引用/图片）经此事件进入 core。没有其他路径。

6. **ingest 不做过滤**：adapter-qq 原样转发（除自身 echo 消息外），过滤逻辑在 Prefilter。

7. **ingest 不等待**：publish 后立即返回，不阻塞 WS 读取循环。

## 三、消息处理管线（Prefilter → Decision → 动作）

**链路**：

```text
Event::MessageReceived
  → bot::handle
    → ① /image 直派命令（先于一切，独立确定性路径）
    → ② Prefilter.check → Drop / Pass
    → ③ Decision::decide（仅 Pass 且 decision 角色可用时）
    → ④ 按 action 分发：
        reply       → bot_chat LLM → reply_engine 发送队列
        send_meme   → meme::pick → reply_engine 发送队列（图片）
        invoke_skill→ SkillRegistry.invoke → reply_engine 发送队列
        start_task  → agent::create_task → TaskCreated 事件
        ignore      → 仅记 DecisionMade 事件
```

**规则**：

8. **/image 直派优先**：`crate::tools::media::command::match_image_command` 命中时跳过 Prefilter + Decision，直接执行。显式命令不参与节流与成本闸。

9. **Prefilter 是硬闸**：Drop 的消息不产生任何事件（除 debug 日志），不进 Decision。

10. **Decision 是分类器不是对话者**：输出严格 JSON schema（action/mood/mention/reply_len/...），两次解析失败兜底 ignore。

11. **Decision 并发（Q52 ✅）**：dispatcher + per-chat worker 拓扑——dispatcher 订阅 broadcast 按 chat_id 派发到 mpsc（cap 32)，每 chat 一个 worker 串行 handle；不同 chat 并发互不堵塞；单 chat 瞬时洪峰超过 32 时降级同步 handle 保底不丢。

12. **10 秒窗口聚合（Q54，部分实现）**：dispatcher 经 `WindowAggregator` 把首条消息作为 anchor，同窗口普通消息作为 others；固定等待 10 秒后，将 anchor 和该窗口消息 ID 上界交给 worker，others 标记为已处理。连续窗口、批次保留、回调顺序和取消机制保持。2026-10-10 起 worker 获取一次回复快照，Decision 携带显式 anchor、窗口消息的精简投影、引用及相关资料，bot_chat 共用快照。完整的全部高优先级消息/末尾 30 条筛选和独立高优先级窗口仍有缺口，详见 [Bot 上下文](bot-context.md)。

13. **成本闸 + 全局并发（Q53 ✅）**：`LlmGateway::chat` 两道闸——① `CostGate` 滑动窗口（decision 30/min 可调，超限排队）;② 全局 `tokio::Semaphore` 4 许可，三角色共享，超出排队。正交不替代。

14. **消息流水持久化（Q55 ✅）**:`messages` 表加 `processed_at INTEGER NULL`(V4 迁移）。ingest 落库时 NULL;`bot::handle` 包装层在 handle_inner 返回后无条件回写当前 ts（含 Prefilter Drop 分支——「处理过」不等于「回复过」)。启动时 `bot::replay_pending` 在 spawn_pipeline 之前扫 `processed_at IS NULL AND sender_pid != 'self'`，按 ts 升序作为回放消息进入管线（跳过 `/image` 直派命令，避免重启刷图；回放消息信任当时 ingest 判定，不再重判 R2)。

## 四、回复发送（reply_engine）

**链路**：

```text
bot_chat LLM 输出 → reply_engine::prepare_and_enqueue → Bubbleizer 分泡 → per-chat 发送队列 → adapter-qq 发送
```

**规则**：

15. **per-chat 串行保序**：同一 chat 的消息按发送队列入队顺序发送，不同 chat 之间并发。这不等于 Q59 对独立窗口创建顺序的完整约束，也不代表 Q63 的高优先级调度已实现。

16. **切句与 @ 解析**：bot_chat 输出中 `::at` 指令转为 mention 段，实际 @ 后补显示间隔。`‖` 只作为候选句末边界，半句接回、纯控制符不发送、代码中的标记保留；short 最多一泡、medium 最多两泡，拆句字数目标不强拆单句。完整规则见[可靠性参考](decision-reliability.md)。

17. **单泡失败不阻断**：一个泡发送失败记 BubbleSent(ok=false) 事件，继续发下一泡。✅ 已实现。

18. **打字延时**：每泡有 base_delay + per_char_ms * 字数 + jitter，模拟真人输入。ReplyCfg 热应用。

19. **回复归属**：2026-10-10 起落实 ADR-0007，普通插话不取消已入队的气泡，发送仍绑定原 anchor。明确取消/更正和高优先级窗口调度尚未补齐。停机时发送队列限时排空，详见[可靠性参考](decision-reliability.md)。

## 五、任务执行（agent）

**链路**：

```text
Decision(start_task + task_goal) → agent::create_task → INSERT tasks + TaskCreated 事件
  → agent::spawn_runner 收到 → spawn 协程跑 run_task 循环
    → agent_exec LLM 每轮 JSON → tool_call/reply/done → 终止条件收尾
    → UPDATE tasks state + TaskFinished 事件
```

**规则**：

20. **任务生命周期静默**：不发群消息（Q-A03）。任务状态只在面板 Tasks 页可见。

21. **budget 硬上限**：10 轮（TASK_BUDGET_MAX_CALLS），用完强制 failed。

22. **工具调用回传**：tool 返回的 summary + data 拼回历史，agent_exec 下一轮能看到。

## 六、记忆与归纳

**链路**：

```text
Decision.memory_write → long_memories 表（explicit + 源消息）
Decision.profile_updates → person_profile_facts（本人原话校验）
夜间归纳 → summaries + long_memories（源窗口）+ 人物简档 + 关系事件
```

**规则**：

23. **memory_write 只写显式事实**：Decision 产出的 memory_write 字段经敏感词过滤后写 long_memories。

24. **归纳是后台侧路**：读取 messages，写每日摘要、长期记忆、通过出处校验的人物简档及关系变化；更新会影响后续回复快照，不改写已生成的本轮快照，也不改人格。

## 七、配置热应用

**规则**：

25. **共享槽模式**：需要热应用的组件持 `Arc<RwLock<T>>` 槽；WebUI config 写回后整体换槽，组件每次读取当前值。已落地：llm / prefilter / reply_cfg / ctx_cfg / steal / backup_cfg / napcat_token。

26. **MCP server 不热改**：新增/删除 MCP server 需重启（spawn 子进程的生命周期管理）。

## 八、观测与追踪

**规则**：

27. **events 表是观测通道**：tracer 订阅 broadcast 全量落库；Decision trace / 任务回放 / 仪表盘数据源。

28. **WS 推送**：webui ws 订阅 broadcast 全量推给浏览器；前端按 kind 过滤渲染。

29. **log 与 event 分离**：tracing 日志进 stdout/stderr + data/logs/；event 进 events 表 + WS。同一事实可能同时存在于两处，但语义不同（log 给开发者，event 给面板）。

## 九、监督树与优雅停机

**规则**：

30. **长活组件包 supervisor**：tracer / pipeline / task_runner / steal_listener 4 个组件由 `core::supervisor::Supervisor` 包裹启动；任一 panic / 异常退出后以指数退避自动拉起（1s → 60s 封顶，MAX_RESTARTS = 20）。取消 supervisor 协程会同时取消被监督组件，避免只丢弃 JoinHandle 而让组件继续运行。adapter-qq 不包（有自己的 WS 重连），consolidation / backup 不包（一次性定时器,语义不同）。

31. **优雅停机路径**：启动回放前注册 Unix SIGTERM/SIGINT，Windows 使用 Ctrl+C；停止入口和生产组件，发送队列最多排空 5 秒，MCP 最多收尾 1 秒，然后检查 WAL checkpoint 并退出。超时取消 worker 并记录未排空，详见[可靠性参考](decision-reliability.md)。

32. **Q55 回放窗口**：`REPLAY_WINDOW_SECS = 3600`。启动回放前先把窗口外未处理消息的 `processed_at` 写为当前时间，标记放弃，再 SELECT `ts >= now - 3600` 升序回放，防历史脏数据雪崩。

---

## 设计裁决与实现对照

本表最初在 2026-10-09 以 `c6f3f5b` 为基线校正编号和完成判断；本轮增量更新连续窗口与 Agent 加固的实现和测试结果。未触及的设计差距不据本轮测试宣称完成。决定含义以[决策台账](../decision-log.md)为准。

| 裁决 | 设计约束 | 实现记录与边界 | 代码来源 |
| --- | --- | --- | --- |
| Q52 | 按 chat 隔离并发 | 已有 dispatcher 与 per-chat worker | [bot.rs](../../crates/core/src/bot.rs) |
| Q53 | 全局 LLM 并发最多 4，速率限制独立 | 已有共享 Semaphore 和成本闸 | [llm.rs](../../crates/core/src/llm.rs) |
| Q54 | 固定 10 秒窗口、原请求归属与独立请求 | 部分实现；连续窗口、突发消息保留和回调顺序已补测试；独立高优先级窗口仍有差距 | [window.rs](../../crates/core/src/window.rs)、[bot.rs](../../crates/core/src/bot.rs) |
| Q55 | 消息流水可恢复，trace 可缺失 | 已有持久化处理标记和最近 1 小时启动回放；不承诺恰好一次或无限期回放 | [bot.rs](../../crates/core/src/bot.rs)、[db.rs](../../crates/core/src/db.rs) |
| Q56 | 不可变 anchor 与 window_messages 输入契约 | 已有共享回复快照及显式 anchor/窗口消息投影；完整窗口筛选仍待补齐 | [Bot 上下文](bot-context.md)、[decision.rs](../../crates/core/src/decision.rs) |
| Q57–Q58 | 窗口优先级、@/引用独立开窗 | 独立高优先级窗口未实现 | [window.rs](../../crates/core/src/window.rs) |
| Q59 | 同 chat 按窗口创建顺序进入发送队列 | 部分实现；已有队列按入队顺序发送，独立窗口调度约束未完整落实 | [bot.rs](../../crates/core/src/bot.rs)、[reply_engine.rs](../../crates/core/src/reply_engine.rs) |
| Q60 | 保留指定窗口消息并限制输入 | 完整窗口消息筛选和预算约束未实现 | [window.rs](../../crates/core/src/window.rs)、[decision.rs](../../crates/core/src/decision.rs) |
| Q61 | 普通消息合入等待窗口，独立请求另开 | 部分实现；普通消息合入与后续窗口已有回归测试，独立请求仍需处理 | [window.rs](../../crates/core/src/window.rs) |
| Q62 | Decision 字符预算与 tokenizer 硬上限 | 已限制结构化 state 为 8,000 字符；完整模板/schema 的 tokenizer 校验未实现 | [decision.rs](../../crates/core/src/decision.rs) |
| Q63 | 高优先级窗口优先获取全局 LLM 槽 | 未实现；普通 Semaphore 不等于业务优先级调度 | [llm.rs](../../crates/core/src/llm.rs) |
| Q64 | 每窗口至多一次可配置失败提示 | 未实现完整契约；现有失败回退或日志不能视为该能力 | [decision.rs](../../crates/core/src/decision.rs)、[bot.rs](../../crates/core/src/bot.rs) |
| Q65 | 通过 MCP 接入专用 decide 服务 | 原工作记录中暂缓，继续使用 OpenAI 兼容路径 | [后端工作记录](../changes/backend-hardening-workbench.md) |

## Agent 任务执行与持久化（2026-10-09 加固）

实现来源：[agent.rs](../../crates/core/src/agent.rs)、[集成测试](../../crates/core/tests/agent_loop.rs)。

- `tasks` 是待办来源，`TaskCreated` 只负责唤醒；启动和每秒兜底查询可找回漏通知任务。查询每次最多取 6 行，执行及待收尾结果合计最多占用 3 个槽，避免在内存中堆积任务。
- 保留现有状态协议：`running` 包含等待槽位的任务；`used_calls=0` 表示尚未开始。每轮 LLM 请求前持久化次数。重启或取消后，已开始但没有终态的任务转为 `failed`，不自动重放工具副作用；尚未开始的任务继续领取。此机制要求应用只有一个 Task runner，不提供多进程任务抢占协议。
- `JoinSet` 观察任务 panic；非法 JSON 连续两次、工具连续三次报错、预算耗尽等均失败收尾。单次工具调用上限 120 秒；超时直接失败，不自动重试。取消 runner 会取消它拥有的工具 future，但不能撤销外部系统已经执行的副作用。
- 创建任务与 `created` 流水、终态与终态流水分别使用同一事务；提交成功后才发布生命周期事件。收尾写入失败保留结果并重试，保留槽位，不再次执行任务。每个任务只产生一条终态流水，序号分配与插入为一条 SQL。
- 2026-10-10 按 ADR-0010 迁移后，Runner 的查询、进度、流水、终态和 `create_task` 均通过 SQLx 异步执行。调用方必须 await；事务原子性及通知时机保持，SQLite 锁等待由驱动 worker 承担。连接池与关闭边界见[异步访问参考](sqlite-access.md)。
- 中文/多字节 LLM 错误摘要按 UTF-8 边界截断。角色诊断仅报告密钥存在与长度，不再输出密钥前缀。

## 仍需处理的实现差距

窗口连续处理故障已有复现与修复验证，详见[后端加固工作记录](../changes/backend-hardening-workbench.md)。这不代表 Q54–Q64 全部完成：独立高优先级请求、窗口上下文预算、tokenizer 上限和发送归属仍按上表追踪。

聚合批次尚无总消息/字节上限；长时间下游阻塞仍可能积累批次。`bot.rs` 的 per-chat worker 队列满时同步保底处理可能破坏串行约束，worker 现通过 JoinSet 随 dispatcher 取消。本轮未改变这些路径，不能把聚合器的顺序测试扩展为整个发送链路的保序保证。

## 修订记录

- 2026-10-09：修复连续窗口及突发丢消息，补充 Agent 有界调度、失败恢复、事务收尾与监督取消的实际行为；验证证据见后端工作记录。
- 2026-10-09：移入实现参考目录；补充代码来源和核对范围，修正 Q59/Q63/Q64 的对应关系，将窗口与输入契约的未完成部分显式列出。

- 2026-10-10：依据远端日志修复模型工具调用误返回、任务误触发、插话截断和 SIGTERM 停机，新增[可靠性参考](decision-reliability.md)。
