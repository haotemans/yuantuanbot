# 云团（yuantuanbot）

长期住在 QQ 群里的 Rust 机器人：Decision 小脑 + 人格回复 + 夜间归纳 + 表情管线 + 十页管理面板，目标是 2C2G 小鸡服务器低占用常驻（SQLite 内嵌、单二进制、协议端走 NapCat/OneBot 11）。

## 快速开始

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

然后浏览器开 `http://127.0.0.1:8085/`：首次输入的密码即为管理员密码（首启引导）→ 「配置 / 模型」页绑定 providers（或手改 providers.toml 后在页面保存触发 LLM 热重建）→ 配好 NapCat 地址即可上线。公网暴露走反代（caddy 等），不要把 8085 直连公网。

### 忘记管理员密码

停进程，清掉密码哈希后重启（回到首启引导状态）：

```bash
sqlite3 data/yuantuan.db "DELETE FROM state_kv WHERE key='admin_pass_hash';"
```

没有 sqlite3 命令行时，直接 `rm -rf data` 重开也可（会丢全部运行数据，慎用）。

前端重新生成：`cd webui-frontend && npm run build`（产物直写 `crates/webui/static/` 并**入库**——这是刻意的：无 node 的机器也能 `cargo build`；构建缺失时内嵌 `static/legacy-index.html` 占位兜底）。

## 工程结构

```
crates/
  core/        全部领域系统（bot/agent/decision/memory/tools/prefilter/
               context_builder/reply_engine/consolidation/state/llm/meme/event/db）
  adapter-qq/  OneBot 11 客户端（正向 WS、段数组映射、echo 回执、断线退避）
  webui/       axum API + /ws 推送 + rust-embed 内嵌前端
  yuantuan/    bin：装配各系统 + main()
webui-frontend/  Vue3 + Vite + Naive UI 管理面板（产物嵌入 crates/webui/static/）
data/          运行时生成（yuantuan.db(WAL)、memes/、artifacts/、logs/…）
docs/          设计文档
```

## 文档地图

- [docs/architecture-v0.1.md](docs/architecture-v0.1.md)：总体架构（人格/记忆/关系/Decision/Prefilter/回复形态/三模型/部署形态）
- [docs/data-model.md](docs/data-model.md)：14 张表结构与更新语义
- [docs/runtime-design.md](docs/runtime-design.md)：四 crate 结构、启动序列/监督树、并发模型、WebUI 设计、adapter 通讯设计
- [docs/governance.md](docs/governance.md)：开发治理协议
- [docs/adr/](docs/adr/)：ADR 0001–0004（NapCat 选型、静态人格、否决自研协议、情绪归属）

## 测试

```bash
cargo test   # 单元 + 集成（mock NapCat WS / mock LLM），不依赖外部服务
```
