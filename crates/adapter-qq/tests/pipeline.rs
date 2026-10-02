//! 集成测试：mock NapCat WS（@消息）+ mock LLM（裸 TCP HTTP）→ 命中管线断言：
//! events 有 DecisionMade 且 action 正确；memory_write 非空时 long_memories 落行；
//! 两次垃圾输出 → 兜底 ignore（fallback=true，retries=1）。

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use yuantuan_adapter_qq::{spawn, NapcatConfig};
use yuantuan_core::bot::{spawn_pipeline, PipelineDeps};
use yuantuan_core::db;
use yuantuan_core::event::{spawn_tracer, EventBus};
use yuantuan_core::llm::LlmGateway;
use yuantuan_core::prefilter::SelfMsgIds;
use yuantuan_core::state::MoodState;

const SELF_QQ: u64 = 10001;

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("yt-{prefix}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn temp_db() -> PathBuf {
    let db = temp_dir("pipeline-db").join("yuantuan.db");
    let mut conn = db::connect(&db).unwrap();
    db::migrate(&mut conn).unwrap();
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
            let have = buf.len() - (pos + 4);
            while have < content_len {
                let n = s.read(&mut chunk).await?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                let have = buf.len() - (pos + 4);
                if have >= content_len {
                    break;
                }
            }
            return Ok(());
        }
    }
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// mock NapCat：回 get_login_info，然后逐条推送事件
async fn mock_napcat(listener: TcpListener, events: Vec<Value>) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut ws = accept_async(stream).await.unwrap();
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

fn wait_until<F: FnMut() -> bool>(mut cond: F, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "超时未完成：{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn decision_pipeline_end_to_end() {
    let db_path = temp_db();

    // mock LLM：三次调用的 content（按请求顺序出队）
    let good_reply = json!({
        "action": "reply", "mood": "happy", "mention": false, "reply_len": "short",
        "meme_type": null, "task_goal": null, "memory_write": null, "reason": "他@我打招呼"
    })
    .to_string();
    let good_memory = json!({
        "action": "ignore", "mood": "calm", "mention": false, "reply_len": "short",
        "meme_type": null, "task_goal": null, "memory_write": "小明是 Rust 爱好者", "reason": "记下技术偏好"
    })
    .to_string();
    let responses = Arc::new(Mutex::new(VecDeque::from(vec![
        good_reply,
        good_memory,
        "这不是 JSON".to_string(),
        "{\"action\": 123}".to_string(),
    ])));
    let llm_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let llm_port = llm_listener.local_addr().unwrap().port();
    tokio::spawn(mock_llm(llm_listener, responses.clone()));

    // providers.toml → LlmGateway
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
    assert!(gateway.role(yuantuan_core::llm::Role::Decision).is_some());

    // bus + tracer + adapter + 管线
    let bus = EventBus::new(128);
    let _tracer = spawn_tracer(&bus, db_path.clone());
    let self_ids = SelfMsgIds::default();
    let nap_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let nap_port = nap_listener.local_addr().unwrap().port();
    let nap_events = vec![
        at_message(201, 2001, "云团在吗"),
        at_message(202, 2001, "我很喜欢 Rust"),
        at_message(203, 2001, "测试兜底"),
    ];
    tokio::spawn(mock_napcat(nap_listener, nap_events));
    let handle = spawn(
        bus.clone(),
        db_path.clone(),
        NapcatConfig {
            ws_url: format!("ws://127.0.0.1:{nap_port}"),
            token: String::new(),
        },
        self_ids.clone(),
    );
    let _pipeline = spawn_pipeline(PipelineDeps {
        bus: bus.clone(),
        db_path: db_path.clone(),
        llm: Some(Arc::new(gateway)),
        self_qq: handle.self_qq_shared(),
        self_ids,
        mood: MoodState::default(),
        prefilter: yuantuan_core::prefilter::Config::default(),
    });

    // 断言 1：三条消息各产出一条 DecisionMade（reply / ignore+memory / fallback ignore）
    wait_until(
        || {
            let conn = db::connect(&db_path).unwrap();
            let n: i64 = conn
                .query_row("SELECT COUNT(*) FROM events WHERE kind = 'DecisionMade'", [], |r| r.get(0))
                .unwrap();
            n == 3
        },
        "三条 DecisionMade 事件",
    );
    let conn = db::connect(&db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT payload FROM events WHERE kind = 'DecisionMade' ORDER BY id")
        .unwrap();
    let payloads: Vec<Value> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();

    let p0 = &payloads[0];
    assert_eq!(p0["action"], "reply");
    assert_eq!(p0["fallback"], false);
    assert_eq!(p0["mood"], "happy");
    assert_eq!(p0["chat_id"], "555666");
    assert!(p0["elapsed_ms"].is_u64());

    let p2 = &payloads[2];
    assert_eq!(p2["action"], "ignore");
    assert_eq!(p2["fallback"], true);
    assert_eq!(p2["retries"], 1);

    // 断言 2：memory_write 落 long_memories（owner=person，source=explicit）
    let (content, source): (String, String) = conn
        .query_row(
            "SELECT content, source FROM long_memories WHERE owner_type = 'person' AND owner_id = 'p_2001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(content, "小明是 Rust 爱好者");
    assert_eq!(source, "explicit");

    // messages 3 条全部落库（摄取未受管线影响）
    let mcount: i64 = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
    assert_eq!(mcount, 3);
}
