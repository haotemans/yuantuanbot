//! State 状态系统（架构文档十章）：本单实装内存 mood。
//! mood 不入库：只活内存，重启即 calm（"睡一觉起来总是平静的"），state_kv 留待轻量状态。

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::Instant;

/// 情绪四态（Decision 输出 Schema 的 mood 枚举）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MoodValue {
    Calm,
    Happy,
    Angry,
    Down,
}

impl Default for MoodValue {
    fn default() -> Self {
        MoodValue::Calm
    }
}

impl MoodValue {
    pub fn as_str(&self) -> &'static str {
        match self {
            MoodValue::Calm => "calm",
            MoodValue::Happy => "happy",
            MoodValue::Angry => "angry",
            MoodValue::Down => "down",
        }
    }
}

impl fmt::Display for MoodValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 内存 mood + 最近变更时刻；启动即 calm
#[derive(Clone)]
pub struct MoodState {
    inner: Arc<RwLock<(MoodValue, Instant)>>,
}

impl Default for MoodState {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new((MoodValue::Calm, Instant::now()))),
        }
    }
}

impl MoodState {
    pub fn get(&self) -> MoodValue {
        self.inner.read().unwrap().0
    }

    /// 写回 mood；返回是否发生变化
    pub fn set(&self, v: MoodValue) -> bool {
        let mut guard = self.inner.write().unwrap();
        if guard.0 == v {
            return false;
        }
        *guard = (v, Instant::now());
        true
    }
}
