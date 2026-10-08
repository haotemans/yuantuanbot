# 进行中的工作

[返回文档索引](../README.md)

这里保留未关闭或验收信息尚不充分的 Workbench。下表采用原记录的状态，不把实现片段或历史测试记录视为整项工作的完成证明。

| 工作记录 | 原记录状态 | 继续前需要核对 | 正式资料与实现来源 |
| --- | --- | --- | --- |
| [后端加固](backend-hardening-workbench.md) | `implementing → verifying`，包含多轮落地记录 | 顶部待办与后续反馈不同步；核对窗口连续处理、优先级、回复锚点及停机边界 | [运行时设计](../design/runtime-design.md)、[信息流参考](../reference/message-flow-rules.md) |
| [备份页](backup-page-workbench.md) | `decided` | 已有备份实现，但原记录缺少收口反馈；需要核对范围和验收证据 | [备份实现](../../crates/core/src/backup.rs)、[管理 API](../../crates/webui/src/backup_api.rs)、[数据模型](../reference/data-model.md) |
| [前端设计](frontend-design-workbench.md) | `implementing` | 页面验收、多媒体后续阶段与旧待办是否仍适用 | [运行时设计第四章](../design/runtime-design.md)、[前端代码](../../webui-frontend) |
| [插件层](plugins-layer-workbench.md) | `verifying` | 群聊端到端验证、插件数据备份、Skill 与 MCP 联动；开发指南仍在待办中 | [插件示例](../../plugins/hello)、[Skill 契约](../../crates/core/src/skills.rs)、[MCP 客户端](../../crates/core/src/mcp.rs) |

## 继续与收口

1. 先读取记录中的来源、已确认决定和最新反馈，再对照当前代码核对待办；前期的“暂无实现”不代表现在仍为空。
2. 有效契约维护在设计或参考文档中，Workbench 通过链接引用；同一编号发生改判时保留前后记录并标明关系。
3. 完成当前范围的验收后再标记 `closed`，把记录移入[历史归档](../archive/README.md)，同步入口与引用。

已关闭的仪表盘、主题、任务执行和信息流文档工作见[归档索引](../archive/README.md)。

## 修订记录

- 2026-10-09：建立活跃工作索引，区分原记录状态、待核对事项和正式来源。
