//! Event commands — all operations route through handle_api_request.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use super::extract_arg;
use super::extract_tags;

pub fn cmd_events(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_default();
    let tags = extract_tags(args, "--tags");

    match args.get(1).map(|s| s.as_str()) {
        Some("list") => {
            let since = extract_arg(args, "--since").and_then(|s| s.parse().ok());
            let until = extract_arg(args, "--until").and_then(|s| s.parse().ok());
            let limit = extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            let offset = extract_arg(args, "--offset").and_then(|s| s.parse().ok());
            kernel.handle_api_request(ApiRequest::ListEvents {
                since, until, tags, event_type: None, agent_id, limit, offset,
            })
        }
        Some("by-time") | Some("text") => {
            let time_expression = args.get(2..)
                .map(|v| v.iter().take_while(|s| !s.starts_with("--")).cloned().collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            if time_expression.is_empty() {
                return ApiResponse::error("events by-time requires a time expression, e.g.: events by-time \"last week\"");
            }
            kernel.handle_api_request(ApiRequest::ListEventsText {
                time_expression, tags, event_type: None, agent_id,
            })
        }
        Some("subscribe") => {
            let event_types = extract_arg(args, "--types")
                .map(|s| s.split(',').map(|t| t.trim().to_string()).collect::<Vec<_>>());
            let agent_ids = extract_arg(args, "--agents")
                .map(|s| s.split(',').map(|a| a.trim().to_string()).collect::<Vec<_>>());
            kernel.handle_api_request(ApiRequest::EventSubscribe {
                agent_id, event_types, agent_ids,
            })
        }
        Some("poll") => {
            let subscription_id = match extract_arg(args, "--sub") {
                Some(s) => s,
                None => return ApiResponse::error("--sub required"),
            };
            kernel.handle_api_request(ApiRequest::EventPoll { subscription_id })
        }
        Some("unsubscribe") => {
            let subscription_id = match extract_arg(args, "--sub") {
                Some(s) => s,
                None => return ApiResponse::error("--sub required"),
            };
            kernel.handle_api_request(ApiRequest::EventUnsubscribe { subscription_id })
        }
        Some("history") => {
            let since_seq = extract_arg(args, "--since").and_then(|s| s.parse().ok());
            let limit = extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            kernel.handle_api_request(ApiRequest::EventHistory {
                since_seq,
                agent_id_filter: if agent_id.is_empty() { None } else { Some(agent_id) },
                limit,
            })
        }
        _ => {
            ApiResponse::error("Unknown events subcommand. Valid: list, by-time, subscribe, poll, unsubscribe, history")
        }
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
    fn test_cmd_events_basic() {
        let kernel = make_test_kernel();
        let args = vec!["events".to_string(), "list".to_string()];
        let r = cmd_events(&kernel, &args);
        assert!(r.error.is_none());
        assert!(r.kernel_events.is_some() || r.events.is_some());
    }

    #[test]
    fn test_cmd_events_with_filter() {
        let kernel = make_test_kernel();
        let args = vec![
            "events".to_string(), "list".to_string(),
            "--agent".to_string(), "cli".to_string(),
        ];
        let r = cmd_events(&kernel, &args);
        assert!(r.error.is_none());
    }

    #[test]
    fn test_cmd_events_history() {
        let kernel = make_test_kernel();
        let args = vec![
            "events".to_string(), "history".to_string(),
            "--agent".to_string(), "cli".to_string(),
        ];
        let r = cmd_events(&kernel, &args);
        assert!(r.error.is_none());
    }

    #[test]
    fn test_cmd_events_subscribe() {
        let kernel = make_test_kernel();
        let args = vec!["events".to_string(), "subscribe".to_string()];
        let r = cmd_events(&kernel, &args);
        assert!(r.error.is_none());
        assert!(r.subscription_id.is_some());
    }

    #[test]
    fn test_cmd_events_unsubscribe() {
        let kernel = make_test_kernel();
        let sub_args = vec!["events".to_string(), "subscribe".to_string()];
        let sub_r = cmd_events(&kernel, &sub_args);
        let sub_id = sub_r.subscription_id.unwrap();

        let args = vec![
            "events".to_string(), "unsubscribe".to_string(),
            "--sub".to_string(), sub_id,
        ];
        let r = cmd_events(&kernel, &args);
        assert!(r.error.is_none());
    }

    #[test]
    fn test_cmd_events_poll_unknown() {
        let kernel = make_test_kernel();
        let args = vec![
            "events".to_string(), "poll".to_string(),
            "--sub".to_string(), "nonexistent-sub".to_string(),
        ];
        let r = cmd_events(&kernel, &args);
        assert!(r.error.is_some());
    }

    #[test]
    fn test_cmd_events_unknown_subcommand() {
        let kernel = make_test_kernel();
        let args = vec!["events".to_string(), "invalid-subcommand".to_string()];
        let r = cmd_events(&kernel, &args);
        assert!(r.error.is_some());
    }
}
