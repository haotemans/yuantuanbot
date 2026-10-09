//! 集成测试：30 条 fixture 消息（A@B×2、A@bot×1、含敏感消息）→ mock LLM 固定 JSON →
//! run_once → 断言 summaries/long_memories/edges/events；重复执行第二次行数不变。

use serde_json::json;
use sqlx::Row;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use yuantuan_core::consolidation::{run_once, ConsolidationCfg, ConsolidationDeps};
use yuantuan_core::db;
use yuantuan_core::db::SqliteExt;
use yuantuan_core::event::{spawn_tracer, EventBus};
use yuantuan_core::llm::LlmGateway;

const SELF_QQ: u64 = 10001;

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn temp_dir(prefix: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128
        + (std::process::id() as u128)
        << 16;
    let dir = std::env::temp_dir().join(format!("yt-{prefix}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 裸 TCP HTTP mock LLM：每个 POST 回同一个固定 content
async fn mock_llm(listener: TcpListener, content: String) {
    loop {
        let (mut s, _) = match listener.accept().await {
            Ok(x) => x,
            Err(_) => return,
        };
        let content = content.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let _ = s.read(&mut buf).await; // 读掉请求（本轮 mock 不必完整解析）
            let body = json!({
                "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}]
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(), body
            );
            let _ = s.write_all(resp.as_bytes()).await;
            let _ = s.shutdown().await;
        });
    }
}

async fn seed_fixture(db_path: &PathBuf) {
    let mut conn = db::connect(db_path).await.unwrap();
    let now = now_secs();
    for (pid, name) in [("p_2001", "小明"), ("p_2002", "阿强"), ("p_2003", "阿芳")] {
        conn.execute(
            "INSERT INTO persons(person_id, display_name, first_seen, last_seen) VALUES (?1, ?2, ?3, ?3)",
            yuantuan_core::db::params![pid, name, now],
        ).await
        .unwrap();
    }
    let t = now - 600;
    let mundane = [
        "早啊",
        "吃了没",
        "今天聊 Rust 吧",
        "借我看看那个文档",
        "晚上打球去",
        "哈哈哈",
    ];
    for i in 0..30i64 {
        // 带 mention 的三条必须由小明（p_2001）发出，否则统计不到 A@B
        let (pid, nick) = match i {
            1 | 4 | 7 => ("p_2001", "小明"),
            _ => match i % 3 {
                0 => ("p_2001", "小明"),
                1 => ("p_2002", "阿强"),
                _ => ("p_2003", "阿芳"),
            },
        };
        let (text, mentions) = match i {
            1 => ("@阿强 文档发你", "[\"p_2002\"]"),      // A@B 第 1 次
            4 => ("@阿强 记得看", "[\"p_2002\"]"),        // A@B 第 2 次
            7 => ("@云团 在吗", "[\"p_10001\"]"),         // A@bot 1 次
            20 => ("我的密码是123456，别告诉别人", "[]"), // 敏感消息（只进流水，不进提炼）
            _ => (mundane[(i as usize) % mundane.len()], "[]"),
        };
        conn.execute(
            "INSERT INTO messages(chat_id, chat_type, sender_pid, nickname, text, mentions, ts)
             VALUES ('555666', 'group', ?1, ?2, ?3, ?4, ?5)",
            yuantuan_core::db::params![pid, nick, text, mentions, t + i * 15],
        )
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn consolidation_end_to_end() {
    let dir = temp_dir("consolidation");
    let db_path = dir.join("yuantuan.db");
    {
        let mut conn = db::connect(&db_path).await.unwrap();
        db::migrate(&mut conn).await.unwrap();
    }
    seed_fixture(&db_path).await;

    // mock LLM：正常 fact + 两个敏感陷阱（手机号、密码词）
    let distill = json!({
        "chat_summary": "今天聊 Rust 很热闹",
        "person_facts": [
            {"person_id": "p_2001", "fact": "小明在组织晚上的球局"},
            {"person_id": "p_2002", "fact": "阿强的手机号是13800001111"},
            {"person_id": "p_2003", "fact": "阿芳的开箱密码是1234"}
        ],
        "group_facts": ["本群近期在玩 Rust"],
        "profile_updates": [
            {"person_id":"p_2001","field":"ongoing_projects","value":"正在分享文档","evidence_msg_id":2,"evidence_quote":"文档发你"},
            {"person_id":"p_2002","field":"technical_preferences","value":"错误归属","evidence_msg_id":2,"evidence_quote":"文档发你"}
        ],
        "rel_events": [
            {"from": "p_2001", "to": "p_2002", "kind": "help", "delta_trust": 0.05, "evidence": "小明发文档给阿强"}
        ]
    })
    .to_string();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let _server = tokio::spawn(mock_llm(listener, distill));
    let providers_path = dir.join("providers.toml");
    std::fs::write(
        &providers_path,
        format!("[provider.mock]\nbase_url = \"http://127.0.0.1:{port}/v1\"\napi_key_env = \"\"\n\n[roles]\ndecision = {{ provider = \"mock\", model = \"m\" }}\n"),
    )
    .unwrap();
    let gateway = Arc::new(LlmGateway::load(&providers_path).unwrap());

    let bus = EventBus::new(64);
    let _tracer = spawn_tracer(&bus, db_path.clone());
    let deps = ConsolidationDeps {
        db_path: db_path.clone(),
        llm: Some(gateway),
        bus: bus.clone(),
        self_qq: Arc::new(AtomicU64::new(SELF_QQ)),
        cfg: ConsolidationCfg::default(),
    };

    run_once(&deps).await;
    tokio::time::sleep(Duration::from_millis(100)).await; // tracer 落库

    let mut conn = db::connect(&db_path).await.unwrap();
    let profiles: Vec<(String, String)> = conn
        .prepare("SELECT person_id,content FROM person_profile_facts")
        .await
        .unwrap()
        .query_map(yuantuan_core::db::params![], |r| {
            Ok((r.try_get(0)?, r.try_get(1)?))
        })
        .await
        .unwrap()
        .collect::<sqlx::Result<_>>()
        .unwrap();
    assert_eq!(profiles, vec![("p_2001".into(), "正在分享文档".into())]);
    let provenance:(String,i64,i64)=conn.query_row("SELECT source_chat_id,source_msg_id,source_end_msg_id FROM long_memories WHERE owner_type='person' AND owner_id='p_2001'", yuantuan_core::db::params![],|r|Ok((r.try_get(0)?,r.try_get(1)?,r.try_get(2)?))).await.unwrap();
    assert_eq!(provenance, ("555666".into(), 1, 30));
    let today: String = conn
        .query_row(
            "SELECT strftime('%Y-%m-%d','now','localtime')",
            yuantuan_core::db::params![],
            |r| r.try_get(0),
        )
        .await
        .unwrap();

    // 1. summaries：chat 一条 + person 三条（speaker 均有）
    let chat_sum: String = conn
        .query_row(
            "SELECT summary FROM summaries WHERE owner_type='chat' AND owner_id='555666' AND period='daily' AND date=?1",
            yuantuan_core::db::params![today], |r| r.try_get(0),
        ).await
        .unwrap();
    assert_eq!(chat_sum, "今天聊 Rust 很热闹");
    for pid in ["p_2001", "p_2002", "p_2003"] {
        let _: String = conn
            .query_row(
                "SELECT summary FROM summaries WHERE owner_type='person' AND owner_id=?1 AND date=?2",
                yuantuan_core::db::params![pid, today], |r| r.try_get(0),
            ).await
            .unwrap_or_else(|_| panic!("person summary 缺失: {pid}"));
    }
    // person 摘要由 person_facts 聚合（p_2001 正常聚合）
    let p_sum: String = conn
        .query_row(
            "SELECT summary FROM summaries WHERE owner_type='person' AND owner_id='p_2001' AND date=?1",
            yuantuan_core::db::params![today], |r| r.try_get(0),
        ).await
        .unwrap();
    assert_eq!(p_sum, "小明在组织晚上的球局");
    // p_2002 的 fact 含手机号被拒 → 摘要回退发言计数，敏感内容不得出现
    let p2_sum: String = conn
        .query_row(
            "SELECT summary FROM summaries WHERE owner_type='person' AND owner_id='p_2002' AND date=?1",
            yuantuan_core::db::params![today], |r| r.try_get(0),
        ).await
        .unwrap();
    assert!(
        !p2_sum.contains("手机") && !p2_sum.contains("13800001111"),
        "敏感内容泄露: {p2_sum}"
    );
    assert!(p2_sum.contains("今日发言"), "应回退计数保底: {p2_sum}");

    // 2. long_memories：正常 fact 落库；两个敏感陷阱被拒
    let ok_fact: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM long_memories WHERE owner_type='person' AND owner_id='p_2001' AND content='小明在组织晚上的球局' AND source='consolidation'", yuantuan_core::db::params![], |r| r.try_get(0),
        ).await
        .unwrap();
    assert_eq!(ok_fact, 1);
    let group_fact: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM long_memories WHERE owner_type='chat' AND owner_id='555666' AND source='consolidation'", yuantuan_core::db::params![], |r| r.try_get(0),
        ).await
        .unwrap();
    assert_eq!(group_fact, 1);
    let bad_facts: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM long_memories WHERE content LIKE '%手机号%' OR content LIKE '%密码%'", yuantuan_core::db::params![], |r| r.try_get(0),
        ).await
        .unwrap();
    assert_eq!(bad_facts, 0, "敏感 fact 不得入库");

    // 3. @统计：A@B×2 → 一条 mention 事件 delta=0.02；A@bot → self→A delta=0.01
    let (ev1, d1): (String, f64) = conn
        .query_row(
            "SELECT kind, delta_familiar FROM relationship_events WHERE from_pid='p_2001' AND to_pid='p_2002' AND kind='mention'", yuantuan_core::db::params![], |r| Ok((r.try_get(0)?, r.try_get(1)?)),
        ).await
        .unwrap();
    assert_eq!(ev1, "mention");
    assert!((d1 - 0.02).abs() < 1e-9, "A@B×2 应得 0.02，实得 {d1}");
    let d2: f64 = conn
        .query_row(
            "SELECT delta_familiar FROM relationship_events WHERE from_pid='self' AND to_pid='p_2001' AND kind='mention'", yuantuan_core::db::params![], |r| r.try_get(0),
        ).await
        .unwrap();
    assert!((d2 - 0.01).abs() < 1e-9);

    // 4. edges：A→B familiar=0.02、trust=0.5+0.05(LLM help)；self→A familiar=0.01
    let (t, f): (f64, f64) = conn
        .query_row(
            "SELECT trust, familiar FROM relationship_edges WHERE from_pid='p_2001' AND to_pid='p_2002'", yuantuan_core::db::params![], |r| Ok((r.try_get(0)?, r.try_get(1)?)),
        ).await
        .unwrap();
    assert!((f - 0.02).abs() < 1e-9);
    assert!((t - 0.55).abs() < 1e-9);
    let fs: f64 = conn
        .query_row(
            "SELECT familiar FROM relationship_edges WHERE from_pid='self' AND to_pid='p_2001'",
            yuantuan_core::db::params![],
            |r| r.try_get(0),
        )
        .await
        .unwrap();
    assert!((fs - 0.01).abs() < 1e-9);

    // 5. ConsolidationDone 事件落表（tracer）
    let done: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE kind='ConsolidationDone'",
            yuantuan_core::db::params![],
            |r| r.try_get(0),
        )
        .await
        .unwrap();
    assert_eq!(done, 1);

    // 6. 幂等：记录行数，再跑一轮 → 全部不变
    let mut count = async |sql: &str| -> i64 {
        conn.query_row(sql, yuantuan_core::db::params![], |r| r.try_get(0))
            .await
            .unwrap()
    };
    let before = [
        count("SELECT COUNT(*) FROM summaries").await,
        count("SELECT COUNT(*) FROM long_memories").await,
        count("SELECT COUNT(*) FROM relationship_events").await,
        count("SELECT COUNT(*) FROM events").await,
    ];
    run_once(&deps).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after = [
        count("SELECT COUNT(*) FROM summaries").await,
        count("SELECT COUNT(*) FROM long_memories").await,
        count("SELECT COUNT(*) FROM relationship_events").await,
        count("SELECT COUNT(*) FROM events").await,
    ];
    // events 会多一条第二轮的 ConsolidationDone，其余必须不变
    assert_eq!(before[0], after[0], "summaries 重复");
    assert_eq!(before[1], after[1], "long_memories 重复");
    assert_eq!(before[2], after[2], "relationship_events 重复");
    assert_eq!(after[3], before[3] + 1);
    let f2: f64 = conn
        .query_row(
            "SELECT familiar FROM relationship_edges WHERE from_pid='p_2001' AND to_pid='p_2002'",
            yuantuan_core::db::params![],
            |r| r.try_get(0),
        )
        .await
        .unwrap();
    assert!((f2 - 0.02).abs() < 1e-9, "重跑后 familiar 被重复累加: {f2}");
}
