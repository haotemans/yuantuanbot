# 云团运行时设计 V0.1

> 对应 architecture-v0.1.md 第十五、十七章的工程落地设计：工程结构、启动序列、并发模型、WebUI 设计、扩展模型。变更走治理协议。

---

# 一、工程结构（workspace 四 crate）

```text
yuantuan/
├── Cargo.toml            # workspace
├── crates/
│   ├── core/             # 全部领域系统（纯逻辑，不碰 Web/网络框架）：
│   │                     #   bot / agent / decision / memory / tools / prefilter
│   │                     #   / context_builder / reply_engine / consolidation / state
│   ├── adapter-qq/       # OneBot 11 客户端（WS 连接、段解析、发送、重连）
│   ├── webui/            # axum API + WS 推送 + rust-embed 内嵌前端
│   └── yuantuan/         # bin：装配各系统 + main()
├── webui-frontend/       # Vue3 + Vite + Naive UI 源码（构建产物嵌入 webui crate）
├── data/                 # 运行时生成（见 data-model.md 存储布局）
└── docs/
```

原则：

- `core` 不依赖 axum、不依赖具体协议端；public API 即模块间协议
- crate 边界 = 硬边界：换 Web 框架、加平台适配都不动 core
- 功能模块以 feature 形态存在于 core 内，config 开关控制启停（关掉零开销）

---

# 二、启动序列与监督树

启动顺序（main）：

1. 加载配置（config.toml / providers.toml）
2. 初始化 `data/` 目录 → SQLite 打开 + 迁移
3. 起 Event Bus → tracer（订阅全事件落 events 表）
4. 起发送队列（per-chat）
5. 连接 NapCat（adapter-qq）
6. 起 Prefilter / Decision 管线（订阅 MessageReceived）
7. 起 WebUI（axum + /ws）
8. 起夜间归纳调度器
9. 打印"云团已上线" + 管理员初始登录指引

监督树：

- 每个长活组件一个 tokio task，挂在 supervisor 下
- 单组件 panic → 只重启该组件（退避），不拖垮进程——群里不能"人没了"
- NapCat 断线自动重连，指数退避 1s → 60s 封顶

优雅停机（SIGTERM）：停止接收消息 → 发送队列排空 → SQLite checkpoint → 退出。

---

# 三、并发与串行点

**串行点：**

- 每 chat 发送队列（保序，已定稿）
- 夜间归纳单实例锁（重跑会脏数据）

**并发与速率限制：**

- 按 chat 隔离消息处理，不同 chat 受限并发，避免一个 chat 等待模型时阻塞其他 chat（Q52，已确认、待实现）
- Decision API：全局速率限制 30 次/分（成本闸，超限排队）；每分钟调用次数不是并发数，不能用信号量数量替代
- Task 执行器：最多 3 个并发任务
- 全局 LLM 请求并发上限固定为 4（Q53，待实现）；与 Task 数量和 Decision 每分钟速率分别约束。
- WebUI 请求：axum 默认并发

Decision 输入（Q56/Q60/Q62，待实现）：每个窗口携带不可变 `anchor` 与有界 `window_messages`。窗口保留 anchor、所有 @/引用云团消息及最后 30 条普通消息，`state` 字符预算 8,000；再使用模型 tokenizer 对完整编译输入（含 chat template 和 schema）强制限制为 8,192 tokens。bot_chat 保留独立 40,000 字符预算。Decision 只能产出结构化动作等字段，Runtime 使用 anchor 的 `msg_id` 与 `sender_pid` 路由，模型不得选择或替换回复对象。

窗口调度（Q57–Q64，待实现）：所有通过 Prefilter 的消息可创建或加入 10 秒窗口；同 chat 有普通等待窗口时，普通消息加入该窗口，不重复开窗；没有等待窗口时才创建普通窗口。@云团、引用云团、私聊为高优先级；@云团和引用云团始终创建独立窗口，只引用他人按普通消息处理。高优先级窗口优先获得全局 LLM 并发槽。多个窗口可异步进行 Decision，但同 chat 必须按窗口创建顺序进入发送队列，前一窗口完成或明确失败后才能发送后一窗口。失败提示每窗口最多一次，文案为空静默，有文案发送给 anchor 请求者；单泡发送失败只记内部事件。

Decision 模型目标为 [Intern-Decision-4B](https://www.modelscope.cn/models/Shanghai_AI_Laboratory/Intern-Decision-4B)：它接收 state 与 1–16 个 choice/score/noul 问题，单次 Hugging Face 前向推理返回候选概率分布，不生成自由文本；模型卡默认 `max_length=8192` tokens，提供 Python 3.12+ 推理模块，ModelScope 页面未提供 API 推理服务。权重文件约 9.1 GB，故 2C2G Bot 主机不能承载该推理进程，需单独确定推理服务部署。现有 `LlmGateway::chat` 的 OpenAI chat/completions JSON 适配不兼容该推理契约；模型服务接入方式与 `task_goal`/`memory_write`/`reason` 自由文本字段的归属待裁决。

Q54（待实现）：A 的消息命中后开启固定 10 秒窗口，期间继续收新消息，窗口不延长，结束后开始 Decision。该窗口不是 API 超时。回复锚点固定为 A；B 普通插话不取消给 A 的回复；B 的独立 @/引用请求异步开启自己的窗口。失败文案为空则静默，有文案才向原请求者发送。

可靠性边界（Q55，ADR-0006，待实现）：消息流水必须可恢复，处理链路需支持从持久化消息恢复消费；Decision/trace 观测事件允许过载缺失，broadcast 通知不能充当唯一可靠消息队列。该裁决不改变 Q47 对 NapCat 断线期间未收到消息的容忍。

**一单写者：**

- SQLite 写操作统一走单连接写队列（WAL：多读单写，杜绝 BUSY）

---

# 四、WebUI 设计

技术栈：Vue3 + Vite + Naive UI；关系网用 vis-network；REST + 单条 WebSocket `/ws` 推送 events；构建产物 rust-embed 嵌入二进制。

壳：左导航（四组） + 顶栏（NapCat 连接灯 · 当前 mood · admin） + 内容区。

```text
总览
  仪表盘（首页）
观察
  Decision trace · 任务回放 · 记忆浏览 · 关系网
配置
  平台连接 · 模型 · 运行参数 · 人格 · Meme · 知识库
系统
  备份 / 日志
```

十一页中三页定调：

- **仪表盘**：四张数字卡（今日收发 / Decision 调用=今日成本 / 活跃任务 / 运行时长）+ NapCat 心跳线 + 最近事件迷你流——3 秒判生死
- **Decision trace**：左实时事件流（WS 滚动）+ 右详情（完整输入/输出 JSON + reason），按 chat / action 过滤——"它刚才为什么不理我"10 秒破译
- **人格编辑器**：左编辑右预览 + 底部版本时间线 + 双版本 diff 对照 + 一键回滚

响应式：桌面优先，平板不毁容，不做专门移动端。

---

# 五、扩展模型（三层）

1. **内置功能模块**：编译进二进制，config 开关（meme / 知识库 / 备份皆此模式）
2. **能力扩展 = 新 Tool**：Tool trait + Registry 注册；`Tool::Builtin`，为二期预留 `Tool::Remote`
3. **生态扩展（二期）= MCP 外挂工具进程**：云团作 MCP client 接第三方工具服务；进程隔离，插件挂云团不死

明确否决：Rust dylib 动态库插件（ABI 不稳）、进程内插件脚本（一个烂插件带走全 bot）。

---

# 六、adapter-qq 通讯设计

## WS 拓扑：正向连接

NapCat 起 WS 服务端（默认 `ws://127.0.0.1:3001`，带 token），云团作客户端主动连接：

- 重连逻辑由 supervisor 管理（指数退避 1s→60s），客户端侧天然顺手
- NapCat 配置不登记云团地址，部署少一步
- 不为事件流额外开端口

## 消息段模型（铁律）

- 只用 OneBot **段数组模式**，废弃 CQ 字符串模式
- 段数组 → 内部 `Message` 模型的映射是 adapter-qq 的**唯一职责**，core 永远接触不到原始段：
  - `text` 段 → 正文拼接
  - `at` 段 → mentions（QQ号 → person_id）
  - `reply` 段 → reply_to
  - `image` 段 → has_image + 偷表情包钩子
  - `face`（小黄脸）→ V1 忽略不存
- 发送侧反向构造段数组；换协议端时只需重写这一层映射

## 双工与回执

- 上下行复用同一条 WS：事件下行，action 上行
- 每条 action 带 echo UUID，等响应回执 **10 秒超时判失败** → 交回复形态引擎的单泡退避重试
- OneBot meta_event 心跳监测，超时触发 supervisor 重连

## 断线策略（V1）

断线窗口期消息 NapCat 不缓存，**接受丢失记事件**（`AdapterDisconnected(start, end)`，面板可见）——人也会错过消息；不补拉历史（各家 history API 实现参差，二期再议）。

---

# 修订记录

- 2026-10-03（Q52）：明确跨 chat 受限并发；区分并发限制与每分钟调用频率，修正串行/并发小节计数措辞。运行代码尚未落实 Q52。
- 2026-10-02 V0.1：定稿（拷问轮 Q38–Q43）：四 crate 工程结构、启动序列 + 监督树 + 优雅停机、三串行两并发一单写者、WebUI 技术栈与十页信息架构、三层扩展模型（二期 MCP）。
- 2026-10-02（拷问轮 Q44–Q47）：新增第六章 adapter-qq 通讯设计——正向 WS、段数组映射铁律（core 不见 CQ 码）、双工 echo 回执 10s 超时、断线接受丢失记事件；WebUI 主题默认亮色（暗色可切换）。
- 2026-10-03：前端工具链升级——vite 8.3.2、typescript 6.0.3（TS7 因 vue-tsc 未兼容其 native 接口暂缓）、vue-tsc 3.3.12、@vitejs/plugin-vue 6.x；并完成全站设计打磨一轮（theme 令牌/品牌区/仪表盘 sparkline/trace 卡片化/空态骨架屏）。
- 2026-10-03（TS7 补票条件，调研自官方源）：TS7 已 GA 但 7.0 无程序化 API；等 ① typescript@7.1 稳定版（API 落地，tracking microsoft/TypeScript#63800，预计 2026 Q4）+ ② vuejs/language-tools PR #6170 合并（vue-tsc 将由 @vue/content-mapper 取代）。两者齐即升级并迁移类型检查链路。
- 2026-10-03：配置中心 2.0——全参数面板化（新增「运行参数」页，页面清单同步十一页）+ 热应用槽扩展（reply/context/consolidation/meme 换槽与定时器重建）+ 模型页连通性测试（/api/llm/test）。
