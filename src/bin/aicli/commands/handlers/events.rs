//! Event commands — list events by time or natural language expression.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use super::extract_arg;
use super::extract_tags;

pub fn cmd_events(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let _agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let tags = extract_tags(args, "--tags");

    match args.get(1).map(|s| s.as_str()) {
        // events list [--since TS] [--until TS] [--tags TAGS]
        Some("list") => {
            let since = extract_arg(args, "--since")
                .and_then(|s| s.parse().ok());
            let until = extract_arg(args, "--until")
                .and_then(|s| s.parse().ok());
            let events = kernel.list_events(since, until, &tags, None);
            if events.is_empty() {
                println!("No events found.");
            } else {
                println!("Events ({} found):", events.len());
                for e in &events {
                    println!("  {} [{:?}]", e.label, e.event_type);
                    if let Some(start) = e.start_time {
                        println!("    Start: {}", start);
                    }
                    println!("    Related: {} items, {} attendees", e.related_count, e.attendee_count);
                }
            }
            ApiResponse::with_events(events)
        }
        // events by-time "last week" [--tags TAGS]
        Some("by-time") | Some("text") => {
            let time_expression = args.get(2..)
                .map(|v| v.iter().take_while(|s| !s.starts_with("--")).cloned().collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            if time_expression.is_empty() {
                println!("Usage: events by-time \"last week\" [--tags TAGS]");
                return ApiResponse::error("Missing time expression".to_string());
            }
            match kernel.list_events_text(&time_expression, &tags, None) {
                Ok(events) => {
                    if events.is_empty() {
                        println!("No events found for '{}'.", time_expression);
                    } else {
                        println!("Events matching '{}' ({} found):", time_expression, events.len());
                        for e in &events {
                            println!("  {} [{:?}]", e.label, e.event_type);
                        }
                    }
                    ApiResponse::with_events(events)
                }
                Err(e) => ApiResponse::error(e.to_string()),
            }
        }
        _ => {
            println!("Usage: events <list|by-time> [options]");
            println!("  list    --since TS --until TS --tags TAGS");
            println!("  by-time \"last week\" --tags TAGS");
            ApiResponse::ok()
        }
    }
}
