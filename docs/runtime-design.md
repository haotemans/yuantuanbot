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

**三串行：**

- 每 chat 发送队列（保序，已定稿）
- 夜间归纳单实例锁（重跑会脏数据）

**两并发：**

- Decision API：全局信号量 30 次/分（成本闸，超限排队）
- Task 执行器：最多 3 个并发任务
- WebUI 请求：axum 默认并发

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
  平台连接 · 模型 · 人格 · Meme · 知识库
系统
  备份 / 日志
```

十页中三页定调：

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

# 修订记录

- 2026-10-02 V0.1：定稿（拷问轮 Q38–Q43）：四 crate 工程结构、启动序列 + 监督树 + 优雅停机、三串行两并发一单写者、WebUI 技术栈与十页信息架构、三层扩展模型（二期 MCP）。
