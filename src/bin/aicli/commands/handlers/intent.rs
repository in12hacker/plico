//! Intent resolution commands — application-layer NL handling.
//!
//! The ChainRouter is created here at the interface layer, not in the kernel.
//! This follows the soul principle: OS provides resources, agents decide how to think.

use plico::intent::{ChainRouter, IntentRouter};
use plico::intent::execution;
use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use super::extract_arg;

fn make_router() -> ChainRouter {
    ChainRouter::new(None)
}

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

    let router = make_router();

    if args.iter().any(|a| a == "--execute") {
        let learn = args.iter().any(|a| a == "--learn");
        let threshold = extract_arg(args, "--threshold")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.7);
        match execution::execute_sync(kernel, &router, &text, &agent_id, threshold, learn) {
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
        let results = match router.resolve(&text, &agent_id) {
            Ok(r) => r,
            Err(e) => {
                println!("Could not resolve: {}", text);
                return ApiResponse::error(e.to_string());
            }
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_kernel() -> plico::kernel::AIKernel {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("EMBEDDING_BACKEND", "stub");
        plico::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel")
    }

    #[test]
    fn test_cmd_intent_resolve_basic() {
        let kernel = make_test_kernel();
        let args = vec![
            "intent".to_string(), "hello world".to_string(),
            "--agent".to_string(), "cli".to_string(),
        ];
        let r = cmd_intent(&kernel, &args);
        // May succeed or fail depending on router training, but should not panic
        // If resolution fails with "Could not resolve", that's an API response, not error
        assert!(r.error.is_none() || r.resolved_intents.is_some());
    }

    #[test]
    fn test_cmd_intent_list_basic() {
        let kernel = make_test_kernel();
        // Submit an intent via --description
        let submit_args = vec![
            "intent".to_string(),
            "--description".to_string(), "test intent for list".to_string(),
            "--agent".to_string(), "cli".to_string(),
        ];
        let r = cmd_intent(&kernel, &submit_args);
        assert!(r.error.is_none());
        assert!(r.intent_id.is_some());
    }

    #[test]
    fn test_cmd_intent_execute_basic() {
        let kernel = make_test_kernel();
        let args = vec![
            "intent".to_string(), "hello".to_string(),
            "--execute".to_string(),
            "--agent".to_string(), "cli".to_string(),
        ];
        let r = cmd_intent(&kernel, &args);
        // --execute may fail if no matching procedural, but should not panic
        // Error response is acceptable
        assert!(r.error.is_none() || r.data.is_some() || r.resolved_intents.is_some());
    }

    #[test]
    fn test_cmd_intent_empty_text_error() {
        let kernel = make_test_kernel();
        // Empty text should return error
        let args = vec!["intent".to_string()];
        let r = cmd_intent(&kernel, &args);
        assert!(r.error.is_some());
    }

    #[test]
    fn test_cmd_intent_with_priority() {
        let kernel = make_test_kernel();
        let args = vec![
            "intent".to_string(),
            "--description".to_string(), "high priority test".to_string(),
            "--priority".to_string(), "high".to_string(),
            "--agent".to_string(), "cli".to_string(),
        ];
        let r = cmd_intent(&kernel, &args);
        assert!(r.error.is_none());
        assert!(r.intent_id.is_some());
    }
}
