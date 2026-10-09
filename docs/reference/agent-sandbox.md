# Agent 开发沙箱

[文档索引](../README.md) · [后端工作记录](../changes/backend-hardening-workbench.md)

## 当前实现

2026-10-09 第一版为 Agent 注册 `sandbox_exec`。后端调用 Docker CLI，在独立 Linux 容器中执行 Bash；镜像预装 Git、Python 3.12（含 pip/venv）、Bun 1.3.0、ripgrep。没有宿主命令执行降级路径。实现入口：[工具](../../crates/core/src/tools/sandbox.rs)、[Agent](../../crates/core/src/agent.rs)、[镜像](../../deploy/sandbox/Dockerfile)。

执行链路：`Agent → Tool Registry → sandbox_exec → Docker CLI → 临时容器 + 当前任务的命名卷`。

- `ToolCtx.task_id` 由执行器注入，模型不能传 task_id、挂载点、Docker 参数、镜像或网络选项。
- 每条命令创建一个容器，完成后删除；同一任务始终挂载 `<namespace>-workspace-<task_id>` 到 `/workspace`，不同任务使用不同卷。卷跨命令和后端重启保留；现有中断任务仍按 Agent 规则失败收尾，不自动重放。
- `cwd` 是 `/workspace` 内的相对目录。每次是新 shell，`cd`、`export` 和后台服务不跨调用保留；文件、Git 仓库、venv 和包缓存保留。
- stdout/stderr 并行读取，各自限长，超限继续排空。返回退出码、超时标记、耗时和截断标记。非零退出码仍作为命令结果返回给模型，不能视为命令成功；124 按 GNU timeout 惯例表示超时，137 可能是强制终止或内存限制，不能据此唯一判断原因。
- 任务回放 `tool_result.details` 保存命令和有界输出；不要在命令文本或打印输出中放凭据。其他工具默认不额外保存详情。
- Tool trait 增加参数 JSON Schema，Agent 目录携带 schema，MCP 转发已有 `inputSchema`。这仍是项目现有 JSON 循环协议，不是新增原生 function-calling 通道。

## 配置

向实际使用的 `config.toml`（Compose 部署为 `deploy/yuantuan-config.toml`）追加：

```toml
[sandbox]
enabled = true
docker_command = "docker"
image = "yuantuan-sandbox:local"
namespace = "yuantuan"
network = "none"
memory_mb = 512
cpus = 1.0
pids_limit = 128
timeout_secs = 60
max_output_bytes = 16384
```

默认 `enabled=false`：旧配置继续运行，不注册沙箱工具，也不调用 Docker。启用后启动检查 Docker daemon 与本地镜像均为 Linux；不可用则启动报错。执行时 `--pull=never`，镜像必须预先构建。修改配置重启生效；配置 API 在写文件前校验沙箱字段，明确返回 `requires_restart`。当前没有专用沙箱设置页面。

`timeout_secs` 上限 90 秒，为 Docker 创建、停止和清理留出时间；Agent 原有 120 秒工具超时、10 轮预算、最多 3 个任务并发保持原值。每次命令可申请更短的超时，不能超过配置上限。

`network="none"` 支持已有文件与预装工具，不支持克隆远端或下载包。管理员设为 `bridge` 后可以访问外网，也可能访问宿主/局域网服务；这里没有域名白名单或内网地址拦截。需要细粒度网络隔离时，在专用 Docker daemon 的网络层配置策略。模型不能改变此设置。

## Linux 部署

推荐后端连接**专用 rootless Docker daemon**，其数据目录和用户只服务 Agent 沙箱。控制端拥有该 daemon 的管理能力；Docker socket 只交给后端，不挂进执行代码的沙箱。rootless daemon 要能通过 cgroup v2/systemd delegation 实际执行 CPU、内存和进程限额。

1. 准备好专用 rootless Docker socket，确认后端容器的 UID 1000 可访问它。以下路径为示例，需要按服务器调整。外层 Compose 仍使用原有 Docker context。

   ```bash
   export YUANTUAN_DOCKER_SOCKET=/run/user/1000/docker.sock
   DOCKER_HOST="unix://${YUANTUAN_DOCKER_SOCKET}" docker info
   DOCKER_HOST="unix://${YUANTUAN_DOCKER_SOCKET}" docker build \
     -t yuantuan-sandbox:local deploy/sandbox
   ```

   构建参数 `BUN_IMAGE`、`PYTHON_IMAGE` 可选其他经验证的版本或 digest。不要在镜像内放 API key、SSH 私钥或业务配置。

2. 修改沙箱配置后，启用可选 Compose overlay：

   ```bash
   docker compose -f deploy/compose.yml -f deploy/compose.sandbox.yml \
     up -d --build yuantuan
   ```

   Overlay 选用后端 `sandbox-runtime` 构建 target（仅增加 Docker CLI），并设置后端的 `DOCKER_HOST`。普通 Dockerfile 默认 target 保持原运行时。Bun/Python/Git 只装在沙箱镜像里。

3. 如果直接运行 Linux 后端二进制：为该进程安装 Docker CLI，设置同一个 `DOCKER_HOST`，配置 `[sandbox]` 后启动即可。

当前工作副本没有修改已有运行配置，也没有连接或部署服务器。

## 验收与使用

在装有 Rust 工具链、能够连接专用 Linux Docker daemon 的机器上执行：

```bash
DOCKER_HOST="unix://${YUANTUAN_DOCKER_SOCKET}" \
  cargo test -p yuantuan-core --test sandbox_docker -- --ignored --nocapture
```

测试验证三种开发工具、同任务文件与 Git 状态保留、跨任务隔离、非 root 身份、根文件系统只读、非零退出码、大输出截断、超时、取消清理与 Docker 限额配置。测试使用随机命名空间，只清理本次的测试卷。默认 `cargo test --workspace` 不要求 Docker，真实容器测试显式标记 ignored；不能把普通测试通过等同于容器验收通过。限额字段验收不替代服务器实际压力测试。

模型可调用：

```json
{"action":"tool_call","tool_name":"sandbox_exec","tool_args":{"command":"git --version; python --version; bun --version"}}
```

写文件、计算、运行 JS 示例：

```json
{"command":"python - <<'PY'\nfrom pathlib import Path\nPath('answer.txt').write_text(str(sum(range(101))))\nprint(Path('answer.txt').read_text())\nPY\nbun -e 'console.log(6 * 7)'"}
```

需要第三方 Python 包时在工作区建立 venv：`python -m venv .venv`，后续使用 `.venv/bin/python` 和 `.venv/bin/pip`。Bun 项目直接在工作区执行 `bun init` / `bun install` / `bun run`；下载依赖需管理员开启网络。Git 可本地 init、diff、commit；远端访问需要网络，私有仓凭据注入尚未实现。

## 资源与保留边界

- 容器使用 UID/GID 1000、只读 rootfs、删除全部 capabilities、禁止新增权限、限制 CPU/内存（禁用额外 swap）/进程数/文件描述符；`/tmp` 为 128 MiB tmpfs。只读根和非 root 不等于虚拟机级隔离，仍与宿主共享内核。
- 容器内 `timeout` 限制命令生命周期，宿主另外限制整个 Docker 调用。正常完成 `--rm`；超时/取消触发 `docker rm --force`。后端被 SIGKILL 或 Docker daemon 不可用时无法承诺立即清理；容器内计时器提供第二道终止机制，清理失败会记日志。后端超时可能无法保留部分输出，返回明确截断标记。
- 工作卷**不会自动删除，也没有逐任务磁盘配额**。应为专用 daemon 的数据目录配置容量约束并安排运维清理；没有复用跨任务包缓存。默认关闭沙箱，适合先在受控任务中验收。
- 查看容器：`docker ps -a --filter label=yuantuan.sandbox=yuantuan`。查看工作卷：`docker volume ls --filter name=yuantuan-workspace-`。这些命令都必须指向沙箱 daemon。
- 任务完成且文件已取出后，由管理员针对明确的卷执行 `docker volume rm <完整卷名>`。不要对整个服务器执行 volume prune 作为沙箱清理。
- 文件目前留在命名卷，不属于后端 `data/artifacts`，也不进入现有备份。管理员可通过临时只读挂载该卷的容器提取文件；网页下载、产物导出 API、自动保留/回收、私有仓凭据和长期开发服务待后续实现。

## 验证状态

本地为 Windows，未安装 Docker；Linux 镜像构建、真实容器隔离/限额、rootless socket 权限与 Compose 部署均待服务器验收。本轮 Rust 检查结果见[后端工作记录](../changes/backend-hardening-workbench.md)。
