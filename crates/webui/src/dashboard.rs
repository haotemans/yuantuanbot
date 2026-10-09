//! 仪表盘：今日收发 / 今日 Decision 调用 / 活跃任务 / 运行秒数 / adapter 连接状态 / 当前 mood。

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;
use sqlx::Row;
use yuantuan_core::db::SqliteExt;

pub async fn dashboard(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut conn = state
        .open_db()
        .await
        .map_err(|e| err(&format!("数据库打开失败: {e}")))?;
    let mut q = async |sql: &str| -> i64 {
        conn.query_row(sql, yuantuan_core::db::params![], |r| r.try_get(0))
            .await
            .unwrap_or(0)
    };
    // 本地零点（localtime→start of day→utc 标准写法）
    const MIDNIGHT: &str = "CAST(strftime('%s','now','localtime','start of day','utc') AS INTEGER)";

    // 进程资源占用（sysinfo 跨平台；每次请求新建 System 代价可接受——仪表盘刷新率低）
    let (mem_rss_bytes, mem_total_bytes, cpu_percent) = sys_info();

    Ok(Json(json!({
        "messages_in_today": q(&format!("SELECT COUNT(*) FROM messages WHERE sender_pid != 'self' AND ts >= {MIDNIGHT}")).await,
        "messages_out_today": q(&format!("SELECT COUNT(*) FROM messages WHERE sender_pid = 'self' AND ts >= {MIDNIGHT}")).await,
        "decision_calls_today": q(&format!("SELECT COUNT(*) FROM events WHERE kind = 'DecisionMade' AND ts >= {MIDNIGHT}")).await,
        "active_tasks": q("SELECT COUNT(*) FROM tasks WHERE state = 'running'").await,
        "tokens_today": q(&format!("SELECT COALESCE(SUM(total_tokens), 0) FROM llm_usage WHERE ts >= {MIDNIGHT}")).await,
        "tokens_in_today": q(&format!("SELECT COALESCE(SUM(prompt_tokens), 0) FROM llm_usage WHERE ts >= {MIDNIGHT}")).await,
        "tokens_out_today": q(&format!("SELECT COALESCE(SUM(completion_tokens), 0) FROM llm_usage WHERE ts >= {MIDNIGHT}")).await,
        "llm_calls_today": q(&format!("SELECT COUNT(*) FROM llm_usage WHERE ts >= {MIDNIGHT}")).await,
        "uptime_secs": state.started.elapsed().as_secs(),
        "adapter_connected": (state.extras.adapter_connected)(),
        "mood": state.extras.mood.get().as_str(),
        "mem_rss_bytes": mem_rss_bytes,
        "mem_total_bytes": mem_total_bytes,
        "cpu_percent": cpu_percent,
    })))
}

/// 本进程 RSS / 系统总内存 / 本进程 CPU 占用率（0-100，跨核归一）。
/// CPU% 需要两次采样间的 delta，所以 System 用静态共享实例（每次调用 refresh 增量）；
/// 首次调用返回 0.0 属正常（无基准）。
fn sys_info() -> (u64, u64, f32) {
    use std::sync::Mutex;
    use sysinfo::{Pid, ProcessesToUpdate, System};
    static SYS: Mutex<Option<System>> = Mutex::new(None);
    let mut guard = match SYS.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let sys = guard.get_or_insert_with(System::new);
    let pid = Pid::from_u32(std::process::id());
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.refresh_memory();
    let rss = sys.process(pid).map(|p| p.memory()).unwrap_or(0);
    let cpu = sys.process(pid).map(|p| p.cpu_usage()).unwrap_or(0.0);
    (rss, sys.total_memory(), cpu)
}

fn err(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
}
