# 历史归档

[返回文档索引](../README.md)

归档按资料生命周期分类。工作记录依据已有 `closed` 标记归档；故障复盘保留当次原因、修复和验证证据。归档不意味着其中列出的范围外事项已经完成。

## 已关闭的工作记录

| 记录 | 关闭范围 | 当前资料或实现来源 | 保留的边界 |
| --- | --- | --- | --- |
| [仪表盘资源与 Token 指标](changes/dashboard-metrics-workbench.md) | 指标采集、用量落库和页面展示 | [仪表盘实现](../../crates/webui/src/dashboard.rs)、[LLM 用量入口](../../crates/core/src/llm.rs) | 历史测试结果仅对应当时版本；非业务探测请求未纳入用量统计 |
| [Task / Agent 执行闭环](changes/task-agent-loop-workbench.md) | 当期执行循环与测试 | [总体架构第四章](../design/architecture-v0.1.md)、[信息流参考](../reference/message-flow-rules.md)、[agent.rs](../../crates/core/src/agent.rs) | 真实群聊端到端验收仍在原记录中列为后续项 |
| [信息流规则文档](changes/message-flow-rules-workbench.md) | 当时的信息流文档整理 | [持续维护的信息流参考](../reference/message-flow-rules.md) | 旧记录中的裁决编号、实现进度和下一步只代表记录当时 |
| [WebUI 主题与字体](changes/webui-theme-workbench.md) | 当期主题 token 收拢和卡片调整 | [全局样式](../../webui-frontend/src/styles.css)、[前端工作记录](../changes/frontend-design-workbench.md) | 亮色模式验证不代表其余主题场景已重新验收 |

## 故障复盘

| 记录 | 结论 | 验证边界 |
| --- | --- | --- |
| [new-api 401](incidents/new-api-401-report.md) | 脱敏占位符误作密钥；当前 provider 已修复 | 容器内模型调用通过，完整群聊收发尚未实测 |

## 查阅规则

- 原有裁决、代码反馈和测试记录作为历史证据保留；当前实现以对应代码和参考文档为准。
- 需要继续范围外工作时，先核对当前状态，再在[活跃工作](../changes/README.md)中推进；不要把旧清单当成新的自动执行指令。
- 故障复盘更正事实时保留修订记录，避免重新传播已被推翻的结论。

## 修订记录

- 2026-10-09：归档四份明确关闭的 Workbench 和一份故障复盘，补充当前入口与未验证边界。
