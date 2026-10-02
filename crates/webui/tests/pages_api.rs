//! 页面后端 API 综合集成测试：真起 serve（临时端口）→ login → 逐端点断言 →
//! /ws 握手收事件（POST /api/config 触发的 ConfigReloaded）。

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use yuantuan_core::db;
use yuantuan_webui::{serve, Extras};

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("yt-{prefix}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn seed(db_path: &PathBuf) {
    let conn = db::connect(db_path).unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    // persons（含 self）+ edges + 消息 + 事件 + 长期记忆 + 摘要 + 任务
    conn.execute(
        "INSERT INTO persons(person_id, display_name, first_seen, last_seen) VALUES ('self','云团',?1,?1),('p_2001','小明',?1,?1)",
        rusqlite::params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO relationship_edges(from_pid, to_pid, trust, familiar, updated_at) VALUES ('p_2001','self',0.7,0.3,?1)",
        rusqlite::params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO messages(chat_id, chat_type, sender_pid, nickname, text, mentions, ts) VALUES ('555666','group','p_2001','小明','早', '[]', ?1)",
        rusqlite::params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO events(kind, payload, ts) VALUES ('DecisionMade', '{\"chat_id\":\"555666\",\"action\":\"reply\"}', ?1)",
        rusqlite::params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO long_memories(owner_type, owner_id, content, source, created_at, updated_at) VALUES ('person','p_2001','小明爱钓鱼','explicit',?1,?1)",
        rusqlite::params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO summaries(owner_type, owner_id, period, date, summary, created_at) VALUES ('chat','555666','daily','2026-10-03','今天很热闹',?1)",
        rusqlite::params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO tasks(task_id, goal, state, budget_max_calls, created_by_pid, chat_id, created_at) VALUES ('t1','写周报','running',10,'p_2001','555666',?1)",
        rusqlite::params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO task_events(task_id, seq, kind, payload, ts) VALUES ('t1',1,'note','\"开工\"',?1)",
        rusqlite::params![now],
    ).unwrap();
    conn.execute(
        "INSERT INTO personality_versions(version_no, content, note, created_by, created_at, active) VALUES (1,'你是云团','初始','admin',?1,1)",
        rusqlite::params![now],
    ).unwrap();
}

struct Rig {
    http: reqwest::Client,
    base: String,
    token: String,
}

async fn start() -> Rig {
    let dir = temp_dir("pages-api");
    let db_path = dir.join("yuantuan.db");
    {
        let mut conn = db::connect(&db_path).unwrap();
        db::migrate(&mut conn).unwrap();
    }
    seed(&db_path);
    let cfg_path = dir.join("config.toml");
    std::fs::write(
        &cfg_path,
        "[prefilter]\nwindow_secs = 60\nself_msg_cap = 12\n\n[napcat]\ntoken = \"top-secret-token\"\n",
    )
    .unwrap();
    let prov_path = dir.join("providers.toml");
    std::fs::write(
        &prov_path,
        "[provider.mock]\nbase_url = \"http://127.0.0.1:9/v1\"\napi_key_env = \"\"\n\n[roles]\ndecision = { provider = \"mock\", model = \"m\" }\n",
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let extras = Extras::for_test(cfg_path, prov_path);
    let db2 = db_path.clone();
    tokio::spawn(async move { let _ = serve(db2, "127.0.0.1", port, extras).await; });
    let http = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    // 等服务就绪
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
    }
}

impl Rig {
    async fn get(&self, path: &str) -> (u16, Value) {
        let r = self
            .http
            .get(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pages_api_full_pass() {
    let rig = start().await;

    // 无 token 401
    let r = rig.http.get(format!("{}/api/dashboard", rig.base)).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401);

    // 仪表盘
    let (s, d) = rig.get("/api/dashboard").await;
    assert_eq!(s, 200);
    assert_eq!(d["messages_in_today"], 1);
    assert_eq!(d["decision_calls_today"], 1);
    assert_eq!(d["active_tasks"], 1);
    assert_eq!(d["adapter_connected"], false);
    assert_eq!(d["mood"], "calm");
    assert!(d["uptime_secs"].is_u64());

    // events 过滤（kind 命中 + chat_id 命中 + 倒序分页）
    let (s, e) = rig.get("/api/events?kind=DecisionMade&chat_id=555666&limit=10").await;
    assert_eq!(s, 200);
    let arr = e["events"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["kind"], "DecisionMade");
    assert_eq!(arr[0]["payload"]["chat_id"], "555666");
    let (s, e2) = rig.get("/api/events?kind=NothingExists").await;
    assert_eq!(s, 200);
    assert_eq!(e2["events"].as_array().unwrap().len(), 0);
    let (_, e3) = rig.get("/api/events?before_id=1").await;
    assert_eq!(e3["events"].as_array().unwrap().len(), 0);

    // 记忆与摘要
    let (_, m) = rig.get("/api/memories?owner_type=person&owner_id=p_2001").await;
    let ms = m["memories"].as_array().unwrap();
    assert_eq!(ms.len(), 1);
    assert_eq!(ms[0]["content"], "小明爱钓鱼");
    let (_, su) = rig.get("/api/summaries?owner_type=chat").await;
    assert_eq!(su["summaries"].as_array().unwrap()[0]["summary"], "今天很热闹");

    // 关系网
    let (_, rj) = rig.get("/api/relations").await;
    let nodes = rj["nodes"].as_array().unwrap();
    assert!(nodes.iter().any(|n| n["id"] == "self"));
    assert!(nodes.iter().any(|n| n["id"] == "p_2001"));
    let edges = rj["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["from"], "p_2001");
    assert!((edges[0]["trust"].as_f64().unwrap() - 0.7).abs() < 1e-9);

    // 配置 GET：napcat.token 掩码、prefilter 原文可读；api_key_present 布尔
    let (_, c) = rig.get("/api/config").await;
    assert_eq!(c["config"]["napcat"]["token"], "***");
    assert_eq!(c["config"]["prefilter"]["window_secs"], 60);
    assert_eq!(c["providers"]["provider"]["mock"]["base_url"], "http://127.0.0.1:9/v1");
    assert_eq!(c["providers"]["provider"]["mock"]["api_key_present"], false);

    // 任务
    let (_, t) = rig.get("/api/tasks?state=running").await;
    assert_eq!(t["tasks"].as_array().unwrap()[0]["task_id"], "t1");
    let (_, te) = rig.get("/api/tasks/t1/events").await;
    assert_eq!(te["events"].as_array().unwrap()[0]["kind"], "note");
    let r = rig
        .http
        .get(format!("{}/api/tasks/nope/events", rig.base))
        .bearer_auth(&rig.token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 404);

    // 人格：版本列表 → 取全文 → 新建 v2 → rollback v1 得 v3（线性）
    let (_, v1) = rig.get("/api/personality/versions").await;
    assert_eq!(v1["versions"].as_array().unwrap().len(), 1);
    let (_, full) = rig.get("/api/personality/versions/1").await;
    assert_eq!(full["content"], "你是云团");
    let r = rig
        .http
        .post(format!("{}/api/personality/versions", rig.base))
        .bearer_auth(&rig.token)
        .json(&json!({"content": "新版本人设", "note": "v2 测试"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let r = rig
        .http
        .post(format!("{}/api/personality/rollback/1", rig.base))
        .bearer_auth(&rig.token)
        .send()
        .await
        .unwrap();
    let rb: Value = r.json().await.unwrap();
    assert_eq!(rb["version_no"], 3);
    let (_, v3) = rig.get("/api/personality/versions/3").await;
    assert_eq!(v3["content"], "你是云团");
    assert_eq!(v3["note"], "回滚自 v1");
    assert_eq!(v3["active"], true);
    // v2 不再 active
    let (_, v2) = rig.get("/api/personality/versions/2").await;
    assert_eq!(v2["active"], false);

    // 知识库占位
    let (_, kb) = rig.get("/api/kb/status").await;
    assert_eq!(kb, json!({"enabled": false, "docs": 0}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_pushes_events_and_config_write_applies() {
    let rig = start().await;

    // tungstenite 握手
    let url = format!("{}/ws?token=bad", rig.base.replace("http", "ws"));
    let bad = tokio_tungstenite::connect_async(url).await;
    assert!(bad.is_err(), "坏 token 应拒绝升级");

    let url = format!("{}/ws?token={}", rig.base.replace("http", "ws"), rig.token);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    // POST /api/config（providers 写回 + prefilter 阈值热应用）→ 应收到 ConfigReloaded
    let apply = rig
        .http
        .post(format!("{}/api/config", rig.base))
        .bearer_auth(&rig.token)
        .json(&json!({
            "providers": {
                "provider": { "mock2": { "base_url": "http://127.0.0.1:8/v1", "api_key_env": "", "models": ["x"] } },
                "roles": { "decision": { "provider": "mock2", "model": "x" } }
            },
            "config": { "prefilter": { "window_secs": 45, "self_msg_cap": 9 } }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(apply.status().as_u16(), 200);
    let applied: Value = apply.json().await.unwrap();
    assert!(applied["applied"].as_array().unwrap().iter().any(|s| s.as_str().unwrap_or("").contains("LLM")));
    assert!(applied["applied"].as_array().unwrap().iter().any(|s| s.as_str().unwrap_or("").contains("prefilter")));

    // ws 收到 ConfigReloaded（kind 字段）
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t)))) => {
                let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                if v["kind"] == "ConfigReloaded" {
                    got = true;
                    break;
                }
            }
            other => panic!("ws 异常: {other:?}"),
        }
    }
    assert!(got, "ws 应收到 ConfigReloaded");

    // 关闭帧
    let _ = ws.send(tokio_tungstenite::tungstenite::Message::Close(None)).await;

    // 结构损坏的 providers 写回被拒（base_url 类型非法，解析不回原结构 → 400）
    let bad = rig
        .http
        .post(format!("{}/api/config", rig.base))
        .bearer_auth(&rig.token)
        .json(&json!({ "providers": { "provider": { "bad": { "base_url": 123 } }, "roles": {} } }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status().as_u16(), 400);

    // prefilter 阈值已热应用（GET 可见原文）
    let (_, c) = rig.get("/api/config").await;
    assert_eq!(c["config"]["prefilter"]["window_secs"], 45);
    assert_eq!(c["config"]["prefilter"]["self_msg_cap"], 9);
}
