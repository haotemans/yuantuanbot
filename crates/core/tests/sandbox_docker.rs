//! 真实 Linux Docker 验收：见 docs/reference/agent-sandbox.md。
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use yuantuan_core::tools::{
    sandbox::{SandboxConfig, SandboxExecTool},
    Tool, ToolCtx,
};

fn ctx(id: &str) -> ToolCtx {
    ToolCtx {
        task_id: Some(id.into()),
        chat_id: "g_docker_test".into(),
        chat_type: "group".into(),
        sender_pid: "test".into(),
        locale: None,
    }
}

async fn docker(args: &[&str]) -> std::process::Output {
    tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new("docker")
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("Docker CLI timeout")
    .expect("Docker CLI unavailable")
}

#[tokio::test]
#[ignore = "requires Linux Docker daemon and locally built yuantuan-sandbox:local image"]
async fn sandbox_cancellation_removes_container_and_applies_limits() {
    let namespace = format!("yt-cancel-{:016x}", rand::random::<u64>());
    let tool = Arc::new(
        SandboxExecTool::new(SandboxConfig {
            enabled: true,
            namespace: namespace.clone(),
            ..SandboxConfig::default()
        })
        .unwrap(),
    );
    tool.check_available().await.unwrap();
    let running = tokio::spawn(async move {
        tool.call(&ctx("cancel"), json!({"command":"sleep 50"}))
            .await
    });
    let filter = format!("label=yuantuan.sandbox={namespace}");
    let container = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let result = docker(&["ps", "-q", "--filter", &filter]).await;
            assert!(result.status.success());
            if !result.stdout.is_empty() {
                break String::from_utf8(result.stdout).unwrap().trim().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("container should start");
    let inspection = docker(&["inspect", "--format", "{{json .HostConfig}}", &container]).await;
    assert!(inspection.status.success());
    let host: serde_json::Value = serde_json::from_slice(&inspection.stdout).unwrap();
    assert_eq!(host["NetworkMode"], "none");
    assert_eq!(host["ReadonlyRootfs"], true);
    assert_eq!(host["Privileged"], false);
    assert_eq!(host["Memory"], 512 * 1024 * 1024);
    assert_eq!(host["MemorySwap"], 512 * 1024 * 1024);
    assert_eq!(host["NanoCpus"], 1_000_000_000);
    assert_eq!(host["PidsLimit"], 128);
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let result = docker(&["ps", "-aq", "--filter", &filter]).await;
            assert!(result.status.success());
            if result.stdout.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("aborted call must remove its container without waiting for command timeout");
    let result = docker(&["volume", "rm", &format!("{namespace}-workspace-cancel")]).await;
    assert!(result.status.success());
}

#[tokio::test]
#[ignore = "requires Linux Docker daemon and locally built yuantuan-sandbox:local image"]
async fn sandbox_tools_persistence_isolation_limits_and_cleanup() {
    let namespace = format!("yt-test-{:016x}", rand::random::<u64>());
    let tool = SandboxExecTool::new(SandboxConfig {
        enabled: true,
        namespace: namespace.clone(),
        max_output_bytes: 1024,
        ..SandboxConfig::default()
    })
    .unwrap();
    tool.check_available().await.unwrap();
    let a = ctx("a");
    let b = ctx("b");
    let result: anyhow::Result<()> = async {
        let versions = tool.call(&a, json!({"command": "set -eu; git --version; python --version; bun --version; git init -q; printf persistent > marker; python -c 'print(6 * 7)'; bun -e 'console.log(7 * 8)'"})).await?;
        assert_eq!(versions.data["exit_code"], 0, "{:?}", versions.data);
        assert!(versions.data["stdout"].as_str().unwrap().contains("42"));
        assert!(versions.data["stdout"].as_str().unwrap().contains("56"));
        let persisted = tool.call(&a, json!({"command": "cat marker; git status --porcelain"})).await?;
        assert_eq!(persisted.data["exit_code"], 0);
        assert!(persisted.data["stdout"].as_str().unwrap().contains("persistent"));
        let isolated = tool.call(&b, json!({"command": "test ! -e marker && test ! -e /run/sandbox-docker.sock && test ! -e /var/run/docker.sock && test ! -e /app/config.toml && test $(id -u) -eq 1000 && ! touch /root-write-test"})).await?;
        assert_eq!(isolated.data["exit_code"], 0);
        let failed = tool.call(&a, json!({"command": "echo diagnostic >&2; exit 7"})).await?;
        assert_eq!(failed.data["exit_code"], 7);
        assert!(failed.data["stderr"].as_str().unwrap().contains("diagnostic"));
        let flood = tool.call(&a, json!({"command": "python -c 'import sys; print(\"x\" * 100000); print(\"y\" * 100000, file=sys.stderr)'"})).await?;
        assert_eq!(flood.data["exit_code"], 0);
        assert_eq!(flood.data["stdout_truncated"], true);
        assert_eq!(flood.data["stderr_truncated"], true);
        let timeout = tool.call(&a, json!({"command": "sleep 20", "timeout_secs": 1})).await?;
        assert_eq!(timeout.data["timed_out"], true);
        let containers = tokio::process::Command::new("docker").args([
            "ps", "-aq", "--filter", &format!("label=yuantuan.sandbox={namespace}"),
        ]).output().await?;
        assert!(containers.status.success());
        assert!(containers.stdout.is_empty(), "completed/timeout containers must be removed");
        Ok(())
    }.await;
    // 仅删除本次测试明确创建的两个卷。
    for id in ["a", "b"] {
        let status = tokio::process::Command::new("docker")
            .args(["volume", "rm", &format!("{namespace}-workspace-{id}")])
            .status()
            .await
            .unwrap();
        assert!(status.success());
    }
    result.unwrap();
}
