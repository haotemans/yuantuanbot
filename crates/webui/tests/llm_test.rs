//! /api/llm/test：连通性测试端点三态——
//! ① 配置齐全 → 200 {ok:true, latency_ms, model}（mock provider 断言收到 ping/max_tokens=8）
//! ② 角色绑定但 API key 环境变量缺失 → 400 {ok:false, error 说明指向环境变量}
//! ③ 角色未绑定 / 非法 role → 400 {ok:false, error 中文说明}

use axum::{routing::post, Json, Router};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use yuantuan_core::db;
use yuantuan_webui::{serve, Extras};

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("yt-llmtest-{prefix}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Rig {
    http: reqwest::Client,
    base: String,
    token: String,
    /// mock provider 收到的请求体（断言 ping / max_tokens）
    llm_seen: Arc<Mutex<Vec<Value>>>,
    mock_base: String,
}

async fn start() -> Rig {
    let dir = temp_dir("rig");
    let db_path = dir.join("yuantuan.db");
    {
        let mut conn = db::connect(&db_path).await.unwrap();
        db::migrate(&mut conn).await.unwrap();
    }

    // mock OpenAI 兼容 provider：收 chat/completions，记住 body，回固定 pong
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let mock = Router::new()
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let seen = seen2.clone();
                async move {
                    seen.lock().unwrap().push(body);
                    Json(json!({"choices": [{"message": {"content": "pong"}}]}))
                }
            }),
        )
        .route(
            "/v1/models",
            axum::routing::get(|| async {
                (
                    axum::http::StatusCode::BAD_GATEWAY,
                    format!("a{}", "中".repeat(120)),
                )
            }),
        );
    let mock_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_port = mock_listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(mock_listener, mock).await.unwrap() });

    let prov_path = dir.join("providers.toml");
    std::fs::write(
        &prov_path,
        format!(
            "[provider.mock]\nbase_url = \"http://127.0.0.1:{mock_port}/v1\"\napi_key_env = \"\"\n\
             [provider.keyless]\nbase_url = \"http://127.0.0.1:{mock_port}/v1\"\napi_key_env = \"YT_TEST_DEFINITELY_MISSING_KEY\"\n\
             [roles]\n\
             decision = {{ provider = \"mock\", model = \"mock-7b\" }}\n\
             bot_chat = {{ provider = \"keyless\", model = \"k-1\" }}\n"
        ),
    )
    .unwrap();

    let extras = Extras::for_test(dir.join("config.toml"), prov_path.clone());
    let gateway = yuantuan_core::llm::LlmGateway::load(&prov_path).unwrap();
    *extras.llm_slot.write().unwrap() = Some(Arc::new(gateway));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let db2 = db_path.clone();
    tokio::spawn(async move {
        let _ = serve(db2, "127.0.0.1", port, extras).await;
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
        llm_seen: seen,
        mock_base: format!("http://127.0.0.1:{mock_port}/v1"),
    }
}

impl Rig {
    async fn llm_test(&self, role: &str) -> (u16, Value) {
        let r = self
            .http
            .post(format!("{}/api/llm/test", self.base))
            .bearer_auth(&self.token)
            .json(&json!({"role": role}))
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.json().await.unwrap())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn llm_test_three_states() {
    let rig = start().await;

    // ① ok：decision 绑定 mock provider
    let (s, v) = rig.llm_test("decision").await;
    assert_eq!(s, 200, "ok 态应 200: {v}");
    assert_eq!(v["ok"], json!(true));
    assert_eq!(v["model"], json!("mock-7b"));
    assert!(v["latency_ms"].as_u64().is_some(), "应带 latency_ms: {v}");
    // mock 侧断言请求形态：ping + max_tokens=8，不带 system
    let seen = rig.llm_seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["max_tokens"], json!(8));
    assert_eq!(seen[0]["messages"][0]["role"], json!("user"));
    assert_eq!(seen[0]["messages"][0]["content"], json!("ping"));
    drop(seen);

    // ② key 缺失：bot_chat 绑定 keyless provider（环境变量未设置）
    let (s, v) = rig.llm_test("bot_chat").await;
    assert_eq!(s, 400, "key 缺失应 400: {v}");
    assert_eq!(v["ok"], json!(false));
    let err = v["error"].as_str().unwrap();
    assert!(
        err.contains("环境变量") && err.contains("YT_TEST_DEFINITELY_MISSING_KEY"),
        "错误应指向环境变量: {err}"
    );

    // ③ 角色未绑定
    let (s, v) = rig.llm_test("agent_exec").await;
    assert_eq!(s, 400);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("未在 providers.toml 绑定"),
        "错误应说明未绑定: {v}"
    );

    // 非法 role
    let (s, v) = rig.llm_test("bogus").await;
    assert_eq!(s, 400);
    assert!(
        v["error"].as_str().unwrap().contains("decision"),
        "错误应提示合法 role: {v}"
    );

    // 无 token 401
    let r = rig
        .http
        .post(format!("{}/api/llm/test", rig.base))
        .json(&json!({"role": "decision"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);

    // 中文错误响应跨过第 200 字节时仍按 API 契约返回 JSON，不 panic/断开连接。
    let response = rig
        .http
        .post(format!("{}/api/llm/models/probe", rig.base))
        .bearer_auth(&rig.token)
        .json(&json!({"base_url": rig.mock_base, "api_key_env": ""}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["ok"], false);
    assert!(body["error"].as_str().unwrap().contains("502"));
    assert!(body["error"].as_str().unwrap().contains(&"中".repeat(60)));
}
