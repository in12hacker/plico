//! Event commands — list events by time or natural language expression,
//! plus event bus subscribe/poll/unsubscribe for reactive workflows.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use super::extract_arg;
use super::extract_tags;

pub fn cmd_events(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent");
    let tags = extract_tags(args, "--tags");

    match args.get(1).map(|s| s.as_str()) {
        Some("list") => {
            let since = extract_arg(args, "--since")
                .and_then(|s| s.parse().ok());
            let until = extract_arg(args, "--until")
                .and_then(|s| s.parse().ok());
            let events = kernel.list_events(since, until, &tags, None, agent_id.as_deref());
            ApiResponse::with_events(events)
        }
        Some("by-time") | Some("text") => {
            let time_expression = args.get(2..)
                .map(|v| v.iter().take_while(|s| !s.starts_with("--")).cloned().collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            if time_expression.is_empty() {
                return ApiResponse::error("events by-time requires a time expression, e.g.: events by-time \"last week\"");
            }
            match kernel.list_events_text(&time_expression, &tags, None, agent_id.as_deref()) {
                Ok(events) => ApiResponse::with_events(events),
                Err(e) => ApiResponse::error(e.to_string()),
            }
        }
        Some("subscribe") => {
            let event_types = extract_arg(args, "--types")
                .map(|s| s.split(',').map(|t| t.trim().to_string()).collect::<Vec<_>>());
            let agent_ids = extract_arg(args, "--agents")
                .map(|s| s.split(',').map(|a| a.trim().to_string()).collect::<Vec<_>>());
            let filter = if event_types.is_some() || agent_ids.is_some() {
                Some(plico::kernel::event_bus::EventFilter { event_types, agent_ids })
            } else {
                None
            };
            let sub_id = kernel.event_subscribe_filtered(filter);
            let mut r = ApiResponse::ok();
            r.subscription_id = Some(sub_id);
            r
        }
        Some("poll") => {
            let sub_id = match extract_arg(args, "--sub") {
                Some(s) => s,
                None => return ApiResponse::error("--sub required".to_string()),
            };
            match kernel.event_poll(&sub_id) {
                Some(events) => {
                    let mut r = ApiResponse::ok();
                    r.kernel_events = Some(events);
                    r
                }
                None => ApiResponse::error(format!("Unknown subscription: {}", sub_id)),
            }
        }
        Some("unsubscribe") => {
            let sub_id = match extract_arg(args, "--sub") {
                Some(s) => s,
                None => return ApiResponse::error("--sub required".to_string()),
            };
            if kernel.event_unsubscribe(&sub_id) {
                ApiResponse::ok()
            } else {
                ApiResponse::error(format!("Unknown subscription: {}", sub_id))
            }
        }
        Some("history") => {
            let since_seq = extract_arg(args, "--since")
                .and_then(|s| s.parse().ok());
            let limit = extract_arg(args, "--limit")
                .and_then(|s| s.parse().ok());
            let req = plico::api::semantic::ApiRequest::EventHistory {
                since_seq,
                agent_id_filter: agent_id.clone(),
                limit,
            };
            kernel.handle_api_request(req)
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
        // list command should return events (may be empty)
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
        // First subscribe to get a valid subscription id
        let sub_args = vec!["events".to_string(), "subscribe".to_string()];
        let sub_r = cmd_events(&kernel, &sub_args);
        let sub_id = sub_r.subscription_id.unwrap();

        // Then unsubscribe
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
