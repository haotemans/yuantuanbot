# 以 NapCat 作为 QQ 协议端（独立容器，OneBot 11 对接）

Status: accepted

云团第一阶段落地平台为 QQ。对接方式定为 NapCat 协议端，经 OneBot 11 over WebSocket（必设 token）通信；NapCat 以独立容器/进程部署，不属于云团本体，内存开销（实测常驻 300~800MB，随运行膨胀）以 swap、容器内存上限、定时重启兜底。选它是因为其 OneBot 11 覆盖最全、维护最活跃（2026 年仍持续发版）、风控面相对最小（走官方客户端协议），且是 AstrBot 官方默认推荐，用户生态无缝。

## Considered Options

- Lagrange.OneBot：内存极优（50~150MB 真无头），但官方已于 2025-05 弃用 OneBot 模块转向 Milky 协议，在我们的 OneBot 11 约束下路线实际关闭。
- go-cqhttp / LLOneBot 等：已停止维护或生态萎缩。

## Consequences

- 2G 服务器上 NapCat + 云团本体（约 300MB）偏紧，依赖 swap 与定期重启；4G 从容。这是运营参数，不影响架构。
- OneBot 端口绝不暴露公网（2025-09 空 token 批量扫描事件教训）。
- 适配层只认 OneBot 11，协议端未来可无痛替换。
- 建议使用有一定年限的老号、控制发言频率以降低风控概率。
