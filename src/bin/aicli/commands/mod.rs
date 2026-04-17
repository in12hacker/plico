//! CLI command handlers — thin wrapper delegating to handlers/ submodules.
//!
//! Original 734-line file split per Ariadne compliance (<700 lines).
//! Command dispatch remains here; implementation moved to handlers/.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;

pub mod handlers;

// Re-export parse helpers for binary main.rs compatibility.
pub use handlers::graph::parse_node_type;
pub use handlers::graph::parse_edge_type;

/// Execute a command locally (direct kernel access).
pub fn execute_local(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    use handlers::*;
    match args.first().map(|s| s.as_str()) {
        Some("put") | Some("create") => cmd_create(kernel, args),
        Some("get") | Some("read") => cmd_read(kernel, args),
        Some("search") => cmd_search(kernel, args),
        Some("update") => cmd_update(kernel, args),
        Some("delete") => cmd_delete(kernel, args),
        Some("agent") => cmd_agent(kernel, args),
        Some("agents") => cmd_agents(kernel, args),
        Some("remember") => cmd_remember(kernel, args),
        Some("recall") => cmd_recall(kernel, args),
        Some("tags") => cmd_tags(kernel, args),
        Some("explore") => cmd_explore(kernel, args),
        Some("deleted") => cmd_deleted(kernel, args),
        Some("restore") => cmd_restore(kernel, args),
        Some("node") => cmd_add_node(kernel, args),
        Some("edge") => cmd_add_edge(kernel, args),
        Some("nodes") => cmd_list_nodes(kernel, args),
        Some("paths") => cmd_find_paths(kernel, args),
        Some("intent") => cmd_intent(kernel, args),
        Some("status") => cmd_agent_status(kernel, args),
        Some("suspend") => cmd_agent_suspend(kernel, args),
        Some("resume") => cmd_agent_resume(kernel, args),
        Some("terminate") => cmd_agent_terminate(kernel, args),
        Some("tool") => cmd_tool(kernel, args),
        Some("send") => cmd_send_message(kernel, args),
        Some("messages") => cmd_read_messages(kernel, args),
        Some("ack") => cmd_ack_message(kernel, args),
        Some("memmove") => cmd_memmove(kernel, args),
        Some("memdelete") => cmd_memdelete(kernel, args),
        Some("events") => cmd_events(kernel, args),
        _ => ApiResponse::error("Unknown command. Run: aicli --help"),
    }
}

// ─── Shared utilities (used by handlers) ────────────────────────────

pub fn extract_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

pub fn extract_tags(args: &[String], flag: &str) -> Vec<String> {
    extract_tags_opt(args, flag).unwrap_or_default()
}

pub fn extract_tags_opt(args: &[String], flag: &str) -> Option<Vec<String>> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(|s| s.split(',').map(String::from).collect())
}

// ─── Output Formatting ──────────────────────────────────────────────

pub fn print_result(response: &ApiResponse) {
    if let Some(cid) = &response.cid {
        println!("CID: {}", cid);
    }
    if let Some(results) = &response.results {
        for (i, r) in results.iter().enumerate() {
            println!("{}. [relevance={:.2}] {}", i + 1, r.relevance, r.cid);
            println!("   Tags: {:?}", r.tags);
        }
    }
    if let Some(tags) = &response.tags {
        println!("All tags ({} total):", tags.len());
        for t in tags {
            println!("  - {}", t);
        }
    }
    if let Some(agents) = &response.agents {
        for a in agents {
            println!("Agent: {} ({}) - {}", a.name, a.id, a.state);
        }
    }
    if let Some(memories) = &response.memory {
        for m in memories {
            println!("{}", m);
        }
    }
    if let Some(neighbors) = &response.neighbors {
        for (i, n) in neighbors.iter().enumerate() {
            println!("{}. [auth={:.3}] {} ({}) {} \"{}\"",
                i + 1, n.authority_score, n.node_id, n.node_type, n.edge_type, n.label);
        }
    }
    if let Some(deleted) = &response.deleted {
        for d in deleted {
            println!("CID: {} (deleted)", d.cid);
            println!("   Tags: {:?}", d.tags);
        }
    }
    if let Some(node_id) = &response.node_id {
        println!("Node ID: {}", node_id);
    }
    if let Some(nodes) = &response.nodes {
        println!("KG nodes ({} total):", nodes.len());
        for n in nodes {
            println!("  {} [{:?}] \"{}\"", n.id, n.node_type, n.label);
        }
    }
    if let Some(paths) = &response.paths {
        println!("Paths ({} found):", paths.len());
        for (i, path) in paths.iter().enumerate() {
            let labels: Vec<&str> = path.iter().map(|n| n.label.as_str()).collect();
            println!("  {}: {}", i + 1, labels.join(" → "));
        }
    }
    if let Some(intent_id) = &response.intent_id {
        println!("Intent ID: {}", intent_id);
    }
    if let Some(state) = &response.agent_state {
        println!("Agent state: {}", state);
    }
    if let Some(pending) = &response.pending_intents {
        println!("Pending intents: {}", pending);
    }
    if let Some(tools) = &response.tools {
        println!("Tools ({} total):", tools.len());
        for t in tools {
            println!("  {} — {}", t.name, t.description);
        }
    }
    if let Some(result) = &response.tool_result {
        if result.success {
            println!("{}", serde_json::to_string_pretty(&result.output).unwrap_or_default());
        } else if let Some(err) = &result.error {
            eprintln!("Tool error: {}", err);
        }
    }
    if let Some(intents) = &response.resolved_intents {
        println!("Resolved intents ({} total):", intents.len());
        for (i, ri) in intents.iter().enumerate() {
            println!("  {}. [{:.2}] {}", i + 1, ri.confidence, ri.explanation);
        }
    }
    if let Some(msgs) = &response.messages {
        println!("Messages ({} total):", msgs.len());
        for m in msgs {
            let status = if m.read { "read" } else { "unread" };
            println!("  [{}] from={} id={}", status, m.from, m.id);
        }
    }
    if !response.ok {
        if let Some(e) = &response.error {
            eprintln!("Error: {}", e);
        }
    }
}
