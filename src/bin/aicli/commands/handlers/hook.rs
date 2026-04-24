//! Hook management commands — all operations route through handle_api_request.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use super::extract_arg;

pub fn cmd_hook(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    match args.get(1).map(|s| s.as_str()) {
        Some("list") => {
            kernel.handle_api_request(ApiRequest::HookList)
        }
        Some("register") => {
            let point = extract_arg(args, "--point").unwrap_or_else(|| "PreToolCall".to_string());
            let action = extract_arg(args, "--action").unwrap_or_else(|| "block".to_string());
            let tool_pattern = extract_arg(args, "--tool");
            let reason = extract_arg(args, "--reason");
            let priority = extract_arg(args, "--priority").and_then(|s| s.parse().ok());

            kernel.handle_api_request(ApiRequest::HookRegister {
                point, action, tool_pattern, reason, priority,
            })
        }
        _ => ApiResponse::error("Usage: hook <list|register> [--point PreToolCall] [--tool cas.delete] [--action block] [--reason \"...\"]"),
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
    fn test_hook_list_empty() {
        let kernel = make_test_kernel();
        let args = vec!["hook".to_string(), "list".to_string()];
        let r = cmd_hook(&kernel, &args);
        assert!(r.ok);
        assert!(r.hook_list.is_some());
    }

    #[test]
    fn test_hook_register_and_list() {
        let kernel = make_test_kernel();
        let args = vec![
            "hook".to_string(), "register".to_string(),
            "--point".to_string(), "PreToolCall".to_string(),
            "--tool".to_string(), "cas.delete".to_string(),
            "--action".to_string(), "block".to_string(),
            "--reason".to_string(), "no deletes allowed".to_string(),
        ];
        let r = cmd_hook(&kernel, &args);
        assert!(r.ok, "register should succeed: {:?}", r.error);

        let list_args = vec!["hook".to_string(), "list".to_string()];
        let r2 = cmd_hook(&kernel, &list_args);
        assert!(r2.ok);
        let hooks = r2.hook_list.unwrap();
        assert!(!hooks.is_empty());
        assert!(hooks.iter().any(|h| h.point.contains("PreToolCall")),
            "Expected a PreToolCall hook in list: {:?}", hooks);
    }

    #[test]
    fn test_hook_register_log_action() {
        let kernel = make_test_kernel();
        let args = vec![
            "hook".to_string(), "register".to_string(),
            "--point".to_string(), "PostToolCall".to_string(),
            "--action".to_string(), "log".to_string(),
        ];
        let r = cmd_hook(&kernel, &args);
        assert!(r.ok);
    }

    #[test]
    fn test_hook_unknown_subcommand() {
        let kernel = make_test_kernel();
        let args = vec!["hook".to_string(), "unknown".to_string()];
        let r = cmd_hook(&kernel, &args);
        assert!(r.error.is_some());
    }
}
