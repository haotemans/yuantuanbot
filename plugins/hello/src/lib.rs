//! Hello 示范插件：证明 plugins/<name>/ 独立 crate + Tool trait 注册链路走通。
//! 这个插件不做正事，只是骨架。删除前请先读完 plugins/README.md。

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use yuantuan_core::tools::{Tool, ToolCtx, ToolOutput};

pub struct HelloTool;

#[async_trait]
impl Tool for HelloTool {
    fn name(&self) -> &'static str {
        "hello"
    }
    fn description(&self) -> &'static str {
        "打招呼示例插件；验证插件链路"
    }

    async fn call(&self, _ctx: &ToolCtx, args: Value) -> Result<ToolOutput> {
        let who = args.get("who").and_then(|v| v.as_str()).unwrap_or("world");
        Ok(ToolOutput {
            summary: format!("hello, {who}!"),
            artifacts: vec![],
            data: json!({ "greeted": who }),
        })
    }
}

/// 装配入口：main 启动时调用，把插件的所有 Tool 加到 registry
pub fn register() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(HelloTool)]
}
