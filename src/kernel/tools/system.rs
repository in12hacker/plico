//! System tool handlers (tools.list, tools.describe, context.load).

use crate::kernel::AIKernel;
use crate::tool::ToolResult;
use serde_json::json;

pub(in crate::kernel) fn handle(kernel: &AIKernel, name: &str, params: &serde_json::Value, agent_id: &str) -> ToolResult {
    match name {
        "tools.list" => {
            let tools = kernel.tool_registry.list();
            ToolResult::ok(serde_json::to_value(&tools).unwrap_or_default())
        }
        "tools.describe" => {
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            match kernel.tool_registry.get(tool_name) {
                Some(desc) => ToolResult::ok(serde_json::to_value(&desc).unwrap_or_default()),
                None => ToolResult::error(format!("tool not found: {}", tool_name)),
            }
        }
        "context.load" => {
            let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
            let layer_str = params.get("layer").and_then(|v| v.as_str()).unwrap_or("L2");
            let layer = match crate::fs::ContextLayer::parse_layer(layer_str) {
                Some(l) => l,
                None => return ToolResult::error(format!("Invalid layer '{}'. Use L0, L1, or L2.", layer_str)),
            };
            match kernel.context_load(cid, layer, agent_id) {
                Ok(loaded) => ToolResult::ok(json!({
                    "cid": loaded.cid,
                    "layer": loaded.layer.name(),
                    "content": loaded.content,
                    "tokens_estimate": loaded.tokens_estimate,
                })),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        _ => ToolResult::error(format!("unknown system tool: {}", name)),
    }
}
