# 前端设计 Workbench

[文档索引](../README.md) · [活跃工作](README.md)

> 工作记录：保留各轮的决定、实施反馈和验证证据。前期“现状”与待办只对应记录当时；继续工作前核对最新反馈和代码，不把旧状态当作当前验收。

## 状态

- status: implementing (MOD-022 a/b/c/d/e-partial 已落地编译通过；待补中文检测/真 LLM 优化/adapter 接入/admin pid 列表)
- owner: hsb + kimi
- last-grill: 2026-10-06
- frontier: 命令链路投产前 4 个收尾项；之后进 MOD-022g WebUI 配置页

## 事实与来源

项目事实（已核对）：

- 技术栈：Vue 3.5 + Vite 8 + Naive UI 2.45 + pinia + vue-router 5 + axios + vis-network；`vue-tsc --noEmit` 作为 build 前置（`webui-frontend/package.json:8-9`）
- 构建产物直写 `crates/webui/static/` 并入库；缺失时内嵌 legacy 占位页兜底（`README.md:44`）
- dev server 已配 proxy：`/api`、`/healthz`、`/ws` → `127.0.0.1:8085`（`webui-frontend/vite.config.js:7-14`）
- 已实现 13 个路由页（`webui-frontend/src/router/index.js:5-19`）
- 壳布局：顶栏 54px + 侧栏 196px 可折叠 + 内容区（`webui-frontend/src/App.vue:6-50`）
- 设计令牌：`theme.js`（Naive overrides）+ `styles.css`（`:root` / `html.dark` 双套变量），亮暗双主题已落地
- 基础设施：axios 401 拦截、单条 `/ws` 3s 重连、`.yt-page` 限宽、`.yt-toolbar`、EmptyState、JsonView
- 文档定调（[docs/design/runtime-design.md:106-112](../design/runtime-design.md)）：仪表盘"3 秒判生死"；trace"10 秒破译"；人格编辑器"对标专业编辑器"；桌面优先、平板不毁容、不做专门移动端
- 雏形页：Backup.vue 13 行、Kb.vue 16 行；最重页 Params.vue 178 行
- 前端无测试设施；项目测试惯例仅在 Rust 侧

## 已确认裁决

### Q001 设计主题与范围

- decision: 四个方向全做 —— 整体视觉刷新 + 特定页面深化 + 交互动效升级 + 组件体系化
- rationale: 用户全选；四者均可落到同一套令牌与组件基座，逐页推进时收益复用
- evidence: AskUserQuestion 2026-10-06
- affected-docs: 本 workbench；收口时回写 [docs/design/runtime-design.md](../design/runtime-design.md) 第四章与 [docs/decision-log.md](../decision-log.md)
- affected-code: `webui-frontend/src/**`
- tests: 无前端测试设施；验收 = vue-tsc typecheck + 浏览器每页人工点检（用户）
- status: confirmed

### Q002 调试与验收方式

- decision: vite dev server + devtools MCP 实时调试；我（kimi）驱动浏览器看真实渲染，用户（hsb）目检确认
- rationale: 迭代最快；proxy 已就绪；产物构建嵌入留待最终收口一次性做
- evidence: AskUserQuestion 2026-10-06；`vite.config.js:7-14`
- affected-docs: 本 workbench
- affected-code: none（工作流裁决，不改代码）
- tests: none
- status: confirmed

### Q003 接入路径协议支持范围

- decision: 方案 A —— Provider 卡片加协议类型下拉（`openai_compat` 默认 / `anthropic_messages`），后端 LLM 抽象层按协议分派请求格式
- rationale: 用户原话"兼容 openai chat response 和 anthropic 的接口就行，自己添加"——双协议 + 用户自填 provider，不做品牌预设
- evidence: 用户对话 2026-10-06（"A可以 然后就是 agent的tools的调用了"）
- affected-docs: 本 workbench；实现时回写 [docs/design/runtime-design.md](../design/runtime-design.md) LLM 章 + [docs/decision-log.md](../decision-log.md) Q53+
- affected-code: `webui-frontend/src/views/Models.vue` + `webui-frontend/src/components/ProviderCard.vue` + `crates/core/src/llm.rs`（协议分派）+ `crates/webui/src/config_api.rs`（schema 透传）+ `providers.toml`（新增 `protocol` 字段）
- tests: 已有 mock 测试应继续通过；需加 anthropic_messages 协议分支单测 + pages_api 集成测试
- status: confirmed

### Q005 Dashboard 实时化

- decision: WS 事件驱动 + HTTP 60s 校准混合模式
- rationale: 用户原话"实时刷新"；完全 WS 推送怕漏事件漂移，完全轮询延迟高，混合模式既实时又健壮
- evidence: 用户对话 2026-10-06
- affected-docs: 本 workbench
- affected-code: `webui-frontend/src/views/Dashboard.vue`（订阅 MessageReceived/BubbleSent/DecisionMade/TaskCreated/TaskFinished 增量更新卡片 + 事件流前插 + 小时桶同步）+ LIVE 标记
- tests: 浏览器手测（WS 连接状态下发消息观察卡片计数）
- status: confirmed

### Q006 OpenAI "generate" 接口兼容（待用户澄清）

- decision: 待用户回答——指 OpenAI legacy `/v1/completions`？第三方代理 `/generate`？还是新 `/v1/responses`？
- rationale: 三种都叫 "generate"，行为差异巨大，不能盲猜
- evidence: 用户对话 2026-10-06
- affected-docs: 本 workbench（待澄清后回写）
- affected-code: 视答案（可能动 `crates/core/src/llm.rs` endpoint 拼接）
- tests: TBD
- status: proposed

### Q007 生图功能实现路径

- decision: 方案 B —— 协议抽象 + 多 provider（不只 NAI），Tool trait + MediaProvider trait 双层架构
- rationale: 用户原话 Q007=B；AstrBot 已验证多 provider 模型可行，协议知识可移植；Tool trait 已在架构文档定调
- evidence: 用户对话 2026-10-06（"q007B q008 AC"）
- affected-docs: workbench 本文件 + [docs/design/runtime-design.md](../design/runtime-design.md) 第五章扩展模型（Tool 落地） + [docs/reference/data-model.md](../reference/data-model.md)（新增 media_providers/media_models/media_tasks 表） + [docs/decision-log.md](../decision-log.md)
- affected-code: `crates/core/src/tools.rs`（trait+registry）+ `crates/core/src/tools/media/`（provider 适配器：nai / openai_image / gemini / xai / volcengine / jimeng / openai_chat）+ `crates/core/src/agent.rs`（agent 执行循环）+ `crates/webui/`（media provider/model 配置 API + 任务可视化）+ `data/`（media 任务表 + 结果存储）
- tests: `crates/core/tests/tools_media.rs` + pages_api.rs 扩展 + 各 provider mock 测试
- status: confirmed
- scope-warning: 大工程，按子阶段拆分（见工程任务 MOD-022 系列）

### Q008 生图触发方式

- decision: 双轨 —— A（Decision 路由 start_task → agent → tool）+ C（显式命令 `/image` `/画` 直派）
- rationale: 用户原话 Q008=AC；显式命令保稳定可靠，自然语言路径体现 Agent 感
- evidence: 用户对话 2026-10-06
- affected-code: `crates/core/src/prefilter.rs`（命令前缀优先拦截，跳过后续规则）+ `crates/core/src/bot.rs`（命令直派分支）
- tests: 集成测试两种触发路径
- status: confirmed

### Q009 生图配额/权限/余额

- decision: 全套（C）—— 每日上限 + 余额扣费 + admin_only/everyone 权限
- rationale: 用户原话 Q009=C；按 AstrBot 模型全量移植：模型可配每日上限，任务入队按"单价 × 数量"原子扣余额，失败不退；每模型 admin_only/everyone 权限受全局开关约束
- evidence: 用户对话 2026-10-06
- affected-code: `data-model.md` 新增 `media_credits` 表（用户余额）+ `media_models.daily_quota` 字段；命令处理处两次校验（入队扣费 + 完成出账）
- tests: 配额满拒新任务；余额不足拒入队；admin_only + 非 admin → 拒
- status: confirmed

### Q010 提示词优化

- decision: 要（A）—— 发生图前先经 prompt optimizer LLM 改写
- rationale: 用户原话"要"；按 AstrBot 模型对 NAI 用"英文动作句 + Danbooru tag + NovelAI 权重语法"，对 anima/a1111/natural 各自一套；优化失败/超时自动 fallback 用户原文
- evidence: 用户对话 2026-10-06
- affected-code: `crates/core/src/tools/media/prompt.rs`（新增）+ 模型表加 `prompt_style` 字段 + 全局默认"优化模型"配置（指向 bot_chat 或独立）
- tests: mock optimizer 返回空 → 用原文；mock optimizer 超时 → 不阻塞队列
- status: confirmed

### Q011 生图 WebUI 配置

- decision: 像 AstrBot 一样做一份完整的"媒体生成"管理 UI，包含 provider CRUD / 模型与别名 / 配额 / 权限 / 任务列表 / 测试连通
- rationale: 用户原话 Q011；"面板化管理"是 WebUI 一等公民哲学（架构文档十五章），媒体生成不例外
- evidence: 用户对话 2026-10-06
- affected-code: `webui-frontend/src/views/Media.vue`（新增，参考 Models.vue 结构：provider 卡 + 模型绑定 + 配额）+ 后端 `/api/media/providers` `/api/media/models` `/api/media/tasks` REST + 路由 + 侧栏入口
- tests: pages_api.rs 扩展（media provider 增删改查 / api_key 掩码 / 任务列表分页）
- status: confirmed

### Q012 生图落地 NapCat 适配

- decision: 复用 meme 管线 —— 图片落盘 `data/artifacts/media/`，发图时构造 `{type:"image", data:{file: file_url(abs_path)}}` 段（沿用 `meme.rs:196` 的 `file_url()`）；传输路径 = OneBot image 段 file:// 本机路径；**贯穿所有 media 调用路径**（命令直派 / Decision 路由 / 图生图均以 image 段发出）
- rationale: 用户原话"顺便适配 napcat"和"对 记得兼容 napcat"（2026-10-06 重申）；NapCat 与 bot 同机部署，file:// 最快最稳，避免 base64 大帧；`meme.rs` 已验证可行
- evidence: 用户对话两次确认；`crates/core/src/meme.rs:196-205`（file_url 实现）；`crates/core/src/reply_engine.rs:346`（image 段构造先例）；`crates/adapter-qq/src/client.rs:284-298`（send_group_msg/send_private_msg 已就绪）
- affected-code: `crates/core/src/tools/media/mod.rs`（落盘 + 段构造）+ `crates/core/src/bot.rs`（命令完成后调用 `send_fn` 把段发出去）+ adapter-qq 现有 send_fn 链路（无需改）
- tests: NAI 返回 b64 → 落盘 → segments 含 image type + file:// 前缀 → NapCat send_fn 收到（沿用现有 send 测试模式）
- status: confirmed
- 关键约束：图片以 **file:// 绝对路径** 通过 OneBot image 段交付 NapCat；不走 base64 inline 段，不进 send_json 大包

### Q013 /image 命令语法（早期记录，后续修订见下）

- decision: 兼容 AstrBot 风格 —— `/image -m <alias> -r <ratio> [-n <count>] [-q <quality>] [--seed N] [--negative TEXT] [--] <prompt>`；别名 `/画` 等价。长 prompt 中遇到 `--` 后所有内容视为提示词（不再解析选项）
- rationale: 用户原话"支持 /image -m nai4.5 -r 1:1 提示词这样子吗"——是，照搬 AstrBot 命令风格降低用户学习成本
- evidence: 用户对话 2026-10-06；AstrBot README 命令格式
- affected-code: `crates/core/src/tools/media/parser.rs`（新增；shlex 风格 tokenize + 选项表 + `--` 终止符 + `-m` 缺省时回退模型默认）
- affected-code-secondary: `data/media_models.alias` 列作为 `-m` 入参解析目标
- tests: parser 单测（含 `--` 终止 / 缺省模型 / 引号内 prompt / 负数 / 中文 prompt）
- status: confirmed

### Q013 /image 命令语法（后续修订）

- decision: 兼容 AstrBot 风格 —— `/image -m <alias> -r <ratio> [-n <count>] [-q <quality>] [--seed N] [--negative TEXT] [--] <prompt>`；别名 `/画` 等价。`--` 后所有内容视为 prompt 原文（不再解析选项）。**`-m` 与 `-r` 均必填**，无降级默认值
- rationale: 用户原话"支持 /image -m nai4.5 -r 1:1 提示词这样子吗"——是，照搬 AstrBot 命令风格降低学习成本；必填模型/比例避免歧义
- evidence: 用户对话 2026-10-06；AstrBot README 命令格式
- affected-code: `crates/core/src/tools/media/parser.rs`（新增；shlex 风格 tokenize + 选项表 + `--` 终止符）
- tests: parser 单测（含 `--` 终止 / 缺省必填报错 / 引号内 prompt / 负数 / 中文 prompt / 缺值报错）
- status: confirmed

### Q016 NAI 双接口兼容（rinko.ai 中转站）—— 修订 NAI 适配器目标

- decision: NAI provider 同时支持 rinko.ai 暴露的 **两种** endpoint：
  - **A. NAI 原生** `POST /v1/nai/generate-image`：payload `{input, model, action:"generate", parameters:{width, height, steps, n_samples, sampler, scale, seed, negative_prompt, noise_schedule, qualityToggle, ucPreset, params_version}}`；响应 `{images:[b64], image_format}`
  - **B. OpenAI 兼容** `POST /v1/images/generations`：payload `{model, prompt, size:"832x1216", n, response_format:"b64_json"}`；响应 `{data:[{b64_json}]}`（OpenAI 标准图片响应）
- rationale: 用户两份代码样例（2026-10-06）+ 原话"双接口兼容 nai 和 openai"——同一家 key（NAI_API_KEY）在两种端点都能用；A 走 NAI 私域协议支持完整参数（seed/sampler/scale/negative），B 与 gpt-image-2 等 OpenAI 生态工具互换使用同一 shape
- evidence: 用户对话两段 Python requests 样例 + 原话
- affected-code: `crates/core/src/tools/media/nai.rs` 实现 `endpoint_style: nai_native | openai_compat` 二选一；`MediaProvider trait` 需要可声明"本 provider 支持哪些 endpoint_style"
- affected-schema: `media_providers.endpoint_style` 字段，可选值 `nai_native | openai_compat`，默认按 provider 类型自动选择
- tests: mock 两种 endpoint 各自一组测试；payload shape 严格匹配样例
- status: confirmed
- 副产：openai_compat 模式 = 与 gpt-image-2 完全同构 → MOD-022h 适配 openai_image 时直接复用
- 默认选择：模型可选择 endpoint_style；不显式时 NAI 走 `nai_native`，OpenAI 系走 `openai_compat`

### Q018 默认质量标签与负面词

- decision: 精选精简配方 —— 质量标签 `masterpiece, best quality, year 2025, highres`（4 个高信号）；负面词 `lowres, bad anatomy, bad hands, missing fingers, extra digits, worst quality, jpeg artifacts, watermark`（8 个高信号）；质量标签自动注入 prompt 头部（用户已含任一标签则跳过，避免重复）
- rationale: 用户原话"year2025之类的，还有负面质量词，但是不要太多了"——砍 AstrBot 17 个负面词至 8 个；质量标签 4 个避免噪声
- evidence: 用户对话 2026-10-06 截图（NAI Diffusion V4.5 full + 28 步 + k_euler_ancestral + karras + 4.5 scale + qualityToggle ✓）
- affected-code: `tools/media/params.rs`（NAI_QUALITY_TAGS / NAI_NEGATIVE_PROMPT / with_quality_tags 函数）+ `tools/media/provider/nai.rs`（nai_native 调用时注入）
- tests: 两个 cargo test 验证（注入逻辑 + 防重复）
- status: confirmed
- 注：openai_compat endpoint **不注入质量标签**（OpenAI 标准接口不期望 Danbooru tag 语法）

### Q017 其它生图 provider 参考标杆

- decision: 参考 **gpt-image-2**（OpenAI 图片接口 `/v1/images/generations` 标准）与 **nanobanana**（gemini-2.0 flash image preview 等；Google 生图接口）作为 OpenAI/Gemini 协议适配器的真实样例
- rationale: 用户原话"参考 gpt-image2 和 nanobanana"
- evidence: 用户对话 2026-10-06
- affected-code: MOD-022h 阶段适配 openai_image 与 gemini 时按真实接口实现，不依赖 AstrBot 移植版
- status: confirmed

### Q014 --seed 取值策略

- decision: 用户显式给定则使用之；否则**复用 chat 内上一个成功任务的 seed**（锁定可复现）；最早无记录时由 provider 默认随机
- rationale: 用户原话"--seed 42 有些固定就行了"——意图是"少改动也能出相似图"：固定 chat 内最近一次 seed，复现性优先
- evidence: 用户对话 2026-10-06
- affected-code: `data/chats.last_media_seed` 字段（热更新）+ parser 缺省逻辑回查 last_media_seed
- tests: 显式 --seed → 透传；不显式 + 上次 seed=42 → 使用 42；不显式 + 无历史 → 缺省（种子字段省略）
- status: confirmed
- 仅 nai_native endpoint 支持 seed 字段；openai_compat endpoint 不支持 seed（OpenAI 标准无此参数）→ 用户显式给 --seed + 模型走 openai_compat 时**警告但继续**（记 trace）

### Q015 命令解析失败的错误反馈

- decision: 严格的"格式不对则提示报错"——任何选项缺值 / 值非法 / 必填缺失，**不入队**，回复一段统一格式的 usage 提示（含错因定位）
- rationale: 用户原话"格式不对则提示报错"——快速失败优于暗中降级
- evidence: 用户对话 2026-10-06
- affected-code: `tools/media/parser.rs` 返回 `Result<ParsedCmd, ParseError>`；错误映射到统一文案
- tests: 每个错误分支期望精确文案匹配
- status: confirmed

### 错误文案模板

```text
❌ /image 命令格式错误

原因：{具体错因}

用法：/image -m <模型别名> -r <比例> [选项] <提示词>
  -m   模型别名，必填（可用：nai4.5, s5p, s5l, ... 见 /genhelp）
  -r   比例，必填（如 1:1 / 16:9 / 9:16 / 2:3 / 3:2 / 21:9）
  -n   数量，可选，默认 1
  -q   质量档位（按模型支持，如 1K/2K/4K）
  --seed <int>        随机种子（不显式则沿用本群上次 seed）
  --negative <text>   负面提示词
  --                  之后所有视为 prompt 原文

示例：/image -m nai4.5 -r 1:1 一只蓝眼睛的猫
```

具体错因映射：

| 触发 | 错因文案 |
| --- | --- |
| `-m` 缺值 | `-m 缺少模型别名` |
| `-m xxx` 未注册 | `模型别名 "xxx" 未注册` |
| `-r` 缺值或非法比例 | `-r 比例非法：` 后跟用户文本 |
| `-r xxx` 该模型不支持 | `模型 "xxx" 不支持比例 "yyy"` |
| `-n` 非正整数 | `-n 数量必须为正整数` |
| 无任何 prompt（也无 `--`） | `缺少提示词` |
| 未知选项 `--foo` | `未知选项 "--foo"` |
| 余额不足 | `余额不足：本次需 {cost}，当前 {balance}` |
| 每日配额满 | `本模型今日配额已用完（{used}/{quota}）` |
| admin_only 限权 | `该模型仅管理员可用` |

## 工程任务

- [x] DOC-001：workbench 建立并持续回写（本文件）
- [x] MOD-001：视觉基座——tokens 升级（yt-ink 三级/soft-bg/ring）+ focus-visible + 页头族（.yt-page-head/title/sub）
- [x] MOD-002：Dashboard 标杆——CountUp 数字滚动 + YtSkeleton 首屏骨架 + TransitionGroup 事件流入场
- [x] MOD-010：Models 页重做——三角色概览条 + 详细绑定卡（hover/动效/连通 pill）+ Provider 卡片含 usage 角标
- [x] MOD-016：Login 页现代化——hero 渐变流动/光斑/网格 + 表单错峰入场 + 错误抖动 + 输入切换 transition
- [x] MOD-017：品牌资源——mascot.png（裁黑边/圆形 512px）替换顶栏与 Login logo + favicon
- [x] MOD-019：Dashboard 实时化（Q005）——WS 事件驱动卡片增量 + 60s HTTP 校准 + LIVE 标记
- [ ] MOD-003~013 剩余：trace/tasks/memories/relations/platform/params/personality/meme/kb/backup
- [ ] MOD-014：交互与动效包深化（路由过场基调已定，待各页铺开）
- [ ] MOD-015：通用组件固化（StatCard / Toolbar / Timeline / 三态包装）
- [ ] MOD-018：Q003 落地 —— Provider 卡片加 protocol 下拉（openai_compat / anthropic_messages）+ 后端 LLM 层协议分派 + providers.toml schema 扩展
- [ ] MOD-020：Q006 待定（等用户澄清 generate 含义）
- [ ] MOD-021：Q004 待启动（Agent Tools 分阶段：P0 messages/tools 通道 → P1 search/web_read → P2 完工交接）

### Q007 落地子任务（MOD-022 系列，按序推进）

- [x] MOD-022a：tools 骨架 —— `crates/core/src/tools.rs` 定义 `Tool` trait + `Registry`
- [x] MOD-022b：media 数据模型 —— 4 张表 + V2 migration（`db.rs`），`media_providers / media_models / media_tasks / media_credits`；`chats.last_media_seed` 走 `state_kv` 通道
- [x] MOD-022c：MediaProvider 抽象 —— `tools/media/provider/mod.rs`（trait + EndpointStyle 四枚举）
- [x] MOD-022d：NAI 适配器（Q016）—— `provider/nai.rs`：双接口（`/v1/nai/generate-image` + `/v1/images/generations`）已按 Q016 真实样例实现
- [x] MOD-022e：命令直派 —— 全链路完成（中文检测 + LLM 真优化 + admin 列表 + NapCat 装配）
- [x] MOD-022i：admin QQ 列表 —— config.toml [media].admin_qq 读入 + pid 匹配回调注入 MediaCtx
- [ ] MOD-022f：Decision 路由 —— Decision Schema 扩展 `start_task` 支持 media 类型 task_goal → agent 调 tool（同 e 走一份执行器）
- [ ] MOD-022g：WebUI 配置页（Q011）—— `views/Media.vue` 新增 + 后端 REST `/api/media/providers|models|tasks|credits` + 侧栏入口 + 路由
- [ ] MOD-022h：其它 provider 适配 —— openai_image / gemini / xai / volcengine / jimeng / openai_chat（按用户使用需求逐个加）

### 前端剩余页面（MOD-003~013）

- [ ] trace / tasks / memories / relations / platform / params / personality / meme / kb / backup
- [ ] DOC-002：收口时回写 [runtime-design.md](../design/runtime-design.md) 第四章 + [decision-log.md](../decision-log.md)

## 代码反馈

（待实现与调试时记录）

## 同步账本

| 裁决 | 文档 | 代码 | 测试/测量 | 证据 | 状态 |
| --- | --- | --- | --- | --- | --- |
| Q001 范围 | 本 workbench | 待改 | 无测试设施 | 用户四选 | confirmed |
| Q002 调试流 | 本 workbench | none | — | 用户选择 + vite.config.js | confirmed |

## 下一轮 frontier

1. 标杆页验收（壳 + Dashboard 先看，用户确认基线风格）
2. 逐页过招顺序：按"对用户价值"排 —— trace → personality → models/params → 观察组其余 → 配置组其余 → 雏形页 → login
3. 每页过招节奏：改完 → 浏览器截图 → 用户 OK/不 OK → 下一页
