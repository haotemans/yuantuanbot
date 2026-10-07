# 信息流传递规则（现状落地版）

> 本文档记录**当前代码实际执行**的信息流规则，与 runtime-design.md 中 Q52-Q65 的设计裁决对应。
> 每条规则标注：✅ 已实现 / ⚠️ 部分实现 / ❌ 未实现。未实现项保留设计意图，不在本文档展开。

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

```
NapCat WS → adapter-qq::ingest → parse 消息段 → Event::MessageReceived → broadcast
```

**规则**：

5. **MessageReceived 是唯一入口**：所有外部消息（群聊/私聊/@/引用/图片）经此事件进入 core。没有其他路径。

6. **ingest 不做过滤**：adapter-qq 原样转发（除自身 echo 消息外），过滤逻辑在 Prefilter。

7. **ingest 不等待**：publish 后立即返回，不阻塞 WS 读取循环。

## 三、消息处理管线（Prefilter → Decision → 动作）

**链路**：

```
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

11. **Decision 并发（Q52 ✅）**：dispatcher + per-chat worker 拓扑——dispatcher 订阅 broadcast 按 chat_id 派发到 mpsc（cap 32)，每 chat 一个 worker 串行 handle；不同 chat 并发互不堵塞；单 chat 瞬时洪峰超过 32 时降级同步 handle 保底不丢。Q54 窗口聚合仍待实现。

12. **成本闸 + 全局并发（Q53 ✅）**：`LlmGateway::chat` 两道闸——① `CostGate` 滑动窗口（decision 30/min 可调，超限排队）;② 全局 `tokio::Semaphore` 4 许可，三角色共享，超出排队。正交不替代。

12a. **消息流水持久化（Q55 ✅）**:`messages` 表加 `processed_at INTEGER NULL`(V4 迁移）。ingest 落库时 NULL;`bot::handle` 包装层在 handle_inner 返回后无条件回写当前 ts（含 Prefilter Drop 分支——「处理过」不等于「回复过」)。启动时 `bot::replay_pending` 在 spawn_pipeline 之前扫 `processed_at IS NULL AND sender_pid != 'self'`，按 ts 升序作为回放消息进入管线（跳过 `/image` 直派命令，避免重启刷图；回放消息信任当时 ingest 判定，不再重判 R2)。

## 四、回复发送（reply_engine）

**链路**：

```
bot_chat LLM 输出 → reply_engine::prepare_and_enqueue → Bubbleizer 分泡 → per-chat 发送队列 → adapter-qq 发送
```

**规则**：

13. **per-chat 串行保序**：同一 chat 的消息按入队顺序发送（Q63 ✅ 已落地）。不同 chat 之间并发。

14. **泡内 @ 解析**：bot_chat 输出中 `::at` 指令由 Bubbleizer 解析为 mention 段。

15. **单泡失败不阻断**：一个泡发送失败记 BubbleSent(ok=false) 事件，继续发下一泡。✅ 已实现。

16. **打字延时**：每泡有 base_delay + per_char_ms * 字数 + jitter，模拟真人输入。ReplyCfg 热应用。

17. **作废核对**：发送前检查是否有新消息到来使回复过时（ReplyInterrupted）。⚠️ 部分实现（仅检测，不取消已入队的泡）。

## 五、任务执行（agent）

**链路**：

```
Decision(start_task + task_goal) → agent::create_task → INSERT tasks + TaskCreated 事件
  → agent::spawn_runner 收到 → spawn 协程跑 run_task 循环
    → agent_exec LLM 每轮 JSON → tool_call/reply/done → 终止条件收尾
    → UPDATE tasks state + TaskFinished 事件
```

**规则**：

18. **任务生命周期静默**：不发群消息（Q-A03）。任务状态只在面板 Tasks 页可见。

19. **budget 硬上限**：10 轮（TASK_BUDGET_MAX_CALLS），用完强制 failed。

20. **工具调用回传**：tool 返回的 summary + data 拼回历史，agent_exec 下一轮能看到。

## 六、记忆与归纳

**链路**：

```
Decision.memory_write → long_memories 表（explicit）
夜间归纳 consolidation::spawn_scheduler → summaries 表 + ConsolidationDone 事件
```

**规则**：

21. **memory_write 只写显式事实**：Decision 产出的 memory_write 字段经敏感词过滤后写 long_memories。

22. **归纳是只读侧路**：consolidation 读 messages → 写 summaries，不影响实时管线。

## 七、配置热应用

**规则**：

23. **共享槽模式**：需要热应用的组件持 `Arc<RwLock<T>>` 槽；WebUI config 写回后整体换槽，组件每次读取当前值。已落地：llm / prefilter / reply_cfg / ctx_cfg / steal / backup_cfg / napcat_token。

24. **MCP server 不热改**：新增/删除 MCP server 需重启（spawn 子进程的生命周期管理）。

## 八、观测与追踪

**规则**：

25. **events 表是观测通道**：tracer 订阅 broadcast 全量落库；Decision trace / 任务回放 / 仪表盘数据源。

26. **WS 推送**：webui ws 订阅 broadcast 全量推给浏览器；前端按 kind 过滤渲染。

27. **log 与 event 分离**：tracing 日志进 stdout/stderr + data/logs/；event 进 events 表 + WS。同一事实可能同时存在于两处，但语义不同（log 给开发者，event 给面板）。

---

## 附：设计裁决待实现清单（不改变现状）

| 裁决 | 内容 | 状态 |
|---|---|---|
| Q52 | 按 chat 隔离受限并发 | ✅ 已落地（dispatcher + per-chat mpsc worker) |
| Q53 | 全局 LLM 并发 ≤4 | ✅ 已落地（`LlmGateway` Semaphore 4，三角色共享） |
| Q54 | 10s 窗口聚合 | ❌ 未实现 |
| Q55 | 消息流水持久化恢复 | ✅ 已落地（messages.processed_at V4 + 启动回放管线；回放跳过 /image 命令） |
| Q56-Q62 | anchor 锚定 + window_messages 有界 | ❌ 未实现 |
| Q64 | 同 chat 按窗口顺序进发送队列 | ✅ 已落地（per-chat 队列保序） |
| Q65 | Decision 走 MCP decide 服务 | ❌ 未实现 |

> 以上裁决在 runtime-design.md 中有完整设计；本文档仅记录现状。实现时以 runtime-design.md 为准。
