# 云团（Cloud Agent）总体架构设计 V0.1

[文档索引](../README.md) · [实现参考](../reference/message-flow-rules.md) · [决策台账](../decision-log.md)

> 文档性质：设计目标与约束。历史修订中的实施进度只对应当时版本；当前实现及差距集中记录在实现参考中。

2026-10-10 Decision 增量按 [ADR-0009](../adr/0009-semantic-participation-policy.md) 实施；对象、承接、依据与参与频率的当前输入输出见[语义参与框架](../reference/decision-framework.md)，下文原始 Schema 保留为设计基线。

> 云团不是传统 Bot，而是一个具有人格、长期记忆、社会关系、工具能力、自主决策能力的长期运行 Agent。

---

## 一、核心理念

云团的目标：

一个长期运行的 AI Agent。

它具备：

- 人格
- 长期记忆
- 社会关系
- 工具能力
- 自主决策能力

核心原则：

- 外部像一个人
- 内部像一个操作系统

用户不应该看到内部执行过程。

不展示：

- Decision 过程
- LLM 调用链
- Tool 调用链
- Memory 查询
- Agent Loop

用户只看到：

- 自然聊天
- 必要反馈
- 最终结果

---

## 二、整体架构

```text
                     云团
                       |
    ┌──────────────────┴──────────────────┐
    Bot系统                             Agent系统
  （交流人格）                        （任务执行）
    |                                   |
长期记忆                              短期记忆
关系系统                              Task状态
人格系统                              工具调用
群聊上下文                            工作上下文
    └──────────────────┬──────────────────┘
                       |
            Prefilter（本地规则预筛）
                       |
                Decision 小脑
                       |
                LLM（API接入）
                       |
                Rust Runtime Kernel
                       |
        QQ适配层（OneBot 11 / WebSocket + token）
                       |
        NapCat 协议端（独立容器，可替换）
```

---

## 三、Bot 系统（长期存在）

负责：

- 群聊
- 私聊
- 人格表现
- 社交关系
- 长期记忆

---

### 3.1 Identity 身份系统

目标：

认识"人"，而不是认识昵称。

结构：

```text
Person
person_id
|
Identity
QQ
Telegram
Discord
微信
```

支持：

- QQ
- Telegram
- Discord
- 微信等

昵称只是属性：

```text
小明
↓
明哥
↓
Ming
```

Identity 不因为昵称变化而变化。

同一个人可以出现在不同的群里：QQ 号在所有群是同一个号，天然收敛为同一 person_id。所有长期数据（记忆、关系）落库一律跟随 person_id，跨群合并；群昵称/群名片只是该群上下文里的临时属性。

第一阶段：仅 QQ（经 NapCat 对接）；Telegram 等平台仅保留适配层接口。

---

### 3.2 Relationship 关系库

目标：

理解人与人之间的关系。

不是：

```text
A 是朋友
```

而是：

```text
A → B

信任:
0.8

熟悉:
0.7

来源:
A帮助B解决问题

时间:
2026-09-01
```

结构：

```text
Person

Relationship Edge

Relationship Event
```

保存：

- 谁帮助过谁
- 谁合作过
- 谁经常交流
- 谁之间关系变化

云团自己也是关系图中的一个节点："云团 ↔ 每个人"的亲密度边从第一天起默默记录，不对外展示，仅影响云团自己的行为（以及 WebUI 关系网可视化）。

关系事件的来源：

- 平台事件（入群、加好友等）直接记录
- @互动统计（A@B → 熟悉度增量，V1 熟悉度的主要驱动）
- 夜间归纳时从消息流水批量提炼（"A 帮助 B 解决问题"类 → 信任分）
- 不做每条消息的实时 LLM 抽取

---

### 3.3 Personality 人格系统

人格不是一个 Prompt。

而是一套长期状态。

保存：

- 说话方式
- 主动程度
- 幽默程度
- 不同人的交流方式
- 长期行为特点

**人格本身不变，行为随记忆与关系演化。** 人设提示词是静态文本：

- 仅管理员可修改（WebUI 人格编辑器）
- 每次修改强制版本化，历史线性单向（回滚 = 新版本号 + 旧内容），可查看 diff、可回滚
- 对话与归纳流程永远不能改写人设，只能通过记忆与关系影响行为

---

### 3.4 Long Memory 长期记忆

属于 Bot。

保存：

用户：

- 喜欢什么
- 正在做什么
- 技术偏好

例如：

```text
A喜欢Rust

A正在研究机器人

A喜欢技术讨论
```

云团经历：

```text
曾经帮助A完成机器人项目
```

不保存：

- 临时日志
- 工具输出
- 中间步骤

写入采用双通道：

1. 显式通道：Decision 裁决时标记重要事实（memory_write），实时写入
2. 归纳通道：固定时间（默认每夜）从消息流水批量提纯

敏感信息（密码、密钥、证件号等）在写入阶段直接拒收，永不入库。

长期记忆三主体（person / chat / self）单表存储，另有每日摘要（每群一条 + 人物按日滚动）作为聊天历史的检索索引。表结构详见 [docs/reference/data-model.md](../reference/data-model.md)。

---

## 四、Agent 系统（负责执行）

Agent 是云团的执行能力。

例如：

用户：

> 帮我写一个网站

流程：

```text
Bot收到消息

↓

Decision判断

↓

创建Agent Task

↓

Agent执行

↓

结果返回Bot

↓

Bot回复用户
```

---

### 4.1 Agent Short Memory

工作记忆。

只服务当前任务。

例如：

```text
Task:
开发网站

目标:
完成登录系统

计划:

1. 前端
2. 后端
3. 测试

状态:
80%
```

任务结束：

- 清理
- 或归档

---

### 4.2 Task 系统

管理任务生命周期。

结构：

```text
Task

task_id

goal

state:
pending
running
finished
failed

context

artifacts
```

执行模型：**带预算的工具循环**。不同任务调用工具的次数不同，不做步数硬限制；Runtime 只卡一条硬预算（默认工具调用次数 / 时长上限，可配置），防失控、防成本爆炸。预算管安全，不管能力。

循环机制：每步 = agent_exec 输出 tool_call → Runtime 执行 → 结果截断后注入 Working Memory → 再喂回模型，直到它产出完工结果或撞预算。

完工交接契约（Agent → Bot）：`result_summary`（≤300 字人话）+ artifacts 路径列表 + 关键数据点。Bot 只拿这三样组织语言，不接触工具流水。

第一阶段：Task 以只读工具为主（search、web_read），写执行类工具（write_code、run_code 等）第二阶段接入。

---

## 五、Capability Tool 系统

云团能力。

### 文件

```text
create_file
read_file
compress
send_file
```

### 编程

```text
write_code
run_code
debug
git
```

### 网络

```text
search
api
download
```

### 多媒体

```text
image
video
audio
```

第一阶段仅接入只读类（search、web_read）；文件/编程/多媒体工具第二阶段按 Task 预算模型接入。

工具调用协议：**OpenAI 原生 function calling 为主**；Provider 声明不支持 tools 时降级为 JSON-in-text + 校验重试（与 Decision 输出校验同一套工程哲学）。

---

## 六、Agent 与 Bot 边界

非常重要：

禁止：

```text
Agent直接聊天
```

正确：

```text
Agent

↓

返回结果

↓

Bot组织语言

↓

发送消息
```

例如：

Agent：

```text
完成:
project.zip

测试通过
```

Bot：

```text
好了，代码整理好了，压缩包发你。
```

Bot 永不直接调用工具；任何工具使用都以 Task 形式存在，哪怕只调一次。Agent 交付给 Bot 的只有完工交接契约（result_summary + artifacts + 关键数据），不是工具流水。

---

## 七、Memory 架构

三层：

```text
              Memory
                 |
     ┌───────────┼─────────────┐
Long Memory   Working Memory   Archive
     Bot           Agent          历史
  人物关系        当前任务        完整轨迹
  用户习惯        工具状态        日志
    人格          中间结果        文件记录
```

### 夜间归纳（Consolidation）

固定时间运行（默认每夜一次）：

- 每个 chat（群 / 私聊）独立取该 chat 最新 500 条消息，跨 chat 不混合取样
- 产出四通道：每日摘要（每群一条 + 人物按日滚动，聊天历史检索索引）、关系边事件（含云团↔人亲密度）、群话题摘要（存该群长期档案）、个人长期记忆
- 落库统一按 person_id 合并
- 敏感信息在提取时拒收

消息流水全量落库（含 mentions 字段，见十五章数据库），归纳与"最近 500 条"窗口都建立在其上。

---

## 八、Agent Archive

任务结束：

```text
任务结束

↓

Memory Consolidation

↓

1. 压缩归档

2. 提取长期信息
```

---

### 压缩归档

保存：

- 任务摘要
- 时间线
- 文件
- trace位置

例如：

```text
T001

完成机器人程序开发

文件:
xxx.zip
```

---

### 提取长期信息

进入 Bot Memory。

保存：

```text
A正在做机器人项目

A喜欢Rust

A有开发需求
```

不保存：

```text
gcc失败三次

修改xxx文件

工具日志
```

---

## 九、Knowledge Base 知识库

作用：

提供额外知识。

场景：

- 不联网
- 私有资料
- 用户文件
- 项目资料

说明：

第一版为可插拔组件，默认关闭。WebUI 提供管理页面（文档上传、解析状态），功能本体后置于管理界面接入，不改变其余系统的零外部依赖特性。

流程：

```text
文件

↓

解析

↓

Embedding

↓

Vector DB

↓

RAG

↓

LLM
```

区别：

Memory：

> 关于人的信息

Knowledge Base：

> 关于知识的信息

---

## 十、State 状态系统

区别：

Memory：

过去发生什么。

State：

现在是什么状态。

保存：

- 当前群
- 当前用户
- 当前话题
- 当前任务
- 云团状态

### 情绪状态（Mood）

人格是气候，情绪是天气。情绪是 State 中的瞬态值，不是人格的一部分。

- 取值：枚举 `calm | happy | angry | down`（基态 calm，可扩展）
- 产生：Decision 每次裁决时基于当前上下文（会话内容 + 关系 + 记忆）顺带输出，无独立事件源、无额外模型调用
- 存储：只活在内存（重启即 calm），带时间戳，随时间衰减回 calm
- 消费端：bot_chat 语气注入（改语气不改人设）、meme 选图类别、Decision 自身倾向（同一裁决内 mood 与 action 天然一致）

二期增强：亲密度调制情绪敏感度（对高亲密的人更"不生气"）。

---

## 十一、Event System

所有变化产生事件：

例如：

```text
收到消息

用户加入群

关系变化

任务完成

Memory更新
```

统一：

```text
Event Bus（进程内 channel）
```

### 事件总线终稿（V1）

- 传输：tokio broadcast 单总线；事件为强类型枚举
- 事件清单：`MessageReceived / MessageSent / BubbleSent / ReplyInterrupted / DecisionMade / TaskCreated / TaskStepDone / TaskFinished / MemoryWritten / RelationshipEventAppended / MoodChanged / MemberJoined / MemberLeft / ConsolidationDone / ConfigReloaded / PersonalityVersionChanged`
- 落库：事件尽力写入 `events` 表（轮转保留 7 天）——Decision trace 页与任务可视化页的数据源。Q55 / ADR-0006 修订可靠性边界：消息流水必须可恢复，Decision/trace 观测事件允许过载缺失；任务业务状态不能依赖可丢失的观测事件恢复。消息恢复消费的实施状态见[信息流参考](../reference/message-flow-rules.md)（表结构见 docs/reference/data-model.md）。
- V1 订阅者四个：Prefilter 管线（订 MessageReceived）、tracer（全订，落表）、WebUI 实时推送（全订）、夜间归纳调度器（定时器驱动，不订消息事件）

---

## 十二、Context 系统（机制定稿）

三类上下文按职责组装。Bot 每轮从 SQLite 读取资料，但本轮消息范围固定为触发窗口的上界；开始处理时建立一份快照供 Decision 与 bot_chat 共用，等待期间不追读新消息。不维护跨轮可变聊天 session。此处按 [ADR-0008](../adr/0008-grounded-bot-context.md) 修订 Q26/Q27；术语见 [CONTEXT.md](../../CONTEXT.md)。

### Bot Context（聊天环境，喂 bot_chat）

回复上下文由以下部分组成：

1. **system**：人设提示词 + 当前情绪一句 + 行为准则（不暴露内部机制、不刷屏、短句风格）
2. **回复锚点与引用原文**：单独标出要回答谁、哪条消息；引用按同会话的外部消息编号解析，缺失或有歧义时明确标记。
3. **回复对象的人物简档**：称呼、技术偏好、长期项目、交流习惯等稳定资料，附本人原话出处；不把每日摘要或 Bot 的旧回答自动当成人物事实。
4. **当前话题相关记忆**：检索当前对象和当前会话的资料，附来源与时间；旧无出处记忆标注未核实。个人资料仍按 person_id 跨群归并。
5. **近期对话及在场身份**：保留本轮消息上界以内的最新 K 条对话，包含 person_id、昵称、时间、引用关系以及人/Bot 区分；身份不能靠昵称反推。

预算：Bot 默认 **40,000 字符**（按 Q62 澄清及现有配置），不是精确 token 数。先裁近期对话，再裁相关记忆与人物简档等可选资料；锚点与必要引用优先保留，原文本身超长时标记截断，必要内容仍放不下就停止回复模型调用。历史记忆与当前明确陈述冲突时核对，不虚构缺失事实。具体条数、字段及验证边界见 [Bot 上下文参考](../reference/bot-context.md)。

### Agent Context（任务环境，喂 agent_exec）

- goal + 交接语境（来自 Decision.start_task 的 task_goal）
- Working Memory：工具调用历史——**最近一步结果全量，更早的压成摘要行**；工具结果全文只落 task_events，进模型前恒经 Runtime 硬性截断（最大截断防爆上下文）
- 循环与预算见 4.2

### Decision Context（给小脑）

字段级输入契约已定稿（每字段有界、摘要优先），见 [docs/reference/data-model.md](../reference/data-model.md) 第十章。

---

## 十三、Decision 小脑

负责：

决策。

不是生成语言。

模型通过 API 接入（见十五章），与聊天生成共用同一个 LLM Provider 层，绑定便宜快速的模型角色（便宜快速指 API 计费与速度，与部署服务器规格无关）。Decision 本质是分类器，不是对话者。

输入消息先经 Runtime 本地 Prefilter 预筛（规则集见下），过滤大部分无需决策的消息，降低 API 成本与延迟；通过预筛的消息才构造 Decision Context 调用模型。主动插话与被动回复走同一条管线，无独立心跳；是否主动开口，同样由 Decision 拿着群聊上下文裁决。

### Prefilter 规则集（终稿）

Q54 设计补充：A 消息命中后先等待固定 10 秒，同时继续接收新消息，然后开始 Decision。回复绑定 A 的原始消息和身份；B 普通插话不替换回复对象，也不使给 A 的回复作废。所有通过 Prefilter 的消息都可以建立窗口；@云团、引用云团、私聊为高优先级，普通群聊为普通优先级。@/引用云团创建独立窗口，普通 B 只补充 A 的窗口。此规则优先于原 Q32 的“任意上下文变化即作废”表述，见 ADR-0007。10 秒不是 API 超时时间。

顺序短路（命中即出结果）：

| # | 规则 | 判定 | 默认 |
| --- | --- | --- | --- |
| R1 | 发送者是自己 | 丢弃 | 常开不可关（防自嗨死循环） |
| R2 | 发送者是其他 bot / 系统消息 | 丢弃 | 开 |
| R3 | @我 | 必放行（跳过后续规则） | 开 |
| R4 | 回复 / 引用我的消息 | 必放行 | 开 |
| R5 | 私聊 | 必放行（私聊几乎必回、首泡秒发；仍留给 Decision 以获得 mood / memory_write / task 分流） | 开 |
| R6 | 群聊普通消息 | 过节流闸，过了放行 | 开 |
| R7 | 无文字纯表情 / 图片（未@我） | 丢弃（偷表情包入库不受影响） | 开 |

节流闸（全部热配）：

- 回复节流：每 chat 每 60 秒最多 4 个回复回合、12 个气泡
- 成本闸：全局每分钟最多 30 次 Decision API 调用，超限排队延迟，不丢弃

输入契约：Decision Context 终稿见 [docs/reference/data-model.md](../reference/data-model.md) 第十章（不可变 anchor、10 秒窗口消息、消息、发送者关系分、场景统计、记忆提示、当前情绪、活动任务）。Runtime 固定按 anchor 路由回复，模型不决定回复对象。

窗口上下文上限：保留 anchor、窗口内全部 @/引用云团消息及最后 30 条普通消息，并受总字符预算约束；裁掉的内容仍保留在消息流水，不送入 Decision。多个窗口的 Decision 可以异步，但发送按同 chat 的窗口创建顺序排队。

职责：

- 是否回复 / 是否无视 / 是否发表情包 / 是否创建任务
- 是否@
- 回复长度
- 当前情绪
- 是否保存记忆（显式通道）

输出（当前通用 chat/completions 原型 Schema；Intern-Decision-4B 适配方式待定，见运行时设计）：

```json
{
  "action": "reply | ignore | send_meme | start_task",
  "mood": "calm | happy | angry | down",
  "mention": false,
  "reply_len": "short | medium | long",
  "meme_type": "类别标签，仅 action=send_meme 时",
  "task_goal": "一句话任务目标，仅 action=start_task 时",
  "memory_write": "提取出的显式事实字符串，无则 null",
  "reason": "一句话，只进 trace 不展示"
}
```

输出经 Schema 校验：校验失败带错误信息重试一次；再失败兜底为"不行动"并记录事件。可靠性由 Runtime 工程手段保障，不依赖模型自觉。

---

## 十四、消息行为系统

目标：

像真人。

### 回复形态（两条铁律）

1. **短句多条**：像真人一样切成多条短气泡连发（一回合 ≤3 泡，**封顶不是配额**——一句话能说完就只发一泡，绝不为分泡而分泡），不发论文式长段
2. **难题 / 长答案 → 合并转发折叠卡**：第 4 泡强制转折叠卡——详情放进 QQ 合并转发消息（"查看N条转发消息"），群里只留一句短引导；卡片内可含步骤、代码、数据

### 回复形态引擎（机制）

发送链路：

```text
bot_chat 生成 → Bubbleizer（拆泡/校验/兜底） → 发送队列（per-chat 串行，异步执行） → 适配层
```

- **输出契约**：bot_chat 定格式输出——气泡之间用 `‖` 分隔，可带指令行（`::at`、`::meme` 等），Runtime 本地解析出气泡与指令；分泡只沿自然语气断点，模型不凑数、机械兜底也不强拆单句——**不为分泡而分泡**；模型不守约定时按标点机械切分兜底，说了的话一定发得出去
- **延时模拟打字**：`delay = clamp(0.6s + 字数×40ms ±30%抖动, 0.8s, 4s)`；首泡快发（300~800ms）保证响应感；整段回复从首泡到末泡总预算 ≤8s
- **异步作废**：每泡发送前比对本 chat 最新消息版本——上下文已变动则作废剩余泡（记 trace），新消息走正常管线重新决策
- **稳定六条**：① 格式漂移 → 单泡兜底 ② per-chat 串行保序，折叠卡与普通回复同队列 ③ 单泡发送失败 → 退避重试一次，仍失败跳过该泡记事件，回复整体不失踪 ④ 节流按气泡计数（非按回复计数），每 chat 每分钟气泡数硬顶，超限并入一条 ⑤ 总时长上限 ≤8s ⑥ 延时系数、泡数上限、节流顶全部 config 热配

普通回应：

```text
@A 我看看
```

任务结果：

```text
摘要

+

文件

+

必要说明
```

避免刷屏：见上"稳定六条"第④条，短句连发按气泡计入 Prefilter 发言节流。

### 表情包（Meme）

- Meme 库：本地归档（`data/memes/`），按类别标签分类，WebUI 管理（导入 / LLM 自动分类建议 / 去重 / 待确认队列）
- 去重：md5 精确 + pHash 感知（本地底座）；DINOv3 魔搭 API 为可选增强槽位，默认关闭，挂了自动降级
- 偷表情包：开关开启后群聊图片进待确认队列，管理员一键收编
- 发表情包 = 普通图片消息，协议端原生能力，无额外协议依赖
- 流程：Decision 依据当前情景输出 `send_meme + meme_type` → Runtime 从该类别按"最久未用"加权随机抽一张发送
- 选图类别与当前情绪状态对齐
- 发表情包同样受 Prefilter 发言节流约束

---

## 十五、技术方向

### Runtime

Rust。

原因：

- 安全
- 并发
- 长时间运行
- 权限控制
- 资源占用小（2C2G 服务器可常驻）

### 模型接入（LLM Provider）

所有模型能力通过 API 接入，本地不做任何推理。

Provider 层统一抽象 OpenAI 兼容协议（chat/completions + tools），运行时消费三个模型角色：

| 角色 | 用途 | 模型要求 | 输出 |
| --- | --- | --- | --- |
| bot_chat | Bot 人格回复 | 中高档模型 | 自然语言 |
| decision | Decision 小脑 | 便宜快速模型 | 固定 JSON Schema |
| agent_exec | 任务执行与工具调用 | 强模型（支持 function calling） | tool_call / 结果整合 |

原则：

- 三角色配置独立；允许绑同一个模型（省钱时 agent_exec 与 bot_chat 绑一家合法），角色不合并
- 换模型只改配置，不改代码
- LLM API 是系统唯一的重型外部依赖（meme 的 DINOv3 为可选增强槽位，默认关闭，非关键路径）

### 平台接入（QQ 适配层）

- QQ 先行，经 NapCat 协议端对接，协议为 OneBot 11 over WebSocket（必设 token）
- NapCat 为独立容器/进程，不属于云团本体；其内存开销（300~800MB，随运行膨胀）以 swap、容器内存上限、定时重启兜底（社区标准做法）
- OneBot 端口绝不暴露公网
- 适配层只认 OneBot 11，协议端可替换；Rust 自研 QQ 协议明确不做（见 ADR-0003）
- 通讯设计定稿见 [docs/design/runtime-design.md](runtime-design.md) 第六章（正向 WS / 段数组映射铁律 / echo 回执 / 断线策略）

### 部署形态（2C2G 小鸡友好）

目标：

单进程、单二进制、零外部服务依赖；2核2G 服务器以 300MB 以内内存常驻运行（QQ 协议端另计）。

选型：

- SQLite（WAL 模式）内嵌数据库，无独立数据库服务
- 向量检索使用 sqlite-vec 扩展（随 Knowledge Base 一同可插拔，默认关闭）
- Event Bus 为进程内 channel（tokio broadcast / mpsc），不依赖 Redis / MQ
- WebUI 静态资源嵌入同一二进制
- 日志 / trace 文件轮转并设上限
- 所有运行数据集中于 `data/` 目录（库、memes、artifacts、archive、logs、backups），单卷挂载即迁移

不内置：

- 本地 LLM / 本地 embedding
- 无头浏览器（网页抓取走 HTTP + 正文解析）
- 重型代码沙箱（代码执行类工具默认关闭，后续评估远程沙箱 API）

交付：

- musl 静态编译单二进制
- install.sh + systemd unit（NapCat 侧提供 docker-compose 参考）
- config 热重载（改配置不重启）

升级到大服务器只是解锁更重能力，不是架构迁移。

### 扩展模型（三层）

1. **内置功能模块**：编译进二进制，config 开关控制启停（meme / 知识库 / 备份皆此模式，关掉零开销）
2. **能力扩展 = 新 Tool**：Tool trait + Registry 注册；为二期预留 `Tool::Remote`
3. **生态扩展（二期）= MCP 外挂工具进程**：云团作 MCP client 接第三方工具服务，进程隔离，插件挂云团不死

明确否决：Rust dylib 动态库插件（ABI 不稳）、进程内插件脚本（一个烂插件带走全 bot）。详见 [docs/design/runtime-design.md](runtime-design.md) 第五章。

### 数据库

初期：

- SQLite（WAL）
- 消息流水全量落库（messages 表含 mentions 字段），为 500 条窗口与夜间归纳供数
- 全部表结构定稿见 [docs/reference/data-model.md](../reference/data-model.md)
- sqlite-vec（向量检索，可选）

后期（可选解锁）：

- PostgreSQL
- Redis
- 独立 Vector DB
- Object Storage

备份：

- 精炼数据（人格版本、长期记忆、关系图）优先备份到独立的 GitHub 私有仓库
- 原始消息流水可选加密压缩后备份
- 备份为运维层定时任务，不在关键路径上

### WebUI

定位：MVP 一等公民，对标 AstrBot 全图形化体验，管理员全程无需 SSH。

技术：axum 提供 REST + 单条 WebSocket（`/ws` 推送事件），前端 Vue3 + Vite + Naive UI（关系网用 vis-network），rust-embed 嵌入二进制。壳布局与三页内页定调见 [docs/design/runtime-design.md](runtime-design.md) 第四章。

十一个页面（左导航四组）：

总览

1. 仪表盘 = 首页（今日收发 / Decision 成本 / 活跃任务 / 运行时长 + NapCat 心跳 + 迷你事件流）

观察

2. Decision trace 流（左实时事件流 + 右输入输出 JSON 详情 + 过滤，它为什么接 / 不接这句话）
3. 任务执行可视化（Task 工具循环逐步回放：每步 tool_call / 结果 / 耗时 / 预算消耗）
4. 记忆浏览（长期记忆 / 群档案 / 每日摘要）
5. 关系网可视化（Person 节点 + 关系边 + 亲密度，人机边一并成图）

配置

6. 平台连接（OneBot 地址 / token）
7. LLM Provider（base_url / key / 三角色绑定 + 连通性测试）
8. 运行参数（夜间归纳 / 上下文预算 / 回复形态 / 节流+成本闸 / Meme 开关，写回即热应用）
9. 人格编辑器（改提示词 + 版本时间线 + diff 对照 + 一键回滚，体验对标专业编辑器）
10. Meme 库管理（导入 / LLM 自动分类建议 / 去重 / 偷表情包开关与待确认队列）
11. 知识库管理（上传 / 解析状态，功能本体默认关闭）

安全：

- 账号密码登录（argon2 哈希存储，首次启动引导设置）
- 配置全部热应用
- API key 面板内掩码显示、只写不回读（防截图泄露）
- HTTPS 通过反代（caddy 等）在文档中指引，不内置

---

## 十六、最终定义

> 云团 = 一个由 Rust Runtime 驱动的长期运行 Agent，拥有 Bot 人格层、关系系统、长期记忆、知识库、Agent执行系统和 Decision 小脑；内部复杂运行，外部保持自然人格。

---

## 十七、第一阶段落地

1. Rust Runtime 骨架
2. AstrBot 二改迁移
3. Bot / Agent 分离
4. SQLite 数据层（messages 全量落库，表结构见 docs/reference/data-model.md）
5. Decision模型接入（Prefilter + Schema 校验）
6. Memory / Relationship基础版（双通道写入 + 夜间归纳 + 每日摘要）
7. Tool系统（只读工具起步 + 带预算的工具循环）
8. WebUI管理端（十一页 + 密码登录）

MVP 闭环：

```text
消息 → Prefilter → Decision → ┬ bot_chat 直答（+情绪 +meme，短句多条）
                              └ 最小Task（只读工具循环）→ Bot组织语言
   → 记忆/亲密度写入 → 夜间归纳（含每日摘要）→ WebUI 管理
```

---

## 十八、后续实现重点

- Runtime模块边界（已定稿：四 crate 结构 / 启动序列 + 监督树 / 并发三串行两并发一单写者，见 docs/design/runtime-design.md）
- Bot ↔ Decision ↔ Agent接口协议
- Event Bus事件模型（已定稿，见十一章）
- Context Builder（无状态滑窗、在场名册、情绪注入、Decision 输入契约组装）
- Memory Schema（已定稿，见 docs/reference/data-model.md）
- Relationship Graph（@统计驱动 + 归纳提炼）
- Task生命周期（预算模型 + 完工交接契约）
- Tool Registry（原生 function calling 优先）
- LLM Provider（三角色路由）
- Decision输入输出Schema（已定稿，见十三章 + [data-model.md](../reference/data-model.md)）
- Prefilter规则集（已定稿，见十三章）
- 情绪系统（已定稿，见十章；亲密度调制为二期）
- 消息流水存储（messages 表）
- 回复形态引擎（已定稿，见十四章：拆泡契约 / 打字延时 / 异步作废 / 发送队列 / 折叠卡）
- 每日摘要生成（群 + 人物）
- 数据备份（GitHub 私有仓）
- 关系网可视化
- 任务执行可视化
- Meme 库管线（去重 / 分类 / 偷表情包队列）
- Trace / Archive
- QQ 适配层（OneBot 11 / NapCat）
- WebUI API（axum）

---

本文作为云团 V0.1 架构基线。

后续实现围绕此架构演进。

---

## 修订记录

- 2026-10-02 V0.1：架构基线建立。
- 2026-10-02：补充模型接入设计（LLM + Decision 双角色走 Provider API，其余零外部依赖）、Decision 前置本地 Prefilter、2C2G 部署形态原则。
- 2026-10-02（拷问轮 Q1–Q20 定稿）：QQ 先行 + NapCat 独立容器（ADR-0001）；不自研 Rust QQ 协议（ADR-0003）；主动/被动统一管线；人格静态化 + 版本化（ADR-0002）；记忆双通道 + 夜间归纳 + 500 条窗口 + person_id 合并 + 敏感拒收；云团↔人亲密度；表情包机制；Task 带预算工具循环；情绪系统（ADR-0004）；消息全量落库 + GitHub 私有仓备份；WebUI 七页 + axum + 密码认证；Decision Schema 终稿。
- 2026-10-02（拷问轮 Q21–Q25 定稿）：数据模型落地为 [docs/reference/data-model.md](../reference/data-model.md)（三主体长期记忆单表、每日摘要索引、mentions 字段、@统计驱动熟悉度、人格线性版本链、meme 去重双档 + DINOv3 可选槽位、mood 免持久化、Decision 输入契约）；WebUI 扩至八页（+Meme 库管理，人格编辑器加 diff 视图）；新增偷表情包待确认队列；`data/` 统一存储布局。
- 2026-10-02（拷问轮 Q26–Q30 定稿）：上下文机制三件套——Bot Context 分层智能拼接（在场名册以 person_id 锚定身份、40k tokens 总预算、超长按层压缩）、全上下文无状态滑窗组装、Agent Working Memory 硬性截断（最近一步全量 + 历史摘要行）；模型角色扩为三（新增 agent_exec，独立配置允许同绑）；工具调用协议定 OpenAI 原生 function calling 优先、JSON-in-text 降级兜底；完工交接契约（result_summary + artifacts + 关键数据）；回复形态两条铁律（短句多条、难题走合并转发折叠卡）；WebUI 扩至九页（+任务执行可视化）。
- 2026-10-02（拷问轮 Q31–Q33 定稿）：回复形态引擎完整机制——bot_chat 格式化输出契约（`‖` 分泡 + `::` 指令行，Runtime 本地解析、机械兜底）；打字延时模型（按字数 clamp + 抖动，首泡快发，总预算 ≤8s）；发送队列 per-chat 串行异步执行 + 上下文变动作废剩余泡；3 泡封顶、第 4 泡强制转折叠卡；节流按气泡计数；稳定六条。
- 2026-10-02（拷问轮 Q34–Q37 定稿）：Prefilter 规则集终稿（R1–R7 顺序短路：自身/其他bot丢弃、@我与回复我必放行、私聊必放行几乎必回、群聊过节流闸、纯图丢弃不惊动决策）；节流放宽为每 chat 60 秒 4 回合 / 12 气泡、全局每分钟 30 次 Decision 成本闸（超限排队不丢弃，全热配）；Event Bus 终稿（16 个事件类型、全量落 events 表 7 天轮转、四类订阅者）。
- 2026-10-02（小改）：回复形态补充原则——3 泡为封顶非配额，不为分泡而分泡；分泡只沿自然语气断点，模型不凑数、兜底不强拆。
- 2026-10-02（拷问轮 Q38–Q43 定稿）：运行时设计落地为 [docs/design/runtime-design.md](runtime-design.md)（四 crate 工程结构、启动序列 + 监督树 + 优雅停机、三串行两并发一单写者、MCP 三层扩展模型）；WebUI 扩至十页（+仪表盘首页，左导航四组信息架构，技术栈定 Vue3 + Naive UI + vis-network + 单条 /ws）。
- 2026-10-02（拷问轮 Q44–Q47 定稿）：前端交付走 CI（仓库只存源码，Actions 构建 musl 单文件发 release），主题默认亮色（暗色可切）；adapter-qq 通讯设计入 [runtime-design.md](runtime-design.md) 第六章——正向 WS（云团=client）、段数组映射铁律（core 不见 CQ 码）、同一 WS 双工 echo 回执 10s 超时、断线接受丢失记事件。
- 2026-10-03：配置中心 2.0——全参数面板化（新增「运行参数」页，页面数 10→11）+ 热应用槽扩展（reply/context/consolidation/meme 换槽与定时器重建）+ 模型页连通性测试（/api/llm/test）。
- 2026-10-09：按文档用途迁移目录，统一标题层级和引用；区分设计基线、实施记录与当前代码来源。
