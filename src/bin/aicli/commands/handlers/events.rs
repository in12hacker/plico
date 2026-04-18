//! Event commands — list events by time or natural language expression.

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
        _ => {
            eprintln!("Usage: events <list|by-time> [options]");
            ApiResponse::error("unknown events subcommand")
        }
    }
}
