# syntax=docker/dockerfile:1
ARG BUN_IMAGE=oven/bun:1.3.0
ARG PYTHON_IMAGE=python:3.12-slim-bookworm
FROM ${BUN_IMAGE} AS bun
FROM ${PYTHON_IMAGE}

RUN apt-get update && apt-get install -y --no-install-recommends \
        bash git ripgrep coreutils ca-certificates libstdc++6 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=bun /usr/local/bin/bun /usr/local/bin/bun
RUN ln -s /usr/local/bin/bun /usr/local/bin/bunx \
    && useradd --uid 1000 --create-home --shell /bin/bash agent \
    && mkdir -p /workspace/.home /workspace/.cache \
    && chown -R 1000:1000 /workspace

ENV HOME=/workspace/.home \
    XDG_CACHE_HOME=/workspace/.cache \
    BUN_INSTALL_CACHE_DIR=/workspace/.cache/bun \
    PIP_CACHE_DIR=/workspace/.cache/pip \
    PYTHONUNBUFFERED=1 \
    GIT_TERMINAL_PROMPT=0
WORKDIR /workspace
USER 1000:1000
CMD ["/bin/bash"]
