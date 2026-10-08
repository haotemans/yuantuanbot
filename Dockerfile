# syntax=docker/dockerfile:1
# yuantuan 一体化镜像 — 内嵌静态资源(rust-embed),运行时只需二进制 + 配置

# ── 阶段 1: 构建 ─────────────────────────────────────────────────
FROM rust:1.92-bookworm AS builder

WORKDIR /app

# cargo-chef 风格缓存:先只复制 manifest,让依赖层可缓存
COPY Cargo.toml Cargo.lock ./
COPY crates/core/Cargo.toml crates/core/
COPY crates/adapter-qq/Cargo.toml crates/adapter-qq/
COPY crates/webui/Cargo.toml crates/webui/
COPY crates/yuantuan/Cargo.toml crates/yuantuan/
COPY plugins/hello/Cargo.toml plugins/hello/

# 占位的 dummy src,让 cargo fetch 只拉依赖
RUN for d in crates/core crates/adapter-qq crates/webui crates/yuantuan plugins/hello; do \
        mkdir -p "$d/src" && echo "fn main(){}" > "$d/src/lib.rs"; \
    done && \
    echo "fn main(){}" > crates/yuantuan/src/main.rs && \
    cargo fetch --locked && \
    rm -rf crates plugins

# 真源码再拷进来,只增量编译业务代码
COPY crates/ crates/
COPY plugins/ plugins/

# release 构建
RUN cargo build --release --locked -p yuantuan

# ── 阶段 2: 运行时 ────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        tzdata \
    && rm -rf /var/lib/apt/lists/*

# uid 1000 = 主流发行版首个普通用户(ubuntu user 在多数 VM 里是 1000);
# 跟宿主机挂载的 config/data 目录所有者 uid 对齐,容器内 yuantuan 可读可写
RUN useradd -r -u 1000 -m -s /usr/sbin/nologin yuantuan 2>/dev/null || \
    useradd -m -s /usr/sbin/nologin yuantuan

WORKDIR /app
COPY --from=builder /app/target/release/yuantuan /usr/local/bin/yuantuan

# 让 config 跟 data 都放 /app 下(volumes 挂进来)
RUN mkdir -p /app/data && chown -R yuantuan:yuantuan /app
USER yuantuan

EXPOSE 8085 6199

# 容器内不 fork、不 detach;Rust 进程 PID 1 镜像(用 tini 可换)
ENTRYPOINT ["/usr/local/bin/yuantuan"]
