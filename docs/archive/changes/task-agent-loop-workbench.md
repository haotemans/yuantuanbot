# Task / Agent 执行闭环 Workbench

[文档索引](../../README.md) · [归档索引](../README.md)

> 历史记录：以下背景、测试与剩余事项对应原记录日期。归档仅改变资料归属，不扩展当时的验收范围。

## 状态

- status: closed（全裁决落地 + 测试齐 + workspace 全过）
- owner: hsb + kimi
- last-grill: 2026-10-07
- frontier: 真实群聊 agent_exec 端到端（等配好 providers.toml 三角色后由用户验证）

## 事实（开工前扫过）

- `tasks` / `task_events` 表已在 V1 迁移建好
- `crates/core/src/agent.rs` 只有 1 行占位注释——**无任何执行器**
- Decision 已能产 `action=start_task` + `task_goal`，但 bot.rs 只记事件跳过（注释"后续单"）
- Event::TaskCreated / TaskFinished 枚举已定义，**没人发**
- webui Tasks.vue 回放页 + tasks_api 只读列表/流水 已就绪（读口先行的老惯例）
- ToolRegistry 已存在（tools.rs），McpToolAdapter 也进同一注册表

## 已确认裁决

### Q-A01 Agent 执行器形态

- decision: 同步工具循环。每轮 agent_exec LLM 返回 JSON `{ action: tool_call | reply | done, tool_name?, tool_args?, text? }`；tool_call 结果拼回上下文继续；到 done / budget 用完 / 连续 2 次 JSON 解析失败 → 收尾
- status: confirmed

### Q-A02 可用工具范围

- decision: ToolRegistry 全部已注册 Tool（hello / media_image / mcp:* / 插件 tool）
- affected-code: agent.rs 持 Registry 引用
- status: confirmed

### Q-A03 任务状态汇报

- decision: **静默**——不发群消息。任务生命周期只走 tasks/task_events 表 + Event::TaskCreated/TaskFinished 事件，面板 Tasks.vue 看
- status: confirmed

### Q-A04 budget_max_calls

- decision: 固定 10 轮
- status: confirmed

## 公开接口设计

### agent::TaskRunner（核心循环）

```rust
pub struct TaskRunnerDeps {
    pub db_path: PathBuf,
    pub llm: SharedLlm,          // 热应用槽：每条任务起时读当前值
    pub tools: Registry,          // 全部已注册工具
    pub bus: EventBus,
    pub mood: MoodState,          // 执行期间 mood 可读（不参与循环决策）
}

pub fn spawn_runner(deps: TaskRunnerDeps) -> JoinHandle<()>
// 订阅 Event::TaskCreated → 对每个新 task spawn 一个执行协程
```

### 循环协议（agent_exec LLM 系统提示内嵌）

```json
{
  "action": "tool_call" | "reply" | "done",
  "tool_name": "string, action=tool_call 时必填，须来自 tools 列表",
  "tool_args": {"…": "…, action=tool_call 时必填"},
  "text": "string, action=reply/done 时必填：reply=中间思考, done=最终答案",
  "reason": "一句话（只进 task_events, 不上屏）"
}
```

### 状态机

- running：任务活跃（budget 未用完且未 done）
- finished：done 收尾（finished_at 写入）
- failed：LLM 连续 2 次输出非法 JSON / 内部 panic / tool 抛错 3 次
- cancelled：面板手动取消（本期不做按钮，留状态枚举）

### task_events.kind 序列

- created（goal + budget）
- llm_round（seq=n, action, tool_name?, elapsed_ms, usage）
- tool_result（seq=n, tool_name, ok, summary, error?）
- reply（text, partial=true 中间思考）
- finished（state=finished, final_text）
- failed（state=failed, error）

## 工程任务

- [x] MOD-A01 agent.rs：TaskRunner + run_task() 循环 + system_prompt 模板（含 tools 描述目录）
- [x] MOD-A02 bot.rs：Decision start_task → INSERT tasks + publish TaskCreated
- [x] MOD-A03 agent.rs runner 接 TaskCreated → spawn 每任务执行协程
- [x] MOD-A04 Event 枚举：TaskCreated / TaskFinished 加 TaskLifecyclePayload（tracer 走 payload_json 序列化，无需改 tracer）
- [x] MOD-A05 main.rs 装配 spawn_runner（deps 注入 registry + llm 槽）
- [x] MOD-A06 webui tasks_api：已有 state 过滤，核实够用（pending/archived 留枚举本期不产生）
- [x] TEST-A01 单测 6 个：parse_round tool_call/done/fenced/garbage + task_id 唯一 + state str
- [x] TEST-A02 集成：mock LLM（本地 axum server，2 轮: tool_call(hello) → done）断言 tasks 行 finished/used=2、task_events 序列（created → llm_round → tool_result → llm_round → finished）、TaskFinished 事件 final_state=finished

## 验证

- cargo test --workspace 全过（core 53 + agent_loop 集成 + 各 crate 既有）
- 启动日志：19 张表 + "Task runner 已启动（等 TaskCreated 事件）"
- Event::TaskCreated/TaskFinished 从裸变体改成携带 TaskLifecyclePayload，全 workspace 无其他构造点（grep 零 hit）

## 代码反馈

- `Event::TaskCreated` 裸变体没法带 task_id → runner 不能消费 → 升级为 TaskLifecyclePayload（task_id/chat_id/goal/final_state/error）
- chat() 只有 system+user 单轮接口 → run_task 循环把对话历史 render 进单个 user prompt；agent_exec 无多轮 messages 支持（本期协议内闭环）
- used_calls 每轮 bump 到 tasks 表（面板进度条数据源），不是终局才写

## 非本期（明确不做）

- 群里发任务状态消息（Q-A03 裁决静默）
- 手动取消按钮 / 任务打断 API
- 多任务并发限流 / SIGINT 恢复
- 流式输出到面板
