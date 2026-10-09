# 语义参与决策框架

[文档索引](../README.md) · [ADR-0009](../adr/0009-semantic-participation-policy.md) · [Bot 上下文](bot-context.md) · [可靠性](decision-reliability.md)

2026-10-10：Decision 在原有的一次分类调用中判断“对谁说、是否在继续对话、开口能否带来新帮助”，Runtime 核对来源、执行请求和频率，再决定最终动作。没有新增一次常规模型调用，没有自动训练或修改人格。

## 输入

`固定窗口 → Prefilter / 成本闸 → 回复快照 → Decision → Runtime 约束 → BotChat / 任务 / 忽略`。

继续使用稳定 person_id、人物简档、话题记忆、锚点、引用和近期消息。新增字段也来自同一次数据库读事务，不在 Decision 等待结束后更换上下文：

| 字段 | 用途与限制 |
| --- | --- |
| `dialogue.reply_target_person_id` | 当前回复对象的稳定身份 |
| `dialogue.last_reply_to_sender` | 同 chat/type、锚点前且近 5 分钟内，Bot 确实发给此人的最后一个气泡；最多 300 字。通过本地 `reply_anchor_id` 查原发送者，不靠昵称或邻近顺序猜测 |
| `active_tasks` | 当前 chat_id、当前创建者的最多 3 个 running 任务，含 task_id、goal（160 字）、used_calls、state；只证明调度状态，不提供成功执行结果 |
| `scene.human_messages_last_30s` | 当前会话近期人类消息数，用于衡量群聊繁忙程度 |
| `scene.my_replies_last_5min` | 当前会话近期 Bot 已发送的气泡数，并非回复轮数 |

Decision state 继续限制为 8,000 字符，可裁剪近期消息、窗口消息、记忆、简档和任务状态。参与判断能引用的 ID 从**最终实际发送给模型的 JSON**中收集；被裁剪消息和记忆中的远期编号不在白名单。此字符限制不等于完整 tokenizer 预算。

## 模型输出

既有 `action / mood / mention / reply_len / reason` 保留；新增 `assessment` 和 `reply_mode`。提示要求每次提供参与判断；旧响应省略时仍能解析并进入保守兼容路径。

| assessment 字段 | 值与含义 |
| --- | --- |
| `audience` | `bot / other / group / unclear`：当前发言的接收对象；bot 始终指云团，不能把发言者本人误当成该字段的对象 |
| `intent` | `question / request / follow_up / sharing / acknowledgment / banter / stop / correction / other` |
| `continuity` | `new_topic / continuing / closing / unclear` |
| `confidence` | `low / medium / high`：模型对语境判断的把握，**不是校准概率** |
| `benefit` | `low / medium / high`：本次回复的具体帮助 |
| `novelty` | `new / repeated / unclear`：是否提供尚未说过的信息 |
| `evidence` | `sufficient / missing / conflicting / not_needed`：事实资料是否足够；一般知识可不依赖个人历史，但过去执行结果不能凭空补全 |
| `evidence_msg_ids` | 1–6 个正数消息 ID，必须包含 anchor；使用对话承接时还需包含已关联的 Bot 回复 ID |

`reply_mode` 为 `answer / clarify / state_uncertainty`。模型先建议，Runtime 可以收紧：低把握或对象不明的回复改为简短澄清；缺失/冲突的事实资料改为说明无法确认，已有澄清方式则保留。该方式通过 system 提示传给 BotChat，并计入正文上下文预算。

## Runtime 最终裁决

1. 先执行既有任务原话与技能目录检查；语义判断不能替代用户的明确执行请求。
2. 检查参与判断引用的消息确实出现在本次输入中且包含 anchor。不合格则 ignore；模型原先选择 ignore 的内容不会被强行变成回复。
3. 停止请求、群聊收尾或明确面向他人时 ignore。独立的群聊附和也不再礼貌性回复一轮。
4. start_task/invoke_skill 除原话检查外，还要求明确面向 Bot、request 意图和 high 把握。
5. 直接 @、私聊、引用 Bot、以“云团”开头，或有真实归属的对话承接，可进入回应路径；这不强制回复。隐式承接要求 high、continuing、Bot 对象、合适的承接意图以及关联消息 ID。模型单独声称 `audience=bot` 不能免除冷却。
6. 普通群聊主动参与必须同时满足 high 把握、high 帮助、new 信息、事实依据足够或不需要个人历史，并且不是附和/玩梗。通过后仍检查冷却，限制为 short 且不自动 @。

| 当前会话负载 | 主动参与冷却 |
| --- | --- |
| 5 分钟内 Bot ≥6 泡，或 30 秒内人类 ≥15 条 | 300 秒 |
| Bot ≤2 泡，且人类 ≤4 条 | 60 秒 |
| 其余 | 120 秒 |

冷却距最近一次**实际发送**计算；即时读取用于限流，不改变事实快照。这些阈值是当前代码策略，尚未加入面板调参，也不是发送队列的预留配额。Prefilter 的既有硬限流仍在模型前运行；队列中尚未发送的回复不计入冷却。

缺少 `assessment` 的旧群聊回复使用上一版“开放问题候选＋固定 120 秒＋收尾词表”的保守规则；此路径无法理解任意省略表达，Trace 会标明。新字段存在但结构错误时按既有 Schema 重试一次，再失败则 ignore。

## 可观察性与验证

`DecisionMade.action` 是最终动作。新增 `assessment / reply_mode / policy`，其中 policy 包含版本 2、模型原始建议动作、消息依据校验结果、所用冷却和 Runtime 调整理由。WebUI Trace 展示建议、最终动作、回复方式和限制原因，旧事件仍可查看；reason 是简短分类理由，不是模型完整思考过程。

- `cargo test --workspace`：覆盖同一人的回复关联、跨人/跨会话隔离、伪造或已裁剪依据拒绝、负载冷却、收尾、回复方式到正文提示及 Trace 的实际管线、V7 回滚和历史行保留。
- `cargo run --locked -p yuantuan-core --example verify_participation -- /path/to/providers.toml`：固定 12 个合成场景调用真实 Decision。检查预期动作、必要的回复方式、无 fallback 和有效消息依据；不连接 QQ、不运行工具或创建实际任务。样本定义在 [participation_cases.json](../../crates/core/tests/fixtures/participation_cases.json)。
- `python3 deploy/verify-bot-runtime.py /path/to/yuantuan /path/to/source.db`：仅在隔离数据库副本中检查 V7 升级和 SIGTERM。

实际结果与生产部署状态见[后端工作记录](../changes/backend-hardening-workbench.md)。

## 剩余边界

引用存在只证明消息可见，不证明模型的语义解释正确；参与判断也不能证明最终正文完全忠实。`last_reply_to_sender` 只保存最后一个气泡，不是完整的问答状态机；旧未关联回复、跨过 5 分钟的承接可能被保守忽略。tasks 表尚无 chat_type，任务状态按既有 chat_id/创建者归属，群号与私聊号碰撞不能据此确认会话类型。

主动发言仍由新消息触发，没有无人发言时的主动定时聊天。任务状态可帮助理解进度，但尚未实现 Agent 完成结果自动回传 Bot，也没有运行时语义去重任务。固定场景用于回归，不能当作生产正确率或幻觉降低比例；语义召回、任务结果证据、完整问答状态与线上反馈评测仍是后续工作。
