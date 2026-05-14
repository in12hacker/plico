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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;
    use crate::kernel::tools::system::handle;

    #[test]
    fn test_tools_list() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "tools.list", &json!({}), "test");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_tools_describe_existing() {
        let (kernel, _tmp) = make_kernel();
        // First list to find a tool name
        let list_result = handle(&*kernel, "tools.list", &json!({}), "test");
        let tools = list_result.output;
        if let Some(arr) = tools.as_array() {
            if let Some(first) = arr.first() {
                let name = first["name"].as_str().unwrap_or("");
                let result = handle(&*kernel, "tools.describe", &json!({"name": name}), "test");
                assert!(result.error.is_none(), "describe should succeed for existing tool");
            }
        }
    }

    #[test]
    fn test_tools_describe_not_found() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "tools.describe", &json!({"name": "nonexistent_tool"}), "test");
        assert!(result.error.is_some());
    }

    #[test]
    fn test_context_load_invalid_layer() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "context.load", &json!({"cid": "test", "layer": "L99"}), "test");
        assert!(result.error.is_some());
    }

    #[test]
    fn test_unknown_system_tool() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "system.nonexistent", &json!({}), "test");
        assert!(result.error.is_some());
    }
}
