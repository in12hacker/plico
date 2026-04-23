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
            return ApiResponse::error(format!("Invalid period '{}'. Valid: last7days, last30days, alltime", period_str));
        }
    };

    let req = plico::api::semantic::ApiRequest::QueryGrowthReport {
        agent_id,
        period,
    };
    kernel.handle_api_request(req)
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
    fn test_cmd_session_start_basic() {
        let kernel = make_test_kernel();
        let args = vec!["--agent".to_string(), "test-session-agent".to_string()];
        let resp = cmd_session_start(&kernel, &args);
        assert!(resp.ok, "session start should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_cmd_session_start_with_intent() {
        let kernel = make_test_kernel();
        let args = vec![
            "--agent".to_string(), "intent-agent".to_string(),
            "--intent".to_string(), "exploring memory tiers".to_string(),
        ];
        let resp = cmd_session_start(&kernel, &args);
        assert!(resp.ok, "session start with --intent should succeed");
    }

    #[test]
    fn test_cmd_session_end_basic() {
        let kernel = make_test_kernel();
        // First start a session to get a session_id
        let start_resp = cmd_session_start(&kernel, &[
            "--agent".to_string(), "end-test-agent".to_string(),
        ]);
        assert!(start_resp.ok, "session start should succeed first");

        // Get session_id from start response
        let session_id = start_resp.session_started
            .as_ref()
            .expect("start response should have session_started")
            .session_id.clone();

        // Now end the session
        let end_args = vec![
            "--agent".to_string(), "end-test-agent".to_string(),
            "--session".to_string(), session_id,
        ];
        let resp = cmd_session_end(&kernel, &end_args);
        assert!(resp.ok, "session end should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_cmd_session_end_requires_session_id() {
        let kernel = make_test_kernel();
        let args = vec!["--agent".to_string(), "no-sess-agent".to_string()];
        let resp = cmd_session_end(&kernel, &args);
        assert!(!resp.ok, "session end without --session should fail");
        assert!(resp.error.is_some(), "should have error message");
    }

    #[test]
    fn test_cmd_growth_basic() {
        let kernel = make_test_kernel();
        let args = vec!["--agent".to_string(), "growth-test-agent".to_string()];
        let resp = cmd_growth(&kernel, &args);
        assert!(resp.ok, "growth query should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_cmd_growth_with_period() {
        let kernel = make_test_kernel();
        let args = vec![
            "--agent".to_string(), "growth-period-agent".to_string(),
            "--period".to_string(), "last30days".to_string(),
        ];
        let resp = cmd_growth(&kernel, &args);
        assert!(resp.ok, "growth with --period last30days should succeed");
    }

    #[test]
    fn test_cmd_growth_invalid_period() {
        let kernel = make_test_kernel();
        let args = vec![
            "--agent".to_string(), "bad-period-agent".to_string(),
            "--period".to_string(), "invalid".to_string(),
        ];
        let resp = cmd_growth(&kernel, &args);
        assert!(!resp.ok, "growth with invalid period should fail");
    }
}
