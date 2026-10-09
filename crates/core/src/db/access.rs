//! SQLx owns connections, worker threads and transaction cancellation. Helpers only
//! bind owned values and map rows; every operation that touches SQLite is async.
use sqlx::{
    sqlite::{
        SqliteArguments, SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow,
    },
    Arguments, Connection as _, Sqlite, SqliteConnection, SqlitePool, Transaction,
};
use std::future::Future;
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};

pub type Result<T> = std::result::Result<T, sqlx::Error>;

pub struct Database {
    pool: SqlitePool,
}
impl Database {
    pub async fn close(&self) {
        // A concurrent release can reach the idle queue after SQLx's last close
        // sweep. Drain it too before reporting that SQLite files can be replaced.
        loop {
            self.pool.close().await;
            if self.pool.size() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    }
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// Weak entries do not keep test/restored databases open. The application owns
/// an Arc for its lifetime; short-lived callers acquire from that same pool.
pub fn database(path: &Path) -> anyhow::Result<Arc<Database>> {
    static POOLS: OnceLock<Mutex<HashMap<PathBuf, Weak<Database>>>> = OnceLock::new();
    let absolute = std::path::absolute(path)?;
    let parent = absolute
        .parent()
        .ok_or_else(|| anyhow::anyhow!("数据库路径缺少父目录"))?
        .canonicalize()?;
    let key = parent.join(
        absolute
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("数据库路径缺少文件名"))?,
    );
    let mut pools = POOLS.get_or_init(Default::default).lock().unwrap();
    pools.retain(|_, p| p.strong_count() > 0);
    if let Some(db) = pools
        .get(&key)
        .and_then(Weak::upgrade)
        .filter(|db| !db.pool.is_closed())
    {
        return Ok(db);
    }
    let options = SqliteConnectOptions::new()
        .filename(&key)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let db = Arc::new(Database {
        pool: SqlitePoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(10))
            .connect_lazy_with(options),
    });
    pools.insert(key, Arc::downgrade(&db));
    Ok(db)
}

pub struct Connection {
    connection: sqlx::pool::PoolConnection<Sqlite>,
    database: Arc<Database>,
}
impl Connection {
    pub fn database(&self) -> Arc<Database> {
        self.database.clone()
    }
    pub async fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        connect(path.as_ref()).await
    }
    pub async fn open_in_memory() -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .in_memory(true)
                    .foreign_keys(true),
            )
            .await?;
        let connection = pool.acquire().await?;
        Ok(Self {
            connection,
            database: Arc::new(Database { pool }),
        })
    }
}
impl Deref for Connection {
    type Target = SqliteConnection;
    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}
impl DerefMut for Connection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.connection
    }
}
pub async fn connect(path: &Path) -> anyhow::Result<Connection> {
    let database = database(path)?;
    let connection = database.pool.acquire().await?;
    Ok(Connection {
        connection,
        database,
    })
}

#[derive(Clone)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}
pub trait ToValue {
    fn to_value(&self) -> Value;
}
impl<T: ToValue + ?Sized> ToValue for &T {
    fn to_value(&self) -> Value {
        (*self).to_value()
    }
}
impl ToValue for Value {
    fn to_value(&self) -> Value {
        self.clone()
    }
}
impl ToValue for str {
    fn to_value(&self) -> Value {
        Value::Text(self.into())
    }
}
impl ToValue for String {
    fn to_value(&self) -> Value {
        Value::Text(self.clone())
    }
}
impl ToValue for bool {
    fn to_value(&self) -> Value {
        Value::Integer(i64::from(*self))
    }
}
impl<T: ToValue> ToValue for Option<T> {
    fn to_value(&self) -> Value {
        self.as_ref().map(ToValue::to_value).unwrap_or(Value::Null)
    }
}
impl ToValue for Vec<u8> {
    fn to_value(&self) -> Value {
        Value::Blob(self.clone())
    }
}
macro_rules! integers { ($($t:ty),*) => { $(impl ToValue for $t { fn to_value(&self) -> Value { Value::Integer(*self as i64) } })* }; }
integers!(i32, i64, u32);
impl ToValue for f64 {
    fn to_value(&self) -> Value {
        Value::Real(*self)
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Self::Text(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Self::Integer(v)
    }
}
pub fn params_from_iter(values: impl IntoIterator<Item = Value>) -> SqliteArguments<'static> {
    let mut args = SqliteArguments::default();
    for v in values {
        match v {
            Value::Null => args.add(Option::<String>::None),
            Value::Integer(v) => args.add(v),
            Value::Real(v) => args.add(v),
            Value::Text(v) => args.add(v),
            Value::Blob(v) => args.add(v),
        }
        .expect("SQLite scalar parameter encoding");
    }
    args
}
#[macro_export]
macro_rules! sql_params { ($($value:expr),* $(,)?) => { $crate::db::params_from_iter(vec![$($crate::db::ToValue::to_value(&$value)),*]) }; }
pub use crate::sql_params as params;

pub trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}
impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

pub struct Statement<'c> {
    connection: &'c mut SqliteConnection,
    sql: String,
}
impl Statement<'_> {
    pub async fn query_map<T, F>(
        &mut self,
        args: SqliteArguments<'static>,
        mut f: F,
    ) -> Result<std::vec::IntoIter<Result<T>>>
    where
        F: FnMut(&SqliteRow) -> Result<T>,
    {
        let rows = sqlx::query_with(&self.sql, args)
            .fetch_all(&mut *self.connection)
            .await?;
        Ok(rows.iter().map(&mut f).collect::<Vec<_>>().into_iter())
    }
}

pub trait SqliteExt {
    fn execute(
        &mut self,
        sql: &str,
        args: SqliteArguments<'static>,
    ) -> impl Future<Output = Result<usize>> + Send;
    fn execute_batch(&mut self, sql: &str) -> impl Future<Output = Result<()>> + Send;
    fn query_row<T, F>(
        &mut self,
        sql: &str,
        args: SqliteArguments<'static>,
        f: F,
    ) -> impl Future<Output = Result<T>> + Send
    where
        F: FnOnce(&SqliteRow) -> Result<T> + Send;
    fn prepare(&mut self, sql: &str) -> impl Future<Output = Result<Statement<'_>>> + Send;
    fn transaction(&mut self) -> impl Future<Output = Result<Transaction<'_, Sqlite>>> + Send;
    fn begin_immediate(&mut self) -> impl Future<Output = Result<Transaction<'_, Sqlite>>> + Send;
    fn last_insert_rowid(&mut self) -> impl Future<Output = Result<i64>> + Send;
}
impl SqliteExt for SqliteConnection {
    async fn execute(&mut self, sql: &str, args: SqliteArguments<'static>) -> Result<usize> {
        Ok(sqlx::query_with(sql, args)
            .execute(self)
            .await?
            .rows_affected() as usize)
    }
    async fn execute_batch(&mut self, sql: &str) -> Result<()> {
        sqlx::Executor::execute(self, sql).await?;
        Ok(())
    }
    async fn query_row<T, F>(
        &mut self,
        sql: &str,
        args: SqliteArguments<'static>,
        f: F,
    ) -> Result<T>
    where
        F: FnOnce(&SqliteRow) -> Result<T> + Send,
    {
        let row = sqlx::query_with(sql, args).fetch_one(self).await?;
        f(&row)
    }
    async fn prepare(&mut self, sql: &str) -> Result<Statement<'_>> {
        sqlx::Executor::prepare(&mut *self, sql).await?;
        Ok(Statement {
            connection: self,
            sql: sql.to_owned(),
        })
    }
    async fn transaction(&mut self) -> Result<Transaction<'_, Sqlite>> {
        self.begin().await
    }
    async fn begin_immediate(&mut self) -> Result<Transaction<'_, Sqlite>> {
        self.begin_with("BEGIN IMMEDIATE").await
    }
    async fn last_insert_rowid(&mut self) -> Result<i64> {
        sqlx::query_scalar("SELECT last_insert_rowid()")
            .fetch_one(self)
            .await
    }
}
