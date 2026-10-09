# 云团（yuantuanbot）

[![CI](https://github.com/haotemans/yuantuanbot/actions/workflows/ci.yml/badge.svg)](https://github.com/haotemans/yuantuanbot/actions/workflows/ci.yml)

长期住在 QQ 群里的 Rust 机器人：Decision 小脑 + 人格回复 + 夜间归纳 + 表情管线 + 十页管理面板，目标是 2C2G 小鸡服务器低占用常驻（SQLite 内嵌、单二进制、协议端走 NapCat/OneBot 11）。

当前技术栈：Vue 3 + Vite + TypeScript；后端 Rust + Axum + Tokio + SQLx + SQLite。SQLx 迁移保留既有数据库文件和 V1–V7 版本，连接池、事务及验证入口见[异步数据库访问](docs/reference/sqlite-access.md)。

## 下载二进制

不想本地编译的话，直接到 [Releases](https://github.com/haotemans/yuantuanbot/releases) 下载 `yuantuan-linux-x86_64`（musl 静态单文件，前端已内嵌，对应校验和同名 `.sha256`）。每个 `v*` tag 由 CI 自动构建并发布；非 tag 提交的构建产物在 Actions 对应 run 的 Artifacts 里。

```bash
chmod +x yuantuan-linux-x86_64
./yuantuan-linux-x86_64   # 首次运行生成 config.toml / providers.toml 模板
```

## 快速开始（源码构建）

前置：Rust 工具链（cargo ≥ 1.85）、Node ≥ 18（仅构建前端需要）、一个 NapCat 实例（OneBot 11 正向 WS）。

```bash
# 1. 前端构建（产物嵌入二进制；已入库可跳过本步直接到 2）
cd webui-frontend && npm install && npm run build && cd ..

# 2. 准备配置（首次运行会自动生成带注释的模板）
#    config.toml     — 主配置（napcat 地址/port、节流、归纳时刻、meme 开关…）
#    providers.toml  — LLM providers 与三角色绑定；密钥放环境变量，api_key_env 只存变量名

# 3. 启动
cargo run -p yuantuan
```

## 地址与端口速查

| 用途 | 地址 / 端口 | 谁监听 | 备注 |
| --- | --- | --- | --- |
| **管理面板（前端）** | http://127.0.0.1:8085/ | yuantuan | 浏览器打开，首次输入的密码即为管理员密码 |
| **后端 API + WS** | http://127.0.0.1:8085/api/* 和 ws://127.0.0.1:8085/ws | yuantuan | 由 axum 提供；与面板同源 |
| **NapCat 反向 WS** | ws://127.0.0.1:6199/ws | **yuantuan** | yuantuan 监听，NapCat 主动连入（AstrBot 同款形态） |
| **NapCat WebUI** | http://127.0.0.1:6099/webui | QQ.exe（NapCat 主进程） | NapCat 自带的管理面板 |
| **vite dev（前端热改）** | http://127.0.0.1:5173/ | `npm run dev` | 仅调前端时启用；已配 proxy 到 8085 |

### 启动顺序

1. `cargo run -p yuantuan`（先把 6199 监听架起来，看 banner 里的 `NapCat 反向 WS 监听 : ws://127.0.0.1:6199/ws`）。
2. NapCat WebUI（`http://127.0.0.1:6099/webui`）→ 网络配置 → 新建 → **Websockets客户端**：
   - 名称：随意（如 `云团`）
   - URL：`ws://127.0.0.1:6199/ws`
   - Token：`config.toml` 里 `[napcat] token`（默认 `RulglMHkkxwQ4iyx`）
   - 启用：开
   - 保存（**无需重启 NapCat 主进程**——Websockets客户端是出站连接，保存即生效）
3. 后端日志出现 `NapCat WS 已连接` + `get_login_info 完成 self_qq=...` 即链路通。
4. 浏览器开 http://127.0.0.1:8085/ 首次输入的密码即为管理员密码 → 「配置 / 模型」页绑定 providers。

公网暴露走反代（caddy 等），不要把 8085 直连公网。

### 忘记管理员密码

停进程，清掉密码哈希后重启（回到首启引导状态）：

```bash
sqlite3 data/yuantuan.db "DELETE FROM state_kv WHERE key='admin_pass_hash';"
```

没有 sqlite3 命令行时，直接 `rm -rf data` 重开也可（会丢全部运行数据，慎用）。

前端重新生成：`cd webui-frontend && npm run build`（产物直写 `crates/webui/static/` 并**入库**——这是刻意的：无 node 的机器也能 `cargo build`；构建缺失时内嵌 `static/legacy-index.html` 占位兜底）。

## 工程结构

```text
crates/
  core/        全部领域系统（bot/agent/decision/memory/tools/prefilter/
               context_builder/reply_engine/consolidation/state/llm/meme/event/db）
  adapter-qq/  OneBot 11 客户端（正向 WS、段数组映射、echo 回执、断线退避）
  webui/       axum API + /ws 推送 + rust-embed 内嵌前端
  yuantuan/    bin：装配各系统 + main()
webui-frontend/  Vue3 + Vite + Naive UI 管理面板（产物嵌入 crates/webui/static/）
data/          运行时生成（yuantuan.db(WAL)、memes/、artifacts/、logs/…）
docs/          文档入口、设计、实现参考、工作记录与归档
```

## 文档

Agent 可选用 Docker 开发沙箱运行 Git、Python、Bun。默认关闭，配置及 Linux 部署见[开发沙箱指南](docs/reference/agent-sandbox.md)。

完整入口见 [docs/README.md](docs/README.md)，其中按用途区分设计目标、实现参考、进行中的工作与历史归档。

- [总体架构](docs/design/architecture-v0.1.md)与[运行时设计](docs/design/runtime-design.md)：了解目标与系统约束。
- [信息流实现参考](docs/reference/message-flow-rules.md)与[数据模型](docs/reference/data-model.md)：核对实现记录、数据基线和代码来源。
- [活跃工作](docs/changes/README.md)、[ADR](docs/adr/README.md)与[历史归档](docs/archive/README.md)：继续任务或查阅决定及故障复盘。
- [开发治理协议](docs/governance.md)：文档与代码维护约定。

## 测试

```bash
cargo test   # 单元 + 集成（mock NapCat WS / mock LLM），不依赖外部服务
```
