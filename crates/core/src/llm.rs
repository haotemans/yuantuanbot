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

pub struct LlmGateway {
    roles: HashMap<Role, ResolvedRole>,
    http: reqwest::Client,
    gate: CostGate,
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
                        match std::env::var(&p.api_key_env) {
                            Ok(k) if !k.is_empty() => Some(k),
                            _ => {
                                tracing::warn!(role = %role, env = %p.api_key_env, "API key 环境变量缺失，该角色不可用");
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

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("构建 LLM HTTP client 失败")?;
        Ok(Self {
            roles,
            http,
            gate: CostGate::new(30),
        })
    }

    pub fn role(&self, role: Role) -> Option<&ResolvedRole> {
        self.roles.get(&role)
    }

    /// 热应用：调整 Decision 成本闸（次/分）
    pub fn set_cost_per_min(&self, n: usize) {
        self.gate.set_per_min(n);
    }

    /// 连通性测试（WebUI /api/llm/test）：发一次最短 chat，不过成本闸；
    /// 返回 (model, latency_ms)。请求/响应体不入日志（防泄 key 路径上的中间日志）。
    pub async fn test_chat(&self, role: Role) -> Result<(String, u64)> {
        let r = self
            .roles
            .get(&role)
            .ok_or_else(|| anyhow!("角色 {role} 未配置或不可用"))?
            .clone();
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
            bail!("provider HTTP {status}: {}", &text[..text.len().min(200)]);
        }
        let v: Value = serde_json::from_str(&text).context("provider 响应非 JSON")?;
        v.pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| anyhow!("provider 响应缺 choices[0].message.content"))?;
        Ok((r.model.clone(), started.elapsed().as_millis() as u64))
    }

    /// chat/completions 调用：先过成本闸；json_mode 时带 response_format=json_object
    pub async fn chat(&self, role: Role, system: &str, user: &str, json_mode: bool) -> Result<String> {
        let r = self
            .roles
            .get(&role)
            .ok_or_else(|| anyhow!("角色 {role} 未配置或不可用"))?
            .clone();
        self.gate.acquire().await;

        let mut body = json!({
            "model": r.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "temperature": if role == Role::Decision { 0.2 } else { 0.7 },
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
        let resp = req.send().await.with_context(|| format!("角色 {role} 请求失败"))?;
        let status = resp.status();
        let text = resp.text().await.context("读取 LLM 响应体失败")?;
        if !status.is_success() {
            bail!("角色 {role} HTTP {status}: {}", &text[..text.len().min(300)]);
        }
        let v: Value = serde_json::from_str(&text).context("LLM 响应非 JSON")?;
        v.pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("LLM 响应缺 choices[0].message.content"))
    }
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
