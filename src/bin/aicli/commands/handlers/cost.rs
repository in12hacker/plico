//! Cost tracking commands — query token cost ledger.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use super::extract_arg;

pub fn cmd_cost(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    // Allow flags without subcommand (e.g., "cost --agent test-agent")
    match args.get(1).map(|s| s.as_str()) {
        Some("session") | Some("--session") => {
            let session_id = extract_arg(args, "--session")
                .unwrap_or_else(|| "".to_string());
            kernel.handle_api_request(ApiRequest::CostSessionSummary { session_id })
        }
        Some("agent") | Some("--agent") => {
            let agent_id = extract_arg(args, "--agent")
                .unwrap_or_else(|| "".to_string());
            let last = extract_arg(args, "--last")
                .and_then(|s| s.parse().ok())
                .unwrap_or(10);
            kernel.handle_api_request(ApiRequest::CostAgentTrend { agent_id, last_n_sessions: last })
        }
        Some("anomaly") | Some("--anomaly") => {
            let agent_id = extract_arg(args, "--agent")
                .unwrap_or_else(|| "".to_string());
            kernel.handle_api_request(ApiRequest::CostAnomalyCheck { agent_id })
        }
        _ => ApiResponse::error("Usage: cost <session --session ID | agent --agent ID [--last N] | anomaly --agent ID>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_kernel() -> plico::kernel::AIKernel {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("EMBEDDING_BACKEND", "stub");
        plico::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel")
    }

    #[test]
    fn test_cost_unknown_subcommand() {
        let kernel = make_test_kernel();
        let args = vec!["cost".to_string(), "unknown".to_string()];
        let r = cmd_cost(&kernel, &args);
        assert!(r.error.is_some());
    }
}