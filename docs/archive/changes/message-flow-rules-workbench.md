# 信息流传递规则文档 Workbench

[文档索引](../../README.md) · [归档索引](../README.md)

> 历史记录：以下背景、测试与剩余事项对应原记录日期。归档仅改变资料归属，不扩展当时的验收范围。

## 状态

- status: closed
- owner: hsb + kimi
- last-grill: 2026-10-07

## 裁决

### Q-F01 本次范围

- decision: 只写文档对齐现状，不动代码。[runtime-design.md](../../design/runtime-design.md) 中 Q52-Q65 设计裁决保持"待实现"状态。
- status: confirmed

## 产出

- [docs/reference/message-flow-rules.md](../../reference/message-flow-rules.md)：现状信息流规则 27 条，按 8 个环节（事件总线 / 消息入口 / 处理管线 / 回复发送 / 任务执行 / 记忆归纳 / 配置热应用 / 观测追踪）组织。每条标注 ✅/⚠️/❌ 实现状态。
- 附设计裁决待实现清单：Q52-Q65 与现状的对照表。

## 代码反馈

- EventBus 容量是 1024 不是文档初稿写的 64（EventBus::default() → Self::new(1024)）
- Event::TaskCreated/TaskFinished 已在 agent 闭环中携带 TaskLifecyclePayload，不再是裸变体
- reply_engine 的 per-chat 发送队列保序已落地（Q64 ✅），与 runtime-design 标注一致

## 下一步 frontier

- Q52-Q65 实现排期：需要用户另行 grill 决定优先级
- Q65（Intern-Decision-4B MCP decide）需要 Python 推理环境 + xGPU 资源，前置条件未满足
