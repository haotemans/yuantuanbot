#!/usr/bin/env bash
# 普通 Docker 下的 Linux 验收入口；构建阶段不挂 Docker socket，rootless 编译路径尚未验证。
set -euo pipefail

repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_dir"
mkdir -p .sandbox-verify/cargo .sandbox-verify/target

docker build -t yuantuan-sandbox:local deploy/sandbox
docker run --rm \
  --name "yuantuan-sandbox-compile-$$" \
  --user "$(id -u):$(id -g)" \
  --memory=1536m --memory-swap=1536m --cpus=1 --pids-limit=256 \
  --mount "type=bind,source=$repo_dir,target=/src" \
  --workdir /src \
  -e CARGO_HOME=/src/.sandbox-verify/cargo \
  -e CARGO_TARGET_DIR=/src/.sandbox-verify/target \
  -e CARGO_BUILD_JOBS=1 \
  -e CARGO_PROFILE_TEST_DEBUG=0 \
  rust:1.92-bookworm \
  cargo test --locked -p yuantuan-core --test sandbox_docker --no-run

# cargo 输出仅有一个测试可执行文件；拒绝缓存中存在多个版本时猜测执行对象。
mapfile -t binaries < <(find .sandbox-verify/target/debug/deps -maxdepth 1 -type f -name 'sandbox_docker-*' -executable)
if (( ${#binaries[@]} != 1 )); then
  printf 'Expected one sandbox test binary, found %s; use a fresh test checkout.\n' "${#binaries[@]}" >&2
  exit 1
fi
"${binaries[0]}" --ignored --nocapture --test-threads=1
