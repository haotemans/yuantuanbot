# 用 SQLx 异步访问既有 SQLite 数据库

Status: accepted

2026-10-10，用户明确要求迁移 SQLx、更新文档并提交推送。数据库仍为现有 SQLite 文件，V1–V7 表结构、user_version 和业务数据保持兼容；本次不引入 PostgreSQL，不恢复已停止的生产服务。

消息接入、Bot、Agent、WebUI、归纳和备份统一使用 SQLx 的异步 SQLite 驱动与受控连接池，数据库等待沿调用链 await。SQLite 驱动后台线程承接同步 SQLite I/O，不阻塞 Tokio 的执行线程；这不会消除单写事务限制。保留 WAL、外键和 busy_timeout，迁移先取得 IMMEDIATE 写事务再读版本，任务创建和终态事务继续原子提交，回复快照继续在同一读事务中获取。连接和事务取消时由 SQLx 回收、回滚，不使用 block_on 或裸 SQL BEGIN/COMMIT 模拟事务生命周期。

选择运行时参数化 SQL，避免要求开发和 CI 在编译时连接业务数据库；所有参数继续绑定，不将消息文本拼入 SQL。保留少量行映射辅助接口以减少 SQL 和业务规则的无关改写，底层连接、事务和执行均为 SQLx。验证覆盖旧库迁移、失败回滚、取消与锁等待、多群收发和管理接口；生产部署与本地/隔离验收分别记录。
