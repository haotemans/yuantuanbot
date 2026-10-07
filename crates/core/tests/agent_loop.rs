//! TEST-A02：Task / Agent 执行闭环集成。
//! mock LLM provider（本地 axum server，OpenAI 兼容 /chat/completions 端点）
//! 两轮输出：tool_call(hello) → done。断言：
//! - tasks 行最终 state=finished、used_calls=2、finished_at 非空
//! - task_events 序列含 created → llm_round → tool_result(ok) → llm_round → finished
//! - Event::TaskFinished 带 final_state=finished
//! - usage_sink 写入 llm_usage（mock 返回 usage 字段）

use std::io::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use yuantuan_core::agent::{self, TaskRunnerDeps};
use yuantuan_core::event::{Event, EventBus};
use yuantuan_core::llm::LlmGateway;
use yuantuan_core::tools::Registry;

/// 起本地 OpenAI 兼容 mock：按调用次数返回不同响应
async fn spawn_mock_llm() -> (String, Arc<AtomicUsize>) {
    use axum::{routing::post, Json, Router};
    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = calls.clone();
    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(_body): Json<serde_json::Value>| {
            let calls = calls2.clone();
            async move {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                let content = match n {
                    0 => r#"{"action":"tool_call","tool_name":"hello","tool_args":{"who":"agent"},"reason":"先打个招呼验证工具链路"}"#,
                    _ => r#"{"action":"done","text":"已完成：问候流程走通","reason":"工具已返回"}"#,
                };
                Json(serde_json::json!({
                    "id": "chatcmpl-mock",
                    "object": "chat.completion",
                    "created": 1,
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": content},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18}
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), calls)
}

fn temp_db() -> (tempfile_guard::TempDir, std::path::PathBuf) {
    let dir = tempfile_guard::TempDir::new();
    let db = dir.path().join("test.db");
    let mut conn = yuantuan_core::db::connect(&db).expect("open test db");
    yuantuan_core::db::migrate(&mut conn).expect("migrate");
    (dir, db)
}

/// 无 tempfile 依赖的最小临时目录守卫
mod tempfile_guard {
    pub struct TempDir(std::path::PathBuf);
    impl TempDir {
        pub fn new() -> Self {
            let unique = format!(
                "yuantuan-agent-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            let p = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_loop_tool_call_then_done() {
    let (base_url, calls) = spawn_mock_llm().await;
    let (_guard, db_path) = temp_db();

    // providers.toml 写入临时文件：三角色全绑 mock provider
    let prov_path = _guard.path().join("providers.toml");
    let mut f = std::fs::File::create(&prov_path).unwrap();
    write!(
        f,
        r#"
[provider.mock]
base_url = "{base_url}"
api_key_env = ""

[roles]
decision = {{ provider = "mock", model = "mock-model" }}
bot_chat = {{ provider = "mock", model = "mock-model" }}
agent_exec = {{ provider = "mock", model = "mock-model" }}
"#
    )
    .unwrap();
    drop(f);

    let gw: Arc<LlmGateway> = Arc::new(LlmGateway::load(&prov_path).expect("load providers"));
    let llm_slot: agent::SharedLlm = Arc::new(std::sync::RwLock::new(Some(gw)));

    // 用 hello 插件同形态的最小 Tool 注册（避免依赖插件 crate）
    let tools = Registry::new();
    tools.register(HelloToolForTest);

    let bus = EventBus::new(64);
    let mut finished_rx = bus.subscribe();

    let _runner = agent::spawn_runner(TaskRunnerDeps {
        db_path: db_path.clone(),
        llm: llm_slot,
        tools,
        bus: bus.clone(),
    });
    // 让 runner 先订阅上再发事件
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let task_id = agent::create_task(&db_path, &bus, "g_test", "p_tester", "验证闭环")
        .expect("create_task");
    let task_id_for_wait = task_id.clone();

    // 等 TaskFinished 事件
    let finished_evt = tokio::time::timeout(std::time::Duration::from_secs(10), async move {
        loop {
            match finished_rx.recv().await {
                Ok(Event::TaskFinished(p)) if p.task_id == task_id_for_wait => break p,
                Ok(_) => continue,
                Err(e) => panic!("事件通道异常: {e}"),
            }
        }
    })
    .await
    .expect("超时未等到 TaskFinished");

    assert_eq!(finished_evt.final_state.as_deref(), Some("finished"));
    assert!(finished_evt.error.is_none());
    assert!(calls.load(Ordering::SeqCst) >= 2, "LLM 至少被调 2 轮");

    // tasks 行终态
    let conn = yuantuan_core::db::connect(&db_path).unwrap();
    let (state, used, finished_at): (String, i64, Option<i64>) = conn
        .query_row(
            "SELECT state, used_calls, finished_at FROM tasks WHERE task_id = ?1",
            rusqlite::params![task_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, "finished");
    assert_eq!(used, 2);
    assert!(finished_at.is_some());

    // task_events 序列
    let mut stmt = conn
        .prepare("SELECT kind FROM task_events WHERE task_id = ?1 ORDER BY seq ASC")
        .unwrap();
    let kinds: Vec<String> = stmt
        .query_map(rusqlite::params![task_id], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(kinds.first().map(String::as_str), Some("created"));
    assert!(kinds.contains(&"llm_round".to_string()));
    assert!(kinds.contains(&"tool_result".to_string()));
    assert_eq!(kinds.last().map(String::as_str), Some("finished"));
}

/// 最小 Tool：和 hello 插件同形态（独立实现避免依赖插件 crate）
struct HelloToolForTest;

#[async_trait::async_trait]
impl yuantuan_core::tools::Tool for HelloToolForTest {
    fn name(&self) -> &'static str {
        "hello"
    }
    fn description(&self) -> &'static str {
        "打招呼测试工具"
    }
    async fn call(
        &self,
        _ctx: &yuantuan_core::tools::ToolCtx,
        args: serde_json::Value,
    ) -> anyhow::Result<yuantuan_core::tools::ToolOutput> {
        let who = args.get("who").and_then(|v| v.as_str()).unwrap_or("world");
        Ok(yuantuan_core::tools::ToolOutput {
            summary: format!("hello, {who}!"),
            artifacts: vec![],
            data: serde_json::json!({"greeted": who}),
        })
    }
}
