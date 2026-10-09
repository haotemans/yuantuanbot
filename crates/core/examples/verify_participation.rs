//! 使用固定场景检查真实模型的语义参与判断；只操作临时库，不连接 QQ、不执行任务。
use anyhow::{ensure, Result};
use serde::Deserialize;
use serde_json::json;
use yuantuan_core::db::SqliteExt;
use yuantuan_core::{context_builder, db, decision, event, llm, state};

#[derive(Deserialize)]
struct Case {
    id: String,
    text: String,
    expected_action: String,
    #[serde(default)]
    at_me: bool,
    #[serde(default)]
    busy: bool,
    prior_owner: Option<String>,
    prior_reply: Option<String>,
    expected_mode: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("providers.toml path required");
    let gateway = llm::LlmGateway::load(std::path::Path::new(&path))?;
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../tests/fixtures/participation_cases.json"))?;
    let dir = std::env::temp_dir().join(format!(
        "yt-participation-probe-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    std::fs::create_dir(&dir)?;
    let db_path = dir.join("test.db");
    let mut conn = db::connect(&db_path).await?;
    db::migrate(&mut conn).await?;
    conn.execute_batch("INSERT INTO persons(person_id,display_name,first_seen,last_seen) VALUES ('p_100','测试成员',1,1),('p_200','其他成员',1,1),('self','云团',1,1)").await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let mut failed = 0;
    let mut retries = 0;
    for (i, case) in cases.iter().enumerate() {
        let chat = (2000 + i).to_string();
        if let Some(reply) = &case.prior_reply {
            let owner = if case.prior_owner.as_deref() == Some("sender") {
                "p_100"
            } else {
                "p_200"
            };
            conn.execute("INSERT INTO messages(chat_id,chat_type,sender_pid,text,ts) VALUES (?1,'group',?2,'帮我解释一下方案',?3)",yuantuan_core::db::params![chat,owner,now-10]).await?;
            let anchor = conn.last_insert_rowid().await?;
            conn.execute("INSERT INTO messages(chat_id,chat_type,sender_pid,text,ts,reply_anchor_id) VALUES (?1,'group','self',?2,?3,?4)",yuantuan_core::db::params![chat,reply,now-5,anchor]).await?;
        }
        if case.busy {
            for _ in 0..20 {
                conn.execute("INSERT INTO messages(chat_id,chat_type,sender_pid,text,ts) VALUES (?1,'group','p_200','大家正在讨论周末安排',?2)",yuantuan_core::db::params![chat,now-1]).await?;
            }
            for _ in 0..6 {
                conn.execute("INSERT INTO messages(chat_id,chat_type,sender_pid,text,ts) VALUES (?1,'group','self','之前的回复',?2)",yuantuan_core::db::params![chat,now-1]).await?;
            }
        }
        conn.execute("INSERT INTO messages(chat_id,chat_type,sender_pid,text,ts) VALUES (?1,'group','p_100',?2,?3)",yuantuan_core::db::params![chat,case.text,now]).await?;
        let msg = event::MessageReceivedPayload {
            msg_id: conn.last_insert_rowid().await?,
            chat_id: chat,
            chat_type: "group".into(),
            sender_pid: "p_100".into(),
            text: case.text.clone(),
            at_me: case.at_me,
            has_image: false,
            reply_to: None,
            sender_bot: false,
            image_urls: vec![],
            ts: now,
        };
        let snapshot = context_builder::capture_reply_snapshot(
            &db_path,
            &msg,
            msg.msg_id,
            &context_builder::ContextCfg::default(),
        )
        .await?;
        let out = decision::decide(
            &db_path,
            &gateway,
            &event::EventBus::default(),
            &state::MoodState::default(),
            &msg,
            None,
            &snapshot,
        )
        .await;
        let mode = serde_json::to_value(out.output.reply_mode)?;
        let passed = !out.fallback
            && out.policy.evidence_valid == Some(true)
            && out.output.action.as_str() == case.expected_action
            && case
                .expected_mode
                .as_ref()
                .is_none_or(|s| mode.as_str() == Some(s.as_str()));
        failed += usize::from(!passed);
        retries += out.retries;
        println!(
            "{}",
            json!({"case":case.id,"passed":passed,"expected_action":case.expected_action,"output":out.output,"policy":out.policy,"retries":out.retries,"elapsed_ms":out.elapsed_ms})
        );
    }
    println!(
        "{}",
        json!({"cases":cases.len(),"failed":failed,"retries":retries,"note":"固定合成样本；不代表真实群聊准确率"})
    );
    drop(conn);
    std::fs::remove_dir_all(dir)?;
    ensure!(failed == 0, "{failed} 个场景未达到预期，检查输出与判断依据");
    Ok(())
}
