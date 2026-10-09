//! Linux Docker 开发沙箱。模型仅控制容器内命令，Docker 参数和工作卷由运行时决定。
//! 部署、保留策略与隔离边界见 docs/reference/agent-sandbox.md。

use super::{Tool, ToolCtx, ToolOutput};
use anyhow::{ensure, Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxConfig {
    pub enabled: bool,
    pub docker_command: String,
    pub image: String,
    /// 同一 Docker daemon 上的不同部署必须使用不同 namespace。
    pub namespace: String,
    pub network: SandboxNetwork,
    pub memory_mb: u32,
    pub cpus: f64,
    pub pids_limit: u32,
    pub timeout_secs: u64,
    /// stdout、stderr 各自的保留字节数；达到上限后继续读取并丢弃，避免管道死锁。
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxNetwork {
    None,
    Bridge,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            docker_command: "docker".into(),
            image: "yuantuan-sandbox:local".into(),
            namespace: "yuantuan".into(),
            network: SandboxNetwork::None,
            memory_mb: 512,
            cpus: 1.0,
            pids_limit: 128,
            timeout_secs: 60,
            max_output_bytes: 16_384,
        }
    }
}

impl SandboxConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.docker_command.trim().is_empty(),
            "sandbox.docker_command 不能为空"
        );
        ensure!(
            identifier(&self.namespace, 32),
            "sandbox.namespace 只能包含字母、数字、_、-，长度 1–32"
        );
        ensure!(
            !self.image.is_empty()
                && !self.image.starts_with('-')
                && !self.image.chars().any(char::is_whitespace),
            "sandbox.image 必须是非空镜像引用"
        );
        ensure!(self.memory_mb >= 128, "sandbox.memory_mb 至少 128");
        ensure!(
            self.cpus.is_finite() && self.cpus > 0.0,
            "sandbox.cpus 必须为正数"
        );
        ensure!(self.pids_limit >= 16, "sandbox.pids_limit 至少 16");
        // 留出 Docker 启停/清理时间，整个调用仍服从 Agent 的 120 秒限制。
        ensure!(
            (1..=90).contains(&self.timeout_secs),
            "sandbox.timeout_secs 必须在 1–90 秒内"
        );
        ensure!(
            (1024..=65_536).contains(&self.max_output_bytes),
            "sandbox.max_output_bytes 必须在 1024–65536 内"
        );
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecArgs {
    command: String,
    #[serde(default)]
    cwd: String,
    timeout_secs: Option<u64>,
}

impl ExecArgs {
    fn validate(&self, max_timeout: u64) -> Result<()> {
        ensure!(
            !self.command.trim().is_empty()
                && self.command.len() <= 16_384
                && !self.command.contains('\0'),
            "command 必须是 1–16384 字节的非空命令，不能含 NUL"
        );
        ensure!(
            self.cwd.len() <= 256
                && !self.cwd.starts_with('/')
                && !self.cwd.contains(['\\', '\0'])
                && !self.cwd.split('/').any(|p| p == ".."),
            "cwd 必须是 /workspace 内的相对路径，不能包含 .. 或反斜杠"
        );
        if let Some(timeout) = self.timeout_secs {
            ensure!(
                (1..=max_timeout).contains(&timeout),
                "timeout_secs 超出服务器允许范围 1–{max_timeout}"
            );
        }
        Ok(())
    }
}

fn identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub struct SandboxExecTool {
    config: SandboxConfig,
}

impl SandboxExecTool {
    pub fn new(config: SandboxConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    /// 显式开启但配置不可用时启动失败；绝不回退到宿主 shell。
    pub async fn check_available(&self) -> Result<()> {
        for args in [
            vec!["info", "--format", "{{.OSType}}"],
            vec![
                "image",
                "inspect",
                "--format",
                "{{.Os}}",
                self.config.image.as_str(),
            ],
        ] {
            let mut command = docker_command(&self.config.docker_command);
            command.args(args);
            let result = capture(&mut command, Duration::from_secs(10), 4096)
                .await
                .context("Docker 沙箱预检失败")?;
            ensure!(
                result.code == Some(0) && !result.timed_out,
                "Docker 沙箱预检失败：{}",
                result.stderr.text
            );
            ensure!(
                result.stdout.text.trim() == "linux",
                "沙箱需要 Linux Docker daemon 和 Linux 镜像"
            );
        }
        Ok(())
    }

    fn workspace_volume(&self, ctx: &ToolCtx) -> Result<String> {
        let task_id = ctx
            .task_id
            .as_deref()
            .context("sandbox_exec 只能由 Agent 任务调用")?;
        ensure!(identifier(task_id, 96), "无效的运行时 task_id");
        Ok(format!("{}-workspace-{task_id}", self.config.namespace))
    }

    fn run_args(&self, name: &str, volume: &str, args: &ExecArgs) -> Vec<String> {
        let cfg = &self.config;
        let timeout = args.timeout_secs.unwrap_or(cfg.timeout_secs);
        let mut argv: Vec<String> = [
            "run",
            "--rm",
            "--init",
            "--pull=never",
            "--read-only",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges:true",
            "--user=1000:1000",
            "--log-driver=none",
            "--ulimit=nofile=1024:1024",
            "--tmpfs=/tmp:rw,nosuid,nodev,size=128m,mode=1777",
            // 固定可信入口；即使配置镜像带 ENTRYPOINT 也不能绕过生命周期计时器。
            "--entrypoint=/usr/bin/timeout",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        argv.extend([
            "--name".into(),
            name.into(),
            "--label".into(),
            format!("yuantuan.sandbox={}", cfg.namespace),
            "--network".into(),
            match cfg.network {
                SandboxNetwork::None => "none",
                SandboxNetwork::Bridge => "bridge",
            }
            .into(),
            "--memory".into(),
            format!("{}m", cfg.memory_mb),
            "--memory-swap".into(),
            format!("{}m", cfg.memory_mb),
            "--cpus".into(),
            cfg.cpus.to_string(),
            "--pids-limit".into(),
            cfg.pids_limit.to_string(),
            "--mount".into(),
            format!("type=volume,source={volume},target=/workspace"),
            "--workdir".into(),
            format!("/workspace/{}", args.cwd),
            cfg.image.clone(),
            "--signal=TERM".into(),
            "--kill-after=2s".into(),
            format!("{timeout}s"),
            "/bin/bash".into(),
            "--noprofile".into(),
            "--norc".into(),
            "-c".into(),
            args.command.clone(),
        ]);
        argv
    }
}

#[async_trait]
impl Tool for SandboxExecTool {
    fn name(&self) -> &'static str {
        "sandbox_exec"
    }

    fn description(&self) -> &'static str {
        "在 Linux Docker 沙箱中执行 Bash 命令；可用 git、python、pip、bun、rg。当前任务的 /workspace 文件跨调用保留；每次调用是新进程，cd/export 不会保留。网络由服务器配置，默认禁用。检查 exit_code；超时后先检查文件状态，避免盲目重试副作用。"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object", "additionalProperties": false,
            "required": ["command"],
            "properties": {
                "command": {"type": "string", "minLength": 1, "maxLength": 16384,
                    "description": "Bash 命令。写文件可用 heredoc；Python 依赖请在 /workspace 创建 venv。"},
                "cwd": {"type": "string", "maxLength": 256, "default": "",
                    "description": "相对于 /workspace 的现存目录；不允许 .."},
                "timeout_secs": {"type": "integer", "minimum": 1,
                    "maximum": self.config.timeout_secs, "default": self.config.timeout_secs}
            }
        })
    }

    fn audit_details(&self, args: &Value, output: &ToolOutput) -> Option<Value> {
        // 仅在参数校验与有界输出收集成功后调用。
        Some(json!({"args": args, "result": output.data}))
    }

    async fn call(&self, ctx: &ToolCtx, value: Value) -> Result<ToolOutput> {
        ensure!(self.config.enabled, "Docker 沙箱未启用");
        let args: ExecArgs = serde_json::from_value(value).context("sandbox_exec 参数错误")?;
        args.validate(self.config.timeout_secs)?;
        let volume = self.workspace_volume(ctx)?;
        let name = format!(
            "{}-exec-{:032x}",
            self.config.namespace,
            rand::random::<u128>()
        );
        let mut cleanup = ContainerCleanup::new(self.config.docker_command.clone(), name.clone());
        let mut command = docker_command(&self.config.docker_command);
        command.args(self.run_args(&name, &volume, &args));
        let started = Instant::now();
        let timeout = args.timeout_secs.unwrap_or(self.config.timeout_secs);
        // 包含容器创建的宿主计时器；容器内另有 timeout，即使后端退出也会终止命令。
        let result = capture(
            &mut command,
            Duration::from_secs(timeout + 15),
            self.config.max_output_bytes,
        )
        .await;
        // --rm 负责正常完成；显式 rm -f 负责超时/取消。清理错误记录日志并由 Drop 再尝试。
        cleanup.remove().await;
        let result = result?;
        let timed_out = result.timed_out || result.code == Some(124);
        Ok(ToolOutput {
            summary: format!(
                "沙箱命令结束：exit_code={:?}, timed_out={timed_out}，工作区 {volume}",
                result.code
            ),
            artifacts: vec![],
            data: json!({
                "success": result.code == Some(0) && !timed_out,
                "exit_code": result.code, "timed_out": timed_out,
                "stdout": result.stdout.text, "stderr": result.stderr.text,
                "stdout_truncated": result.stdout.truncated, "stderr_truncated": result.stderr.truncated,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "workspace_volume": volume, "cwd": args.cwd,
            }),
        })
    }
}

fn docker_command(executable: &str) -> Command {
    let mut command = Command::new(executable);
    command.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    command
}

struct Captured {
    text: String,
    truncated: bool,
}

async fn drain(mut reader: impl AsyncRead + Unpin, limit: usize) -> Result<Captured> {
    let mut kept = Vec::with_capacity(limit);
    let mut buf = [0u8; 8192];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let retain = n.min(limit.saturating_sub(kept.len()));
        kept.extend_from_slice(&buf[..retain]);
        truncated |= retain < n;
    }
    // 非 UTF-8 输出不影响执行器；修复字符也计入最终字节上限。
    let mut text = String::from_utf8_lossy(&kept).into_owned();
    if text.len() > limit {
        text.truncate(text.floor_char_boundary(limit));
        truncated = true;
    }
    Ok(Captured { text, truncated })
}

struct ProcessResult {
    code: Option<i32>,
    timed_out: bool,
    stdout: Captured,
    stderr: Captured,
}

async fn capture(command: &mut Command, timeout: Duration, limit: usize) -> Result<ProcessResult> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().context("启动 Docker CLI 失败")?;
    let stdout = child.stdout.take().context("stdout unavailable")?;
    let stderr = child.stderr.take().context("stderr unavailable")?;
    // 三个 future 同时驱动，不创建脱离调用生命周期的读任务。
    let collect = async {
        tokio::try_join!(
            async { child.wait().await.context("等待 Docker CLI 失败") },
            drain(stdout, limit),
            drain(stderr, limit)
        )
    };
    match tokio::time::timeout(timeout, collect).await {
        Ok(result) => {
            let (status, stdout, stderr) = result?;
            Ok(ProcessResult {
                code: status.code(),
                timed_out: false,
                stdout,
                stderr,
            })
        }
        Err(_) => {
            let _ = child.kill().await;
            Ok(ProcessResult {
                code: None,
                timed_out: true,
                stdout: Captured {
                    text: String::new(),
                    truncated: true,
                },
                stderr: Captured {
                    text: "Docker 调用超时，输出未完整收集；容器清理已请求".into(),
                    truncated: true,
                },
            })
        }
    }
}

struct ContainerCleanup {
    executable: String,
    name: String,
    armed: bool,
}

impl ContainerCleanup {
    fn new(executable: String, name: String) -> Self {
        Self {
            executable,
            name,
            armed: true,
        }
    }

    async fn remove(&mut self) {
        if remove_container(&self.executable, &self.name).await {
            self.armed = false;
        }
    }
}

impl Drop for ContainerCleanup {
    fn drop(&mut self) {
        if self.armed {
            let executable = self.executable.clone();
            let name = self.name.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    remove_container(&executable, &name).await;
                });
            }
        }
    }
}

async fn remove_container(executable: &str, name: &str) -> bool {
    let mut command = docker_command(executable);
    command.args(["rm", "--force", name]);
    match capture(&mut command, Duration::from_secs(5), 1024).await {
        Ok(r)
            if !r.timed_out
                && (r.code == Some(0) || r.stderr.text.contains("No such container")) =>
        {
            true
        }
        _ => {
            tracing::warn!(
                container = name,
                "沙箱容器清理未确认，需检查 Docker；容器内计时器仍限制命令生命周期"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn ctx(id: &str) -> ToolCtx {
        ToolCtx {
            task_id: Some(id.into()),
            chat_id: "g_test".into(),
            chat_type: "group".into(),
            sender_pid: "test".into(),
            locale: None,
        }
    }

    #[test]
    fn rejects_boundary_overrides_and_invalid_limits() {
        assert!(
            serde_json::from_value::<ExecArgs>(json!({"command": "pwd", "task_id": "other"}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<ExecArgs>(json!({"command": "pwd", "mount": "/"})).is_err()
        );
        for cwd in ["/etc", "../other", "a/../../b", "C:\\host", "a\0b"] {
            let args: ExecArgs =
                serde_json::from_value(json!({"command": "pwd", "cwd": cwd})).unwrap();
            assert!(args.validate(60).is_err(), "{cwd:?}");
        }
        for timeout in [0, 61] {
            let args: ExecArgs =
                serde_json::from_value(json!({"command": "pwd", "timeout_secs": timeout})).unwrap();
            assert!(args.validate(60).is_err());
        }
        for setting in [
            json!({"network": "host"}),
            json!({"timeout_secs": 120}),
            json!({"namespace": "../escape"}),
            json!({"cpus": 0}),
            json!({"max_output_bytes": 0}),
        ] {
            let config = serde_json::from_value::<SandboxConfig>(setting);
            assert!(config.is_err() || config.unwrap().validate().is_err());
        }
    }

    #[tokio::test]
    async fn workspace_is_derived_from_runtime_and_disabled_never_spawns() {
        let tool = SandboxExecTool::new(SandboxConfig::default()).unwrap();
        assert_eq!(
            tool.workspace_volume(&ctx("task-1")).unwrap(),
            tool.workspace_volume(&ctx("task-1")).unwrap()
        );
        assert_ne!(
            tool.workspace_volume(&ctx("task-1")).unwrap(),
            tool.workspace_volume(&ctx("task-2")).unwrap()
        );
        assert!(tool.workspace_volume(&ctx("../other")).is_err());
        let mut no_task = ctx("unused");
        no_task.task_id = None;
        assert!(tool.workspace_volume(&no_task).is_err());
        assert!(tool
            .call(&ctx("task-1"), json!({"command":"pwd"}))
            .await
            .unwrap_err()
            .to_string()
            .contains("未启用"));
    }

    #[test]
    fn command_is_one_container_argument_and_no_host_paths_are_mounted() {
        let tool = SandboxExecTool::new(SandboxConfig::default()).unwrap();
        let payload = "echo '你好'; $(touch /tmp/test)\nprintf x";
        let args: ExecArgs = serde_json::from_value(json!({"command":payload})).unwrap();
        let argv = tool.run_args("test-container", "test-volume", &args);
        assert_eq!(argv.last().unwrap(), payload);
        assert_eq!(argv.iter().filter(|a| a.as_str() == payload).count(), 1);
        assert!(argv.contains(&"type=volume,source=test-volume,target=/workspace".into()));
        assert!(!argv
            .iter()
            .any(|a| a.contains("type=bind") || a.contains("docker.sock") || a == "--privileged"));
        for flag in [
            "--read-only",
            "--cap-drop=ALL",
            "--user=1000:1000",
            "--pull=never",
            "--entrypoint=/usr/bin/timeout",
        ] {
            assert!(argv.iter().any(|a| a == flag));
        }
        assert_eq!(
            argv[argv.iter().position(|a| a == "--network").unwrap() + 1],
            "none"
        );
    }

    #[tokio::test]
    async fn truncated_output_is_drained_without_deadlock_and_stays_bounded() {
        let (mut write, read) = tokio::io::duplex(64);
        let producer = async move {
            // 非 UTF-8 字节触发 replacement 扩容；最终结果仍遵守字节上限。
            write.write_all(&vec![0xff; 100_000]).await.unwrap();
            drop(write);
        };
        let (_, output) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(producer, drain(read, 1024))
        })
        .await
        .expect("reader must keep draining after limit");
        let output = output.unwrap();
        assert!(output.truncated);
        assert!(output.text.len() <= 1024);
        assert!(!output.text.is_empty());
    }

    // 通过测试可执行文件提供跨平台子进程，无需本机 Python / Docker。
    #[test]
    fn subprocess_fixture() {
        match std::env::var("YUANTUAN_SANDBOX_TEST_CHILD").as_deref() {
            Ok("output") => {
                use std::io::Write;
                std::io::stdout().write_all(&vec![b'x'; 200_000]).unwrap();
                std::io::stderr().write_all(&vec![b'y'; 200_000]).unwrap();
                std::process::exit(7);
            }
            Ok("wait") => std::thread::sleep(Duration::from_secs(20)),
            _ => {}
        }
    }

    fn fixture(mode: &str) -> Command {
        let mut command = docker_command(std::env::current_exe().unwrap().to_str().unwrap());
        command.args([
            "--exact",
            "tools::sandbox::tests::subprocess_fixture",
            "--nocapture",
        ]);
        command.env("YUANTUAN_SANDBOX_TEST_CHILD", mode);
        command
    }

    #[tokio::test]
    async fn process_drains_both_streams_and_preserves_nonzero_exit() {
        let output = capture(&mut fixture("output"), Duration::from_secs(5), 1024)
            .await
            .unwrap();
        assert_eq!(output.code, Some(7));
        assert!(output.stdout.truncated && output.stderr.truncated);
        assert!(!output.timed_out);
        assert_eq!(output.stdout.text.len(), 1024);
        assert_eq!(output.stderr.text.len(), 1024);
    }

    #[tokio::test]
    async fn process_timeout_returns_promptly() {
        let start = Instant::now();
        let output = capture(&mut fixture("wait"), Duration::from_millis(100), 1024)
            .await
            .unwrap();
        assert!(output.timed_out);
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
