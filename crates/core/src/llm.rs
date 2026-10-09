//! LLM Provider 层（架构文档十五章）：OpenAI 兼容 chat/completions，providers.toml 多 provider 配置，
//! 运行时消费三角色（decision 便宜快速 / bot_chat 中高档 / agent_exec 强模型），角色独立不合并。
//! 角色未配置或 api_key 环境变量缺失 → 该角色不可用（warn 日志），调用方自行降级。
//! 成本闸：全局滑动窗口 30 次/分，超限排队延迟不丢弃（架构十三章节流数值）。

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// Q53：全局 LLM 并发上限（runtime-design 三章「全局 LLM 请求并发上限固定为 4」）
pub const DEFAULT_LLM_CONCURRENCY: usize = 4;

/// 三模型角色（架构十五章表格）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Decision,
    BotChat,
    AgentExec,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Role::Decision => "decision",
            Role::BotChat => "bot_chat",
            Role::AgentExec => "agent_exec",
        })
    }
}

impl Role {
    /// "decision" | "bot_chat" | "agent_exec" → Role（WebUI /api/llm/test 入参解析）
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "decision" => Some(Role::Decision),
            "bot_chat" => Some(Role::BotChat),
            "agent_exec" => Some(Role::AgentExec),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ProvidersFile {
    #[serde(default)]
    provider: HashMap<String, ProviderCfg>,
    #[serde(default)]
    roles: RolesCfg,
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderCfg {
    base_url: String,
    /// 指向环境变量名；留空表示无需密钥（如本地 mock）
    #[serde(default)]
    api_key_env: String,
    #[allow(dead_code)]
    #[serde(default)]
    models: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RolesCfg {
    decision: Option<RoleBind>,
    bot_chat: Option<RoleBind>,
    agent_exec: Option<RoleBind>,
}

#[derive(Debug, Clone, Deserialize)]
struct RoleBind {
    provider: String,
    model: String,
}

/// 解析后的角色绑定（可直接发请求）
#[derive(Debug, Clone)]
pub struct ResolvedRole {
    pub provider: String,
    pub model: String,
    base_url: String,
    api_key: Option<String>,
}

impl ResolvedRole {
    /// 诊断只报告密钥是否配置及长度，不输出任何密钥片段。
    pub fn debug_descriptor(&self) -> String {
        let key_info = match &self.api_key {
            None => "None".to_string(),
            Some(k) => format!("configured len={}", k.len()),
        };
        format!(
            "provider={} model={} base_url={} api_key={}",
            self.provider, self.model, self.base_url, key_info
        )
    }
}

/// 判断 api_key_env 字段值是「直接密钥」还是「环境变量名」。
/// 规则:以 sk- 开头(主流 LLM 密钥前缀)或包含环境变量名禁用字符(- / 空格 等)→ 直接密钥;
/// 否则视为环境变量名。
pub fn looks_like_direct_key(value: &str) -> bool {
    value.starts_with("sk-")
        || value
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '_'))
}

/// 解析 api_key_env 字段 → 实际的 key。空 → None;直接密钥 → 自身;
/// 环境变量名 → process env 查找,缺失/为空 → None(调用方自行告警)
pub fn resolve_api_key(api_key_env: &str) -> Option<String> {
    let v = api_key_env.trim();
    if v.is_empty() {
        return None;
    }
    if looks_like_direct_key(v) {
        return Some(v.to_string());
    }
    std::env::var(v).ok().filter(|s| !s.is_empty())
}

/// 全局成本闸：滑动窗口计数，超限排队延迟不丢弃；per_min 运行时可调（节流热应用）
#[derive(Debug, Clone)]
pub struct CostGate {
    inner: Arc<Mutex<VecDeque<Instant>>>,
    per_min: Arc<AtomicUsize>,
}

impl CostGate {
    pub fn new(per_min: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::new())),
            per_min: Arc::new(AtomicUsize::new(per_min.max(1))),
        }
    }

    pub fn per_min(&self) -> usize {
        self.per_min.load(Ordering::Relaxed)
    }

    /// 热应用：调整每分钟许可数（下限 1）
    pub fn set_per_min(&self, n: usize) {
        self.per_min.store(n.max(1), Ordering::Relaxed);
    }

    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut q = self.inner.lock().unwrap();
                let cutoff = Instant::now() - Duration::from_secs(60);
                while q.front().map(|t| *t < cutoff).unwrap_or(false) {
                    q.pop_front();
                }
                if q.len() < self.per_min() {
                    q.push_back(Instant::now());
                    return;
                }
                (*q.front().unwrap() + Duration::from_secs(60))
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(500))
            };
            tracing::info!(per_min = self.per_min(), "Decision 成本闸已满，排队延迟");
            tokio::time::sleep(wait.max(Duration::from_millis(20))).await;
        }
    }
}

/// 一次 LLM 调用的 token 用量（OpenAI 兼容端点 usage 字段；缺失时全 0）
#[derive(Debug, Clone, Default)]
pub struct LlmUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// 用量记录（交给 sink 落库/统计）
#[derive(Debug, Clone)]
pub struct LlmUsageRecord {
    pub role: Role,
    pub model: String,
    pub usage: LlmUsage,
}

/// 用量汇聚回调：装配侧注入（一般写 llm_usage 表）；LLM 网关不依赖 db（core 纯净性）
pub type UsageSink = std::sync::Arc<dyn Fn(LlmUsageRecord) + Send + Sync>;

pub struct LlmGateway {
    roles: HashMap<Role, ResolvedRole>,
    /// 原始 provider 配置（base_url + api_key），用于 WebUI 的「获取可用模型」功能
    providers: HashMap<String, ProviderRuntime>,
    http: reqwest::Client,
    gate: CostGate,
    /// Q53 全局 LLM 并发上限（三角色共享；与成本闸正交）
    concurrency: Arc<Semaphore>,
    /// 可选：每次成功 chat 后回调。None = 不统计（测试/未装配）
    usage_sink: Option<UsageSink>,
}

/// 运行时的 provider 凭据（解析后），用于按名查询
#[derive(Debug, Clone)]
struct ProviderRuntime {
    base_url: String,
    api_key: Option<String>,
}

impl LlmGateway {
    /// 解析 providers.toml；任一 provider 缺 base_url 或 roles 引用了不存在的 provider 都会报错
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("读取 providers.toml 失败: {}", path.display()))?;
        let file: ProvidersFile = toml::from_str(&text)
            .with_context(|| format!("解析 providers.toml 失败: {}", path.display()))?;

        let mut roles = HashMap::new();
        for (role, bind) in [
            (Role::Decision, file.roles.decision),
            (Role::BotChat, file.roles.bot_chat),
            (Role::AgentExec, file.roles.agent_exec),
        ] {
            let Some(bind) = bind else { continue };
            match file.provider.get(&bind.provider) {
                None => {
                    tracing::warn!(role = %role, provider = %bind.provider, "roles 引用了不存在的 provider，该角色不可用");
                }
                Some(p) => {
                    let api_key = if p.api_key_env.is_empty() {
                        None
                    } else {
                        match resolve_api_key(&p.api_key_env) {
                            Some(k) => Some(k),
                            None => {
                                tracing::warn!(role = %role, env = %p.api_key_env, "API key 缺失或环境变量未设,该角色不可用(可直粘 sk- 或 export 环境名)");
                                continue;
                            }
                        }
                    };
                    roles.insert(
                        role,
                        ResolvedRole {
                            provider: bind.provider.clone(),
                            model: bind.model,
                            base_url: p.base_url.trim_end_matches('/').to_string(),
                            api_key,
                        },
                    );
                }
            }
        }

        // 同时落一份原始 provider 凭据，供「获取可用模型」按名查找
        let mut providers = HashMap::new();
        for (name, p) in &file.provider {
            let api_key = if p.api_key_env.is_empty() {
                None
            } else {
                resolve_api_key(&p.api_key_env)
            };
            providers.insert(
                name.clone(),
                ProviderRuntime {
                    base_url: p.base_url.trim_end_matches('/').to_string(),
                    api_key,
                },
            );
        }

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("构建 LLM HTTP client 失败")?;
        // 启动诊断：把每个角色解析结果（脱敏）打出来，方便定位「toml 写的 ≠ 内存跑的」
        for (role, r) in &roles {
            tracing::info!(role = %role, resolved = %r.debug_descriptor(), "LlmGateway 角色已绑定");
        }
        Ok(Self {
            roles,
            providers,
            http,
            gate: CostGate::new(30),
            concurrency: Arc::new(Semaphore::new(DEFAULT_LLM_CONCURRENCY)),
            usage_sink: None,
        })
    }

    /// 装配侧注入 usage sink（main 里写 llm_usage 表）
    pub fn set_usage_sink(&mut self, sink: UsageSink) {
        self.usage_sink = Some(sink);
    }

    pub fn role(&self, role: Role) -> Option<&ResolvedRole> {
        self.roles.get(&role)
    }

    /// 热应用：调整 Decision 成本闸（次/分）
    pub fn set_cost_per_min(&self, n: usize) {
        self.gate.set_per_min(n);
    }

    /// Q53：全局并发许可上限（固定值，观测/测试用）
    pub fn concurrency_limit(&self) -> usize {
        DEFAULT_LLM_CONCURRENCY
    }

    /// 当前可用并发许可数（观测/调试用）
    pub fn concurrency_available(&self) -> usize {
        self.concurrency.available_permits()
    }

    /// 连通性测试（WebUI /api/llm/test）：发一次最短 chat，不过成本闸；
    /// 返回 (model, latency_ms)。请求/响应体不入日志（防泄 key 路径上的中间日志）。
    pub async fn test_chat(&self, role: Role) -> Result<(String, u64)> {
        let r = self
            .roles
            .get(&role)
            .ok_or_else(|| anyhow!("角色 {role} 未配置或不可用"))?
            .clone();
        // 连通性测试同样占并发槽（Q53）：避免面板手动测试绕过全局上限
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|_| anyhow!("LLM 并发信号量已关闭"))?;
        let body = json!({
            "model": r.model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 8,
        });
        let mut req = self
            .http
            .post(format!("{}/chat/completions", r.base_url))
            .json(&body);
        if let Some(k) = &r.api_key {
            req = req.bearer_auth(k);
        }
        let started = Instant::now();
        let resp = req.send().await.context("连接 provider 失败")?;
        let status = resp.status();
        let text = resp.text().await.context("读取 provider 响应失败")?;
        if !status.is_success() {
            bail!(
                "provider HTTP {status}: {}",
                &text[..text.floor_char_boundary(200)]
            );
        }
        let v: Value = serde_json::from_str(&text).context("provider 响应非 JSON")?;
        v.pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| anyhow!("provider 响应缺 choices[0].message.content"))?;
        Ok((r.model.clone(), started.elapsed().as_millis() as u64))
    }

    /// 拉取指定 provider 的可用模型列表（调 `GET {base_url}/models`，OpenAI 兼容）。
    /// 返回模型 id 数组（已排序去重）；provider 不存在 / 请求失败 / 响应非预期格式都会报错。
    pub async fn fetch_models(&self, provider_name: &str) -> Result<Vec<String>> {
        let p = self
            .providers
            .get(provider_name)
            .ok_or_else(|| anyhow!("provider `{provider_name}` 不存在"))?
            .clone();
        let mut req = self.http.get(format!("{}/models", p.base_url));
        if let Some(k) = &p.api_key {
            req = req.bearer_auth(k);
        }
        let resp = req.send().await.context("连接 provider 失败")?;
        let status = resp.status();
        let text = resp.text().await.context("读取响应失败")?;
        if !status.is_success() {
            bail!("HTTP {status}: {}", &text[..text.floor_char_boundary(200)]);
        }
        let v: Value = serde_json::from_str(&text).context("响应非 JSON")?;
        let arr = v
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| anyhow!("响应缺 data 数组"))?;
        let mut out: Vec<String> = arr
            .iter()
            .filter_map(|m| {
                m.get("id")
                    .and_then(|id| id.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// chat/completions 调用：先过成本闸，再占全局并发槽（Q53）；json_mode 时带 response_format=json_object
    pub async fn chat(
        &self,
        role: Role,
        system: &str,
        user: &str,
        json_mode: bool,
    ) -> Result<String> {
        let r = self
            .roles
            .get(&role)
            .ok_or_else(|| anyhow!("角色 {role} 未配置或不可用"))?
            .clone();
        self.gate.acquire().await;
        // Q53：全局并发上限——三角色共享 4 许可，超出排队等待
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|_| anyhow!("LLM 并发信号量已关闭"))?;

        let mut body = json!({
            "model": r.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "temperature": if role == Role::Decision { 0.2 } else { 0.7 },
            // 本接口没有声明任何工具。上游编码代理可能擅自返回 bash 调用，
            // 必须显式禁用；Agent 的工具协议由 agent.rs 自己处理 JSON。
            "tool_choice": "none",
        });
        if json_mode {
            body["response_format"] = json!({"type": "json_object"});
        }
        let mut req = self
            .http
            .post(format!("{}/chat/completions", r.base_url))
            .json(&body);
        if let Some(k) = &r.api_key {
            req = req.bearer_auth(k);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("角色 {role} 请求失败"))?;
        let status = resp.status();
        let text = resp.text().await.context("读取 LLM 响应体失败")?;
        if !status.is_success() {
            bail!(
                "角色 {role} HTTP {status}: {}",
                &text[..text.floor_char_boundary(300)]
            );
        }
        let v: Value = serde_json::from_str(&text).context("LLM 响应非 JSON")?;
        // 用量统计：有 sink 就上报；没 sink 或响应不带 usage 都静默
        if let Some(sink) = &self.usage_sink {
            let usage = v
                .get("usage")
                .map(|u| LlmUsage {
                    prompt_tokens: u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
                    completion_tokens: u
                        .get("completion_tokens")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0),
                    total_tokens: u.get("total_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
                })
                .unwrap_or_default();
            if usage.total_tokens > 0 {
                sink(LlmUsageRecord {
                    role,
                    model: r.model.clone(),
                    usage,
                });
            }
        }
        response_text(&v)
    }
}

fn response_text(v: &Value) -> Result<String> {
    let choice = v.pointer("/choices/0").context("LLM 响应缺 choices[0]")?;
    let message = &choice["message"];
    if choice["finish_reason"] == "tool_calls"
        || choice["finish_reason"] == "function_call"
        || message["tool_calls"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
        || !message["function_call"].is_null()
    {
        bail!("provider 返回了未授权的工具调用（已请求 tool_choice=none），未执行；请检查模型路由");
    }
    if choice["finish_reason"] == "length" {
        bail!("provider 输出被长度限制截断，拒绝使用不完整回答");
    }
    if message["refusal"]
        .as_str()
        .is_some_and(|s| !s.trim().is_empty())
        || choice["finish_reason"] == "content_filter"
    {
        bail!("provider 拒绝生成本次回答");
    }
    message["content"]
        .as_str()
        .map(str::to_owned)
        .context("LLM 响应缺 choices[0].message.content")
}

/// providers.toml 默认模板（配置缺失时写出，key 从环境变量读，永不上库）
pub const DEFAULT_PROVIDERS_TEMPLATE: &str = r#"# 云团 LLM Provider 配置（架构文档十五章）
# OpenAI 兼容 chat/completions；三角色独立绑定（允许绑同一家，角色不合并）。
# api_key_env 为环境变量名（密钥不入库不入仓）；留空则无需密钥（如本地 mock）。

# [provider.example]
# base_url = "https://api.example.com/v1"
# api_key_env = "EXAMPLE_API_KEY"
# models = ["some-model"]

[roles]
# decision   = { provider = "example", model = "便宜快速模型" }
# bot_chat   = { provider = "example", model = "中高档模型" }
# agent_exec = { provider = "example", model = "强模型" }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unexpected_tools_and_incomplete_answers_never_become_text() {
        let tool = json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"", "tool_calls":[{"function":{"name":"bash","arguments":"{\"command\":\"pwd\"}"}}]}}]});
        assert!(response_text(&tool)
            .unwrap_err()
            .to_string()
            .contains("未授权的工具调用"));
        let mixed = json!({"choices":[{"finish_reason":"stop","message":{"content":"已经做完了", "tool_calls":[{}]}}]});
        assert!(response_text(&mixed).is_err());
        for reason in ["length", "content_filter"] {
            assert!(response_text(
                &json!({"choices":[{"finish_reason":reason,"message":{"content":"部分内容"}}]})
            )
            .is_err());
        }
        assert_eq!(response_text(&json!({"choices":[{"finish_reason":"stop","message":{"content":"回答","refusal":null}}]})).unwrap(), "回答");
    }

    #[test]
    fn descriptor_never_exposes_key_fragments() {
        for key in [
            "sk-demo",
            "sk-placeholder-key-for-tests",
            "测试密钥测试密钥测试密钥",
        ] {
            let role = ResolvedRole {
                provider: "mock".into(),
                model: "mock".into(),
                base_url: "http://localhost".into(),
                api_key: Some(key.into()),
            };
            let descriptor = role.debug_descriptor();
            assert!(!descriptor.contains(key));
            assert!(!descriptor.contains("sk-"));
            assert!(!descriptor.contains("测试"));
            assert!(descriptor.contains(&format!("configured len={}", key.len())));
        }
    }

    #[tokio::test]
    async fn unicode_http_errors_propagate_from_all_gateway_paths() {
        use axum::{
            http::StatusCode,
            routing::{get, post},
            Router,
        };
        async fn failure() -> (StatusCode, String) {
            (StatusCode::BAD_GATEWAY, format!("a{}", "中".repeat(120)))
        }
        let app = Router::new()
            .route("/chat/completions", post(failure))
            .route("/models", get(failure));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let path =
            std::env::temp_dir().join(format!("yt-llm-error-{}.toml", rand::random::<u64>()));
        std::fs::write(
            &path,
            format!(
                r#"
[provider.mock]
base_url = "{url}"
api_key_env = ""
[roles]
agent_exec = {{ provider = "mock", model = "mock" }}
"#
            ),
        )
        .unwrap();
        let gateway = LlmGateway::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let errors = [
            gateway
                .chat(Role::AgentExec, "system", "user", true)
                .await
                .unwrap_err(),
            gateway.test_chat(Role::AgentExec).await.unwrap_err(),
            gateway.fetch_models("mock").await.unwrap_err(),
        ];
        for error in errors {
            let text = error.to_string();
            assert!(text.contains("502"), "{text}");
            assert!(text.contains(&"中".repeat(60)), "{text}");
        }
        server.abort();
    }
}
