//! Agent 系统：Task 工具循环执行器（Q-A01 同步工具循环裁决）。
//!
//! 链路：Decision(start_task + task_goal) → bot.rs INSERT tasks + TaskCreated 事件 →
//!       本模块 spawn_runner 订阅 TaskCreated → 每任务一协程跑 run_task 循环：
//!
//! 每轮：agent_exec LLM 返回 JSON `{ action: tool_call | reply | done, ... }`
//!   - tool_call → 查 ToolRegistry → tool.call() → 结果拼回上下文 → 继续
//!   - reply     → 记 task_events(kind=reply) 中间思考 → 继续
//!   - done      → 记 finished + TaskFinished 事件 → 退出
//!
//! 终止条件（任一命中）：done / budget 用完（Q-A04 固定 10 轮）/ 连续 2 次 JSON 解析失败 /
//!   tool 连续 3 次抛错 / agent_exec 角色未配置
//!
//! 铁律：core 不依赖协议端；任务状态不发群消息（Q-A03 静默），只走 tasks/task_events 表 + 事件。

use crate::event::{Event, EventBus, TaskLifecyclePayload};
use crate::llm::{LlmGateway, Role};
use crate::tools::{Registry, ToolCtx};
use anyhow::{Context, Result};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// agent_exec 角色共享槽（与 bot.rs 同一形态：WebUI 热应用换槽即生效）
pub type SharedLlm = Arc<std::sync::RwLock<Option<Arc<LlmGateway>>>>;

/// 固定预算（Q-A04）
pub const TASK_BUDGET_MAX_CALLS: i64 = 10;
/// LLM 输出连续非法 JSON 轮数上限，超过则任务 failed
const MAX_PARSE_FAILURES: u32 = 2;
/// tool 连续抛错上限
const MAX_TOOL_FAILURES: u32 = 3;

pub struct TaskRunnerDeps {
    pub db_path: PathBuf,
    pub llm: SharedLlm,
    pub tools: Registry,
    pub bus: EventBus,
}

/// 订阅 TaskCreated → 每个新 task spawn 一个执行协程
pub fn spawn_runner(deps: TaskRunnerDeps) -> JoinHandle<()> {
    tokio::spawn(async move {
        let deps = Arc::new(deps);
        let mut rx = deps.bus.subscribe();
        info!("Task runner 已启动（等 TaskCreated 事件）");
        loop {
            match rx.recv().await {
                Ok(Event::TaskCreated(p)) => {
                    let deps = deps.clone();
                    tokio::spawn(async move {
                        if let Err(e) = run_task(&deps, &p).await {
                            warn!(task_id = %p.task_id, error = %e, "任务执行异常");
                            finish_task(
                                &deps.db_path,
                                &deps.bus,
                                &p,
                                TaskState::Failed,
                                None,
                                Some(format!("runner 异常: {e:#}")),
                            );
                        }
                    });
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "Task runner 消费滞后，跳过旧事件");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    info!("Task runner 退出：事件总线已关闭");
                    break;
                }
            }
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Finished,
    Failed,
}

impl TaskState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskState::Finished => "finished",
            TaskState::Failed => "failed",
        }
    }
}

/// LLM 每轮输出协议
#[derive(Debug, Deserialize)]
pub struct RoundOutput {
    pub action: String,
    pub tool_name: Option<String>,
    pub tool_args: Option<Value>,
    pub text: Option<String>,
    pub reason: Option<String>,
}

/// 执行单个任务到终态
async fn run_task(deps: &TaskRunnerDeps, p: &TaskLifecyclePayload) -> Result<()> {
    let task_id = p.task_id.clone();
    let gw = deps
        .llm
        .read()
        .unwrap()
        .clone()
        .filter(|g| g.role(Role::AgentExec).is_some());
    let Some(gw) = gw else {
        warn!(task_id = %task_id, "agent_exec 角色未配置，任务直接 failed");
        finish_task(&deps.db_path, &deps.bus, p, TaskState::Failed, None, Some("agent_exec 角色未配置".into()));
        return Ok(());
    };

    let tools_catalog = build_tools_catalog(&deps.tools);
    let system = build_system_prompt(&tools_catalog);
    // 对话历史：user0 = goal；此后每轮把 assistant 输出 + tool 结果拼回
    let mut history: Vec<Value> = vec![json!({"role": "user", "content": format!("任务目标：{}", p.goal)})];

    let mut used_calls: i64 = 0;
    let mut parse_failures = 0u32;
    let mut tool_failures = 0u32;

    loop {
        if used_calls >= TASK_BUDGET_MAX_CALLS {
            info!(task_id = %task_id, used_calls, "budget 用完，任务 failed");
            finish_task(&deps.db_path, &deps.bus, p, TaskState::Failed, None,
                Some(format!("budget 用完（{TASK_BUDGET_MAX_CALLS} 轮）")));
            return Ok(());
        }

        let user = render_history(&history);
        let t0 = std::time::Instant::now();
        let raw = gw.chat(Role::AgentExec, &system, &user, true).await;
        used_calls += 1;
        bump_used_calls(&deps.db_path, &task_id, used_calls);

        let content = match raw {
            Ok(c) => c,
            Err(e) => {
                warn!(task_id = %task_id, error = %e, "agent_exec 调用失败");
                record_event(&deps.db_path, &task_id, "llm_error",
                    json!({"error": e.to_string(), "round": used_calls}));
                finish_task(&deps.db_path, &deps.bus, p, TaskState::Failed, None,
                    Some(format!("LLM 调用失败: {e}")));
                return Ok(());
            }
        };

        let round: RoundOutput = match parse_round(&content) {
            Ok(r) => {
                parse_failures = 0;
                r
            }
            Err(err) => {
                parse_failures += 1;
                record_event(&deps.db_path, &task_id, "parse_error",
                    json!({"round": used_calls, "error": err, "raw_preview": &content[..content.len().min(200)]}));
                if parse_failures >= MAX_PARSE_FAILURES {
                    finish_task(&deps.db_path, &deps.bus, p, TaskState::Failed, None,
                        Some(format!("连续 {MAX_PARSE_FAILURES} 轮输出非法 JSON")));
                    return Ok(());
                }
                // 把错误反馈给 LLM 让它下一轮修正
                history.push(json!({"role": "user", "content": format!(
                    "你的上一次输出未通过 JSON 校验：{err}。请严格按协议只输出 JSON。")}));
                continue;
            }
        };

        record_event(&deps.db_path, &task_id, "llm_round", json!({
            "round": used_calls,
            "action": round.action,
            "tool_name": round.tool_name,
            "reason": round.reason,
            "elapsed_ms": t0.elapsed().as_millis() as u64,
        }));

        match round.action.as_str() {
            "done" => {
                let final_text = round.text.unwrap_or_else(|| "(未给出最终答案)".into());
                record_event(&deps.db_path, &task_id, "finished",
                    json!({"round": used_calls, "final_text": final_text}));
                finish_task(&deps.db_path, &deps.bus, p, TaskState::Finished, Some(final_text), None);
                info!(task_id = %task_id, used_calls, "任务 done");
                return Ok(());
            }
            "reply" => {
                let text = round.text.clone().unwrap_or_default();
                record_event(&deps.db_path, &task_id, "reply",
                    json!({"round": used_calls, "text": text}));
                history.push(json!({"role": "assistant", "content": content}));
                history.push(json!({"role": "user", "content":
                    "已记录你的中间思考。继续：调用工具或返回 done。"}));
            }
            "tool_call" => {
                let Some(name) = round.tool_name.clone() else {
                    history.push(json!({"role": "user", "content":
                        "protocol error: action=tool_call 必须带 tool_name"}));
                    continue;
                };
                let Some(tool) = deps.tools.get(&name) else {
                    history.push(json!({"role": "user", "content": format!(
                        "tool `{name}` 未注册。可用工具见系统提示列表，别编造。")}));
                    continue;
                };
                let args = round.tool_args.unwrap_or(json!({}));
                let ctx = ToolCtx {
                    chat_id: p.chat_id.clone(),
                    chat_type: "group".into(),
                    sender_pid: "task_runner".into(),
                    locale: Some("zh-CN".into()),
                };
                history.push(json!({"role": "assistant", "content": content}));
                match tool.call(&ctx, args).await {
                    Ok(out) => {
                        tool_failures = 0;
                        record_event(&deps.db_path, &task_id, "tool_result", json!({
                            "round": used_calls, "tool": name, "ok": true,
                            "summary": out.summary,
                        }));
                        history.push(json!({"role": "user", "content": format!(
                            "工具 `{name}` 返回：\n{}\n数据：{}",
                            out.summary,
                            serde_json::to_string_pretty(&out.data).unwrap_or_default()
                        )}));
                    }
                    Err(e) => {
                        tool_failures += 1;
                        record_event(&deps.db_path, &task_id, "tool_result", json!({
                            "round": used_calls, "tool": name, "ok": false,
                            "error": e.to_string(),
                        }));
                        if tool_failures >= MAX_TOOL_FAILURES {
                            finish_task(&deps.db_path, &deps.bus, p, TaskState::Failed, None,
                                Some(format!("连续 {MAX_TOOL_FAILURES} 次工具调用失败（最后: {name}: {e}）")));
                            return Ok(());
                        }
                        history.push(json!({"role": "user", "content": format!(
                            "工具 `{name}` 抛错：{e}。可换工具或直接 done。")}));
                    }
                }
            }
            other => {
                history.push(json!({"role": "user", "content": format!(
                    "未知 action `{other}`。允许值：tool_call / reply / done。")}));
            }
        }
    }
}

fn finish_task(
    db_path: &std::path::Path,
    bus: &EventBus,
    p: &TaskLifecyclePayload,
    state: TaskState,
    final_text: Option<String>,
    error: Option<String>,
) {
    let now = now_secs();
    if let Ok(conn) = crate::db::connect(db_path) {
        let _ = conn.execute(
            "UPDATE tasks SET state = ?1, finished_at = ?2 WHERE task_id = ?3",
            params![state.as_str(), now, p.task_id],
        );
        let kind = match state {
            TaskState::Finished => "finished",
            TaskState::Failed => "failed",
        };
        record_event(db_path, &p.task_id, kind, json!({
            "final_text": final_text,
            "error": error,
        }));
    }
    bus.publish(Event::TaskFinished(TaskLifecyclePayload {
        task_id: p.task_id.clone(),
        chat_id: p.chat_id.clone(),
        goal: p.goal.clone(),
        final_state: Some(state.as_str().into()),
        error,
    }));
}

/// 用完即更新 used_calls（面板进度条数据源）
fn bump_used_calls(db_path: &std::path::Path, task_id: &str, used: i64) {
    if let Ok(conn) = crate::db::connect(db_path) {
        let _ = conn.execute(
            "UPDATE tasks SET used_calls = ?1 WHERE task_id = ?2",
            params![used, task_id],
        );
    }
}

/// task_events seq 自增：取当前 max(seq)+1
fn record_event(db_path: &std::path::Path, task_id: &str, kind: &str, payload: Value) {
    let Ok(conn) = crate::db::connect(db_path) else { return };
    let next_seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM task_events WHERE task_id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .unwrap_or(1);
    let _ = conn.execute(
        "INSERT INTO task_events(task_id, seq, kind, payload, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![task_id, next_seq, kind, payload.to_string(), now_secs()],
    );
}

/// 把对话历史塞回单个 user prompt（agent_exec 无多轮 messages 接口限制——chat() 只接受 system+user）
fn render_history(history: &[Value]) -> String {
    let mut out = String::from("以下是你到目前为止的执行记录（按时间序）：\n\n");
    for (i, msg) in history.iter().enumerate() {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("?");
        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!("--- [{}] {} ---\n{}\n\n", i + 1, role, content));
    }
    out.push_str("请输出下一轮 JSON（action: tool_call / reply / done）：");
    out
}

/// tools → 目录片段（含 mcp:*），每工具一行
fn build_tools_catalog(registry: &Registry) -> String {
    let mut names = registry.names();
    names.sort();
    if names.is_empty() {
        return "（当前无可用工具；只能 reply / done）".into();
    }
    let mut out = String::new();
    for n in names {
        if let Some(t) = registry.get(n) {
            out.push_str(&format!("- {n}: {}\n", t.description()));
        }
    }
    out
}

fn build_system_prompt(tools_catalog: &str) -> String {
    format!(
        r#"你是云团的 agent_exec：一个严格执行任务的循环执行体。每轮只输出严格 JSON，不要任何其他文字。

每轮输出协议（四选一 action）：
{{"action": "tool_call", "tool_name": "<工具名>", "tool_args": {{...}}, "reason": "一句话"}}
{{"action": "reply", "text": "中间思考 / 阶段性小结", "reason": "一句话"}}
{{"action": "done", "text": "最终答案（给用户看的完成态陈述）", "reason": "一句话"}}

规则：
- tool_args 必须符合目标工具语义；不知道传什么就先 reply 思考
- 信息足够就尽快 done；别为了凑轮数无意义调工具
- done 的 text 是给用户的最终交付：直接陈述结果，别解释过程

可用工具（name: 描述）：
{tools_catalog}
"#
    )
}

/// 解析一轮输出：容忍 ```json 围栏；严格字段
pub fn parse_round(content: &str) -> std::result::Result<RoundOutput, String> {
    let s = content.trim();
    let s = if let Some(rest) = s.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        rest.strip_suffix("```").map(|x| x.trim()).unwrap_or(rest.trim())
    } else {
        s
    };
    serde_json::from_str::<RoundOutput>(s).map_err(|e| e.to_string())
}

/// 由 bot.rs 调用：Decision(start_task + task_goal) → 建任务行 + 发 TaskCreated 事件
/// 返回 task_id（ULID 风格的时间戳+随机后缀，避免新依赖）
pub fn create_task(
    db_path: &std::path::Path,
    bus: &EventBus,
    chat_id: &str,
    created_by_pid: &str,
    goal: &str,
) -> Result<String> {
    let task_id = new_task_id();
    let now = now_secs();
    let conn = crate::db::connect(db_path).context("打开数据库失败")?;
    conn.execute(
        "INSERT INTO tasks(task_id, goal, state, budget_max_calls, used_calls, created_by_pid, chat_id, created_at)
         VALUES (?1, ?2, 'running', ?3, 0, ?4, ?5, ?6)",
        params![task_id, goal, TASK_BUDGET_MAX_CALLS, created_by_pid, chat_id, now],
    ).context("插入 tasks 失败")?;
    record_event_static(&conn, &task_id, "created", json!({
        "goal": goal,
        "budget_max_calls": TASK_BUDGET_MAX_CALLS,
        "created_by_pid": created_by_pid,
    }));
    let payload = TaskLifecyclePayload {
        task_id: task_id.clone(),
        chat_id: chat_id.to_string(),
        goal: goal.to_string(),
        final_state: None,
        error: None,
    };
    bus.publish(Event::TaskCreated(payload));
    info!(task_id = %task_id, chat_id, goal, "任务已创建");
    Ok(task_id)
}

fn record_event_static(conn: &rusqlite::Connection, task_id: &str, kind: &str, payload: Value) {
    let next_seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM task_events WHERE task_id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .unwrap_or(1);
    let _ = conn.execute(
        "INSERT INTO task_events(task_id, seq, kind, payload, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![task_id, next_seq, kind, payload.to_string(), now_secs()],
    );
}

fn new_task_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let rand: u32 = rand::random();
    format!("t_{nanos:x}_{rand:08x}")
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_accepts_tool_call() {
        let j = r#"{"action":"tool_call","tool_name":"hello","tool_args":{"who":"x"},"reason":"试一下"}"#;
        let r = parse_round(j).unwrap();
        assert_eq!(r.action, "tool_call");
        assert_eq!(r.tool_name.as_deref(), Some("hello"));
    }

    #[test]
    fn parse_round_accepts_done() {
        let j = r#"{"action":"done","text":"最终答案","reason":"完成"}"#;
        let r = parse_round(j).unwrap();
        assert_eq!(r.action, "done");
        assert_eq!(r.text.as_deref(), Some("最终答案"));
    }

    #[test]
    fn parse_round_accepts_fenced() {
        let j = "```json\n{\"action\":\"reply\",\"text\":\"想一下\"}\n```";
        assert!(parse_round(j).is_ok());
    }

    #[test]
    fn parse_round_rejects_garbage() {
        assert!(parse_round("这不是 JSON").is_err());
    }

    #[test]
    fn new_task_id_unique() {
        assert_ne!(new_task_id(), new_task_id());
    }

    #[test]
    fn task_state_str() {
        assert_eq!(TaskState::Finished.as_str(), "finished");
        assert_eq!(TaskState::Failed.as_str(), "failed");
    }
}
