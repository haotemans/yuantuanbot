use std::{path::PathBuf, sync::Arc, time::Duration};
use yuantuan_core::{backup, db};

struct TestDb {
    root: PathBuf,
    path: PathBuf,
    database: Arc<db::Database>,
}
impl TestDb {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "yt-sqlx-{}-{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("yuantuan.db");
        let mut conn = db::connect(&path).await.unwrap();
        db::migrate(&mut conn).await.unwrap();
        let database = conn.database();
        drop(conn);
        Self {
            root,
            path,
            database,
        }
    }
    async fn cleanup(self) {
        self.database.close().await;
        assert_eq!(self.database.pool().size(), 0);
        std::fs::remove_dir_all(self.root).unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn shared_pool_is_bounded_and_all_connections_enforce_sqlite_settings() {
    let rig = TestDb::new().await;
    let alias = db::database(&rig.root.join(".").join("yuantuan.db")).unwrap();
    assert!(Arc::ptr_eq(&alias, &rig.database));
    let mut held = Vec::new();
    for _ in 0..4 {
        let mut conn = db::connect(&rig.path).await.unwrap();
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let wal: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!((fk, timeout, wal.as_str()), (1, 5000, "wal"));
        held.push(conn);
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(50), db::connect(&rig.path))
            .await
            .is_err()
    );
    drop(held.pop());
    let conn = tokio::time::timeout(Duration::from_secs(1), db::connect(&rig.path))
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    drop(held);
    rig.cleanup().await;
}

#[tokio::test(flavor = "current_thread")]
async fn sqlite_write_lock_wait_does_not_block_tokio_and_cancelled_transaction_rolls_back() {
    use db::SqliteExt;
    let rig = TestDb::new().await;
    let mut conn = db::connect(&rig.path).await.unwrap();
    let mut transaction = conn.begin_immediate().await.unwrap();
    transaction
        .execute(
            "INSERT INTO state_kv VALUES ('first','committed',1)",
            db::params![],
        )
        .await
        .unwrap();
    let path = rig.path.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let waiting = tokio::spawn(async move {
        let mut c = db::connect(&path).await.unwrap();
        started.send(()).unwrap();
        c.execute(
            "INSERT INTO state_kv VALUES ('second','committed',1)",
            db::params![],
        )
        .await
        .unwrap();
    });
    ready.await.unwrap();
    // This timer must progress on the single Tokio thread while SQLite waits on its worker.
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!waiting.is_finished());
    transaction.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap()
        .unwrap();
    drop(conn);

    let path = rig.path.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let cancelled = tokio::spawn(async move {
        let mut c = db::connect(&path).await.unwrap();
        let mut tx = c.begin_immediate().await.unwrap();
        tx.execute(
            "INSERT INTO state_kv VALUES ('cancelled','must roll back',1)",
            db::params![],
        )
        .await
        .unwrap();
        started.send(()).unwrap();
        std::future::pending::<()>().await;
        tx.commit().await.unwrap();
    });
    ready.await.unwrap();
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    let mut c = db::connect(&rig.path).await.unwrap();
    // Acquiring the write lock also waits for the cancelled transaction's queued rollback.
    let mut tx = c.begin_immediate().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM state_kv WHERE key='cancelled'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 0);
    tx.execute(
        "INSERT INTO state_kv VALUES ('after','works',1)",
        db::params![],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    drop(c);
    rig.cleanup().await;
}

#[tokio::test]
async fn backup_restores_a_consistent_snapshot_during_concurrent_writes() {
    use db::SqliteExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    let rig = TestDb::new().await;
    let mut c = db::connect(&rig.path).await.unwrap();
    c.execute_batch("CREATE TABLE pair (id INTEGER PRIMARY KEY, n INTEGER); INSERT INTO pair VALUES (1,0),(2,0);").await.unwrap();
    let value = "中文与引号 '; DROP TABLE persons; --";
    c.execute(
        "INSERT INTO state_kv VALUES ('text',?1,1)",
        db::params![value],
    )
    .await
    .unwrap();
    drop(c);
    std::fs::create_dir(rig.root.join("artifacts")).unwrap();
    std::fs::write(rig.root.join("artifacts/output.txt"), "retained").unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let path = rig.path.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn(async move {
        let mut c = db::connect(&path).await.unwrap();
        started.send(()).unwrap();
        while !flag.load(Ordering::Relaxed) {
            let mut tx = c.begin_immediate().await.unwrap();
            tx.execute("UPDATE pair SET n=n+1 WHERE id=1", db::params![])
                .await
                .unwrap();
            tokio::task::yield_now().await;
            tx.execute("UPDATE pair SET n=n+1 WHERE id=2", db::params![])
                .await
                .unwrap();
            tx.commit().await.unwrap();
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    ready.await.unwrap();
    let artifact = backup::run_backup(&backup::BackupCfg::default(), &rig.root)
        .await
        .unwrap();
    stop.store(true, Ordering::Relaxed);
    writer.await.unwrap();
    assert!(!artifact.pushed);
    let restored = rig.root.join("restored");
    std::fs::create_dir_all(restored.join("backups")).unwrap();
    let name = artifact.path.file_name().unwrap();
    std::fs::copy(&artifact.path, restored.join("backups").join(name)).unwrap();
    std::fs::write(restored.join(".restore-pending"), name.to_str().unwrap()).unwrap();
    backup::restore_from_pending(&restored)
        .await
        .unwrap()
        .unwrap();
    let mut c = db::connect(&restored.join("yuantuan.db")).await.unwrap();
    let restored_db = c.database();
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut *c)
        .await
        .unwrap();
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *c)
        .await
        .unwrap();
    let pair: (i64, i64) = sqlx::query_as("SELECT MIN(n),MAX(n) FROM pair")
        .fetch_one(&mut *c)
        .await
        .unwrap();
    let text: String = sqlx::query_scalar("SELECT value FROM state_kv WHERE key='text'")
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert_eq!((integrity.as_str(), version), ("ok", 7));
    assert_eq!(pair.0, pair.1, "备份不能落在写事务的中间");
    assert_eq!(text, value);
    assert_eq!(
        std::fs::read_to_string(restored.join("artifacts/output.txt")).unwrap(),
        "retained"
    );
    assert!(!std::fs::read_dir(rig.root.join("backups"))
        .unwrap()
        .any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".snapshot-")));
    drop(c);
    restored_db.close().await;
    rig.cleanup().await;
}
