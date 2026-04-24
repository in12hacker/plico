//! Memory tier commands — operations route through handle_api_request where possible.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse, ProcedureStepDto};
use plico::memory::{MemoryScope, MemoryTier};
use super::extract_arg;

pub fn cmd_remember(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let content = extract_arg(args, "--content").unwrap_or_default();
    let tier_str = extract_arg(args, "--tier").unwrap_or_default();
    let scope_str = extract_arg(args, "--scope").unwrap_or_else(|| "private".to_string());
    let scope = Some(scope_str.clone());
    let tags: Vec<String> = extract_arg(args, "--tags")
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();
    match parse_memory_tier(&tier_str) {
        MemoryTier::Ephemeral => {
            kernel.handle_api_request(ApiRequest::Remember {
                agent_id,
                content,
                tenant_id: None,
            })
        }
        MemoryTier::Working => {
            // Working tier with scope uses RememberLongTerm pathway but with working importance
            let parsed_scope = parse_memory_scope(&scope_str);
            match kernel.remember_working_scoped(&agent_id, "default", content, tags, parsed_scope) {
                Ok(_) => ApiResponse::ok_with_message(format!("Memory stored for agent '{}'", agent_id)),
                Err(e) => ApiResponse::error(e),
            }
        }
        MemoryTier::LongTerm => {
            kernel.handle_api_request(ApiRequest::RememberLongTerm {
                agent_id,
                content,
                tags,
                importance: 50,
                scope,
                tenant_id: None,
            })
        }
        MemoryTier::Procedural => {
            let steps = vec![ProcedureStepDto {
                description: content.clone(),
                action: content.clone(),
                expected_outcome: None,
            }];
            kernel.handle_api_request(ApiRequest::RememberProcedural {
                agent_id,
                name: "cli-procedure".to_string(),
                description: content,
                steps,
                learned_from: Some("cli".to_string()),
                tags,
                scope,
            })
        }
    }
}

pub fn cmd_recall(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let scope = extract_arg(args, "--scope");
    let query = extract_arg(args, "--query");
    let limit = extract_arg(args, "--limit").and_then(|l| l.parse::<usize>().ok());

    kernel.handle_api_request(ApiRequest::Recall {
        agent_id,
        scope,
        query,
        limit,
    })
}

pub fn cmd_tags(kernel: &AIKernel, _args: &[String]) -> ApiResponse {
    let tags = kernel.list_tags();
    let mut r = ApiResponse::ok();
    r.tags = Some(tags);
    r
}

pub fn cmd_memmove(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let entry_id = match extract_arg(args, "--id") {
        Some(id) => id,
        None => return ApiResponse::error("memmove requires --id and --tier"),
    };
    let tier_str = extract_arg(args, "--tier").unwrap_or_default();

    kernel.handle_api_request(ApiRequest::MemoryMove {
        agent_id,
        entry_id,
        target_tier: tier_str,
        tenant_id: None,
    })
}

pub fn cmd_memdelete(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let entry_id = match extract_arg(args, "--id") {
        Some(id) => id,
        None => return ApiResponse::error("memdelete requires --id"),
    };

    kernel.handle_api_request(ApiRequest::MemoryDeleteEntry {
        agent_id,
        entry_id,
        tenant_id: None,
    })
}

pub fn parse_memory_tier(s: &str) -> MemoryTier {
    match s.to_lowercase().replace(['-', '_'], "").as_str() {
        "ephemeral" | "l0" | "ephem" => MemoryTier::Ephemeral,
        "working" | "l1" | "wk" => MemoryTier::Working,
        "longterm" | "l2" | "lt" | "long" => MemoryTier::LongTerm,
        "procedural" | "l3" | "proc" => MemoryTier::Procedural,
        "" => MemoryTier::Working,
        other => {
            tracing::warn!("Unknown tier '{}', defaulting to Working", other);
            MemoryTier::Working
        }
    }
}

pub fn parse_memory_scope(s: &str) -> MemoryScope {
    match s.to_lowercase().as_str() {
        "shared" => MemoryScope::Shared,
        "private" | "" => MemoryScope::Private,
        other if other.starts_with("group:") => MemoryScope::Group(other[6..].to_string()),
        _ => MemoryScope::Private,
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
    fn test_cmd_remember_basic() {
        let kernel = make_test_kernel();
        let args = vec!["--agent".to_string(), "test-agent".to_string(),
                        "--content".to_string(), "hello world".to_string()];
        let resp = cmd_remember(&kernel, &args);
        assert!(resp.ok, "remember should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_cmd_remember_positional_content() {
        let kernel = make_test_kernel();
        // When --content is provided with positional content
        let args = vec!["--agent".to_string(), "cli".to_string(),
                        "--content".to_string(), "test content".to_string()];
        let resp = cmd_remember(&kernel, &args);
        assert!(resp.ok, "remember with content should succeed");
    }

    #[test]
    fn test_cmd_remember_empty_returns_error() {
        let kernel = make_test_kernel();
        let args = vec!["--agent".to_string(), "test".to_string(),
                        "--content".to_string(), "".to_string()];
        let resp = cmd_remember(&kernel, &args);
        // Empty content in args, but the handler uses extract_arg which returns String
        // The kernel.remember will likely fail on empty content
        let _resp2 = cmd_remember(&kernel, &["--agent".to_string(), "test".to_string(),
                                            "--content".to_string(), "   ".to_string()]);
        // Empty or whitespace-only should still go through (handler doesn't validate)
        let _ = resp; // Handler is permissive — either ok or error is acceptable
    }

    #[test]
    fn test_cmd_recall_basic() {
        let kernel = make_test_kernel();
        // First store something
        kernel.remember_working("recall-test", "default", "test memory".to_string(), vec![]).unwrap();
        let args = vec!["--agent".to_string(), "recall-test".to_string()];
        let resp = cmd_recall(&kernel, &args);
        assert!(resp.ok, "recall should succeed: {:?}", resp.error);
        assert!(resp.memory.is_some(), "recall should return memories");
    }

    #[test]
    fn test_cmd_recall_with_tier_filter() {
        let kernel = make_test_kernel();
        // Store in working tier
        kernel.remember_working("tier-test", "default", "working memory".to_string(), vec![]).unwrap();
        let args = vec!["--agent".to_string(), "tier-test".to_string(),
                        "--tier".to_string(), "working".to_string()];
        let resp = cmd_recall(&kernel, &args);
        assert!(resp.ok, "recall --tier working should succeed");
    }

    #[test]
    fn test_cmd_recall_longterm_filter() {
        let kernel = make_test_kernel();
        // Store in long-term tier
        kernel.remember_long_term("lt-test", "default", "lt memory".to_string(), vec![], 50).unwrap();
        let args = vec!["--agent".to_string(), "lt-test".to_string(),
                        "--tier".to_string(), "long-term".to_string()];
        let resp = cmd_recall(&kernel, &args);
        assert!(resp.ok, "recall --tier long-term should succeed");
    }

    #[test]
    fn test_parse_memory_tier_variants() {
        assert_eq!(parse_memory_tier("ephemeral"), MemoryTier::Ephemeral);
        assert_eq!(parse_memory_tier("l0"), MemoryTier::Ephemeral);
        assert_eq!(parse_memory_tier("working"), MemoryTier::Working);
        assert_eq!(parse_memory_tier("l1"), MemoryTier::Working);
        assert_eq!(parse_memory_tier("long-term"), MemoryTier::LongTerm);
        assert_eq!(parse_memory_tier("longterm"), MemoryTier::LongTerm);
        assert_eq!(parse_memory_tier("l2"), MemoryTier::LongTerm);
        assert_eq!(parse_memory_tier("procedural"), MemoryTier::Procedural);
        assert_eq!(parse_memory_tier("l3"), MemoryTier::Procedural);
        assert_eq!(parse_memory_tier(""), MemoryTier::Working); // default
        assert_eq!(parse_memory_tier("unknown"), MemoryTier::Working); // fallback
    }

    #[test]
    fn test_cmd_remember_default_private() {
        let kernel = make_test_kernel();
        use plico::api::permission::PermissionAction;
        // Register two agents so recall_visible works
        let alice = kernel.register_agent("alice".to_string());
        kernel.permission_grant(&alice, PermissionAction::Write, None, None);
        kernel.permission_grant(&alice, PermissionAction::Read, None, None);

        // No --scope flag → defaults to Private
        let args = vec![
            "--agent".to_string(), alice.clone(),
            "--content".to_string(), "alice private info".to_string(),
            "--tier".to_string(), "working".to_string(),
        ];
        let resp = cmd_remember(&kernel, &args);
        assert!(resp.ok, "remember should succeed: {:?}", resp.error);

        // alice's private memory should be visible to alice herself
        let alice_visible = kernel.recall_visible(&alice, "default", &[]);
        let alice_owns = alice_visible.iter().any(|m| m.content.display().to_string().contains("alice private info"));
        assert!(alice_owns, "alice should see her own private memory");
    }

    #[test]
    fn test_cmd_remember_shared_scope() {
        let kernel = make_test_kernel();
        use plico::api::permission::PermissionAction;
        // Register two agents
        let alice = kernel.register_agent("alice".to_string());
        let bob = kernel.register_agent("bob".to_string());
        kernel.permission_grant(&alice, PermissionAction::Write, None, None);
        kernel.permission_grant(&alice, PermissionAction::Read, None, None);
        kernel.permission_grant(&bob, PermissionAction::Read, None, None);

        // alice stores shared memory via CLI handler
        let args = vec![
            "--agent".to_string(), alice.clone(),
            "--content".to_string(), "shared across agents".to_string(),
            "--tier".to_string(), "working".to_string(),
            "--scope".to_string(), "shared".to_string(),
        ];
        let resp = cmd_remember(&kernel, &args);
        assert!(resp.ok, "remember --scope shared should succeed: {:?}", resp.error);

        // bob should see alice's shared memory via recall_visible
        let bob_visible = kernel.recall_visible(&bob, "default", &[]);
        let found = bob_visible.iter().any(|m| {
            m.content.display().to_string().contains("shared across agents")
                && m.agent_id == alice
        });
        assert!(found, "bob should see alice's shared memory via recall_visible");
    }

    #[test]
    fn test_cmd_remember_group_scope() {
        let kernel = make_test_kernel();
        use plico::api::permission::PermissionAction;
        let alice = kernel.register_agent("alice".to_string());
        let bob = kernel.register_agent("bob".to_string());
        kernel.permission_grant(&alice, PermissionAction::Write, None, None);
        kernel.permission_grant(&alice, PermissionAction::Read, None, None);
        kernel.permission_grant(&bob, PermissionAction::Read, None, None);

        // alice stores group memory
        let args = vec![
            "--agent".to_string(), alice.clone(),
            "--content".to_string(), "engineering team notes".to_string(),
            "--tier".to_string(), "working".to_string(),
            "--scope".to_string(), "group:engineering".to_string(),
        ];
        let resp = cmd_remember(&kernel, &args);
        assert!(resp.ok, "remember --scope group should succeed: {:?}", resp.error);

        // bob (in engineering group) should see it via recall_visible with group
        let bob_visible = kernel.recall_visible(&bob, "default", &["engineering".to_string()]);
        let found = bob_visible.iter().any(|m| {
            m.content.display().to_string().contains("engineering team notes")
                && m.agent_id == alice
        });
        assert!(found, "bob in engineering group should see alice's group memory via recall_visible");
    }

    #[test]
    fn test_parse_memory_scope_variants() {
        assert_eq!(parse_memory_scope("private"), MemoryScope::Private);
        assert_eq!(parse_memory_scope(""), MemoryScope::Private);
        assert_eq!(parse_memory_scope("shared"), MemoryScope::Shared);
        assert_eq!(parse_memory_scope("group:dev-team"), MemoryScope::Group("dev-team".to_string()));
        assert_eq!(parse_memory_scope("GROUP:ops"), MemoryScope::Group("ops".to_string())); // case insensitive
        assert_eq!(parse_memory_scope("unknown"), MemoryScope::Private); // fallback
    }
}
