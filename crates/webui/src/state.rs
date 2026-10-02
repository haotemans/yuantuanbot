//! WebUI 共享状态：db 路径、内存 session 表、启动时刻，以及装配层注入的 Extras。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use yuantuan_core::llm::LlmGateway;

pub const SESSION_TTL: Duration = Duration::from_secs(24 * 3600);

/// 装配层注入（main 组装；测试自行构造）
#[derive(Clone)]
pub struct Extras {
    /// Event Bus（/ws 全量推送）
    pub bus: yuantuan_core::event::EventBus,
    /// LLM 共享槽（config 写回后整体换槽 → 组件级热应用）
    pub llm_slot: Arc<RwLock<Option<Arc<LlmGateway>>>>,
    /// Prefilter 阈值共享槽（含 Decision 成本闸）
    pub prefilter_slot: Arc<RwLock<yuantuan_core::prefilter::Config>>,
    /// 回复形态参数共享槽
    pub reply_slot: yuantuan_core::reply_engine::SharedReplyCfg,
    /// 上下文预算共享槽
    pub ctx_slot: yuantuan_core::context_builder::SharedContextCfg,
    /// 偷表情包开关共享槽
    pub steal_slot: yuantuan_core::meme::SharedSteal,
    /// 夜间归纳调度器句柄（[consolidation] 写回后取消旧定时器按新配置重建）
    pub consolidation: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// self QQ 号共享原子（重建归纳调度器需要注入）
    pub self_qq: Arc<std::sync::atomic::AtomicU64>,
    /// adapter 连接状态探针（断连 >30s 视为离线，由装配侧换算好）
    pub adapter_connected: Arc<dyn Fn() -> bool + Send + Sync>,
    /// 当前情绪（与 Decision 写回共享的同一实例）
    pub mood: yuantuan_core::state::MoodState,
    pub config_path: PathBuf,
    pub providers_path: PathBuf,
}

impl Extras {
    /// 测试装配：全部槽取默认值，归纳句柄空，self_qq 为 0
    pub fn for_test(config_path: PathBuf, providers_path: PathBuf) -> Self {
        Self {
            bus: yuantuan_core::event::EventBus::new(64),
            llm_slot: Arc::new(RwLock::new(None)),
            prefilter_slot: Arc::new(RwLock::new(yuantuan_core::prefilter::Config::default())),
            reply_slot: Arc::new(RwLock::new(yuantuan_core::reply_engine::ReplyCfg::default())),
            ctx_slot: Arc::new(RwLock::new(yuantuan_core::context_builder::ContextCfg::default())),
            steal_slot: Arc::new(RwLock::new(true)),
            consolidation: Arc::new(Mutex::new(None)),
            self_qq: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            adapter_connected: Arc::new(|| false),
            mood: yuantuan_core::state::MoodState::default(),
            config_path,
            providers_path,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db_path: PathBuf,
    /// data/memes 根（由 db_path 推导）
    pub memes_dir: PathBuf,
    sessions: Arc<Mutex<HashMap<String, Instant>>>,
    pub started: Instant,
    pub extras: Extras,
}

impl AppState {
    pub fn new(db_path: PathBuf, extras: Extras) -> Self {
        let memes_dir = db_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("memes");
        Self {
            db_path,
            memes_dir,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            started: Instant::now(),
            extras,
        }
    }

    pub fn open_db(&self) -> rusqlite::Result<rusqlite::Connection> {
        let conn = rusqlite::Connection::open(&self.db_path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
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
