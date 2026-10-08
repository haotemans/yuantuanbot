# Backup 备份页 Workbench

[文档索引](../README.md) · [活跃工作](README.md)

> 工作记录：保留各轮的决定、实施反馈和验证证据。前期“现状”与待办只对应记录当时；继续工作前核对最新反馈和代码，不把旧状态当作当前验收。

## 状态

- status: decided
- owner: hsb + kimi
- last-grill: 2026-10-06
- frontier: 无（所有裁决 confirmed，进入实现阶段）

## 事实与来源

### 现状

- `crates/webui/src/backup.rs` 不存在；仅 `Backup.vue` 13 行占位（`webui-frontend/src/views/Backup.vue`）
- 文档只一句 "精炼数据优先推 GitHub 私有仓，原始流水可选加密压缩"（`Backup.vue:4`）——空裁决
- 数据落点：`data/yuantuan.db`（WAL）、`data/memes/`、`data/artifacts/`、`data/logs/`、`data/backups/`

### 参考（待确认）

- AstrBot 备份策略：未查证
- 通用最佳实践：sqlite3 .backup 命令 vs 文件级 copy、WAL 模式下的快照一致性、增量 vs 全量

## 已确认裁决

### Q-B01 插件（Tools）未来布局

- decision: **B** - `plugins/<name>/` 代码（git/包管理器管） + `data/plugins/<name>/` 数据（备份收）
- rationale: 代码和数据必须分家。备份打包只走数据层，代码卷走仓库。诊断时一看就懂位置
- affected-docs: [docs/reference/data-model.md](../reference/data-model.md) 第一章「存储布局」需补一行
- affected-code:
  - 当前代码无插件目录，本次改动只补文档+备份范围；
  - 未来插件加载时逐字遵守此布局
- status: confirmed

### Q-B02 备份存储位置

- decision: 推 GitHub 私有仓（git push 而非本地）
- rationale: 用户要求（"推 GitHub"）；本地单点故障在小鸡服务器上不可接受
- affected-code: 新文件 `crates/core/src/backup.rs`（git binary 调起）；config.toml `[backup]` 节
- status: confirmed

### Q-B03 触发方式

- decision: 手动按钮 + 每日定时（03:00，与 consolidation 同时）
- rationale: 用户要求
- affected-code: 手动按钮 + 与 consolidation 调度器同点的 tokio::time 任务
- status: confirmed

### Q-B04 备份范围（新裁决）

- decision: 备份 yuantuan.db + memes/ + artifacts/ + （未来）data/plugins/。logs + backups 不入备份
- rationale: 内容不可再生 vs 可再生分层明确
- affected-code: backup.rs tar 命令行
- status: confirmed

### Q-B05 GitHub 凭据填法

- decision: 面板填 PAT + repo URL（类似 Provider 体验）；密钥回写时不回显
- rationale: 用户要求
- affected-code: Backup.vue + webui/backup.rs API + config.rs [backup] 节点
- status: confirmed

### Q-B06 备份数量上限

- decision: 本地 backups/ 保留最近 7 天；GitHub 仓每根 commit 是一个完整快照（不做 squash/force-push）
- rationale: 用户要求 "本地留7天 git留一个一个文件夹快照"
- affected-code: backup.rs 清理逻辑（按文件名日期戳 mtime 滚动删除）
- status: confirmed

### Q-B07 恢复路径

- decision: **面板一键恢复**（用户要求）；通过 `data/.restore-pending` 标记文件实现：
  1. WebUI 点恢复 → POST /api/backup/restore {file}
  2. 后端写好标记文件 `data/.restore-pending = <tar 文件名>`
  3. 用户手动任务管理器停掉 yuantuan.exe
  4. `cargo run -p yuantuan` 启动时检测到此标记 → 先解压恢复再继续启动
- rationale: 单二进制哲学下不能 sentinel 看守进程（复杂度爆炸）；标记文件 + 启动引导是最简方案
- affected-code: main.rs 启动引导序列里加恢复检测 + Backup.vue 一键恢复按钮 + 教程说明
- status: confirmed

## Frontier（要决定的）

- **Q-B01 备份范围**：只备份 db，还是 db + memes + artifacts + logs？
- **Q-B02 存储位置**：仅本地 backups/ 目录 + WebUI 下载，还是要推 GitHub/远程？
- **Q-B03 触发方式**：手动按钮、定时（每天 03:00 跟 consolidation 同时）、还是事件驱动（消息量阈值）？
- **Q-B04 恢复路径**：面板选某个备份点恢复，还是只做下载不让恢复？
- **Q-B05 加密**：要密码保护（AES-GCM）吗？备份里有 API key 等敏感信息吗？
- **Q-B06 数量上限**：保留多少个备份点（按时间或按数量滚动）？

## 工程任务

### MOD-B01 backup 核心模块

- ID: MOD-B01
- 目标工程面：backend
- 公开接口：
  - `pub fn run_backup(cfg: &BackupCfg, data_dir: &Path) -> Result<BackupArtifact>` 执行一次备份
  - `pub fn spawn_daily_scheduler(cfg: SharedBackupCfg, data_dir: PathBuf, bus: EventBus) -> JoinHandle<()>`
  - `pub fn restore_from_pending(data_dir: &Path) -> Result<Option<PathBuf>>` 启动时检测 .restore-pending 并解压
- 实现文件：`crates/core/src/backup.rs`（新建）
- 测试场景：`crates/core/tests/backup.rs` —— mock tar+git 调用，断言：
  - tar 包含 yuantuan.db + memes/ + artifacts/ 不含 logs/
  - 本地 backups/ 滚动删除 >7 天的旧文件
  - restore 标记文件检测 + 解压到临时目录再覆盖
- 完成证据：cargo test 全绿；tar 解压后 sqlite3 能 .recover

### MOD-B02 配置节 + 面板字段

- 目标工程面：backend + front
- 公开接口：config.toml `[backup] enabled/repo_url/pat_env/daily_time/keep_days`
- 实现文件：`crates/yuantuan/src/config.rs` + `crates/webui/src/backup_api.rs`（新建）+ `Backup.vue` 重写
- 测试场景：`webui/tests/backup_api.rs` —— POST 配置触发热应用、GET 列表返回 backups/

### MOD-B03 git push 流程

- 公开接口：backup 完成后 `git -C <local-repo> add . && commit && push`（用 std::process::Command）
- 关键点：repo 本地 clone 在 `data/backups/_repo/`（运行时缓存目录），首次运行 clone、后续 pull --rebase + push
- 失败策略：git 失败只记 error log，不阻断主流程；tar 文件已本地保留

### MOD-B04 WebUI 面板

- 三态显示：未配置 / 已配置未测试 / 已测试通过
- 「立即备份」按钮 → POST /api/backup/run，返回 artifact 大小+耗时
- 「历史备份」表：文件名/大小/时间戳，行内「下载」「恢复」按钮
- 「恢复」走 n-popconfirm → POST /api/backup/restore → 返回标记已写入；前端提示「请手动重启 yuantuan」

### TEST-B01 集成测试

- 跑真备份（不真实推 GitHub，用 mock git 仓）→ 校验 tar 内容 → restore → 起服务后 sqlite 正常打开

### DOC-B01 同步

- [docs/reference/data-model.md](../reference/data-model.md) 第一章存储布局补 `plugins/` 行（尽管当前没实现插件加载，先记录布局裁决）
- [docs/changes/backup-page-workbench.md](backup-page-workbench.md) status → implementing

## 代码反馈

（暂无）
