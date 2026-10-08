# 云团文档索引

按阅读目的查找文档：先确定要了解设计目标、核对实现，还是继续一项工作。文档整理于 2026-10-09；历史记录中的验收结果只对应当时的版本。

## 从这里开始

| 需要了解什么 | 入口 | 内容边界 |
| --- | --- | --- |
| 产品目标、系统边界与总体方案 | [总体架构](design/architecture-v0.1.md) | 设计目标，包含尚未落地的能力 |
| 启动、并发、WebUI 与 QQ 通讯设计 | [运行时设计](design/runtime-design.md) | 设计约束，不以其中的历史进度描述判断完成情况 |
| 消息实际如何流转、实现有哪些缺口 | [信息流实现参考](reference/message-flow-rules.md) | 实现记录与已知差距，附代码来源和核对范围 |
| 数据布局、表结构与 Decision 输入 | [数据模型](reference/data-model.md) | V0.1 设计基线；实际迁移以 `db.rs` 为准 |
| 为什么做出某个决定 | [决策台账](decision-log.md)、[ADR 索引](adr/README.md) | 按时间保留决定、改判与被否方案 |
| 接下来还有哪些工作 | [进行中的工作](changes/README.md) | 未关闭或尚待核对的工作记录 |
| 查阅已结束的工作与故障 | [历史归档](archive/README.md) | 已关闭工作记录与故障复盘，保留原验证边界 |
| 如何维护文档与提交修改 | [开发治理协议](governance.md) | 仓库维护约定 |

## 按工程单元定位

仓库由四个应用 crate、前端和示例插件组成；workspace 成员以 [Cargo.toml](../Cargo.toml) 为准。这里只提供导航，各单元的契约在对应正文维护。

| 工程单元 | 代码入口与事实来源 | 对应文档 |
| --- | --- | --- |
| `core`：领域逻辑、消息与任务管线 | [crate](../crates/core)、[bot.rs](../crates/core/src/bot.rs)、[agent.rs](../crates/core/src/agent.rs)、[db.rs](../crates/core/src/db.rs) | [总体架构](design/architecture-v0.1.md)、[信息流参考](reference/message-flow-rules.md)、[数据模型](reference/data-model.md) |
| `adapter-qq`：OneBot 接入 | [crate](../crates/adapter-qq) | [运行时设计第六章](design/runtime-design.md)、[信息流参考](reference/message-flow-rules.md) |
| `webui`：管理 API 与事件推送 | [crate](../crates/webui)、[路由入口](../crates/webui/src/lib.rs) | [运行时设计第四章](design/runtime-design.md)、[信息流参考](reference/message-flow-rules.md) |
| `yuantuan`：配置、装配与进程生命周期 | [main.rs](../crates/yuantuan/src/main.rs)、[config.rs](../crates/yuantuan/src/config.rs) | [运行时设计](design/runtime-design.md)、[后端加固工作记录](changes/backend-hardening-workbench.md) |
| `webui-frontend`：页面、主题与交互 | [前端目录](../webui-frontend)、[路由](../webui-frontend/src/router/index.js) | [前端工作记录](changes/frontend-design-workbench.md)、[主题历史](archive/changes/webui-theme-workbench.md) |
| `plugins/hello`：示例插件 | [插件目录](../plugins/hello)、[Skill 契约](../crates/core/src/skills.rs)、[MCP 客户端](../crates/core/src/mcp.rs) | [插件工作记录](changes/plugins-layer-workbench.md) |
| 部署与运行配置 | [Compose](../deploy/compose.yml)、[部署脚本](../deploy/deploy.sh) | [项目运行入口](../README.md)、[后端部署记录](changes/backend-hardening-workbench.md)、[401 故障复盘](archive/incidents/new-api-401-report.md) |

## 目录与资料归属

```text
docs/
├── README.md              文档总入口、归属与阅读方式
├── governance.md          跨文档的维护约定
├── decision-log.md        按时间追加的裁决台账
├── design/                总体架构与运行时设计目标
├── reference/             数据基线、实现参考与已知差距
├── adr/                   重要取舍，保留既有编号
├── changes/               进行中或尚待核对的工作记录
└── archive/
    ├── changes/           明确标记 closed 的历史工作记录
    └── incidents/         已定位故障的复盘
```

同一结论在一个位置维护，其他页面引用它。设计文档回答“应该怎样”，实现参考回答“代码目前做到了什么”，Workbench 记录推进过程和待办，ADR 与台账解释决定的来由。

## 如何理解状态

- ADR 的 `accepted` 表示设计决定已接受，不代表实现或验收完成。
- Workbench 的状态沿用原记录；本次仅将明确 `closed` 的四份记录归档，未据文件存在或编译通过推断其他工作已完成。
- 归档不抹掉剩余事项。任务执行记录中的真实群聊验收、401 复盘中的完整收发验证等仍保留在原文，并由归档索引提示。
- 数据模型保留 V0.1 设计基线，后续数据库迁移以代码为事实来源；设计与实现有差距时必须显式记录，不能通过改写设计宣称修复。
- 实现参考中的代码核对不等于运行测试。重新验收时，应记录所用版本、输入和验证范围。

## 维护与修订

新增内容先选择已有归属，不为每个文件创建独立模块说明或工作台。变更路径时同步更新 Markdown 链接、README 导航和代码中的文档引用，遵循[治理协议](governance.md)。

| 日期 | 内容 |
| --- | --- |
| 2026-10-09 | 按知识用途整理全部文档，建立工程入口与权威来源映射；分离四份已关闭工作记录和故障复盘，保留未决工作与历史裁决 |
