//! Tool management commands.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiResponse, ApiRequest};
use super::extract_arg;

pub fn cmd_tool(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    match args.get(1).map(|s| s.as_str()) {
        Some("list") => {
            let req = ApiRequest::ToolList { agent_id: "cli".to_string() };
            kernel.handle_api_request(req)
        }
        Some("describe") => {
            let name = args.get(2).cloned().unwrap_or_default();
            let req = ApiRequest::ToolDescribe { tool: name, agent_id: "cli".to_string() };
            kernel.handle_api_request(req)
        }
        Some("call") => {
            let name = args.get(2).cloned().unwrap_or_default();
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let params_str = extract_arg(args, "--params").unwrap_or_else(|| "{}".to_string());
            let params: serde_json::Value = serde_json::from_str(&params_str).unwrap_or_default();
            let req = ApiRequest::ToolCall { tool: name, params, agent_id };
            kernel.handle_api_request(req)
        }
        _ => {
            eprintln!("Usage: tool <list|describe|call> ...");
            ApiResponse::error("unknown tool subcommand")
        }
    }
}
