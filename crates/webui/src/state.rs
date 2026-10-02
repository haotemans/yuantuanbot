//! WebUI 共享状态：db 路径、内存 session 表、启动时刻。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SESSION_TTL: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone)]
pub struct AppState {
    pub db_path: PathBuf,
    sessions: Arc<Mutex<HashMap<String, Instant>>>,
    pub started: Instant,
}

impl AppState {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            started: Instant::now(),
        }
    }

    pub fn open_db(&self) -> rusqlite::Result<rusqlite::Connection> {
        let conn = rusqlite::Connection::open(&self.db_path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(conn)
    }

    pub fn issue_session(&self, token: String) {
        let mut guard = self.sessions.lock().unwrap();
        // 顺手清理过期 session
        guard.retain(|_, exp| *exp > Instant::now());
        guard.insert(token, Instant::now() + SESSION_TTL);
    }

    pub fn valid_session(&self, token: &str) -> bool {
        let mut guard = self.sessions.lock().unwrap();
        match guard.get(token) {
            Some(exp) if *exp > Instant::now() => true,
            Some(_) => {
                guard.remove(token);
                false
            }
            None => false,
        }
    }
}
