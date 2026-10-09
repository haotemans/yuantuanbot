//! POST /api/config 逐节热应用断言：
//! ① reply/context/prefilter/meme 各槽换值可读回（Extras 槽 + GET /api/config 回读一致）
//! ② consolidation：取消旧定时器按新 daily_time 重建——以 run_on_startup=true 触发立即执行，
//!    订阅 EventBus 断言 ConsolidationDone 到达（定时器确以新配置重建的证据）
//! ③ applied / requires_restart 名单准确性：napcat 变更落 requires_restart，槽内节全落 applied

use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use yuantuan_core::db;
use yuantuan_core::event::Event;
use yuantuan_webui::{serve, Extras};

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("yt-cfghot-{prefix}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Rig {
    http: reqwest::Client,
    base: String,
    token: String,
    extras: Extras,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_config_validates_before_write_and_requires_restart() {
    let rig = start().await;
    let before = rig.get_config().await;
    for sandbox in [
        json!({"network":"host"}),
        json!({"timeout_secs":121}),
        json!({"cpus":0}),
    ] {
        let (status, _) = rig.post_config(json!({"sandbox":sandbox})).await;
        assert_eq!(status, 400);
        assert_eq!(
            rig.get_config().await,
            before,
            "invalid sandbox config must not overwrite file"
        );
    }
    let config = json!({"sandbox":{"enabled":false,"network":"none","timeout_secs":30}});
    let (status, result) = rig.post_config(config.clone()).await;
    assert_eq!(status, 200, "{result}");
    assert!(result["requires_restart"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str().unwrap().contains("sandbox")));
    assert!(result["applied"].as_array().unwrap().is_empty());
    assert_eq!(rig.get_config().await["config"], config);
    let (status, result) = rig.post_config(config).await;
    assert_eq!(status, 200);
    assert!(result["requires_restart"].as_array().unwrap().is_empty());
    let (status, result) = rig.post_config(json!({"log":{"level":"info"}})).await;
    assert_eq!(status, 200);
    assert!(result["requires_restart"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str().unwrap().contains("sandbox")));
}

async fn start() -> Rig {
    let dir = temp_dir("rig");
    let db_path = dir.join("yuantuan.db");
    {
        let mut conn = db::connect(&db_path).await.unwrap();
        db::migrate(&mut conn).await.unwrap();
    }
    let cfg_path = dir.join("config.toml");
    std::fs::write(
        &cfg_path,
        "[prefilter]\nwindow_secs = 60\nself_msg_cap = 12\ndecision_cost_per_min = 30\n\n\
         [reply]\nper_char_ms = 40\ntotal_budget_ms = 8000\nbubble_cap = 3\nbubble_char_cap = 500\n\n\
         [context]\nbudget_chars = 40000\nk = 20\nroster_mem_per = 3\n\n\
         [meme]\nsteal_enabled = true\n\n\
         [consolidation]\nenabled = false\ndaily_time = \"03:00\"\nrun_on_startup = false\n",
    )
    .unwrap();

    // 句柄注册表空（启动时 enabled=false），POST 后应被填上
    let extras = Extras::for_test(cfg_path, dir.join("providers.toml"));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let db2 = db_path.clone();
    let extras2 = extras.clone();
    tokio::spawn(async move {
        let _ = serve(db2, "127.0.0.1", port, extras2).await;
    });
    let http = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(r) = http.get(format!("{base}/healthz")).send().await {
            if r.status().is_success() {
                break;
            }
        }
        assert!(Instant::now() < deadline, "WebUI 未就绪");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let login: Value = http
        .post(format!("{base}/api/auth/login"))
        .json(&json!({"password": "pw"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    Rig {
        http,
        base,
        token: login["token"].as_str().unwrap().to_string(),
        extras,
    }
}

impl Rig {
    async fn post_config(&self, config: Value) -> (u16, Value) {
        let r = self
            .http
            .post(format!("{}/api/config", self.base))
            .bearer_auth(&self.token)
            .json(&json!({"config": config}))
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.json().await.unwrap())
    }
    async fn get_config(&self) -> Value {
        self.http
            .get(format!("{}/api/config", self.base))
            .bearer_auth(&self.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn config_hot_apply_full_pass() {
    let rig = start().await;
    assert!(
        rig.extras.consolidation.lock().unwrap().is_none(),
        "启动时归纳停用，句柄应为空"
    );

    // 订阅事件总线（先于 POST，断言 ConsolidationDone 用）
    let mut rx = rig.extras.bus.subscribe();

    let (s, v) = rig
        .post_config(json!({
            "prefilter": { "window_secs": 45, "self_msg_cap": 8, "decision_cost_per_min": 20 },
            "reply": { "per_char_ms": 88, "total_budget_ms": 6000, "bubble_cap": 4, "bubble_char_cap": 360 },
            "context": { "budget_chars": 32000, "k": 16, "roster_mem_per": 2 },
            "meme": { "steal_enabled": false },
            "consolidation": { "enabled": true, "daily_time": "04:30", "run_on_startup": true }
        }))
        .await;
    assert_eq!(s, 200, "POST 应 200: {v}");
    assert_eq!(v["ok"], json!(true));
    let applied: Vec<&str> = v["applied"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();

    // —— ① 槽换值断言
    assert_eq!(
        rig.extras.reply_slot.read().unwrap().per_char_ms,
        88,
        "reply_slot 应已换值"
    );
    assert_eq!(rig.extras.reply_slot.read().unwrap().bubble_cap, 4);
    assert_eq!(rig.extras.reply_slot.read().unwrap().bubble_char_cap, 360);
    assert_eq!(
        rig.extras.ctx_slot.read().unwrap().budget_chars,
        32000,
        "ctx_slot 应已换值"
    );
    assert_eq!(rig.extras.ctx_slot.read().unwrap().k_init, 16);
    assert_eq!(
        rig.extras.prefilter_slot.read().unwrap().window_secs,
        45,
        "prefilter_slot 应已换值"
    );
    assert_eq!(
        rig.extras
            .prefilter_slot
            .read()
            .unwrap()
            .decision_cost_per_min,
        20
    );
    assert_eq!(
        *rig.extras.steal_slot.read().unwrap(),
        false,
        "steal_slot 应已换值"
    );

    // —— ③ 名单准确性：五个槽内节全部 applied，无 requires_restart
    assert!(
        applied.iter().any(|a| a.contains("prefilter")),
        "applied 缺 prefilter: {applied:?}"
    );
    assert!(
        applied.iter().any(|a| a.contains("reply")),
        "applied 缺 reply: {applied:?}"
    );
    assert!(
        applied.iter().any(|a| a.contains("context")),
        "applied 缺 context: {applied:?}"
    );
    assert!(
        applied.iter().any(|a| a.contains("meme")),
        "applied 缺 meme: {applied:?}"
    );
    assert!(
        applied
            .iter()
            .any(|a| a.contains("consolidation") && a.contains("重建")),
        "applied 缺归纳定时器重建: {applied:?}"
    );
    assert!(
        v["requires_restart"].as_array().unwrap().is_empty(),
        "本批不应有需重启项: {v}"
    );

    // —— ② 归纳定时器重建：句柄已填 + run_on_startup 立即执行 → ConsolidationDone 到达
    assert!(
        rig.extras.consolidation.lock().unwrap().is_some(),
        "POST 后句柄应已注册"
    );
    let got = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match rx.recv().await {
                Ok(Event::ConsolidationDone(_)) => break true,
                Ok(_) => continue,
                Err(e) => panic!("事件总线异常: {e}"),
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(
        got,
        "run_on_startup=true 的重建定时器应立即执行并发布 ConsolidationDone"
    );

    // —— GET 回读一致（面板「保存后读回」路径）
    let back = rig.get_config().await;
    assert_eq!(
        back["config"]["reply"]["per_char_ms"],
        json!(88),
        "GET 回读 reply 不一致: {back}"
    );
    assert_eq!(
        back["config"]["context"]["k"],
        json!(16),
        "GET 回读 context 不一致: {back}"
    );
    assert_eq!(
        back["config"]["consolidation"]["daily_time"],
        json!("04:30"),
        "GET 回读归纳时刻不一致: {back}"
    );

    // —— ③b requires_restart 名单：napcat 变更
    let (s, v) = rig
        .post_config(json!({
            "napcat": { "enabled": true, "ws_url": "ws://127.0.0.1:3999", "token": "" }
        }))
        .await;
    assert_eq!(s, 200, "单独 napcat 写回应 200: {v}");
    let rr: Vec<&str> = v["requires_restart"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert!(
        rr.iter().any(|a| a.contains("napcat")),
        "napcat 应落 requires_restart: {rr:?}"
    );
}
