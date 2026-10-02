//! 集成测试：mock NapCat WS 服务端 → get_login_info 响应 + 2 条群消息（一条@bot）+ 1 条私聊，
//! 断言 messages/persons/identities/member_profiles/events 与段数组映射规则。

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH, Instant};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use yuantuan_adapter_qq::{spawn, NapcatConfig};
use yuantuan_core::db;
use yuantuan_core::event::{spawn_tracer, EventBus};
use yuantuan_core::prefilter::SelfMsgIds;

const SELF_QQ: u64 = 10001;

fn temp_db() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("yt-adapter-test-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("yuantuan.db");
    let mut conn = db::connect(&db).unwrap();
    db::migrate(&mut conn).unwrap();
    db
}

async fn mock_napcat(listener: TcpListener) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut ws = accept_async(stream).await.unwrap();

    // 期待 get_login_info，并原样回 echo
    let req = ws.next().await.unwrap().unwrap();
    let v: Value = serde_json::from_str(&req.into_text().unwrap()).unwrap();
    assert_eq!(v["action"], "get_login_info");
    let echo = v["echo"].as_str().unwrap().to_string();
    ws.send(Message::Text(
        json!({
            "status": "ok", "retcode": 0,
            "data": {"user_id": SELF_QQ, "nickname": "云团"},
            "echo": echo
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    let t = 1759400000i64;
    let group_msg = |id: u64, uid: u64, card: &str, nick: &str, msg: Value| json!({
        "post_type": "message", "message_type": "group", "time": t, "message_id": id,
        "group_id": 555666, "user_id": uid,
        "sender": {"user_id": uid, "nickname": nick, "card": card},
        "message": msg
    });
    // 1. 群纯文本
    let g1 = group_msg(111, 2001, "小明", "明",
        json!([{"type": "text", "data": {"text": "大家早上好"}}]));
    // 2. 群 @bot + 文本
    let g2 = group_msg(112, 2002, "", "阿强",
        json!([
            {"type": "at", "data": {"qq": SELF_QQ.to_string()}},
            {"type": "text", "data": {"text": " 在吗"}}
        ]));
    // 3. 私聊：文本 + 图片
    let p1 = json!({
        "post_type": "message", "message_type": "private", "time": t, "message_id": 113,
        "user_id": 2001,
        "sender": {"user_id": 2001, "nickname": "明", "card": ""},
        "message": [
            {"type": "text", "data": {"text": "私聊测试"}},
            {"type": "image", "data": {"file": "x.jpg"}}
        ]
    });
    for ev in [g1, g2, p1] {
        ws.send(Message::Text(ev.to_string().into())).await.unwrap();
    }
    // 给客户端留处理时间后再断开
    tokio::time::sleep(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ingest_group_and_private_messages() {
    let db_path = temp_db();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(mock_napcat(listener));

    let bus = EventBus::new(64);
    let _tracer = spawn_tracer(&bus, db_path.clone());
    let _handle = spawn(
        bus.clone(),
        db_path.clone(),
        NapcatConfig {
            ws_url: format!("ws://127.0.0.1:{port}"),
            token: String::new(),
        },
        SelfMsgIds::default(),
    );

    // 轮询等待摄取完成（最多 8s）
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let conn = db::connect(&db_path).unwrap();
        let msgs: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        let evs: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        if msgs == 3 && evs == 3 {
            break;
        }
        assert!(Instant::now() < deadline, "超时未摄取完成：messages={msgs} events={evs}");
        drop(conn);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let conn = db::connect(&db_path).unwrap();
    // g1：群文本，名片取 card
    let (text, chat_type, chat_id, nick, at_me, has_image): (String, String, String, String, i64, i64) =
        conn.query_row(
            "SELECT text, chat_type, chat_id, nickname, at_me, has_image FROM messages WHERE msg_id = 1",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        ).unwrap();
    assert_eq!((text.as_str(), chat_type.as_str(), chat_id.as_str(), nick.as_str(), at_me, has_image),
               ("大家早上好", "group", "555666", "小明", 0, 0));

    // g2：@bot → at_me=1，mentions 为 person_id 列表
    let (mentions, at_me2, nick2): (String, i64, String) = conn.query_row(
        "SELECT mentions, at_me, nickname FROM messages WHERE msg_id = 2",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(mentions, json!([format!("p_{SELF_QQ}")]).to_string());
    assert_eq!(at_me2, 1);
    assert_eq!(nick2, "阿强"); // card 为空回落 nickname

    // p1：私聊 dm_ 前缀，has_image=1
    let (chat_id3, chat_type3, has_image3): (String, String, i64) = conn.query_row(
        "SELECT chat_id, chat_type, has_image FROM messages WHERE msg_id = 3",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!((chat_id3.as_str(), chat_type3.as_str(), has_image3), ("dm_2001", "private", 1));

    // 档案：两个发言人建档；QQ 号反查 person 稳定
    let person_count: i64 = conn.query_row("SELECT COUNT(*) FROM persons", [], |r| r.get(0)).unwrap();
    assert_eq!(person_count, 2);
    let pid: String = conn.query_row(
        "SELECT person_id FROM identities WHERE platform = 'qq' AND platform_uid = '2001'",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(pid, "p_2001");
    let card: String = conn.query_row(
        "SELECT card FROM member_profiles WHERE chat_id = '555666' AND person_id = 'p_2001'",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(card, "小明");

    // 事件：tracer 落库 3 条 MessageReceived，payload 为含 chat_id 的 JSON
    let mut stmt = conn.prepare("SELECT kind, payload FROM events ORDER BY id").unwrap();
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(rows.iter().all(|(k, _)| k == "MessageReceived"));
    assert_eq!(rows.len(), 3);
    let first: Value = serde_json::from_str(&rows[0].1).unwrap();
    assert_eq!(first["kind"], "MessageReceived");
    assert_eq!(first["chat_id"], "555666");

    server.abort();
}
