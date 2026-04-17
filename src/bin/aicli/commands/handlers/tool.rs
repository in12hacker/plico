//! Tool management commands.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiResponse, ApiRequest};
use super::extract_arg;

pub fn cmd_tool(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    match args.get(1).map(|s| s.as_str()) {
        Some("list") => {
            let req = ApiRequest::ToolList { agent_id: "cli".to_string() };
            let resp = kernel.handle_api_request(req);
            if let Some(ref tools) = resp.tools {
                println!("Available tools ({} total):", tools.len());
                for t in tools {
                    println!("  {} — {}", t.name, t.description);
                }
            }
            resp
        }
        Some("describe") => {
            let name = args.get(2).cloned().unwrap_or_default();
            let req = ApiRequest::ToolDescribe { tool: name, agent_id: "cli".to_string() };
            let resp = kernel.handle_api_request(req);
            if let Some(ref tools) = resp.tools {
                if let Some(t) = tools.first() {
                    println!("Tool: {}", t.name);
                    println!("Description: {}", t.description);
                    println!("Schema: {}", serde_json::to_string_pretty(&t.schema).unwrap_or_default());
                }
            }
            resp
        }
        Some("call") => {
            let name = args.get(2).cloned().unwrap_or_default();
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let params_str = extract_arg(args, "--params").unwrap_or_else(|| "{}".to_string());
            let params: serde_json::Value = serde_json::from_str(&params_str).unwrap_or_default();
            let req = ApiRequest::ToolCall { tool: name, params, agent_id };
            let resp = kernel.handle_api_request(req);
            if let Some(ref result) = resp.tool_result {
                if result.success {
                    println!("{}", serde_json::to_string_pretty(&result.output).unwrap_or_default());
                } else {
                    eprintln!("Tool error: {}", result.error.as_deref().unwrap_or("unknown"));
                }
            }
            resp
        }
        _ => {
            println!("Usage: tool <list|describe|call> ...");
            println!("  tool list                  — list all available tools");
            println!("  tool describe <name>       — describe a specific tool");
            println!("  tool call <name> --params JSON --agent ID  — call a tool");
            ApiResponse::error("unknown tool subcommand")
        }
    }
}
