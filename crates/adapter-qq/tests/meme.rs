//! 集成测试：meme 管线两路 + 偷图审批。
//! A: decision→send_meme(meme_type=开心) → adapter 收到 image 段（file:// 路径存在）→ use_count 增加
//! B: 群 image 消息 → steal 下载入 _inbox pending → 重复消息去重 → WebUI approve 转 active 改类别
//!    → meme-file 预览字节一致 → reject 第二行删文件删行

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use yuantuan_adapter_qq::{send_fn, spawn, NapcatConfig};
use yuantuan_core::bot::{spawn_pipeline, PipelineDeps};
use yuantuan_core::db;
use yuantuan_core::event::{spawn_tracer, EventBus};
use yuantuan_core::llm::LlmGateway;
use yuantuan_core::meme;
use yuantuan_core::prefilter::SelfMsgIds;
use yuantuan_core::reply_engine::{ReplyCfg, ReplyEngine};
use yuantuan_core::state::MoodState;

const SELF_QQ: u64 = 10001;

fn temp_dir(prefix: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128
        + (std::process::id() as u128) << 16;
    let dir = std::env::temp_dir().join(format!("yt-{prefix}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 造一张合法 PNG（用 image crate，避免内嵌字节）
fn make_png(path: &PathBuf) {
    let mut img = image::RgbImage::new(32, 32);
    for y in 0..32u32 {
        for x in 0..32u32 {
            let v = if (x / 8 + y / 8) % 2 == 0 { 240u8 } else { 20u8 };
            img.put_pixel(x, y, image::Rgb([v, v / 2, 200]));
        }
    }
    img.save(path).unwrap();
}

// ---------- 用例 A：decision send_meme → 发图 ----------

async fn mock_llm_decision_meme(listener: TcpListener) {
    loop {
        let (mut s, _) = match listener.accept().await {
            Ok(x) => x,
            Err(_) => return,
        };
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let _ = s.read(&mut buf).await;
            let content = json!({
                "action": "send_meme", "mood": "happy", "mention": false, "reply_len": "short",
                "meme_type": "开心", "task_goal": null, "memory_write": null, "reason": "开心一下"
            })
            .to_string();
            let body = json!({
                "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}]
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(), body
            );
            let _ = s.write_all(resp.as_bytes()).await;
        });
    }
}

type Captures = Arc<Mutex<Vec<Value>>>;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn mock_napcat_capture(port: u16, initial: Vec<Value>, captures: Captures) {
    tokio::time::sleep(Duration::from_millis(200)).await;
    let url = format!("ws://127.0.0.1:{port}/ws");
    let (ws, _) = connect_async(&url).await.expect("mock NapCat 连不上");
    let (w, mut r) = ws.split();
    let w = Arc::new(tokio::sync::Mutex::new(w));
    let req = r.next().await.unwrap().unwrap();
    let v: Value = serde_json::from_str(&req.into_text().unwrap()).unwrap();
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
        w.lock().await.send(Message::Text(ev.to_string().into())).await.unwrap();
    }
    let mut seq = 800i64;
    while let Some(Ok(frame)) = r.next().await {
        let Message::Text(t) = frame else { continue };
        let v: Value = match serde_json::from_str(&t) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v["action"].as_str().unwrap_or("").starts_with("send_") {
            captures.lock().unwrap().push(v["params"]["message"].clone());
            seq += 1;
            let echo = v["echo"].as_str().unwrap_or("").to_string();
            let _ = w
                .lock()
                .await
                .send(Message::Text(
                    json!({"status":"ok","retcode":0,"data":{"message_id":seq},"echo":echo})
                        .to_string()
                        .into(),
                ))
                .await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn decision_send_meme_sends_image() {
    let data = temp_dir("meme-data");
    let memes = data.join("memes");
    std::fs::create_dir_all(memes.join("开心")).unwrap();
    let png_path = memes.join("开心").join("m1.png");
    make_png(&png_path);
    let db_path = data.join("yuantuan.db");
    {
        let mut conn = db::connect(&db_path).unwrap();
        db::migrate(&mut conn).unwrap();
    }
    // 入库（走正式扫描管线）
    let stats = meme::scan_and_ingest(&memes, &db_path).unwrap();
    assert_eq!((stats.scanned, stats.added, stats.skipped_dup), (1, 1, 0));
    // 再扫一次幂等
    let stats2 = meme::scan_and_ingest(&memes, &db_path).unwrap();
    assert_eq!(stats2.added, 0);

    // mock LLM + mock NapCat
    let ll = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ll_port = ll.local_addr().unwrap().port();
    tokio::spawn(mock_llm_decision_meme(ll));
    let cfg_dir = temp_dir("meme-cfg");
    let providers_path = cfg_dir.join("providers.toml");
    std::fs::write(
        &providers_path,
        format!("[provider.mock]\nbase_url = \"http://127.0.0.1:{ll_port}/v1\"\napi_key_env = \"\"\n\n[roles]\ndecision = {{ provider = \"mock\", model = \"m\" }}\n"),
    )
    .unwrap();
    let gateway = Arc::new(LlmGateway::load(&providers_path).unwrap());

    let captures: Captures = Arc::new(Mutex::new(Vec::new()));
    let nl_port = free_port();
    tokio::spawn(mock_napcat_capture(nl_port, vec![at_message(501, 2001, "来个开心图")], captures.clone()));

    let bus = EventBus::new(128);
    let _tracer = spawn_tracer(&bus, db_path.clone());
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
    let engine = ReplyEngine::new(
        db_path.clone(),
        bus.clone(),
        send_fn(adapter.clone()),
        self_ids.clone(),
        Arc::new(std::sync::RwLock::new(ReplyCfg::default())),
        MoodState::default(),
    )
    .spawn();
    let _pipeline = spawn_pipeline(PipelineDeps {
        bus: bus.clone(),
        db_path: db_path.clone(),
        llm: Arc::new(std::sync::RwLock::new(Some(gateway))),
        self_qq: adapter.self_qq_shared(),
        self_ids,
        mood: MoodState::default(),
        prefilter: Arc::new(std::sync::RwLock::new(yuantuan_core::prefilter::Config::default())),
        reply: Some(engine),
        reply_cfg: Arc::new(std::sync::RwLock::new(ReplyCfg::default())),
        ctx_cfg: Arc::new(std::sync::RwLock::new(yuantuan_core::context_builder::ContextCfg::default())),
        memes_dir: memes.clone(),
        media_ctx: None,
        skill_registry: None,
    });

    // 等 1 次发送
    let deadline = Instant::now() + Duration::from_secs(30);
    while captures.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "30s 内未发出 meme 图");
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    let segs = captures.lock().unwrap()[0].clone();
    let arr = segs.as_array().unwrap();
    let img = arr.iter().find(|s| s["type"] == "image").expect("应含 image 段");
    let file = img["data"]["file"].as_str().unwrap().to_string();
    assert!(file.starts_with("file:///"), "file 应为 file:// 绝对路径: {file}");
    let local = file.trim_start_matches("file:///").replace('/', std::path::MAIN_SEPARATOR_STR);
    assert!(PathBuf::from(&local).exists(), "图文件应存在: {local}");

    // use_count 增加、last_used_ts 已写
    tokio::time::sleep(Duration::from_millis(200)).await;
    let conn = db::connect(&db_path).unwrap();
    let (use_count, last_used): (i64, Option<i64>) = conn
        .query_row("SELECT use_count, last_used_ts FROM meme_library WHERE id = 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(use_count, 1);
    assert!(last_used.is_some());
    // self 图片消息落流水
    let self_img: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE sender_pid='self' AND has_image=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(self_img, 1);
}

// ---------- 用例 B：偷图 → pending → approve / reject ----------

/// 极简图片 HTTP 服务：任何 GET 都回同一张 PNG
async fn mock_image_server(listener: TcpListener, png: Vec<u8>) {
    loop {
        let (mut s, _) = match listener.accept().await {
            Ok(x) => x,
            Err(_) => return,
        };
        let png = png.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            let _ = s.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: image/png\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                png.len()
            );
            let _ = s.write_all(resp.as_bytes()).await;
            let _ = s.write_all(&png).await;
        });
    }
}

fn image_message(id: u64, uid: u64, url: &str) -> Value {
    json!({
        "post_type": "message", "message_type": "group", "time": 1759400000, "message_id": id,
        "group_id": 555666, "user_id": uid,
        "sender": {"user_id": uid, "nickname": "阿强", "card": ""},
        "message": [{"type": "image", "data": {"file": "x.jpg", "url": url}}]
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn steal_then_approve_and_reject() {
    let data = temp_dir("steal-data");
    let memes = data.join("memes");
    std::fs::create_dir_all(&memes).unwrap();
    let db_path = data.join("yuantuan.db");
    {
        let mut conn = db::connect(&db_path).unwrap();
        db::migrate(&mut conn).unwrap();
    }

    // 图片源
    let src_png = temp_dir("steal-src").join("src.png");
    make_png(&src_png);
    let png_bytes = std::fs::read(&src_png).unwrap();
    let il = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let img_port = il.local_addr().unwrap().port();
    tokio::spawn(mock_image_server(il, png_bytes.clone()));
    let img_url = format!("http://127.0.0.1:{img_port}/pic.png");

    // mock NapCat 推两条相同图片消息（第二条应被去重跳过）
    let nl_port = free_port();
    let captures: Captures = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn(mock_napcat_capture(
        nl_port,
        vec![image_message(601, 2002, &img_url), image_message(602, 2002, &img_url)],
        captures,
    ));

    let bus = EventBus::new(128);
    let _tracer = spawn_tracer(&bus, db_path.clone());
    let self_ids = SelfMsgIds::default();
    let _adapter = spawn(
        bus.clone(),
        db_path.clone(),
        NapcatConfig {
            listen_addr: format!("127.0.0.1:{nl_port}"),
            token: yuantuan_adapter_qq::shared_token(""),
        },
        self_ids,
    );
    // 测试用 sample_rate=1.0 保证确定性（生产默认 0.005）
    let _steal = meme::spawn_steal_listener_with_rate(
        &bus,
        db_path.clone(),
        memes.clone(),
        Arc::new(std::sync::RwLock::new(true)),
        1.0,
    );

    // 等 pending 行落库
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let conn = db::connect(&db_path).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM meme_library WHERE status='pending' AND added_by='steal'", [], |r| r.get(0))
            .unwrap();
        if n == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "30s 内 pending 未落库（当前 {n}）");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // 去重：第二条相同图片不再产生 pending
    tokio::time::sleep(Duration::from_secs(1)).await;
    let conn = db::connect(&db_path).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM meme_library WHERE status='pending'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "重复图片应被去重");
    let (id, file): (i64, String) = conn
        .query_row("SELECT id, file FROM meme_library WHERE status='pending' LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert!(file.starts_with("_inbox/"));
    assert!(memes.join(&file).exists(), "_inbox 文件应存在");

    // WebUI：login → approve → meme-file → reject
    let wl = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let wport = wl.local_addr().unwrap().port();
    drop(wl);
    let db2 = db_path.clone();
    let extras = yuantuan_webui::Extras::for_test(
        temp_dir("meme-noconfig").join("config.toml"),
        temp_dir("meme-noconfig").join("providers.toml"),
    );
    tokio::spawn(async move { let _ = yuantuan_webui::serve(db2, "127.0.0.1", wport, extras).await; });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let http = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{wport}");
    let login: Value = http
        .post(format!("{base}/api/auth/login"))
        .json(&json!({"password": "t3st-meme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();

    // 未登录列表应 401
    let r = http.get(format!("{base}/api/memes")).send().await.unwrap();
    assert_eq!(r.status(), 401);

    // approve（顺带改类别）
    let r = http
        .post(format!("{base}/api/memes/{id}/approve"))
        .bearer_auth(&token)
        .json(&json!({"category": "开心"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let (status, cat): (String, String) = conn
        .query_row("SELECT status, category FROM meme_library WHERE id = ?1", rusqlite::params![id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!((status.as_str(), cat.as_str()), ("active", "开心"));

    // meme-file 预览：字节与源图一致
    let bytes = http
        .get(format!("{base}/api/meme-file/{id}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&bytes[..], &png_bytes[..]);

    // reject：手工插一条 pending（含文件），拒后行与文件都消失
    let file2 = memes.join("_inbox").join("todelete.png");
    std::fs::create_dir_all(file2.parent().unwrap()).unwrap();
    std::fs::write(&file2, &png_bytes).unwrap();
    // 直接 SQL 插（同图 md5 相同不影响手工行）
    conn.execute(
        "INSERT INTO meme_library(file, category, md5, added_by, status, created_at) VALUES ('_inbox/todelete.png', '_inbox', 'manual', 'steal', 'pending', 1)",
        [],
    )
    .unwrap();
    let id2: i64 = conn
        .query_row("SELECT id FROM meme_library WHERE file = '_inbox/todelete.png'", [], |r| r.get(0))
        .unwrap();
    let r = http
        .post(format!("{base}/api/memes/{id2}/reject"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let gone: i64 = conn
        .query_row("SELECT COUNT(*) FROM meme_library WHERE id = ?1", rusqlite::params![id2], |r| r.get(0))
        .unwrap();
    assert_eq!(gone, 0);
    assert!(!file2.exists(), "reject 应删除文件");
}
