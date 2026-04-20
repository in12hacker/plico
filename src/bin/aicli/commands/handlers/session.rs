//! Session lifecycle commands — session-start and session-end.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiResponse, GrowthPeriod};
use super::extract_arg;

/// Start a new session for an agent.
pub fn cmd_session_start(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let intent_hint = extract_arg(args, "--intent");
    let last_seen_seq = extract_arg(args, "--last-seq").and_then(|s| s.parse().ok());

    let req = plico::api::semantic::ApiRequest::StartSession {
        agent_id,
        agent_token: None,
        intent_hint,
        load_tiers: vec![],
        last_seen_seq,
    };
    kernel.handle_api_request(req)
}

/// End an active session for an agent.
pub fn cmd_session_end(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let session_id = match extract_arg(args, "--session") {
        Some(s) => s,
        None => return ApiResponse::error("--session required".to_string()),
    };
    let auto_checkpoint = !args.iter().any(|a| a == "--no-checkpoint");

    let req = plico::api::semantic::ApiRequest::EndSession {
        agent_id,
        session_id,
        auto_checkpoint,
    };
    kernel.handle_api_request(req)
}

/// Query growth report for an agent.
pub fn cmd_growth(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let period_str = extract_arg(args, "--period").unwrap_or_else(|| "last7days".to_string());
    let period = match period_str.to_lowercase().as_str() {
        "last7days" | "7d" => GrowthPeriod::Last7Days,
        "last30days" | "30d" => GrowthPeriod::Last30Days,
        "alltime" | "all" => GrowthPeriod::AllTime,
        _ => {
            eprintln!("Invalid period '{}'. Use: last7days, last30days, or alltime", period_str);
            return ApiResponse::error(format!("invalid period: {}", period_str));
        }
    };

    let req = plico::api::semantic::ApiRequest::QueryGrowthReport {
        agent_id,
        period,
    };
    kernel.handle_api_request(req)
}
