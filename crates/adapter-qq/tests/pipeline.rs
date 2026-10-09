//! 集成测试：mock NapCat WS（@消息）+ mock LLM（裸 TCP HTTP）→ 命中管线断言：
//! events 有 DecisionMade 且 action 正确；memory_write 非空时 long_memories 落行；
//! 两次垃圾输出 → 兜底 ignore（fallback=true，retries=1）。

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sqlx::Row;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use yuantuan_adapter_qq::{spawn, NapcatConfig};
use yuantuan_core::bot::{spawn_pipeline, PipelineDeps};
use yuantuan_core::db;
use yuantuan_core::db::SqliteExt;
use yuantuan_core::event::{spawn_tracer, EventBus};
use yuantuan_core::llm::LlmGateway;
use yuantuan_core::prefilter::SelfMsgIds;
use yuantuan_core::state::MoodState;

const SELF_QQ: u64 = 10001;

fn temp_dir(prefix: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128
        + (std::process::id() as u128)
        << 16;
    let dir = std::env::temp_dir().join(format!("yt-{prefix}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn temp_db() -> PathBuf {
    let db = temp_dir("pipeline-db").join("yuantuan.db");
    let mut conn = db::connect(&db).await.unwrap();
    db::migrate(&mut conn).await.unwrap();
    db
}

/// 裸 TCP HTTP mock：每个连接读一个请求，从队列取一个 content 回 chat/completions 响应
async fn mock_llm(listener: TcpListener, responses: Arc<Mutex<VecDeque<String>>>) {
    loop {
        let (mut s, _) = match listener.accept().await {
            Ok(x) => x,
            Err(_) => return,
        };
        let responses = responses.clone();
        tokio::spawn(async move {
            let _ = read_request(&mut s).await;
            let content = responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| "{}".into());
            let body = json!({
                "id": "chatcmpl-mock",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}]
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = s.write_all(resp.as_bytes()).await;
        });
    }
}

async fn read_request(s: &mut tokio::net::TcpStream) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    // 读头
    let mut content_len = 0usize;
    loop {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
            for line in headers.lines() {
                if let Some(v) = line.strip_prefix("content-length:") {
                    content_len = v.trim().parse().unwrap_or(0);
                }
            }
            while buf.len() - (pos + 4) < content_len {
                let n = s.read(&mut chunk).await?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            return Ok(());
        }
    }
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// mock NapCat：反向 WS 形态，主动 connect 我们监听的 /ws，回 get_login_info，再推送事件
async fn mock_napcat(port: u16, events: Vec<Value>) {
    tokio::time::sleep(Duration::from_millis(200)).await;
    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut ws, _) = connect_async(&url).await.expect("mock NapCat 连不上");
    let req = ws.next().await.unwrap().unwrap();
    let v: Value = serde_json::from_str(&req.into_text().unwrap()).unwrap();
    assert_eq!(v["action"], "get_login_info");
    let echo = v["echo"].as_str().unwrap().to_string();
    ws.send(Message::Text(
        json!({"status":"ok","retcode":0,"data":{"user_id": SELF_QQ, "nickname":"云团"},"echo": echo})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    for ev in events {
        ws.send(Message::Text(ev.to_string().into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn at_message(id: u64, uid: u64, text: &str) -> Value {
    json!({
        "post_type": "message", "message_type": "group", "time": 1759400000, "message_id": id,
        "group_id": 555666, "user_id": uid,
        "sender": {"user_id": uid, "nickname": "小明", "card": "小明"},
        "message": [
            {"type": "at", "data": {"qq": SELF_QQ.to_string()}},
            {"type": "text", "data": {"text": text}}
        ]
    })
}

async fn wait_until<F: AsyncFnMut() -> bool>(mut cond: F, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(25); // Q54 窗口 10s,加 buffer
    while !cond().await {
        assert!(Instant::now() < deadline, "超时未完成：{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Q54 语义:同 chat 窗口内 N 条消息聚合 1 次 Decide。
/// 三条消息 @bot 同 chat 紧凑发送 → 1 次窗口 → 1 次 DecisionMade(回复锚定 A)。
/// mock LLM 只喂一次响应,验证 memory_write / 落库在 Q54 语义下仍成立。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn decision_pipeline_q54_window_aggregation() {
    let db_path = temp_db().await;

    // Q54 下 3 条同 chat 消息 = 1 次 Decide;只喂一个 content
    let good = json!({
        "action": "ignore", "mood": "calm", "mention": false, "reply_len": "short",
        "meme_type": null, "task_goal": null,
        "memory_write": "小明是 Rust 爱好者",
        "reason": "Q54 窗口聚合一次 Decide"
    })
    .to_string();
    let responses = Arc::new(Mutex::new(VecDeque::from(vec![good])));
    let llm_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let llm_port = llm_listener.local_addr().unwrap().port();
    tokio::spawn(mock_llm(llm_listener, responses.clone()));

    let dir = temp_dir("pipeline-cfg");
    let providers_path = dir.join("providers.toml");
    std::fs::write(
        &providers_path,
        format!(
            "[provider.mock]\nbase_url = \"http://127.0.0.1:{llm_port}/v1\"\napi_key_env = \"\"\n\n[roles]\ndecision = {{ provider = \"mock\", model = \"m\" }}\n"
        ),
    )
    .unwrap();
    let gateway = LlmGateway::load(&providers_path).unwrap();

    let bus = EventBus::new(128);
    let _tracer = spawn_tracer(&bus, db_path.clone());
    let self_ids = SelfMsgIds::default();
    let nap_port = free_port();
    let nap_events = vec![
        at_message(201, 2001, "云团在吗"),
        at_message(202, 2001, "我很喜欢 Rust"),
        at_message(203, 2001, "Q54 同窗口追加"),
    ];
    tokio::spawn(mock_napcat(nap_port, nap_events));
    let handle = spawn(
        bus.clone(),
        db_path.clone(),
        NapcatConfig {
            listen_addr: format!("127.0.0.1:{nap_port}"),
            token: yuantuan_adapter_qq::shared_token(""),
        },
        self_ids.clone(),
    );
    let _pipeline = spawn_pipeline(PipelineDeps {
        bus: bus.clone(),
        db_path: db_path.clone(),
        llm: Arc::new(std::sync::RwLock::new(Some(Arc::new(gateway)))),
        self_qq: handle.self_qq_shared(),
        self_ids,
        mood: MoodState::default(),
        prefilter: Arc::new(std::sync::RwLock::new(
            yuantuan_core::prefilter::Config::default(),
        )),
        reply: None,
        reply_cfg: Arc::new(std::sync::RwLock::new(
            yuantuan_core::reply_engine::ReplyCfg::default(),
        )),
        ctx_cfg: Arc::new(std::sync::RwLock::new(
            yuantuan_core::context_builder::ContextCfg::default(),
        )),
        memes_dir: temp_dir("pipeline-memes"),
        media_ctx: None,
        skill_registry: None,
        per_chat_cap: std::sync::Arc::new(std::sync::RwLock::new(32)),
    });

    // Q54 断言:同 chat 3 条 → 恰好 1 次 DecisionMade(窗口到期才发)
    wait_until(
        async || {
            let mut conn = db::connect(&db_path).await.unwrap();
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE kind = 'DecisionMade'",
                    yuantuan_core::db::params![],
                    |r| r.try_get(0),
                )
                .await
                .unwrap();
            n == 1
        },
        "Q54 同 chat 3 条 → 1 次 DecisionMade(10s 窗口到期)",
    )
    .await;
    // 再多等一段时间,确认不会突然冒出第二次(即不会 leak 二窗)
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mut conn = db::connect(&db_path).await.unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE kind = 'DecisionMade'",
            yuantuan_core::db::params![],
            |r| r.try_get(0),
        )
        .await
        .unwrap();
    assert_eq!(n, 1, "Q54 窗口聚合恰好 1 次 Decide");

    let payload_str: String = conn
        .query_row(
            "SELECT payload FROM events WHERE kind = 'DecisionMade' ORDER BY id LIMIT 1",
            yuantuan_core::db::params![],
            |r| r.try_get(0),
        )
        .await
        .unwrap();
    let p: Value = serde_json::from_str(&payload_str).unwrap();
    // 回复锚定第一条(Q54 设计:anchor 固定为 A)
    assert_eq!(p["chat_id"], "555666");
    assert_eq!(p["sender_pid"], "p_2001");
    assert_eq!(p["fallback"], false);
    assert_eq!(p["action"], "ignore");

    // memory_write 落 long_memories(Q54 单 decide 也应该写)
    let (content, source): (String, String) = conn
        .query_row(
            "SELECT content, source FROM long_memories WHERE owner_type = 'person' AND owner_id = 'p_2001'", yuantuan_core::db::params![],
            |r| Ok((r.try_get(0)?, r.try_get(1)?)),
        ).await
        .unwrap();
    assert_eq!(content, "小明是 Rust 爱好者");
    assert_eq!(source, "explicit");

    // messages 3 条全部落库(摄取未受影响)且 processed_at 全标记(Q54 others 立即回写)
    let mcount: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages",
            yuantuan_core::db::params![],
            |r| r.try_get(0),
        )
        .await
        .unwrap();
    assert_eq!(mcount, 3);
    let unprocessed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE processed_at IS NULL",
            yuantuan_core::db::params![],
            |r| r.try_get(0),
        )
        .await
        .unwrap();
    assert_eq!(unprocessed, 0, "Q54 others 也要立刻 processed_at 回写");
}

/// 垃圾输出 fallback 兜底:第一次返回非 JSON → 重试一次仍是非法 → fallback ignore,retries=1。
/// 需要独立窗口(独立 chat_id)避免与 Q54 主流程事件混在一起。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn decision_pipeline_fallback_retries() {
    let db_path = temp_db().await;
    let responses = Arc::new(Mutex::new(VecDeque::from(vec![
        "这不是 JSON".to_string(),
        "{\"action\": 123}".to_string(),
    ])));
    let llm_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let llm_port = llm_listener.local_addr().unwrap().port();
    tokio::spawn(mock_llm(llm_listener, responses.clone()));

    let dir = temp_dir("pipeline-cfg");
    let providers_path = dir.join("providers.toml");
    std::fs::write(
        &providers_path,
        format!(
            "[provider.mock]\nbase_url = \"http://127.0.0.1:{llm_port}/v1\"\napi_key_env = \"\"\n\n[roles]\ndecision = {{ provider = \"mock\", model = \"m\" }}\n"
        ),
    )
    .unwrap();
    let gateway = LlmGateway::load(&providers_path).unwrap();

    let bus = EventBus::new(128);
    let _tracer = spawn_tracer(&bus, db_path.clone());
    let self_ids = SelfMsgIds::default();
    let nap_port = free_port();
    let nap_events = vec![at_message(301, 2009, "独立 chat 触发 fallback")];
    tokio::spawn(mock_napcat(nap_port, nap_events));
    let handle = spawn(
        bus.clone(),
        db_path.clone(),
        NapcatConfig {
            listen_addr: format!("127.0.0.1:{nap_port}"),
            token: yuantuan_adapter_qq::shared_token(""),
        },
        self_ids.clone(),
    );
    let _pipeline = spawn_pipeline(PipelineDeps {
        bus: bus.clone(),
        db_path: db_path.clone(),
        llm: Arc::new(std::sync::RwLock::new(Some(Arc::new(gateway)))),
        self_qq: handle.self_qq_shared(),
        self_ids,
        mood: MoodState::default(),
        prefilter: Arc::new(std::sync::RwLock::new(
            yuantuan_core::prefilter::Config::default(),
        )),
        reply: None,
        reply_cfg: Arc::new(std::sync::RwLock::new(
            yuantuan_core::reply_engine::ReplyCfg::default(),
        )),
        ctx_cfg: Arc::new(std::sync::RwLock::new(
            yuantuan_core::context_builder::ContextCfg::default(),
        )),
        memes_dir: temp_dir("pipeline-memes"),
        media_ctx: None,
        skill_registry: None,
        per_chat_cap: std::sync::Arc::new(std::sync::RwLock::new(32)),
    });

    wait_until(
        async || {
            let mut conn = db::connect(&db_path).await.unwrap();
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE kind = 'DecisionMade'",
                    yuantuan_core::db::params![],
                    |r| r.try_get(0),
                )
                .await
                .unwrap();
            n >= 1
        },
        "fallback 触发 DecisionMade",
    )
    .await;

    let mut conn = db::connect(&db_path).await.unwrap();
    let payload_str: String = conn
        .query_row(
            "SELECT payload FROM events WHERE kind = 'DecisionMade' ORDER BY id LIMIT 1",
            yuantuan_core::db::params![],
            |r| r.try_get(0),
        )
        .await
        .unwrap();
    let p: Value = serde_json::from_str(&payload_str).unwrap();
    assert_eq!(p["action"], "ignore");
    assert_eq!(p["fallback"], true);
    assert_eq!(p["retries"], 1);
}
