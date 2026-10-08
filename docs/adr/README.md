# 架构决策记录

[返回文档索引](../README.md) · [完整决策台账](../decision-log.md)

ADR 保存重要取舍与原因。`accepted` 表示接受该决定，不表示代码已完成；当时的实施状态保留为历史，当前差距见[信息流参考](../reference/message-flow-rules.md)和[活跃工作](../changes/README.md)。

| 编号 | 决定 | 关联 |
| --- | --- | --- |
| [0001](0001-napcat-as-qq-protocol-endpoint.md) | 使用 NapCat 作为 QQ 协议端 | QQ 接入与独立部署 |
| [0002](0002-static-personality-prompt.md) | 人格提示词由管理员维护并版本化 | 人格系统 |
| [0003](0003-no-rust-native-qq-protocol.md) | 不自研 Rust QQ 协议 | QQ 接入边界 |
| [0004](0004-emotion-in-state-via-decision.md) | 情绪归属 State，由 Decision 同次调用产出 | 情绪与决策 |
| 0005（预留，未建文档） | 插件设计议题尚待独立裁决 | 见台账中的未编号裁决；不补造已接受决定 |
| [0006](0006-message-and-trace-reliability.md) | 消息恢复与 trace 可靠性分开约束 | Q55，修订 Q37 |
| [0007](0007-reply-anchor-and-interjections.md) | 固定回复锚点，普通插话不取消原回复 | Q54，修订 Q32；后续澄清见台账 Q56–Q64 |

编号保持稳定，修订和取代关系遵循[治理协议](../governance.md)。

## 修订记录

- 2026-10-09：补充索引与状态解释，保留既有编号及 ADR-0005 的预留状态。
