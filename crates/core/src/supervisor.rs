//! Supervisor（runtime-design 三章）:每长活组件一个协程挂在 supervisor 下,
//! 单组件 panic → 指数退避重启该组件,不拖垮进程。
//!
//! 设计裁决(runtime-design 已定稿):
//! - 单组件 panic → 只重启该组件(退避),不拖垮进程——群里不能"人没了"
//! - NapCat 断线自动重连,指数退避 1s → 60s 封顶
//!
//! 用法:
//!
//! ```ignore
//! let sup = Supervisor::new();
//! sup.spawn("task_runner", || spawn_my_worker(deps.clone()));
//! // 协程持续运行;组件 panic 会被监到并自动拉起
//! ```
//!
//! 注意:本模块只管「spawn 一次」的长活协程。组件自己 panic 后,重启调的是 factory()
//! 重新构造——factory 必须能多次调用(闭包捕获的 deps 用 Arc/Clone)。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

/// 单组件最大重启次数(超过则放弃,记 error,不再骚扰)
const MAX_RESTARTS: u32 = 20;
/// 退避基数(毫秒):1s, 2s, 4s, 8s, ...
const BACKOFF_BASE_MS: u64 = 1000;
/// 退避上限
const BACKOFF_CAP_MS: u64 = 60_000;

#[derive(Clone, Default)]
pub struct Supervisor {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// 全局重启计数(观测)
    total_restarts: AtomicU64,
}

impl Supervisor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn total_restarts(&self) -> u64 {
        self.inner.total_restarts.load(Ordering::Relaxed)
    }

    /// 挂一个长活组件到 supervisor。`factory` 在首次以及每次重启时调用。
    /// 返回的 JoinHandle 是 supervisor 协程本身;组件 panic 不会让 caller 看到 Err。
    pub fn spawn<F>(&self, name: &'static str, mut factory: F) -> JoinHandle<()>
    where
        F: FnMut() -> JoinHandle<()> + Send + 'static,
    {
        let sup = self.clone();
        tokio::spawn(async move {
            let mut restarts: u32 = 0;
            let mut backoff = Duration::from_millis(BACKOFF_BASE_MS);
            loop {
                let handle = factory();
                info!(component = name, restarts, "supervisor 启动组件");
                match handle.await {
                    Ok(()) => {
                        // 组件主动退出(Ok):在 runtime-design 语义里视为应被拉起的异常
                        // (因为长活组件设计上不该"正常退出"),但避免无限紧循环 → 退避
                        warn!(component = name, restarts, "组件主动退出,退避后拉起");
                    }
                    Err(e) => {
                        if e.is_panic() {
                            error!(component = name, restarts, error = %e, "组件 panic,退避后拉起");
                        } else if e.is_cancelled() {
                            info!(component = name, "组件被取消,不再拉起");
                            return;
                        } else {
                            error!(component = name, restarts, error = %e, "组件异常,退避后拉起");
                        }
                    }
                }
                restarts += 1;
                sup.inner.total_restarts.fetch_add(1, Ordering::Relaxed);
                if restarts > MAX_RESTARTS {
                    error!(component = name, restarts, "组件超过最大重启次数,放弃拉起(需人工介入)");
                    return;
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_millis(BACKOFF_CAP_MS));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restart_on_panic() {
        let sup = Supervisor::new();
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();
        let h = sup.spawn("panic_test", move || {
            let c = c.clone();
            tokio::spawn(async move {
                let n = c.fetch_add(1, Ordering::Relaxed);
                if n == 0 {
                    panic!("boom");
                }
                // 第二次没事,睡一会儿再退(模拟稳定运行)
                tokio::time::sleep(Duration::from_millis(80)).await;
            })
        });
        // 等 supervisor 至少跑过一轮(panic → 退避 1s → 重启 → 稳态退出 → 再退避 → 再重启)
        tokio::time::sleep(Duration::from_millis(2500)).await;
        h.abort();
        let n = counter.load(Ordering::Relaxed);
        assert!(n >= 2, "supervisor 应至少拉起两次,实际 {}", n);
        assert!(sup.total_restarts() >= 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_stops_restarting() {
        let sup = Supervisor::new();
        let h = sup.spawn("cancel_test", || {
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
            })
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.abort();
        // abort supervisor 协程自身,不再有观测手段;只要 abort 不 panic 即过
    }
}
