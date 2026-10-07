# Plugins 插件层 Workbench

## 状态
- status: verifying（Phase 2 Skills 抽象 completed + Phase 3 MCP stdio client core + 面板 completed，待群里端到端验证 /Decision 在群里选定 invoke_skill）
- owner: hsb + kimi
- last-grill: 2026-10-07
- frontier: 群里真人发"帮我整理一下"看 Decision 是否返回 invoke_skill；Phase 4 备份范围扩大待定

## 已确认裁决

### Q-P01 插件形态
- decision: 编译期加载（Rust crate）
- rationale: 用户原话"安全，可以交给 AI 自己去实现"。类型安全 + 不泄漏 + Rust 静态分析；放弃 AstrBot 那种运行时装载脚本
- affected-docs: 本文档 + docs/data-model.md 布局
- status: confirmed

### Q-P02 插件范围
- decision: Tools + Skills + MCP 客户端三者
- rationale: 用户要求；跟 Q-B01 的"data/plugins/<name>/ 数据"布局呼应
- affected-code:
  - Tools：实现 Tool trait（ crates/core/src/tools.rs 已有）
  - Skills：待定（语义 = 提示词级别复用，类似 AstrBot 的 plugin.skill）
  - MCP：主机进程起 MCP client，子进程起 MCP server
- status: confirmed

### Q-P03 代码组织
- decision: `plugins/<name>/` 作为独立 cargo crate
- rationale: 独立 Cargo.toml，可推独立 repo，与主 crate 解耦；发布时作为 cargo workspace member 一起 build
- affected-code: workspace Cargo.toml `members += plugins/*`；build.rs conditional include
- status: confirmed

### Q-P04 面板启禁
- decision: 面板支持启禁热改（写 plugins/<name>/enabled 标记文件，build 时 conditional include）
- rationale: 用户要求
- affected-code: 新页 Plugins.vue + 后端 /api/plugins/{list,set_enabled}；基于 build.rs 能力
- status: confirmed

## Frontier（要拆分期）

### Q-P05 Tools/Skills/MCP 抽象怎么定？
- 三种形态在后端是不是共用同一个 Tool trait？还是分开三套？
- 影响面：crates/core/src/tools.rs 的设计、未来插件的接入难度
- 建议先做 Tools 形态，Skills/MCP 后续追加，不由初版定死

### Q-P06 build.rs conditional include 怎么写？
- plugins/<name>/enabled 是空文件标记，build.rs 读目录扫描决定把它作为 feature/optional dependency 编译进来
- 影响：workspace 结构、CI、独立 plugin repo 的开发体验

### Q-P07 plugins/<name>/data 怎么进 backup？
- Q-B01 决定的"data/plugins/<name>/" 需要在 backup.rs 里扫描每个插件目录并打包
- 影响：backup.rs 里 INCLUDE 数组要不要变成动态扫描

## 工程任务（分期）

### Phase 1: Tools 框架 + 插件骨架（本期）
- [ ] MOD-P01 定义 plugins/<name>/ 目录规范（Cargo.toml / src/lib.rs / enabled 标记 / data 子目录约定）
- [ ] MOD-P02 主 Cargo.toml workspace members 包含 plugins/*
- [ ] MOD-P03 写一个示范插件 `plugins/hello/`，实现 Tool trait
- [ ] MOD-P04 main.rs 启动时扫描 plugins/ 把启用的 Tool 注册到 Registry
- [ ] MOD-P05 Plugins.vue 面板：列出每个插件（name/version/enabled/usage），启禁开关 → 写 enabled 文件
- [ ] MOD-P06 面板后端 /api/plugins/list + /api/plugins/set_enabled
- [ ] MOD-P07 文档：docs/modules/plugins.md 插件开发指南

### Phase 2: Skills 抽象（本期，2026-10-07）

**本期裁决（Q-S01/S02/S03/S04，全 confirmed）**

| Q | 裁决 | 理由 |
|---|---|---|
| Q-S01 Skill 形态 | **Rust 代码 trait**（非 Markdown） | crates/core/src/skills.rs 定义 Skill trait；插件可用 Rust 代码挂钩子，能带状态 |
| Q-S02 Skill 调用 | **Decision 模型自-driven** | Decision JSON 加 `action=invoke_skill + skill_name + slots`；小脑权衡是否触发 |
| Q-S03 长文何时用 Skill | Decision 根据消息语义（"会议纪要"、"翻译"等关键词）匹配 Skill.description | 不做 vector search，先靠 description 字符串匹配 |
| Q-S04 Skill 返回 | Skill 返回 `ToolOutput`（复用）；由 Bot 决定怎么发 | 避免再发明一套输出协议 |

**工程任务**

- [x] MOD-S01 `crates/core/src/skills.rs`：Skill trait + SkillDef + SkillRegistry（9 单测通过）
- [x] MOD-S02 `crates/core/src/decision.rs`：DecisionAction::InvokeSkill + skill_name + skill_slots + build_system_prompt(catalog)；Decision 选未知 skill 自动退 reply
- [x] MOD-S03 `crates/core/src/bot.rs`：识别 invoke_skill → 查 SkillRegistry → skill.invoke(ctx, slots) → 渲染结果包 Bubble 走 reply_engine 队列
- [x] MOD-S04 示范 skill：`plugins/hello/` 提供 `meeting_notes` + `translate_zh_en`；内置 `llm_assisted_invoke`：LLM_SLOT 装配则 BotChat 加工，未装配则退为模板渲染（向下兼容）
- [x] MOD-S05 Plugins.vue 显示 skills（chip + 描述）；/api/plugins/list 返回 skills + tools + skill_catalog
- [x] MOD-S06 main.rs 装配顺序：plugin load → LlmGateway → yuantuan_plugin_hello::set_llm(gw) → PipelineDeps.skill_registry；Extras.skill_registry 暴露给 webui
- [x] TEST-S01 单测（parse_decision invoke_skill + missing fields）+ 集成（assemble_then_decide_then_invoke 走完整 4 步链路）全部通过

**公开接口、不变量**

```rust
pub struct SkillDef {
    pub name: &'static str,         // "meeting_notes"
    pub description: &'static str,  // 中文一句：注入 Decision prompt，让模型知道何时用
    pub prompt_template: String,    // {user_text} {context} 槽
    pub tools: Vec<&'static str>,   // 可选联动 Tool 名
    pub role_hint: Option<&'static str>, // 提示注入 chat role 的 system；默认 None
}

#[async_trait]
pub trait Skill: Send + Sync {
    fn def(&self) -> SkillDef;
    /// Bot 调用，把 Decision 裁决的 slots 填进 prompt_template，调 chat LLM
    async fn invoke(&self, ctx: &ToolCtx, slots: Value) -> Result<ToolOutput>;
}

pub struct SkillRegistry { /* 类似 tools::Registry，RwLock<HashMap> */ }
```

**Decision 侧改动**（关键 import:MOD-S02）
```json
{
  "action": "invoke_skill",
  "skill_name": "meeting_notes",
  "skill_slots": { "user_text": "..." },
  ...其余字段同前
}
```

### Phase 3: MCP stdio client（本期 completed）

**裁决（Q-M01 confirmed）**：stdio client 调外部 MCP server。配置在 panel 填 `command + args + env`；yuantuan 启动时 spawn 子进程 + JSON-RPC over stdio（Content-Length 头 + JSON frame）。插件 crates 只写"配置评分器"不内嵌 server binary。

**工程任务**

- [x] MOD-M01 `crates/core/src/mcp.rs`：McpServerConfig + McpConfig + McpClient（spawn + initialize handshake + notifications/initialized + tools/list + tools/call）；Content-Length 帧 read_loop 后台任务；pending oneshot 表
- [x] MOD-M02 `crates/webui/src/mcp_api.rs`：GET /api/mcp/list（合并磁盘 cfg + 运行时 clients）；POST /api/mcp/save（整体回写 config.toml 的 [mcp] 段，先备份 .bak）；POST /api/mcp/call（debug 直通）
- [x] MOD-M03 McpToolAdapter：每个 server.tools[i] 暴露为本地 Tool（stable_name = `mcp:<server>:<tool>`）；Decision/Skill 看到的还是统一 Tool，MCP 细节封装在 adapter 里（isError=true 时作为 Err 上抛）
- [x] MOD-M04 main.rs 装配：McpManager::spawn_all → 失败 warn 不阻塞；register_tools 注入 ToolRegistry；Extras.mcp_manager 暴露给 webui
- [x] MOD-M05 `webui-frontend/src/views/Mcp.vue`：server 列表卡片（name/command/args/env 编辑 + enabled 开关 + tools 展示）+ 新增/删除 + 保存全部按钮（dirty 状态突出）
- [x] 集成测试：4 单测（config roundtrip / default_enabled / summarize text/non-text）全过；浏览器后端连通验证通过（保存链路写回 config.toml OK）

### Phase 4: 备份范围扩大（下一期）
- [ ] backup.rs 动态扫描 data/plugins/*/ → 加入 tar（替换硬编码 "plugins" 占位）
- [ ] 集成测试

### Phase 5: Skills ↔ MCP 联动（待 grill）
- [ ] SkillDef.tools 字段当前只是声明，未被 invoke 链路自动调用；Phase 5 再 grill 是否要做"Skill 执行完后自动 chain 调声明的 tools"
- [ ] chain_with_tools helper 已预留（skills.rs 末尾）

## 代码反馈

### 2026-10-07 装配时发现的事
- **plugins/hello 的 Skills 想调 LLM 但 Skill trait 不该依赖 LlmGateway**：通过 OnceLock + set_llm() 后置注入解耦；Skill trait 本身保持纯净（只返回 SkillDef + accept slots）。LLM 未装配时退化为模板渲染。
- **Decision 返回 invoke_skill 但 skill 不存在时**：决策侧 defer 失败 = 自动 fallback 到 reply + reason 注解；避免黑盒。
- **Skill 输出 → 发送链路**：包 Bubble 走 reply_engine.JobKind::Bubbles，享受打字延时与作废核对，而不是裸调 send_fn。**铁律不破**：core 不碰协议端。
- **MCP spawn_and_init tools 字段**：初版用 Vec 不可变导致无法 handshake 后填充；改 RwLock<Vec<...>> 就顺了。
- **MCP stdio 在 Windows**：需要 CREATE_NO_WINDOW 标志避免 npx 弹黑窗；但不用 use std::os::windows::process::CommandExt（tokio::process::Command 自己也暴露 .creation_flags()）。
- **WeUI Extras.mcp_manager 是 Option<Arc<...>>**：测试 for_test 给 None，生产 Some；调用方需 None-check。

## 代码反馈

（暂无）
