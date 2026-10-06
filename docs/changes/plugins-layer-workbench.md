# Plugins 插件层 Workbench

## 状态
- status: decided（裁决全 confirmed，进入分期实现）
- owner: hsb + kimi
- last-grill: 2026-10-06
- frontier: 分期拆分

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

### Phase 2: Skills 抽象（下一期）
- [ ] 定义 Skill = 预定义提示词模板 + LLM role，跟 Tool 一起被 Decision 调用
- [ ] plugins/<name>/ 可同时提供多 Skill（声明式 YAML/TOML）
- [ ] Skills.vue 面板（同 Plugins 模板）

### Phase 3: MCP 客户端（再下一期）
- [ ] 实现 MCP client（stdio），yuantuan 调外部 MCP server
- [ ] plugins/<name>/mcp-server/ 目录的 binary 被识别并 spawn
- [ ] MCPs.vue 面板

### Phase 4: 备份范围扩大
- [ ] backup.rs 动态扫描 data/plugins/*/ → 加入 tar（替换硬编码 "plugins" 占位）
- [ ] 集成测试

## 代码反馈

（暂无）
