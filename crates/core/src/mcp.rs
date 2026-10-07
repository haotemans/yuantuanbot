//! MCP（Model Context Protocol）stdio client（Phase 3 裁决：Q-M01）。
//!
//! 位置：yuantuan 作为 MCP client，外部 MCP server 通过本地子进程 stdio 通讯。
//! 典型用户故事：面板里新增 `{ name: "fs", command: "npx", args: ["-y", "@modelcontextprotocol/server-filesystem", "/data/docs"] }`，
//! 主进程启动时 spawn 该命令，握手后列出其 tools 注册进 ToolRegistry，Decision / Skill 看到的还是一致的 Tool 接口。
//!
//! 协议要点（只覆盖我们现阶段用到的子集）：
//! - 传输：stdio，Content-Length 帧 + JSON-RPC 2.0
//! - 生命周期：initialize → initialized → tools/list → tools/call
//! - 详细 spec：<https://modelcontextprotocol.io/specification/2025-06-18>
//!
//! 错误与失败策略：
//! - spawn 失败：log + 该 server 标记 down，不阻断其他
//! - handshake 失败：关闭子进程，标 down
//! - tools/list 空：警告但保留 server（后续可能动态增加）
//! - tools/call 返回 isError=true：作为 Err 抛给调用方，不吞

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use tracing::{debug, info, warn};

use crate::tools::{Tool, ToolCtx, ToolOutput};

/// 单个 MCP server 的 spawn 配置（面板编辑，存 config.toml）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// 面板展示/日志用的稳定 ID（snake_case）
    pub name: String,
    /// 可执行程序名（在 PATH 里查）或绝对路径
    pub command: String,
    /// spawn argv 参数
    #[serde(default)]
    pub args: Vec<String>,
    /// 附加环境变量
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// false 时启动跳过
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// server 端 tools/list 返回的单个 tool 描述
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
}

/// 调用结果（tools/call 响应 content 段保持原始 Value，由 ToolOutput.data 携带）
#[derive(Debug)]
pub struct McpClient {
    name: String,
    child: AsyncMutex<Child>,
    stdin: AsyncMutex<ChildStdin>,
    /// request_id → pending oneshot
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    next_id: Arc<AtomicI64>,
    /// tools/list 拿到的描述（启动后填充；RwLock 因为 spawn_and_init 需要先 handshake 再写）
    tools: std::sync::RwLock<Vec<McpToolDescriptor>>,
}

impl McpClient {
    /// spawn + 握手 + tools/list；任何一步失败都返回 Err（子进程被关闭）
    pub async fn spawn_and_init(cfg: &McpServerConfig) -> Result<Arc<Self>> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()) // server 日志直通我们 stderr 方便诊断
            .kill_on_drop(true);
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        // Windows: 不弹黑窗
        #[cfg(target_os = "windows")]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn MCP server `{}` 失败 ({})", cfg.name, cfg.command))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("MCP server `{}` stdin unavailable", cfg.name))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("MCP server `{}` stdout unavailable", cfg.name))?;

        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending.clone();

        // 后台读循环：Content-Length 帧 → JSON-RPC distribute
        let server_name = cfg.name.clone();
        tokio::spawn(async move {
            if let Err(e) = read_loop(stdout, pending_clone).await {
                warn!(server = %server_name, error = %e, "MCP read loop 退出");
            }
        });

        let client = Arc::new(Self {
            name: cfg.name.clone(),
            child: AsyncMutex::new(child),
            stdin: AsyncMutex::new(stdin),
            pending,
            next_id: Arc::new(AtomicI64::new(1)),
            tools: std::sync::RwLock::new(vec![]),
        });

        // 握手 initialize
        let init_resp = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "yuantuan", "version": "0.1.0" }
                }),
            )
            .await
            .context("MCP initialize 失败")?;
        debug!(server = %cfg.name, resp = ?init_resp, "MCP initialize ok");

        // 通知 initialized（无 id，无需响应）
        client.notify("notifications/initialized", json!({})).await?;

        // tools/list
        let list_resp = client.request("tools/list", json!({})).await.context("MCP tools/list 失败")?;
        let tools_raw = list_resp
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut tools = Vec::new();
        for t in tools_raw {
            match serde_json::from_value::<McpToolDescriptor>(t) {
                Ok(d) => tools.push(d),
                Err(e) => warn!(server = %cfg.name, error = %e, "tools/list 单项解析失败，跳过"),
            }
        }
        *client.tools.write().expect("mcp tools poisoned") = tools;

        Ok(client)
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let frame = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let mut stdin = self.stdin.lock().await;
        write_frame(&mut *stdin, &frame).await
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let frame = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        {
            let mut stdin = self.stdin.lock().await;
            write_frame(&mut *stdin, &frame).await?;
        }
        let resp = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .context("MCP request 超时")?
            .context("MCP read loop 已终止")?;
        if let Some(err) = resp.get("error") {
            anyhow::bail!("MCP {method} 错误: {err}");
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value> {
        self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": args,
            }),
        )
        .await
    }

    pub fn tools(&self) -> Vec<McpToolDescriptor> {
        self.tools.read().expect("mcp tools poisoned").clone()
    }

    pub fn server_name(&self) -> &str {
        &self.name
    }

    pub async fn shutdown(&self) -> Result<()> {
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        Ok(())
    }
}

/// 把 MCP server 的某个 tool 适配为本地 Tool trait
pub struct McpToolAdapter {
    pub server: Arc<McpClient>,
    pub descriptor: McpToolDescriptor,
    pub stable_name: String, // 通常 "<server>:<tool>"，避免冲突
}

#[async_trait::async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &'static str {
        // 注意：McpToolAdapter 的 stable_name 是 String，但 Tool trait 要求 &'static str。
        // 妥协：stable_name 用 Box::leak 转为 'static（插件生命周期内不释放，进程结束时回收）。
        Box::leak(self.stable_name.clone().into_boxed_str())
    }
    fn description(&self) -> &'static str {
        let s = self
            .descriptor
            .description
            .clone()
            .unwrap_or_else(|| format!("MCP `{}` tool `{}`", self.server.server_name(), self.descriptor.name));
        Box::leak(s.into_boxed_str())
    }

    async fn call(&self, _ctx: &ToolCtx, args: Value) -> Result<ToolOutput> {
        let resp = self.server.call_tool(&self.descriptor.name, args).await?;
        // tools/call 返回 { content: [...], isError?: bool }
        if resp.get("isError").and_then(|v| v.as_bool()).unwrap_or(false) {
            let text = summarize_content(&resp);
            anyhow::bail!("MCP tool `{}` 失败: {text}", self.descriptor.name);
        }
        let text = summarize_content(&resp);
        Ok(ToolOutput {
            summary: text.clone(),
            artifacts: vec![],
            data: json!({ "mcp": true, "server": self.server.server_name(), "tool": self.descriptor.name, "content": resp, "text": text }),
        })
    }
}

fn summarize_content(resp: &Value) -> String {
    resp.get("content")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                        item.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(非文本结果)".into())
}

/// 一帧 = "Content-Length: N\r\n\r\n{json}"
async fn write_frame(stdin: &mut ChildStdin, frame: &Value) -> Result<()> {
    let payload = serde_json::to_string(frame)?;
    let header = format!("Content-Length: {}\r\n\r\n", payload.as_bytes().len());
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(payload.as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

async fn read_loop(
    stdout: ChildStdout,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
) -> Result<()> {
    let mut reader = BufReader::new(stdout);
    loop {
        // 读 header 行：Content-Length: N
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                anyhow::bail!("MCP stdout EOF");
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if content_length.is_some() {
                    break;
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("Content-Length:") {
                content_length = Some(rest.trim().parse::<usize>()?);
            }
            // Other headers ignored（spec 只定义 Content-Length）
        }
        let len = content_length.ok_or_else(|| anyhow!("MCP 帧缺 Content-Length"))?;
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).await?;
        let value: Value = serde_json::from_slice(&buf)?;
        if let Some(id) = value.get("id").and_then(|v| v.as_i64()) {
            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                let _ = tx.send(value);
            }
        } else {
            debug!(?value, "MCP server notification");
        }
    }
}

/// McpManager：多 server 管理 + ToolRegistry 注入
pub struct McpManager {
    pub clients: Vec<Arc<McpClient>>,
}

impl McpManager {
    /// 根据配置 spawn 全部 enabled server；返回 (manager, 失败列表)
    pub async fn spawn_all(cfg: &McpConfig) -> (Self, Vec<(String, String)>) {
        let mut clients = Vec::new();
        let mut failures = Vec::new();
        for server in cfg.servers.iter().filter(|s| s.enabled) {
            match McpClient::spawn_and_init(server).await {
                Ok(c) => {
                    info!(server = %server.name, tools = c.tools().len(), "MCP server 就绪");
                    clients.push(c.clone());
                }
                Err(e) => {
                    warn!(server = %server.name, error = %e, "MCP server 启动失败，跳过");
                    failures.push((server.name.clone(), format!("{e}")));
                }
            }
        }
        (Self { clients }, failures)
    }

    /// 把所有 server 的所有 tool 注册到 ToolRegistry
    pub fn register_tools(&self, registry: &crate::tools::Registry) {
        for client in &self.clients {
            for desc in client.tools() {
                let stable_name = format!("mcp:{}:{}", client.server_name(), desc.name);
                registry.register_arc(Arc::new(McpToolAdapter {
                    server: client.clone(),
                    descriptor: desc,
                    stable_name,
                }));
            }
        }
    }

    pub async fn shutdown_all(&self) {
        for c in &self.clients {
            let _ = c.shutdown().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip() {
        let toml_text = r#"
[[servers]]
name = "fs"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
enabled = true

[[servers]]
name = "web"
command = "uvx"
args = ["mcp-server-fetch"]
enabled = false
"#;
        let cfg: McpConfig = toml::from_str(toml_text).unwrap();
        assert_eq!(cfg.servers.len(), 2);
        assert_eq!(cfg.servers[0].name, "fs");
        assert!(cfg.servers[0].enabled);
        assert!(!cfg.servers[1].enabled);
    }

    #[test]
    fn config_default_enabled() {
        let toml_text = r#"
[[servers]]
name = "x"
command = "y"
"#;
        let cfg: McpConfig = toml::from_str(toml_text).unwrap();
        assert!(cfg.servers[0].enabled);
    }

    #[test]
    fn summarize_text_content() {
        let resp = json!({
            "content": [
                { "type": "text", "text": "hello" },
                { "type": "text", "text": "world" }
            ]
        });
        assert_eq!(summarize_content(&resp), "hello\nworld");
    }

    #[test]
    fn summarize_non_text() {
        let resp = json!({ "content": [{ "type": "image", "data": "..." }] });
        assert_eq!(summarize_content(&resp), "(非文本结果)");
    }
}
