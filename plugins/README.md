# Plugins 目录

云团插件层（Q-P01：编译期加载 Rust crate）。每个子目录是一个独立 cargo crate。

## 目录规范

```
plugins/
  hello/                        # 插件名（sk-task-name）
    Cargo.toml                  # 声明 lib + tool-meta
    src/
      lib.rs                    # 实现 Plugin::register -> Vec<Arc<dyn Tool>>
    enabled                     # 空标记文件；存在则 build，缺失则跳过
    data/                       # 插件数据目录（备份收）
      .gitkeep
```

## 开发一个 Tool 插件

最小实现：

```rust
// plugins/hello/Cargo.toml
[package]
name = "yuantuan-plugin-hello"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["rlib"]

[dependencies]
yuantuan-core = { path = "../../crates/core" }
async-trait = "0.1"
anyhow = "1"
serde_json = "1"
```

```rust
// plugins/hello/src/lib.rs
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use yuantuan_core::tools::{Tool, ToolCtx, ToolOutput};

pub struct HelloTool;

#[async_trait]
impl Tool for HelloTool {
    fn name(&self) -> &'static str { "hello" }
    fn description(&self) -> &'static str { "打招呼示例插件" }
    async fn call(&self, _ctx: &ToolCtx, args: Value) -> Result<ToolOutput> {
        let who = args.get("who").and_then(|v| v.as_str()).unwrap_or("world");
        Ok(ToolOutput {
            text: format!("hello, {who}!"),
            artifacts: vec![],
        })
    }
}

// 注册入口（main 调用时拿到 tool 实例加入 registry）
pub fn register() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(HelloTool)]
}
```

## 启禁

- `enabled` 文件存在 = 启用
- 缺失 = 跳过（build.rs 不拉进 workspace build）
- 面板启禁只动这个文件；已经加载的旧实例仍存活到下次重启

## 数据目录约定

`data/plugins/<plugin_name>/`：插件运行时自己创建。备份扫描 `data/` 时会带上（Q-B04 裁决）。
