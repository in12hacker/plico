//! Trace CLI commands — list, show, failures.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use super::extract_arg;

pub fn cmd_trace(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let sub = args.get(1).map(|s| s.as_str());
    match sub {
        Some("list") => {
            let agent_id = extract_arg(args, "--agent");
            let since = extract_arg(args, "--since");
            let until = extract_arg(args, "--until");
            let tool_name = extract_arg(args, "--tool");
            let status = extract_arg(args, "--status");
            let limit = extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            kernel.handle_api_request(ApiRequest::TraceList {
                agent_id,
                since,
                until,
                tool_name,
                status,
                limit,
            })
        }
        Some("show") => {
            let trace_id = args.get(2).cloned().unwrap_or_default();
            kernel.handle_api_request(ApiRequest::TraceShow { trace_id })
        }
        Some("failures") => {
            let agent_id = extract_arg(args, "--agent");
            let since = extract_arg(args, "--since");
            kernel.handle_api_request(ApiRequest::TraceFailures { agent_id, since })
        }
        _ => ApiResponse::error("Usage: aicli trace list [--agent X] [--since 7d] [--tool X] [--status error]\n       aicli trace show <trace_id>\n       aicli trace failures [--agent X] [--since 7d]"),
    }
}
