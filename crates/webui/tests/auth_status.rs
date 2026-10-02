//! /api/auth/status：首启状态三态——未设密码 true → 设后 false → 删除后又 true。

use serde_json::Value;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use yuantuan_core::db;
use yuantuan_core::event::EventBus;
use yuantuan_core::state::MoodState;
use yuantuan_webui::{serve, Extras};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auth_status_reflects_setup_state() {
    let nanos = {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128 + (std::process::id() as u128) << 16
    };
    let dir = std::env::temp_dir().join(format!("yt-auth-status-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("yuantuan.db");
    {
        let mut conn = db::connect(&db_path).unwrap();
        db::migrate(&mut conn).unwrap();
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let extras = Extras {
        bus: EventBus::new(16),
        llm_slot: Arc::new(RwLock::new(None)),
        prefilter_slot: Arc::new(RwLock::new(yuantuan_core::prefilter::Config::default())),
        adapter_connected: Arc::new(|| false),
        mood: MoodState::default(),
        config_path: dir.join("config.toml"),
        providers_path: dir.join("providers.toml"),
    };
    let db2 = db_path.clone();
    tokio::spawn(async move { let _ = serve(db2, "127.0.0.1", port, extras).await; });
    let http = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // 服务未就绪时连接会被拒，返回 Option 让就绪等待循环能容忍
    let get_status = || async {
        let r: Value = http.get(format!("{base}/api/auth/status")).send().await.ok()?.json().await.ok()?;
        r["need_setup"].as_bool()
    };

    // 未就绪前等服务
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut first = None;
    while Instant::now() < deadline {
        if let Ok(Some(v)) = tokio::time::timeout(Duration::from_secs(2), get_status()).await {
            first = Some(v);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(first, Some(true), "无密码应 need_setup=true");

    // 首启设密码 → false
    http.post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({"password": "pw"}))
        .send()
        .await
        .unwrap();
    assert_eq!(get_status().await, Some(false));

    // 删除密码 → true（忘记密码的运维路径，README 有指引）
    let conn = db::connect(&db_path).unwrap();
    conn.execute("DELETE FROM state_kv WHERE key = 'admin_pass_hash'", []).unwrap();
    assert_eq!(get_status().await, Some(true));
}
