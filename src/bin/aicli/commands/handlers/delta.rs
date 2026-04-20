//! Delta-aware change tracking command.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use super::extract_arg;

/// Query changes since a given event sequence number.
pub fn cmd_delta(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let since_seq = extract_arg(args, "--since")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let watch_cids: Vec<String> = extract_arg(args, "--watch-cids")
        .map(|s| s.split(',').map(String::from).collect())
        .unwrap_or_default();
    let watch_tags: Vec<String> = extract_arg(args, "--watch-tags")
        .map(|s| s.split(',').map(String::from).collect())
        .unwrap_or_default();
    let limit = extract_arg(args, "--limit").and_then(|s| s.parse().ok());

    let req = plico::api::semantic::ApiRequest::DeltaSince {
        agent_id,
        since_seq,
        watch_cids,
        watch_tags,
        limit,
    };
    kernel.handle_api_request(req)
}
