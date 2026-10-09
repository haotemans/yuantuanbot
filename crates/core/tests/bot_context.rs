use rusqlite::params;
use serde_json::Value;
use yuantuan_core::{
    context_builder::{capture_reply_snapshot, render_bot_context, ContextCfg},
    db,
    event::MessageReceivedPayload,
    memory::{self, ProfileField, ProfileUpdate},
    state::MoodValue,
};

struct Rig {
    dir: std::path::PathBuf,
    path: std::path::PathBuf,
}
impl Rig {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "yt-context-{}-{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        db::migrate(&mut db::connect(&path).unwrap()).unwrap();
        Self { dir, path }
    }
    fn message(
        &self,
        person: &str,
        chat: &str,
        text: &str,
        external: Option<i64>,
        reply: Option<i64>,
    ) -> MessageReceivedPayload {
        let conn = db::connect(&self.path).unwrap();
        conn.execute("INSERT OR IGNORE INTO persons(person_id,display_name,first_seen,last_seen) VALUES (?1,'同名',1,1)",[person]).unwrap();
        conn.execute("INSERT INTO messages(chat_id,chat_type,sender_pid,nickname,text,ts,external_msg_id,reply_to) VALUES (?1,'group',?2,'同名',?3,100,?4,?5)",params![chat,person,text,external,reply]).unwrap();
        MessageReceivedPayload {
            msg_id: conn.last_insert_rowid(),
            chat_id: chat.into(),
            chat_type: "group".into(),
            sender_pid: person.into(),
            text: text.into(),
            at_me: true,
            has_image: false,
            reply_to: reply,
            sender_bot: false,
            image_urls: vec![],
            ts: 100,
        }
    }
    fn fact(&self, owner_type: &str, owner: &str, content: &str, time: i64) {
        db::connect(&self.path).unwrap().execute(
            "INSERT INTO long_memories(owner_type,owner_id,content,source,created_at,updated_at) VALUES (?1,?2,?3,'explicit',?4,?4)",params![owner_type,owner,content,time],
        ).unwrap();
    }
}
impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn update(message: &MessageReceivedPayload, value: &str) -> ProfileUpdate {
    ProfileUpdate {
        field: ProfileField::TechnicalPreferences,
        value: value.into(),
        evidence_msg_id: message.msg_id,
        evidence_quote: message.text.clone(),
    }
}

#[test]
fn default_persona_is_visible_idempotent_and_preserves_admin_versions() {
    let rig = Rig::new();
    let conn = db::connect(&rig.path).unwrap();
    assert!(yuantuan_core::context_builder::ensure_default_persona(&conn).unwrap());
    assert!(!yuantuan_core::context_builder::ensure_default_persona(&conn).unwrap());
    conn.execute(
        "UPDATE personality_versions SET content='管理员自定义',active=0",
        [],
    )
    .unwrap();
    assert!(!yuantuan_core::context_builder::ensure_default_persona(&conn).unwrap());
    let content: String = conn
        .query_row("SELECT content FROM personality_versions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(content, "管理员自定义");
}

#[tokio::test]
async fn model_cannot_start_tasks_without_a_source_request() {
    use serde_json::json;
    let rig = Rig::new();
    let app = axum::Router::new().route("/chat/completions", axum::routing::post(|axum::Json(body): axum::Json<Value>| async move {
        assert_eq!(body["tool_choice"],"none");
        let ctx: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert!(ctx["recent_messages"].is_array());
        let out = json!({"action":"start_task","mood":"calm","mention":false,"reply_len":"short","reason":"模拟模型过度主动接单",
            "task_goal":"运行测试","request_quote":ctx["anchor"]["text"]});
        axum::Json(json!({"choices":[{"message":{"content":out.to_string()},"finish_reason":"stop"}]}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let path = rig.dir.join("providers.toml");
    std::fs::write(&path,format!("[provider.mock]\nbase_url=\"http://{addr}\"\napi_key_env=\"\"\n[roles]\ndecision={{provider=\"mock\",model=\"mock\"}}\n")).unwrap();
    let gw = yuantuan_core::llm::LlmGateway::load(&path).unwrap();
    for (text, directed, expected) in [
        ("版本更新说明：新增地图，下周开放", false, "ignore"),
        ("版本更新说明：新增地图，下周开放", true, "reply"),
        ("有人说：请帮我运行测试", false, "ignore"),
        ("请帮我运行 Python 测试", true, "start_task"),
    ] {
        let mut msg = rig.message("p_a", "123", text, None, None);
        msg.at_me = directed;
        let snapshot =
            capture_reply_snapshot(&rig.path, &msg, msg.msg_id, &ContextCfg::default()).unwrap();
        let out = yuantuan_core::decision::decide(
            &rig.path,
            &gw,
            &yuantuan_core::event::EventBus::new(16),
            &yuantuan_core::state::MoodState::default(),
            &msg,
            None,
            &snapshot,
        )
        .await;
        assert_eq!(out.output.action.as_str(), expected, "{text}");
        assert!(!out.fallback);
        assert_eq!(out.retries, 0);
    }
    server.abort();
}

#[test]
fn dialogue_links_only_recent_replies_to_the_same_person_and_chat() {
    let rig = Rig::new();
    let a = rig.message("p_a", "123", "帮我选语言", None, None);
    let qa = rig.message("self", "123", "你打算用哪种语言？", Some(901), None);
    let b = rig.message("p_b", "123", "帮我选框架", None, None);
    let qb = rig.message("self", "123", "你准备选哪个框架？", Some(902), None);
    let conn = db::connect(&rig.path).unwrap();
    conn.execute(
        "UPDATE messages SET reply_anchor_id=?1 WHERE msg_id=?2",
        params![a.msg_id, qa.msg_id],
    )
    .unwrap();
    conn.execute(
        "UPDATE messages SET reply_anchor_id=?1 WHERE msg_id=?2",
        params![b.msg_id, qb.msg_id],
    )
    .unwrap();
    rig.message("self", "123", "旧消息没有归属不能猜", None, None);
    let followup = rig.message("p_a", "123", "Python", None, None);
    let snap = capture_reply_snapshot(
        &rig.path,
        &followup,
        followup.msg_id,
        &ContextCfg::default(),
    )
    .unwrap();
    assert_eq!(snap.dialogue["last_reply_to_sender"]["msg_id"], qa.msg_id);
    assert_eq!(snap.dialogue["reply_target_person_id"], "p_a");
    let elsewhere = rig.message("p_a", "456", "Python", None, None);
    let snap = capture_reply_snapshot(
        &rig.path,
        &elsewhere,
        elsewhere.msg_id,
        &ContextCfg::default(),
    )
    .unwrap();
    assert!(snap.dialogue["last_reply_to_sender"].is_null());
    conn.execute("UPDATE messages SET ts=-201 WHERE msg_id=?1", [qa.msg_id])
        .unwrap();
    let snap = capture_reply_snapshot(
        &rig.path,
        &followup,
        followup.msg_id,
        &ContextCfg::default(),
    )
    .unwrap();
    assert!(
        snap.dialogue["last_reply_to_sender"].is_null(),
        "超过5分钟不默认是承接"
    );
    for (id, person, chat) in [
        ("t1", "p_a", "123"),
        ("t2", "p_b", "123"),
        ("t3", "p_a", "456"),
    ] {
        conn.execute("INSERT INTO tasks(task_id,goal,state,budget_max_calls,created_by_pid,chat_id,created_at) VALUES (?1,'进行中的任务','running',10,?2,?3,1)",params![id,person,chat]).unwrap();
    }
    let snap = capture_reply_snapshot(
        &rig.path,
        &followup,
        followup.msg_id,
        &ContextCfg::default(),
    )
    .unwrap();
    assert_eq!(snap.active_tasks.len(), 1);
    assert_eq!(snap.active_tasks[0]["task_id"], "t1");
}

#[test]
fn participation_load_does_not_mix_private_chat_with_same_group_number() {
    let rig = Rig::new();
    let private = rig.message("self", "123", "私聊回复", None, None);
    let conn = db::connect(&rig.path).unwrap();
    conn.execute(
        "UPDATE messages SET chat_type='private' WHERE msg_id=?1",
        [private.msg_id],
    )
    .unwrap();
    let anchor = rig.message("p_a", "123", "群里消息", None, None);
    let snap =
        capture_reply_snapshot(&rig.path, &anchor, anchor.msg_id, &ContextCfg::default()).unwrap();
    assert_eq!(snap.scene["my_replies_last_5min"], 0);
    assert_eq!(snap.scene["human_messages_last_30s"], 1);
    assert!(snap.scene["seconds_since_my_reply"].is_null());
}

#[tokio::test]
async fn semantic_evidence_must_survive_decision_context_pruning() {
    use serde_json::json;
    let rig = Rig::new();
    let old = rig.message("p_a", "123", "原先的 Docker 问题", None, None);
    for i in 0..10 {
        rig.fact(
            "person",
            "p_a",
            &format!("Docker {i} {}", "历史资料".repeat(190)),
            i,
        );
    }
    let anchor = rig.message("p_a", "123", "Docker 怎么配置", None, None);
    let snapshot = capture_reply_snapshot(
        &rig.path,
        &anchor,
        anchor.msg_id,
        &ContextCfg {
            roster_mem_per: 10,
            ..ContextCfg::default()
        },
    )
    .unwrap();
    let app = axum::Router::new().route("/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| async move {
        let ctx: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(ctx["context_pruned"], true);
        assert!(ctx["recent_messages"].as_array().unwrap().is_empty());
        let out = json!({"action":"reply","mood":"calm","mention":false,"reply_len":"short","reason":"引用已裁剪的消息",
            "assessment":{"audience":"bot","intent":"question","continuity":"new_topic","confidence":"high","benefit":"high","novelty":"new","evidence":"sufficient","evidence_msg_ids":[ctx["anchor"]["msg_id"],old.msg_id]}});
        axum::Json(json!({"choices":[{"message":{"content":out.to_string()},"finish_reason":"stop"}]}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let path = rig.dir.join("providers.toml");
    std::fs::write(&path, format!("[provider.mock]\nbase_url=\"http://{addr}\"\napi_key_env=\"\"\n[roles]\ndecision={{provider=\"mock\",model=\"mock\"}}\n")).unwrap();
    let gateway = yuantuan_core::llm::LlmGateway::load(&path).unwrap();
    let out = yuantuan_core::decision::decide(
        &rig.path,
        &gateway,
        &yuantuan_core::event::EventBus::default(),
        &yuantuan_core::state::MoodState::default(),
        &anchor,
        None,
        &snapshot,
    )
    .await;
    assert!(!out.fallback);
    assert_eq!(out.output.action.as_str(), "ignore");
    assert_eq!(out.policy.evidence_valid, Some(false));
    server.abort();
}

#[test]
fn profile_requires_own_exact_evidence_and_new_sources_win() {
    let rig = Rig::new();
    let old = rig.message("p_a", "g1", "我长期使用 Rust", Some(900), None);
    let new = rig.message("p_a", "g2", "我现在长期使用 Python", Some(901), None);
    let other = rig.message("p_b", "g1", "我长期使用 Java", Some(902), None);
    let conn = db::connect(&rig.path).unwrap();
    assert!(memory::update_profile(
        &conn,
        "p_a",
        "g1",
        old.msg_id,
        old.msg_id,
        &update(&old, "偏好 Rust")
    )
    .unwrap());
    assert!(memory::update_profile(
        &conn,
        "p_a",
        "g2",
        new.msg_id,
        new.msg_id,
        &update(&new, "偏好 Python")
    )
    .unwrap());
    assert!(!memory::update_profile(
        &conn,
        "p_a",
        "g1",
        old.msg_id,
        old.msg_id,
        &update(&old, "偏好 Rust")
    )
    .unwrap());
    assert!(memory::update_profile(
        &conn,
        "p_a",
        "g1",
        1,
        other.msg_id,
        &update(&other, "偏好 Java")
    )
    .is_err());
    let mut forged = update(&new, "偏好 Rust");
    forged.evidence_quote = "这句话不存在".into();
    assert!(memory::update_profile(&conn, "p_a", "g2", 1, other.msg_id, &forged).is_err());
    assert!(memory::update_profile(
        &conn,
        "self",
        "g1",
        1,
        other.msg_id,
        &update(&old, "偏好 Rust")
    )
    .is_err());
    let profile = memory::profile(&conn, "p_a", other.msg_id).unwrap();
    assert_eq!(profile.len(), 1);
    assert_eq!(profile[0].content, "偏好 Python");
    assert_eq!(profile[0].source_msg_id, new.msg_id);
    assert!(memory::profile(&conn, "p_b", other.msg_id)
        .unwrap()
        .is_empty());
    // 无按当时版本重建能力时宁可缺失，不能把未来资料带回旧窗口。
    assert!(memory::profile(&conn, "p_a", old.msg_id)
        .unwrap()
        .is_empty());
}

#[test]
fn renewed_confirmation_blocks_older_contradicting_consolidation() {
    let rig = Rig::new();
    let a = rig.message("p_a", "g", "我长期使用 Rust", None, None);
    let b = rig.message("p_a", "g", "我长期使用 Java", None, None);
    let c = rig.message("p_a", "g", "我仍然长期使用 Rust", None, None);
    let conn = db::connect(&rig.path).unwrap();
    for msg in [&a, &c] {
        memory::update_profile(
            &conn,
            "p_a",
            "g",
            msg.msg_id,
            msg.msg_id,
            &update(msg, "偏好 Rust"),
        )
        .unwrap();
    }
    assert!(!memory::update_profile(
        &conn,
        "p_a",
        "g",
        b.msg_id,
        b.msg_id,
        &update(&b, "偏好 Java")
    )
    .unwrap());
    assert_eq!(
        memory::profile(&conn, "p_a", c.msg_id).unwrap()[0].content,
        "偏好 Rust"
    );
}

#[test]
fn topic_recall_finds_old_relevant_memories_and_scopes_owners() {
    let rig = Rig::new();
    let anchor = rig.message("p_a", "g", "Docker 部署怎么做", None, None);
    rig.fact("person", "p_a", "Docker 部署采用 compose", 1);
    rig.fact("chat", "g", "Docker 部署端口仅本机访问", 2);
    rig.fact("person", "p_b", "Docker 部署属于同名的另一个人", 999);
    rig.fact("chat", "other", "Docker 部署属于另一群", 999);
    for i in 0..200 {
        rig.fact("person", "p_a", &format!("今天晚饭吃什么 {i}"), i + 100);
    }
    let conn = db::connect(&rig.path).unwrap();
    let memories = memory::recall(&conn, "p_a", "g", anchor.msg_id, &anchor.text, 3).unwrap();
    assert_eq!(memories.len(), 2);
    assert!(memories
        .iter()
        .all(|m| m.content.contains("Docker") && !m.content.contains("另")));
    assert!(memories
        .iter()
        .all(|m| m.evidence_status == "legacy_unverified"));
    assert!(
        memory::recall(&conn, "p_a", "g", anchor.msg_id, "这个呢", 3)
            .unwrap()
            .is_empty()
    );
    assert!(
        memory::recall(&conn, "p_a", "g", anchor.msg_id, "Docker", 0)
            .unwrap()
            .is_empty()
    );
    assert!(memory::query_terms("部署方案").contains(&"部署".to_owned()));
}

#[test]
fn snapshot_preserves_anchor_identity_and_excludes_future_messages() {
    let rig = Rig::new();
    let anchor = rig.message("p_a", "g", "Docker 部署问题", Some(90), None);
    let other = rig.message("p_b", "g", "我只是在插话", Some(91), None);
    let bot = rig.message("self", "g", "以前的回答不是事实证明", Some(92), None);
    let cfg = ContextCfg::default();
    let snapshot = capture_reply_snapshot(&rig.path, &anchor, bot.msg_id, &cfg).unwrap();
    let before = render_bot_context(&snapshot, MoodValue::Calm, &cfg)
        .unwrap()
        .user;
    rig.message("p_b", "g", "未来的新话题不能混入", None, None);
    rig.fact("person", "p_a", "Docker 新记忆不能混入已取快照", 500);
    assert_eq!(
        render_bot_context(&snapshot, MoodValue::Calm, &cfg)
            .unwrap()
            .user,
        before
    );
    let value: Value = serde_json::from_str(&before).unwrap();
    assert_eq!(value["reply_target"]["person_id"], "p_a");
    assert_eq!(value["anchor"]["msg_id"], anchor.msg_id);
    assert!(snapshot
        .recent_messages
        .iter()
        .any(|m| m.msg_id == other.msg_id && m.person_id == "p_b"));
    assert!(snapshot
        .recent_messages
        .iter()
        .any(|m| m.msg_id == bot.msg_id && m.speaker_kind == "bot"));
    let again = capture_reply_snapshot(&rig.path, &anchor, bot.msg_id, &cfg).unwrap();
    assert!(!again
        .recent_messages
        .iter()
        .any(|m| m.text.contains("未来")));
}

#[test]
fn references_use_external_ids_and_never_guess_local_ids_or_other_chats() {
    let rig = Rig::new();
    let quote = rig.message("p_b", "g", "很久之前的部署原文", Some(8000), None);
    for _ in 0..30 {
        rig.message("p_b", "g", "插话", None, None);
    }
    let anchor = rig.message("p_a", "g", "这个后来怎么样", Some(8001), Some(8000));
    let cfg = ContextCfg::default();
    let s = capture_reply_snapshot(&rig.path, &anchor, anchor.msg_id, &cfg).unwrap();
    assert_eq!(s.quoted_messages[0].msg_id, quote.msg_id);
    assert_eq!(s.quoted_messages[0].person_id, "p_b");
    assert!(!s.recent_messages.iter().any(|m| m.msg_id == quote.msg_id));
    rig.message("p_b", "other", "别的群同编号原文", Some(123), None);
    let missing = rig.message("p_a", "g", "找不到引用", None, Some(123));
    let s = capture_reply_snapshot(&rig.path, &missing, missing.msg_id, &cfg).unwrap();
    assert!(s.quoted_messages.is_empty());
    let collision = rig.message(
        "p_a",
        "g",
        "外部编号碰巧等于本地编号",
        None,
        Some(quote.msg_id),
    );
    let s = capture_reply_snapshot(&rig.path, &collision, collision.msg_id, &cfg).unwrap();
    assert!(s.quoted_messages.is_empty());
    assert!(s.limitations.iter().any(|s| s.contains("未找到")));
}

#[test]
fn conflicting_external_id_is_reported_even_after_duplicate_reports() {
    let rig = Rig::new();
    rig.message("p_b", "g", "原文一", Some(40), None);
    rig.message("p_b", "g", "原文一", Some(40), None);
    rig.message("p_c", "g", "冲突原文", Some(40), None);
    let anchor = rig.message("p_a", "g", "引用哪条", None, Some(40));
    let s =
        capture_reply_snapshot(&rig.path, &anchor, anchor.msg_id, &ContextCfg::default()).unwrap();
    assert!(s.quoted_messages.is_empty());
    assert!(s.limitations.iter().any(|s| s.contains("歧义")));
}

#[test]
fn budget_preserves_target_and_quotes_or_refuses_generation() {
    let rig = Rig::new();
    let quote = rig.message("p_b", "g", "必要的引用原文", Some(88), None);
    let anchor = rig.message("p_a", "g", &"很长的当前问题".repeat(900), None, Some(88));
    let mut cutoff = anchor.msg_id;
    for _ in 0..20 {
        cutoff = rig
            .message("p_b", "g", &"插话".repeat(1800), None, None)
            .msg_id;
    }
    let cfg = ContextCfg {
        budget_chars: 8000,
        ..ContextCfg::default()
    };
    let s = capture_reply_snapshot(&rig.path, &anchor, cutoff, &cfg).unwrap();
    let rendered = render_bot_context(&s, MoodValue::Calm, &cfg).unwrap();
    assert!(rendered.system.chars().count() + rendered.user.chars().count() <= 8000);
    let value: Value = serde_json::from_str(&rendered.user).unwrap();
    assert_eq!(value["anchor"]["msg_id"], anchor.msg_id);
    assert_eq!(value["anchor"]["text_truncated"], true);
    assert_eq!(value["quoted_messages"][0]["msg_id"], quote.msg_id);
    assert_eq!(value["optional_context_pruned"], true);
    assert!(render_bot_context(
        &s,
        MoodValue::Calm,
        &ContextCfg {
            budget_chars: 100,
            ..cfg
        }
    )
    .is_err());
}

#[test]
fn daily_summaries_are_not_promoted_to_stable_profiles() {
    let rig = Rig::new();
    let anchor = rig.message("p_a", "g", "你好", None, None);
    let conn = db::connect(&rig.path).unwrap();
    conn.execute("INSERT INTO summaries(owner_type,owner_id,period,date,summary,created_at) VALUES ('person','p_a','daily','2026-10-10','他开玩笑说自己是宇航员',1)",[]).unwrap();
    let s =
        capture_reply_snapshot(&rig.path, &anchor, anchor.msg_id, &ContextCfg::default()).unwrap();
    assert!(s.profile.is_empty());
    assert!(
        !render_bot_context(&s, MoodValue::Calm, &ContextCfg::default())
            .unwrap()
            .user
            .contains("宇航员")
    );
}

#[tokio::test]
async fn oversized_required_decision_evidence_never_reaches_provider() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let rig = Rig::new();
    let text = "\u{0001}".repeat(4000); // JSON 转义后的体积显著大于原文字数。
    rig.message("p_b", "g", &text, Some(90), None);
    rig.message("p_b", "g", &text, Some(91), Some(90));
    rig.message("p_b", "g", &text, Some(92), Some(91));
    let anchor = rig.message("p_a", "g", &text, Some(93), Some(92));
    let snapshot =
        capture_reply_snapshot(&rig.path, &anchor, anchor.msg_id, &ContextCfg::default()).unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async { axum::Json(serde_json::json!({"choices":[{"message":{"content":"{}"}}]})) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let path = rig.dir.join("providers.toml");
    std::fs::write(&path,format!("[provider.mock]\nbase_url=\"http://{addr}\"\napi_key_env=\"\"\n[roles]\ndecision={{provider=\"mock\",model=\"mock\"}}\n")).unwrap();
    let gateway = yuantuan_core::llm::LlmGateway::load(&path).unwrap();
    let outcome = yuantuan_core::decision::decide(
        &rig.path,
        &gateway,
        &yuantuan_core::event::EventBus::new(8),
        &yuantuan_core::state::MoodState::default(),
        &anchor,
        None,
        &snapshot,
    )
    .await;
    assert!(outcome.fallback);
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    server.abort();
}
