# SQLite 的 SQLx 异步访问层

[文档索引](../README.md) · [ADR-0010](../adr/0010-sqlx-sqlite-access.md) · [数据模型](data-model.md) · [后端验收记录](../changes/backend-hardening-workbench.md)

2026-10-10：数据库驱动从 rusqlite 迁移到 SQLx 0.8。数据库仍是原来的 `data/yuantuan.db`，没有迁往 PostgreSQL，也没有新增表结构版本。本次前端没有变化。

## 连接与并发

- [db/access.rs](../../crates/core/src/db/access.rs) 管理 SQLx `SqlitePool`。按规范化后的父目录和数据库文件名共享池；进程入口和 WebUI 持有 `Arc<Database>`，注册表只保留弱引用，避免临时库永久留在全局注册表中。
- 每个数据库文件最多 4 个池连接，获取连接最多等待 10 秒。每个实际连接启用 WAL、外键和 5 秒 busy_timeout。这是当前代码配置，不是新增面板参数。
- `db::connect(path).await` 借出一个绑定到具体连接的句柄；同一事务、`last_insert_rowid` 和连续读操作不会在多个连接间漂移。使用完毕归还连接，不能在长时间模型请求或外部工具执行期间无必要地占用池连接。
- SQLx 的 SQLite worker 在线程中执行 SQLite 操作，调用方异步等待。消息接入、Bot、Agent、归纳、WebUI、用量记录和备份均沿调用链 `await`；没有用 `block_on` 包装成同步调用。压缩/解压等文件工作仍使用 `spawn_blocking`。
- SQLite 仍是单写事务。多个群不会各自得到独立写锁；连接池限制资源与等待，不承诺消除 BUSY，也没有引入全局写入队列或声称吞吐提升百分比。

## 查询 API

新代码可以直接使用 `database.pool()` 和 SQLx 的 `query / query_as / query_scalar`。已有业务 SQL 保留少量异步行映射辅助方法 `SqliteExt`、`Statement::query_map` 和 `params!`，它们实际调用 SQLx，不依赖 rusqlite。

辅助方法中的查询均为运行时参数化 SQL；`params!` 将基础类型转成拥有所有权的 SQLx 参数。行读取使用 SQLx `try_get`，无结果的可选查询仅将 `RowNotFound` 变成 None。`query_map` 先获取拥有所有权的结果再映射，调用方仍应给业务列表设置合适的 LIMIT。需要流式读取大量记录时应直接使用 SQLx 流式接口。

```rust,ignore
use yuantuan_core::db;

let database = db::database(&db_path)?; // 应由组件状态持有，不能每次请求临时建池
let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE chat_id=?1")
    .bind(chat_id)
    .fetch_one(database.pool())
    .await?;
```

没有启用 `query!` 的编译期数据库检查，因此编译/CI 不需要连接真实数据库或提供 `DATABASE_URL`。SQL 和字段契约通过迁移、集成测试及实际运行验证。Cargo 只启用 SQLx 的 Tokio/SQLite 功能，保留 bundled SQLite，移除所有 crate 的 rusqlite 依赖。

## 事务与既有数据

- 沿用 V1–V7 的 SQL 与 `PRAGMA user_version`。不额外建立 `_sqlx_migrations`，不把新驱动误当成数据库内容迁移；旧 V5 库仍按现有 V6/V7 升级，现有 V7 库幂等跳过。
- 每步迁移由 SQLx `Transaction` 持有 `BEGIN IMMEDIATE`，取得写锁后重新读取版本，再检查列、加列、创建索引和推进版本。并发启动串行完成，失败不留下半步升级。
- 任务创建及终态/流水仍在同一事务里；提交成功后才广播创建/完成事件。工具不因数据库重试被再次执行。
- 消息摄取中人物、身份、群名片、消息行保持同一事务。回复快照继续在一个读事务里读取，异步等待不改变快照上界或回复对象。
- 事务取消/丢弃由 SQLx 排队回滚，连接归还时检查状态；不手写裸 BEGIN/COMMIT 来绕过事务生命周期。测试覆盖取消后的回滚及后续写入可用。

## 停机与备份

停机仍先停止入口/生产组件、限时排空回复，然后做 WAL checkpoint。checkpoint 包括获取连接最多等待 2 秒，单次 SQLite busy_timeout 临时降为 500 ms；连接池关闭最多额外等待 1 秒，超时有日志说明。

`Database::close()` 在 SQLx 关闭之后继续检查池大小，处理并发归还的连接落在最后一次关闭扫描之后的情况。确认没有连接后才能立即替换/删除数据库文件，Windows 上有实际回归覆盖。

备份使用参数绑定的 `VACUUM INTO` 生成独立、一致的 SQLite 数据库副本，再由阻塞工作线程把副本以 `yuantuan.db` 的名字写入原 tar.gz 格式。源数据库仍可有并发写入；不再依赖“checkpoint 后复制仍在变化的主文件”。临时快照在归档工作完成后清理。需要额外容纳一份数据库副本的磁盘空间；memes/artifacts/plugins 文件仍按既有目录备份方式处理，不宣称它们与数据库组成跨文件原子快照。

恢复仍在启动打开数据库之前执行，不能在线覆盖一个正在使用的 SQLite 文件。备份恢复测试覆盖正常归档、完整性、版本、中文/引号参数、文件产物，以及并发事务两行更新不会被备份成一半。

## 验证入口与范围

- `cargo test --workspace`：既有收发/任务/上下文/管理 API，以及迁移回滚、旧数据保留。
- `cargo test -p yuantuan-core --test sqlx_storage`：连接池复用与 4 连接上限、每连接 PRAGMA、单线程 Tokio 中写锁等待、取消事务回滚、并发写入下的备份恢复。
- `python3 deploy/verify-bot-runtime.py /path/to/yuantuan /path/to/source.db`：只读备份源库到独立副本，在无网络的临时容器中启动、升级、检查完整性和两种 SIGTERM 时机。

本地测试、165 服务器隔离验收、生产部署分别记录在[后端工作记录](../changes/backend-hardening-workbench.md)。运行测试不会连接 QQ，也不会自动恢复生产 Bot。SQLite 单写限制、精确性能基准、跨进程写队列和 PostgreSQL 迁移均不属于已完成能力。
