//! TEST-A02：Task / Agent 执行闭环集成。
//! mock LLM provider（本地 axum server，OpenAI 兼容 /chat/completions 端点）
//! 两轮输出：tool_call(hello) → done。断言：
//! - tasks 行最终 state=finished、used_calls=2、finished_at 非空
//! - task_events 序列含 created → llm_round → tool_result(ok) → llm_round → finished
//! - Event::TaskFinished 带 final_state=finished
//! - usage_sink 写入 llm_usage（mock 返回 usage 字段）

use sqlx::Row;
use std::io::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use yuantuan_core::agent::{self, TaskRunnerDeps};
use yuantuan_core::db::SqliteExt;
use yuantuan_core::event::{Event, EventBus};
use yuantuan_core::llm::LlmGateway;
use yuantuan_core::tools::Registry;

/// 起本地 OpenAI 兼容 mock：按调用次数返回不同响应
async fn spawn_mock_llm() -> (String, Arc<AtomicUsize>) {
    spawn_mock_responses(vec![
        r#"{"action":"tool_call","tool_name":"hello","tool_args":{"who":"agent"},"reason":"验证工具链路"}"#.into(),
        r#"{"action":"done","text":"已完成：问候流程走通","reason":"工具已返回"}"#.into(),
    ]).await
}

async fn spawn_mock_responses(responses: Vec<String>) -> (String, Arc<AtomicUsize>) {
    use axum::{routing::post, Json, Router};
    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = calls.clone();
    let responses = Arc::new(responses);
    let app = Router::new().route(
        "/chat/completions",
        post(move |Json(_body): Json<serde_json::Value>| {
            let calls = calls2.clone();
            let responses = responses.clone();
            async move {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                let content = &responses[n.min(responses.len() - 1)];
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

async fn temp_db() -> (tempfile_guard::TempDir, std::path::PathBuf) {
    let dir = tempfile_guard::TempDir::new();
    let db = dir.path().join("test.db");
    let mut conn = yuantuan_core::db::connect(&db).await.expect("open test db");
    yuantuan_core::db::migrate(&mut conn)
        .await
        .expect("migrate");
    (dir, db)
}

/// 无 tempfile 依赖的最小临时目录守卫
mod tempfile_guard {
    pub struct TempDir(std::path::PathBuf);
    impl TempDir {
        pub fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let unique = format!(
                "yuantuan-agent-test-{}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
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
    let (_guard, db_path) = temp_db().await;

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
        .await
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
    let mut conn = yuantuan_core::db::connect(&db_path).await.unwrap();
    let (state, used, finished_at): (String, i64, Option<i64>) = conn
        .query_row(
            "SELECT state, used_calls, finished_at FROM tasks WHERE task_id = ?1",
            yuantuan_core::db::params![task_id],
            |r| Ok((r.try_get(0)?, r.try_get(1)?, r.try_get(2)?)),
        )
        .await
        .unwrap();
    assert_eq!(state, "finished");
    assert_eq!(used, 2);
    assert!(finished_at.is_some());

    // task_events 序列
    let mut stmt = conn
        .prepare("SELECT kind FROM task_events WHERE task_id = ?1 ORDER BY seq ASC")
        .await
        .unwrap();
    let kinds: Vec<String> = stmt
        .query_map(yuantuan_core::db::params![task_id], |r| r.try_get(0))
        .await
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(kinds.first().map(String::as_str), Some("created"));
    assert!(kinds.contains(&"llm_round".to_string()));
    assert!(kinds.contains(&"tool_result".to_string()));
    assert_eq!(kinds.last().map(String::as_str), Some("finished"));
    assert_eq!(kinds.iter().filter(|kind| *kind == "finished").count(), 1);
    _runner.abort();
    let _ = _runner.await;
}

async fn assert_task_fails(content: String, tools: Registry, expected: &str, used_calls: i64) {
    let (base_url, _) = spawn_mock_responses(vec![content]).await;
    let (guard, db_path) = temp_db().await;
    let prov_path = guard.path().join("providers.toml");
    std::fs::write(
        &prov_path,
        format!(
            r#"
[provider.mock]
base_url = "{base_url}"
api_key_env = ""
[roles]
agent_exec = {{ provider = "mock", model = "mock-model" }}
"#
        ),
    )
    .unwrap();
    let gw = Arc::new(LlmGateway::load(&prov_path).unwrap());
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let runner = agent::spawn_runner(TaskRunnerDeps {
        db_path: db_path.clone(),
        llm: Arc::new(std::sync::RwLock::new(Some(gw))),
        tools,
        bus: bus.clone(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let id = agent::create_task(&db_path, &bus, "g_test", "p_test", "failure regression")
        .await
        .unwrap();
    let finished = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Event::TaskFinished(p) = rx.recv().await.unwrap() {
                if p.task_id == id {
                    break p;
                }
            }
        }
    })
    .await
    .expect("failed tasks must reach a terminal state");
    assert_eq!(finished.final_state.as_deref(), Some("failed"));
    assert!(finished.error.unwrap().contains(expected));
    let mut conn = yuantuan_core::db::connect(&db_path).await.unwrap();
    let row: (String, i64, Option<i64>) = conn
        .query_row(
            "SELECT state, used_calls, finished_at FROM tasks WHERE task_id=?1",
            yuantuan_core::db::params![&id],
            |r| Ok((r.try_get(0)?, r.try_get(1)?, r.try_get(2)?)),
        )
        .await
        .unwrap();
    assert_eq!(row.0, "failed");
    assert_eq!(row.1, used_calls);
    assert!(row.2.is_some());
    runner.abort();
    let _ = runner.await;
}

#[tokio::test]
async fn malformed_unicode_output_fails_after_two_rounds() {
    assert_task_fails("中".repeat(100), Registry::new(), "非法 JSON", 2).await;
}

struct PanicTool;
#[async_trait::async_trait]
impl yuantuan_core::tools::Tool for PanicTool {
    fn name(&self) -> &'static str {
        "panic_tool"
    }
    fn description(&self) -> &'static str {
        "intentional panic for regression"
    }
    async fn call(
        &self,
        _: &yuantuan_core::tools::ToolCtx,
        _: serde_json::Value,
    ) -> anyhow::Result<yuantuan_core::tools::ToolOutput> {
        panic!("intentional tool panic");
    }
}

#[tokio::test]
async fn tool_panic_reaches_failed_state() {
    let tools = Registry::new();
    tools.register(PanicTool);
    assert_task_fails(
        r#"{"action":"tool_call","tool_name":"panic_tool"}"#.into(),
        tools,
        "panic",
        1,
    )
    .await;
}

fn test_deps(
    path: &std::path::Path,
    base_url: &str,
    bus: &EventBus,
    tools: Registry,
) -> TaskRunnerDeps {
    let prov = path.with_extension("providers.toml");
    std::fs::write(
        &prov,
        format!(
            r#"
[provider.mock]
base_url = "{base_url}"
api_key_env = ""
[roles]
agent_exec = {{ provider = "mock", model = "mock-model" }}
"#
        ),
    )
    .unwrap();
    TaskRunnerDeps {
        db_path: path.to_owned(),
        bus: bus.clone(),
        tools,
        llm: Arc::new(std::sync::RwLock::new(Some(Arc::new(
            LlmGateway::load(&prov).unwrap(),
        )))),
    }
}

async fn wait_finished(
    rx: &mut tokio::sync::broadcast::Receiver<Event>,
    id: &str,
) -> yuantuan_core::event::TaskLifecyclePayload {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Event::TaskFinished(p) = rx.recv().await.unwrap() {
                if p.task_id == id {
                    return p;
                }
            }
        }
    })
    .await
    .expect("task must finish")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_backlog_and_duplicate_events_execute_once_with_three_slots() {
    use axum::{routing::post, Json, Router};
    let calls = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (c, a, p) = (calls.clone(), active.clone(), peak.clone());
    let app = Router::new().route("/chat/completions", post(move || {
        let (calls, active, peak) = (c.clone(), a.clone(), p.clone());
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            active.fetch_sub(1, Ordering::SeqCst);
            Json(serde_json::json!({"choices":[{"message":{"content":r#"{"action":"done","text":"ok"}"#}}]}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (_guard, db_path) = temp_db().await;
    let bus = EventBus::new(4);
    // No runner yet: every notification is lost, but the durable tasks remain.
    let mut ids = Vec::new();
    for _ in 0..12 {
        ids.push(
            agent::create_task(&db_path, &bus, "g_test", "p_test", "backlog")
                .await
                .unwrap(),
        );
    }
    let runner = agent::spawn_runner(test_deps(&db_path, &base_url, &bus, Registry::new()));
    // Overflow the event receiver and repeat the same task event.
    for _ in 0..50 {
        bus.publish(Event::TaskCreated(
            yuantuan_core::event::TaskLifecyclePayload {
                task_id: ids[0].clone(),
                chat_id: "g_test".into(),
                goal: "backlog".into(),
                final_state: None,
                error: None,
            },
        ));
    }
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let mut conn = yuantuan_core::db::connect(&db_path).await.unwrap();
            let finished: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM tasks WHERE state='finished'",
                    yuantuan_core::db::params![],
                    |r| r.try_get(0),
                )
                .await
                .unwrap();
            if finished == 12 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("all durable tasks should run despite lost events");
    assert_eq!(calls.load(Ordering::SeqCst), 12);
    assert!((2..=3).contains(&peak.load(Ordering::SeqCst)));
    runner.abort();
    let _ = runner.await;
    server.abort();
}

struct WaitingTool {
    entered: tokio::sync::mpsc::UnboundedSender<()>,
    dropped: tokio::sync::mpsc::UnboundedSender<()>,
}

#[async_trait::async_trait]
impl yuantuan_core::tools::Tool for WaitingTool {
    fn name(&self) -> &'static str {
        "wait"
    }
    fn description(&self) -> &'static str {
        "wait until cancelled"
    }
    async fn call(
        &self,
        _: &yuantuan_core::tools::ToolCtx,
        _: serde_json::Value,
    ) -> anyhow::Result<yuantuan_core::tools::ToolOutput> {
        struct Dropped(tokio::sync::mpsc::UnboundedSender<()>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let _guard = Dropped(self.dropped.clone());
        self.entered.send(()).unwrap();
        std::future::pending().await
    }
}

async fn waiting_task() -> (
    tempfile_guard::TempDir,
    TaskRunnerDeps,
    String,
    tokio::sync::broadcast::Receiver<Event>,
    tokio::task::JoinHandle<()>,
    tokio::sync::mpsc::UnboundedReceiver<()>,
) {
    let (url, _) =
        spawn_mock_responses(vec![r#"{"action":"tool_call","tool_name":"wait"}"#.into()]).await;
    let (guard, path) = temp_db().await;
    let bus = EventBus::new(64);
    let rx = bus.subscribe();
    let (entered, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let (dropped, dropped_rx) = tokio::sync::mpsc::unbounded_channel();
    let tools = Registry::new();
    tools.register(WaitingTool { entered, dropped });
    let deps = test_deps(&path, &url, &bus, tools);
    let runner = agent::spawn_runner(deps.clone());
    let id = agent::create_task(&path, &bus, "g_test", "p_test", "waiting")
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx.recv())
        .await
        .unwrap()
        .unwrap();
    (guard, deps, id, rx, runner, dropped_rx)
}

#[tokio::test]
async fn runner_cancellation_drops_tools_and_restart_fails_interrupted_task() {
    let (_guard, deps, id, mut rx, runner, mut dropped) = waiting_task().await;
    runner.abort();
    let _ = runner.await;
    tokio::time::timeout(std::time::Duration::from_secs(1), dropped.recv())
        .await
        .unwrap()
        .unwrap();
    let restarted = agent::spawn_runner(deps);
    let p = wait_finished(&mut rx, &id).await;
    assert_eq!(p.final_state.as_deref(), Some("failed"));
    assert!(p.error.unwrap().contains("被中断"));
    restarted.abort();
    let _ = restarted.await;
}

#[tokio::test]
async fn stalled_tool_times_out_without_retrying_side_effects() {
    let (_guard, _deps, id, mut rx, runner, mut dropped) = waiting_task().await;
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(121)).await;
    tokio::task::yield_now().await;
    tokio::time::resume();
    let p = wait_finished(&mut rx, &id).await;
    assert_eq!(p.final_state.as_deref(), Some("failed"));
    assert!(p.error.unwrap().contains("超时"));
    assert_eq!(dropped.recv().await, Some(()));
    runner.abort();
    let _ = runner.await;
}

#[tokio::test]
async fn terminal_write_failure_rolls_back_and_retries_without_reexecution() {
    let (url, calls) = spawn_mock_responses(vec![r#"{"action":"done","text":"ok"}"#.into()]).await;
    let (_guard, path) = temp_db().await;
    let mut conn = yuantuan_core::db::connect(&path).await.unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_finished BEFORE INSERT ON task_events WHEN NEW.kind='finished'
        BEGIN SELECT RAISE(ABORT,'injected failure'); END;",
    )
    .await
    .unwrap();
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let runner = agent::spawn_runner(test_deps(&path, &url, &bus, Registry::new()));
    let id = agent::create_task(&path, &bus, "g_test", "p_test", "commit retry")
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let row: (String, Option<i64>) = conn
        .query_row(
            "SELECT state,finished_at FROM tasks WHERE task_id=?1",
            yuantuan_core::db::params![&id],
            |r| Ok((r.try_get(0)?, r.try_get(1)?)),
        )
        .await
        .unwrap();
    assert_eq!(row, ("running".into(), None));
    while let Ok(event) = rx.try_recv() {
        assert!(!matches!(event, Event::TaskFinished(_)));
    }
    conn.execute_batch("DROP TRIGGER reject_finished")
        .await
        .unwrap();
    assert_eq!(
        wait_finished(&mut rx, &id).await.final_state.as_deref(),
        Some("finished")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM task_events WHERE kind='finished'",
            yuantuan_core::db::params![],
            |r| r.try_get::<i64, _>(0)
        )
        .await
        .unwrap(),
        1
    );
    runner.abort();
    let _ = runner.await;
}

#[tokio::test]
async fn task_creation_is_atomic_with_its_audit_event() {
    let (_guard, path) = temp_db().await;
    let mut conn = yuantuan_core::db::connect(&path).await.unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_created BEFORE INSERT ON task_events WHEN NEW.kind='created'
        BEGIN SELECT RAISE(ABORT,'injected failure'); END;",
    )
    .await
    .unwrap();
    let bus = EventBus::new(16);
    let mut rx = bus.subscribe();
    assert!(
        agent::create_task(&path, &bus, "g_test", "p_test", "atomic create")
            .await
            .is_err()
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM tasks",
            yuantuan_core::db::params![],
            |r| r.try_get::<i64, _>(0)
        )
        .await
        .unwrap(),
        0
    );
    assert!(matches!(
        rx.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
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
        ctx: &yuantuan_core::tools::ToolCtx,
        args: serde_json::Value,
    ) -> anyhow::Result<yuantuan_core::tools::ToolOutput> {
        assert!(ctx.task_id.as_ref().is_some_and(|id| !id.is_empty()));
        let who = args.get("who").and_then(|v| v.as_str()).unwrap_or("world");
        Ok(yuantuan_core::tools::ToolOutput {
            summary: format!("hello, {who}!"),
            artifacts: vec![],
            data: serde_json::json!({"greeted": who}),
        })
    }
}
