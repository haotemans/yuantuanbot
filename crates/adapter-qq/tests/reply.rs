//! 集成测试：聊天闭环——@消息 → decision(reply) → bot_chat('哈‖确实不错‖我去试试') →
//! 三次 send_group_msg（首泡带 at 段、非零延时、合计 ≤8s）→ self 回复落库。
//! 第二用例：首泡发出后注入新群消息 → 继续回复原对象，保留全部气泡。

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use yuantuan_adapter_qq::{send_fn, spawn, NapcatConfig};
use yuantuan_core::bot::{spawn_pipeline, PipelineDeps};
use yuantuan_core::db;
use yuantuan_core::event::{spawn_tracer, EventBus};
use yuantuan_core::llm::LlmGateway;
use yuantuan_core::prefilter::SelfMsgIds;
use yuantuan_core::reply_engine::{ReplyCfg, ReplyEngine};
use yuantuan_core::state::MoodState;

const SELF_QQ: u64 = 10001;

// ---------- 通用 mock 设施 ----------

fn temp_dir(prefix: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128
        + (std::process::id() as u128)
        << 16;
    let dir = std::env::temp_dir().join(format!("yt-{prefix}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn temp_db() -> PathBuf {
    let db = temp_dir("reply-db").join("yuantuan.db");
    let mut conn = db::connect(&db).unwrap();
    db::migrate(&mut conn).unwrap();
    db
}

#[derive(Default)]
struct LlmQueues {
    decision: Mutex<VecDeque<String>>,
    chat: Mutex<VecDeque<String>>,
    requests: Mutex<Vec<Value>>,
    inject_db: Mutex<Option<PathBuf>>,
}

/// 裸 TCP HTTP mock：读完整请求体，按 model 字段分派到对应角色的响应队列
async fn mock_llm(listener: TcpListener, queues: Arc<LlmQueues>) {
    loop {
        let (mut s, _) = match listener.accept().await {
            Ok(x) => x,
            Err(_) => return,
        };
        let queues = queues.clone();
        tokio::spawn(async move {
            let body = read_body(&mut s).await.unwrap_or_default();
            queues
                .requests
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&body).unwrap());
            let model = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v["model"].as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let content = if model == "m-decision" {
                // 模型已经收到本轮上下文后，模拟另一人在等待期间的新话题。
                if let Some(path)=queues.inject_db.lock().unwrap().take() {
                    let conn=db::connect(&path).unwrap();
                    conn.execute_batch("INSERT OR IGNORE INTO persons(person_id,display_name,first_seen,last_seen) VALUES ('p_2002','小明',1,1);
                        INSERT INTO messages(chat_id,chat_type,sender_pid,nickname,text,ts) VALUES ('555666','group','p_2002','小明','等待期间出现的新话题',1759400001);").unwrap();
                }
                queues.decision.lock().unwrap().pop_front()
            } else {
                queues.chat.lock().unwrap().pop_front()
            }
            .unwrap_or_else(|| "{}".into());
            let out = json!({
                "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}]
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                out.len(), out
            );
            let _ = s.write_all(resp.as_bytes()).await;
        });
    }
}

async fn read_body(s: &mut tokio::net::TcpStream) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    loop {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Ok(buf);
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
            let mut content_len = 0usize;
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
            return Ok(buf.split_off(pos + 4));
        }
    }
}

type Captures = Arc<Mutex<Vec<(Instant, Value)>>>;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// mock NapCat：反向 WS 形态，主动 connect /ws → 登录握手 → 推初始事件 → 捕获 send_*_msg action（记录时刻+段数组，回 echo ok）
/// inject_after_first：首个 action 响应完成后等 150ms 注入该事件（插话保留测试用）
async fn mock_napcat(
    port: u16,
    initial: Vec<Value>,
    captures: Captures,
    first_action: Arc<Notify>,
    inject_after_first: Option<Value>,
) {
    tokio::time::sleep(Duration::from_millis(200)).await;
    let url = format!("ws://127.0.0.1:{port}/ws");
    let (ws, _) = connect_async(&url).await.expect("mock NapCat 连不上");
    let (w, mut r) = ws.split();
    let w = Arc::new(tokio::sync::Mutex::new(w));

    // 握手
    let req = r.next().await.unwrap().unwrap();
    let v: Value = serde_json::from_str(&req.into_text().unwrap()).unwrap();
    assert_eq!(v["action"], "get_login_info");
    let echo = v["echo"].as_str().unwrap().to_string();
    w.lock()
        .await
        .send(Message::Text(
            json!({"status":"ok","retcode":0,"data":{"user_id":SELF_QQ,"nickname":"云团"},"echo":echo})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    for ev in initial {
        w.lock()
            .await
            .send(Message::Text(ev.to_string().into()))
            .await
            .unwrap();
    }

    // 注入任务
    if let Some(ev) = inject_after_first {
        let w2 = w.clone();
        let notify2 = first_action.clone();
        tokio::spawn(async move {
            notify2.notified().await;
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = w2
                .lock()
                .await
                .send(Message::Text(ev.to_string().into()))
                .await;
        });
    }

    // 读 action 并回执
    let mut seq = 900i64;
    while let Some(Ok(frame)) = r.next().await {
        let Message::Text(t) = frame else { continue };
        let v: Value = match serde_json::from_str(&t) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let action = v["action"].as_str().unwrap_or("");
        if action == "send_group_msg" || action == "send_private_msg" {
            captures
                .lock()
                .unwrap()
                .push((Instant::now(), v["params"]["message"].clone()));
            seq += 1;
            let echo = v["echo"].as_str().unwrap_or("").to_string();
            let first = captures.lock().unwrap().len() == 1;
            let _ = w
                .lock()
                .await
                .send(Message::Text(
                    json!({"status":"ok","retcode":0,"data":{"message_id":seq},"echo":echo})
                        .to_string()
                        .into(),
                ))
                .await;
            if first {
                first_action.notify_one();
            }
        }
    }
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

fn plain_group_message(id: u64, uid: u64, text: &str) -> Value {
    json!({
        "post_type": "message", "message_type": "group", "time": 1759400000, "message_id": id,
        "group_id": 555666, "user_id": uid,
        "sender": {"user_id": uid, "nickname": "阿强", "card": ""},
        "message": [{"type": "text", "data": {"text": text}}]
    })
}

struct Rig {
    _db_path: PathBuf,
    _bus: EventBus,
    _handles: Vec<tokio::task::JoinHandle<()>>,
}

async fn build_rig(
    db_path: PathBuf,
    queues: Arc<LlmQueues>,
    nap_events: Vec<Value>,
    captures: Captures,
    notify: Arc<Notify>,
    inject: Option<Value>,
    engine_cfg: ReplyCfg,
) -> Rig {
    // mock LLM
    let ll = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ll_port = ll.local_addr().unwrap().port();
    let q2 = queues.clone();
    let h1 = tokio::spawn(mock_llm(ll, q2));

    let dir = temp_dir("reply-cfg");
    let providers_path = dir.join("providers.toml");
    std::fs::write(
        &providers_path,
        format!(
            "[provider.mock]\nbase_url = \"http://127.0.0.1:{ll_port}/v1\"\napi_key_env = \"\"\n\n[roles]\ndecision = {{ provider = \"mock\", model = \"m-decision\" }}\nbot_chat = {{ provider = \"mock\", model = \"m-chat\" }}\n"
        ),
    )
    .unwrap();
    let gateway = Arc::new(LlmGateway::load(&providers_path).unwrap());

    // mock NapCat（反向 WS：主动 connect 我们的 /ws）
    let nl_port = free_port();
    let h2 = tokio::spawn(mock_napcat(nl_port, nap_events, captures, notify, inject));

    let bus = EventBus::new(256);
    let _t = spawn_tracer(&bus, db_path.clone());
    let self_ids = SelfMsgIds::default();
    let adapter = spawn(
        bus.clone(),
        db_path.clone(),
        NapcatConfig {
            listen_addr: format!("127.0.0.1:{nl_port}"),
            token: yuantuan_adapter_qq::shared_token(""),
        },
        self_ids.clone(),
    );
    let reply_slot = Arc::new(std::sync::RwLock::new(engine_cfg));
    let engine = ReplyEngine::new(
        db_path.clone(),
        bus.clone(),
        send_fn(adapter.clone()),
        self_ids.clone(),
        reply_slot.clone(),
        MoodState::default(),
    )
    .spawn();
    let pipeline = spawn_pipeline(PipelineDeps {
        bus: bus.clone(),
        db_path: db_path.clone(),
        llm: Arc::new(std::sync::RwLock::new(Some(gateway))),
        self_qq: adapter.self_qq_shared(),
        self_ids,
        mood: MoodState::default(),
        prefilter: Arc::new(std::sync::RwLock::new(
            yuantuan_core::prefilter::Config::default(),
        )),
        reply: Some(engine),
        reply_cfg: reply_slot,
        ctx_cfg: Arc::new(std::sync::RwLock::new(
            yuantuan_core::context_builder::ContextCfg::default(),
        )),
        memes_dir: temp_dir("reply-memes"),
        media_ctx: None,
        skill_registry: None,
        per_chat_cap: std::sync::Arc::new(std::sync::RwLock::new(32)),
    });
    Rig {
        _db_path: db_path,
        _bus: bus,
        _handles: vec![h1, h2, pipeline, _t],
    }
}

fn decision_json(mention: bool) -> String {
    json!({
        "action": "reply", "mood": "calm", "mention": mention, "reply_len": "long",
        "meme_type": null, "task_goal": null, "memory_write": null, "reason": "测试"
    })
    .to_string()
}

// ---------- 用例 1：三泡回复闭环 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn reply_loop_sends_three_bubbles() {
    let db_path = temp_db();
    let mut decision: Value = serde_json::from_str(&decision_json(true)).unwrap();
    decision["memory_write"] = json!("长期使用 Rust");
    decision["profile_updates"] = json!([{"field":"technical_preferences","value":"长期使用 Rust","evidence_msg_id":1,"evidence_quote":"我长期使用 Rust"}]);
    let queues = Arc::new(LlmQueues {
        decision: Mutex::new(VecDeque::from(vec![decision.to_string()])),
        chat: Mutex::new(VecDeque::from(vec!["".into(), "哈。‖确实不错。‖::at\n我去试试。".into()])),
        inject_db: Mutex::new(Some(db_path.clone())),
        ..Default::default()
    });
    let captures: Captures = Arc::new(Mutex::new(Vec::new()));
    let notify = Arc::new(Notify::new());
    let rig = build_rig(
        db_path.clone(),
        queues.clone(),
        vec![at_message(
            301,
            2001,
            "我长期使用 Rust，云团觉得这个库怎么样",
        )],
        captures.clone(),
        notify,
        None,
        ReplyCfg::default(),
    )
    .await;
    let _ = &rig;

    // 等三次发送（首泡 ≤0.8s + 两泡各 ≈0.8~1s + LLM 两次往返，裕量到 15s）
    let deadline = Instant::now() + Duration::from_secs(60);
    while captures.lock().unwrap().len() < 3 {
        assert!(Instant::now() < deadline, "Duration 内未收满 3 个泡");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(1200)).await; // 让 self 落库完成
    let caps = captures.lock().unwrap().clone();
    assert_eq!(caps.len(), 3, "应恰好 3 次发送（3 泡）");

    // 顺序与内容
    let texts: Vec<String> = caps
        .iter()
        .map(|(_, segs)| {
            segs.as_array()
                .unwrap()
                .iter()
                .filter(|s| s["type"] == "text")
                .map(|s| s["data"]["text"].as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join("")
        })
        .collect();
    assert_eq!(texts, vec![" 哈。", "确实不错。", " 我去试试。"]);

    // 首泡带 at 段（decision mention=true）
    let first = caps[0].1.as_array().unwrap();
    assert_eq!(first[0]["type"], "at");
    assert_eq!(first[0]["data"]["qq"], "2001");
    assert_eq!(first[1]["data"]["text"], " 哈。", "自动 @ 后必须有间隔");
    let third = caps[2].1.as_array().unwrap();
    assert_eq!(third[0]["type"], "at");
    assert_eq!(third[0]["data"]["qq"], "2001");
    assert_eq!(third[1]["data"]["text"], " 我去试试。", "::at 后也必须有间隔");
    assert!(caps[1]
        .1
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["type"] != "at"));

    // 延时：二、三泡相对前泡有非零延时；首末合计 ≤8s
    let d21 = caps[1].0.duration_since(caps[0].0);
    let d32 = caps[2].0.duration_since(caps[1].0);
    let total = caps[2].0.duration_since(caps[0].0);
    assert!(d21 >= Duration::from_millis(500), "第二泡延时过小: {d21:?}");
    assert!(d32 >= Duration::from_millis(500), "第三泡延时过小: {d32:?}");
    assert!(total <= Duration::from_secs(8), "总时长超预算: {total:?}");

    // self 回复落库，保持会话连贯
    let conn = db::connect(&db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT text FROM messages WHERE sender_pid = 'self' AND chat_id = '555666' ORDER BY msg_id")
        .unwrap();
    let self_texts: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(self_texts, vec!["哈。", "确实不错。", "我去试试。"]);
    let external_ids: Vec<i64> = conn
        .prepare("SELECT external_msg_id FROM messages WHERE sender_pid='self' ORDER BY msg_id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(external_ids, vec![901, 902, 903]);
    let profile: (String, i64) = conn
        .query_row(
            "SELECT content,source_msg_id FROM person_profile_facts WHERE person_id='p_2001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(profile, ("长期使用 Rust".into(), 1));
    let provenance: (String, i64) = conn
        .query_row(
            "SELECT source_chat_id,source_msg_id FROM long_memories WHERE owner_id='p_2001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(provenance, ("555666".into(), 1));
    let requests = queues.requests.lock().unwrap();
    let chat = requests.iter().find(|r| r["model"] == "m-chat").unwrap();
    let content = chat["messages"][1]["content"].as_str().unwrap();
    let context: Value = serde_json::from_str(content).unwrap();
    assert_eq!(context["reply_target"]["person_id"], "p_2001");
    assert!(content.contains("云团觉得这个库怎么样"));
    assert!(!content.contains("等待期间出现的新话题"));
    assert_eq!(context["capabilities"]["tools_available"], false);
    assert_eq!(context["capabilities"]["cross_chat_history_available"], false);
    assert!(chat["messages"][0]["content"].as_str().unwrap().contains("本次回复长度：long"));
    let chats: Vec<_> = requests.iter().filter(|r| r["model"] == "m-chat").collect();
    assert_eq!(chats.len(), 2, "空正文只重试一次");
    assert_eq!(chats[0]["messages"], chats[1]["messages"], "重试保持同一上下文");
    assert!(requests.iter().all(|r| r["tool_choice"] == "none"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn short_reply_rejoins_broken_sentence_and_sends_one_message() {
    let path=temp_db();
    let mut decision:Value=serde_json::from_str(&decision_json(true)).unwrap();
    decision["reply_len"]=json!("short");
    let queues=Arc::new(LlmQueues {
        decision:Mutex::new(VecDeque::from([decision.to_string()])),
        chat:Mutex::new(VecDeque::from(["::at这个库的‖性能不错。‖可以试试。‖::at".into()])),
        ..Default::default()
    });
    let captures:Captures=Arc::new(Mutex::new(Vec::new()));
    let _rig=build_rig(path.clone(),queues,vec![at_message(601,2001,"云团，这个库怎么样")],captures.clone(),Arc::new(Notify::new()),None,ReplyCfg::default()).await;
    let deadline=Instant::now()+Duration::from_secs(30);
    loop {
        let conn=db::connect(&path).unwrap();
        let count:i64=conn.query_row("SELECT COUNT(*) FROM messages WHERE sender_pid='self'",[],|r|r.get(0)).unwrap();
        if count==1 { break; }
        assert!(Instant::now()<deadline,"short 回复未发送");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let caps=captures.lock().unwrap();
    assert_eq!(caps.len(),1);
    assert_eq!(caps[0].1[0]["data"]["qq"],"2001");
    assert_eq!(caps[0].1[1]["data"]["text"]," 这个库的性能不错。\n可以试试。");
}

// ---------- 用例 2：插话不打断 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn new_message_preserves_remaining_bubbles_and_reply_target() {
    let db_path = temp_db();
    let ignore = json!({
        "action": "ignore", "mood": "calm", "mention": false, "reply_len": "short",
        "meme_type": null, "task_goal": null, "memory_write": null, "reason": "普通消息"
    })
    .to_string();
    let queues = Arc::new(LlmQueues {
        decision: Mutex::new(VecDeque::from(vec![decision_json(false), ignore])),
        chat: Mutex::new(VecDeque::from(vec!["第一。‖第二。‖第三。".into()])),
        ..Default::default()
    });
    let captures: Captures = Arc::new(Mutex::new(Vec::new()));
    let notify = Arc::new(Notify::new());
    let rig = build_rig(
        db_path.clone(),
        queues,
        vec![at_message(401, 2001, "云团在吗")],
        captures.clone(),
        notify,
        Some(plain_group_message(402, 2009, "插入新消息")),
        ReplyCfg {
            // 加大后续泡延时下限，给高负载下的摄取新消息留出确定性余量（防测试时序抖动）
            min_delay_ms: 2500,
            ..ReplyCfg::default()
        },
    )
    .await;
    let _ = &rig;

    // 首泡必到
    let deadline = Instant::now() + Duration::from_secs(60);
    while captures.lock().unwrap().is_empty() {
        if Instant::now() >= deadline {
            let c = db::connect(&db_path).unwrap();
            let msgs: Vec<String> = c
                .prepare("SELECT msg_id, sender_pid, chat_id, substr(text,1,24) FROM messages ORDER BY msg_id")
                .unwrap()
                .query_map([], |r| Ok(format!("{}|{}|{}|{}", r.get::<_,i64>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?, r.get::<_,String>(3)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            let evs: Vec<String> = c
                .prepare("SELECT id, kind FROM events ORDER BY id")
                .unwrap()
                .query_map([], |r| {
                    Ok(format!(
                        "{}|{}",
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?
                    ))
                })
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            panic!(
                "首泡未发出。messages={msgs:?} events={evs:?} captures={:?}",
                captures.lock().unwrap().len()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // 三个泡全部完成，普通插话不能取消原回复。
    let deadline2 = Instant::now() + Duration::from_secs(30);
    while captures.lock().unwrap().len() < 3 {
        assert!(Instant::now() < deadline2, "原回答应发完整");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let caps = captures.lock().unwrap().clone();
    let texts: Vec<_> = caps.iter().map(|(_,s)| s[0]["data"]["text"].as_str().unwrap().to_owned()).collect();
    assert_eq!(texts, ["第一。","第二。","第三。"]);
    let conn = db::connect(&db_path).unwrap();
    let interrupted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE kind = 'ReplyInterrupted'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(interrupted, 0, "普通插话不触发 ReplyInterrupted");
    let self_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE sender_pid = 'self'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(self_count, 3);
    // 注入的新消息自身也正常落库决策（ignore）
    let incoming: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE sender_pid = 'p_2009'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(incoming, 1);
}
