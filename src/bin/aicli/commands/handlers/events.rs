//! Event commands — list events by time or natural language expression,
//! plus event bus subscribe/poll/unsubscribe for reactive workflows.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use super::extract_arg;
use super::extract_tags;

pub fn cmd_events(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let _agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let tags = extract_tags(args, "--tags");

    match args.get(1).map(|s| s.as_str()) {
        Some("list") => {
            let since = extract_arg(args, "--since")
                .and_then(|s| s.parse().ok());
            let until = extract_arg(args, "--until")
                .and_then(|s| s.parse().ok());
            let events = kernel.list_events(since, until, &tags, None);
            ApiResponse::with_events(events)
        }
        Some("by-time") | Some("text") => {
            let time_expression = args.get(2..)
                .map(|v| v.iter().take_while(|s| !s.starts_with("--")).cloned().collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            if time_expression.is_empty() {
                eprintln!("Usage: events by-time \"last week\" [--tags TAGS]");
                return ApiResponse::error("Missing time expression".to_string());
            }
            match kernel.list_events_text(&time_expression, &tags, None) {
                Ok(events) => ApiResponse::with_events(events),
                Err(e) => ApiResponse::error(e.to_string()),
            }
        }
        Some("subscribe") => {
            let sub_id = kernel.event_subscribe();
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
        _ => {
            eprintln!("Usage: events <list|by-time|subscribe|poll|unsubscribe> [options]");
            ApiResponse::error("unknown events subcommand")
        }
    }
}
