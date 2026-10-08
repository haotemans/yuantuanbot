# 不自研 Rust 原生 QQ 协议

[文档索引](../README.md) · [ADR 索引](README.md) · [决策台账](../decision-log.md)

> 决策记录：保留决定形成时的背景与实施状态；`accepted` 不表示实现已完成。后续实施差距见[信息流参考](../reference/message-flow-rules.md)。

Status: accepted

曾有提议用 Rust 重写 NapCat 以消除其内存开销与外部依赖，决议不做。NapCat 并非协议实现而是挂载官方 NTQQ 客户端；"Rust 重写"的真实含义是逆向整个 NTQQ 协议栈（Lagrange.Core 量级：专职团队、数年、数千 commit），且 QQ 协议维护是永久跑步机——go-cqhttp、ricq/rs-qq 均死于追随腾讯升级的消耗战。云团的生死不应挂在"追不追得上腾讯"上；省下的 300~800MB 内存由 swap + 定时重启零研发成本兜底。

## Considered Options

- 自研 Rust QQ 协议库：见上，放弃。保留为远期独立开源项目的备案选项，有自身节奏，不绑在云团的发布线上。

## Consequences

- 平台对接风险集中于第三方协议端的存续，以适配层只认 OneBot 11 来缓解（协议端可替换）。
- 部署内存预算中 QQ 协议端单独列支（300~800MB）。
