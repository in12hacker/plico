//! Intent resolution commands.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use super::extract_arg;

pub fn cmd_intent(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    if extract_arg(args, "--description").is_some() {
        let description = extract_arg(args, "--description").unwrap_or_default();
        let priority_str = extract_arg(args, "--priority").unwrap_or_else(|| "medium".to_string());
        let action = extract_arg(args, "--action");
        let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

        let priority = match priority_str.to_lowercase().as_str() {
            "critical" => plico::scheduler::IntentPriority::Critical,
            "high" => plico::scheduler::IntentPriority::High,
            "medium" => plico::scheduler::IntentPriority::Medium,
            _ => plico::scheduler::IntentPriority::Low,
        };

        let id = match kernel.submit_intent(priority, description, action, Some(agent_id)) {
            Ok(id) => id,
            Err(e) => return ApiResponse::error(e),
        };
        println!("Intent submitted: {}", id);
        let mut r = ApiResponse::ok();
        r.intent_id = Some(id);
        return r;
    }

    let text = args.iter().skip(1)
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    if text.is_empty() {
        return ApiResponse::error("Usage: intent \"<text>\" [--execute] [--learn] [--threshold N]");
    }

    if args.iter().any(|a| a == "--execute") {
        let learn = args.iter().any(|a| a == "--learn");
        let threshold = extract_arg(args, "--threshold")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.7);
        match kernel.intent_execute_sync(&text, &agent_id, threshold, learn) {
            Ok(result) => {
                if result.executed {
                    println!("Executed: {} (success={})", text, result.success);
                } else {
                    println!("Not executed: {}", result.output);
                }
                let mut r = if result.success { ApiResponse::ok() } else { ApiResponse::error(result.output.clone()) };
                r.resolved_intents = Some(result.resolved);
                r.data = Some(result.output);
                r
            }
            Err(e) => ApiResponse::error(e),
        }
    } else {
        let results = kernel.intent_resolve(&text, &agent_id);
        if results.is_empty() {
            println!("Could not resolve: {}", text);
            return ApiResponse::error("No intent resolved");
        }

        println!("Resolved {} action(s):", results.len());
        for (i, ri) in results.iter().enumerate() {
            println!("  {}. [{:.2}] {}", i + 1, ri.confidence, ri.explanation);
        }

        let mut r = ApiResponse::ok();
        r.resolved_intents = Some(results);
        r
    }
}
