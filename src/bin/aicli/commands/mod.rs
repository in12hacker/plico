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
    let agent_id = extract_agent_id(args);
    kernel.track_cli_usage(&agent_id);
    let response = execute_local_inner(kernel, args);
    let json = serde_json::to_string(&response).unwrap_or_default();
    let tokens = plico::api::semantic::estimate_tokens(&json) as u64;
    kernel.track_cli_token_usage(&agent_id, tokens);
    response
}

fn execute_local_inner(kernel: &AIKernel, args: &[String]) -> ApiResponse {
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
        Some("get-node") => cmd_get_node(kernel, args),
        Some("edges") => cmd_list_edges(kernel, args),
        Some("rm-node") => cmd_rm_node(kernel, args),
        Some("rm-edge") => cmd_rm_edge(kernel, args),
        Some("update-node") => cmd_update_node(kernel, args),
        Some("edge-history") => cmd_edge_history(kernel, args),
        Some("complete") => cmd_agent_complete(kernel, args),
        Some("fail") => cmd_agent_fail(kernel, args),
        Some("intent") => cmd_intent(kernel, args),
        Some("status") => cmd_agent_status(kernel, args),
        Some("suspend") => cmd_agent_suspend(kernel, args),
        Some("resume") => cmd_agent_resume(kernel, args),
        Some("terminate") => cmd_agent_terminate(kernel, args),
        Some("checkpoint") => cmd_agent_checkpoint(kernel, args),
        Some("restore-checkpoint") => cmd_agent_restore(kernel, args),
        Some("tool") => cmd_tool(kernel, args),
        Some("send") => cmd_send_message(kernel, args),
        Some("messages") => cmd_read_messages(kernel, args),
        Some("ack") => cmd_ack_message(kernel, args),
        Some("memmove") => cmd_memmove(kernel, args),
        Some("memdelete") => cmd_memdelete(kernel, args),
        Some("events") => cmd_events(kernel, args),
        Some("context") => cmd_context(kernel, args),
        Some("history") => cmd_history(kernel, args),
        Some("rollback") => cmd_rollback(kernel, args),
        Some("skills") => cmd_skills(kernel, args),
        Some("quota") => cmd_quota(kernel, args),
        Some("discover") => cmd_discover(kernel, args),
        Some("delegate") => cmd_delegate(kernel, args),
        Some("session-start") => cmd_session_start(kernel, args),
        Some("session-end") => cmd_session_end(kernel, args),
        Some("delta") => cmd_delta(kernel, args),
        Some("growth") => cmd_growth(kernel, args),
        Some("hybrid") => cmd_hybrid(kernel, args),
        Some("permission") | Some("perm") => cmd_permission(kernel, args),
        Some("hook") => cmd_hook(kernel, args),
        Some("system-status") => {
            kernel.handle_api_request(plico::api::semantic::ApiRequest::SystemStatus)
        }
        Some("health") => {
            kernel.handle_api_request(plico::api::semantic::ApiRequest::HealthReport)
        }
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

// F-2: Unified agent ID extraction — reads --agent or --from
pub fn extract_agent_id(args: &[String]) -> String {
    extract_arg(args, "--agent")
        .or_else(|| extract_arg(args, "--from"))
        .unwrap_or_else(|| "cli".to_string())
}

// ─── Output Formatting ──────────────────────────────────────────────

/// If AICLI_OUTPUT=json, print ApiResponse as JSON; otherwise human-readable.
/// Returns true if the command succeeded (ok), false if it failed.
pub fn print_result(response: &ApiResponse) -> bool {
    let format = std::env::var("AICLI_OUTPUT").unwrap_or_else(|_| "json".to_string());
    if format != "human" {
        println!("{}", serde_json::to_string_pretty(response).unwrap_or_default());
        return response.ok;
    }

    // A-8d: Check tool_result success first for tool call failure
    if let Some(ref result) = response.tool_result {
        if !result.success {
            eprintln!("Tool error: {}", result.error.clone().unwrap_or_default());
            return false;
        }
    }

    // Human-readable output
    if let Some(cid) = &response.cid {
        println!("CID: {}", cid);
    }
    if let Some(tags) = &response.tags {
        if tags.is_empty() {
            println!("No tags in filesystem.");
        } else {
            println!("All tags ({} total):", tags.len());
            for t in tags {
                println!("  - {}", t);
            }
        }
    }
    if let Some(data) = &response.data {
        if response.cid.is_some() {
            println!("---");
        }
        println!("{}", data);
    }
    if let Some(results) = &response.results {
        if results.is_empty() {
            println!("No results.");
        } else {
            for (i, r) in results.iter().enumerate() {
                println!("{}. [relevance={:.2}] {}", i + 1, r.relevance, r.cid);
                println!("   Tags: {:?}", r.tags);
            }
        }
    }
    if let Some(agents) = &response.agents {
        if agents.is_empty() {
            println!("No active agents.");
        } else {
            for a in agents {
                println!("Agent: {} ({}) - {}", a.name, a.id, a.state);
            }
        }
    }
    if let Some(memories) = &response.memory {
        if memories.is_empty() {
            println!("No memories.");
        } else {
            for m in memories {
                println!("{}", m);
            }
        }
    }
    if let Some(neighbors) = &response.neighbors {
        if neighbors.is_empty() {
            println!("No graph neighbors.");
        } else {
            for (i, n) in neighbors.iter().enumerate() {
                println!("{}. [auth={:.3}] {} ({}) {} \"{}\"",
                    i + 1, n.authority_score, n.node_id, n.node_type, n.edge_type, n.label);
            }
        }
    }
    if let Some(deleted) = &response.deleted {
        if deleted.is_empty() {
            println!("Recycle bin is empty.");
        } else {
            println!("Recycle bin ({} items):", deleted.len());
            for d in deleted {
                println!("  CID: {}", d.cid);
                println!("    Tags: {:?}", d.tags);
            }
        }
    }
    if let Some(node_id) = &response.node_id {
        println!("Node ID: {}", node_id);
    }
    if let Some(agent_id) = &response.agent_id {
        println!("Agent ID: {}", agent_id);
    }
    if let Some(nodes) = &response.nodes {
        if nodes.is_empty() {
            println!("No KG nodes found.");
        } else {
            println!("KG nodes ({} total):", nodes.len());
            for n in nodes {
                println!("  {} [{:?}] \"{}\"", n.id, n.node_type, n.label);
            }
        }
    }
    if let Some(paths) = &response.paths {
        if paths.is_empty() {
            println!("No paths found.");
        } else {
            println!("Paths ({} found):", paths.len());
            for (i, path) in paths.iter().enumerate() {
                let labels: Vec<&str> = path.iter().map(|n| n.label.as_str()).collect();
                println!("  {}: {}", i + 1, labels.join(" → "));
            }
        }
    }
    if let Some(edges) = &response.edges {
        if edges.is_empty() {
            println!("No edges found.");
        } else {
            println!("Edges ({} total):", edges.len());
            for e in edges {
                println!("  {} --[{:?} w={:.2}]--> {}", e.src, e.edge_type, e.weight, e.dst);
            }
        }
    }
    if let Some(events) = &response.events {
        if events.is_empty() {
            println!("No events found.");
        } else {
            println!("Events ({} found):", events.len());
            for e in events {
                println!("  {} [{:?}]", e.label, e.event_type);
            }
        }
    }
    if let Some(ctx) = &response.context_data {
        let layer_str = if ctx.degraded {
            format!("{}, degraded from {}", ctx.layer, ctx.actual_layer.as_deref().unwrap_or("?"))
        } else {
            ctx.layer.clone()
        };
        println!("Context [{}] for CID: {}", layer_str, ctx.cid);
        println!("Tokens estimate: {}", ctx.tokens_estimate);
        if let Some(ref reason) = ctx.degradation_reason {
            println!("Degradation: {}", reason);
        }
        println!("---");
        println!("{}", ctx.content);
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
            if let Some(schema) = t.schema.as_object() {
                if !schema.is_empty() {
                    println!("  Parameters: {}", serde_json::to_string_pretty(&t.schema).unwrap_or_default());
                }
            }
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
        if msgs.is_empty() {
            println!("No messages.");
        } else {
            println!("Messages ({} total):", msgs.len());
            for m in msgs {
                let status = if m.read { "read" } else { "unread" };
                println!("  [{}] from={} id={}", status, m.from, m.id);
            }
        }
    }
    if let Some(sub_id) = &response.subscription_id {
        println!("Subscription ID: {}", sub_id);
    }
    if let Some(ke) = &response.kernel_events {
        if ke.is_empty() {
            println!("No pending kernel events.");
        } else {
            println!("Kernel events ({} pending):", ke.len());
            for e in ke {
                println!("  {:?}", e);
            }
        }
    }
    if let Some(status) = &response.system_status {
        println!("System Status (at {}ms):", status.timestamp_ms);
        println!("  CAS objects: {}", status.cas_object_count);
        println!("  Agents:      {}", status.agent_count);
        println!("  Tags:        {}", status.tag_count);
        println!("  KG nodes:    {}", status.kg_node_count);
        println!("  KG edges:    {}", status.kg_edge_count);
    }
    if let Some(assembly) = &response.context_assembly {
        println!("Context Assembly ({}/{} tokens, {}/{} items):",
            assembly.total_tokens, assembly.budget,
            assembly.candidates_included, assembly.candidates_considered);
        for item in &assembly.items {
            println!("  [{}] {} (~{} tokens)", item.layer.name(), &item.cid[..16.min(item.cid.len())], item.tokens_estimate);
        }
    }
    if let Some(usage) = &response.agent_usage {
        println!("Agent: {}", usage.agent_id);
        println!("  Memory: {}/{}", usage.memory_entries,
            if usage.memory_quota == 0 { "unlimited".to_string() } else { usage.memory_quota.to_string() });
        println!("  Tool calls: {}", usage.tool_call_count);
        println!("  CPU quota: {}",
            if usage.cpu_time_quota == 0 { "unlimited".to_string() } else { format!("{}ms", usage.cpu_time_quota) });
        if usage.allowed_tools.is_empty() {
            println!("  Tools: all allowed");
        } else {
            println!("  Tools: {:?}", usage.allowed_tools);
        }
        if usage.last_active_ms > 0 {
            println!("  Last active: {}ms", usage.last_active_ms);
        }
    }
    if let Some(cards) = &response.agent_cards {
        if cards.is_empty() {
            println!("No agents discovered.");
        } else {
            println!("Discovered agents ({} total):", cards.len());
            for c in cards {
                println!("  {} ({}) — state={}, tools={}, mem={}, calls={}",
                    c.name, c.agent_id, c.state, c.tools.len(), c.memory_entries, c.tool_call_count);
            }
        }
    }
    if let Some(d) = &response.delegation {
        println!("Delegated: {} → {}", d.from, d.to);
        println!("  Intent: {}", d.intent_id);
        println!("  Message: {}", d.message_id);
    }
    if let Some(history) = &response.event_history {
        if history.is_empty() {
            println!("No event history.");
        } else {
            println!("Event history ({} events):", history.len());
            for e in history {
                println!("  seq={} t={}ms {:?}", e.seq, e.timestamp_ms, e.event);
            }
        }
    }
    if let Some(skills) = &response.discovered_skills {
        if skills.is_empty() {
            println!("No skills discovered.");
        } else {
            println!("Discovered skills ({} total):", skills.len());
            for s in skills {
                println!("  {} [{}] — {} (agent: {})",
                    s.name, s.node_id, s.description, s.agent_id);
                if !s.tags.is_empty() {
                    println!("    tags: {:?}", s.tags);
                }
            }
        }
    }
    if let Some(ss) = &response.session_started {
        println!("Session started: {}", ss.session_id);
        if let Some(ref wc) = ss.warm_context {
            println!("  Warm context: {}", wc);
        }
        if let Some(ref cp) = ss.restored_checkpoint {
            println!("  Restored checkpoint: {}", cp.checkpoint_id);
        }
        println!("  Changes since last: {} (est. {} tokens)", ss.changes_since_last.len(), ss.token_estimate);
    }
if let Some(se) = &response.session_ended {
        println!("Session ended");
        if let Some(ref cid) = se.checkpoint_id {
            println!("  Checkpoint: {}", cid);
        }
        println!("  Last seq: {}", se.last_seq);
        // F-6: Display consolidation report
        if let Some(ref c) = se.consolidation {
            println!("  Consolidation: reviewed {} ephemeral, {} working",
                c.ephemeral_before, c.working_before);
            if c.promoted > 0 { println!("    ↑ {} promoted", c.promoted); }
            if c.evicted > 0 { println!("    ✕ {} evicted", c.evicted); }
            if c.linked > 0 { println!("    🔗 {} KG edges", c.linked); }
        }
    }
    if let Some(delta) = &response.delta_result {
        if delta.changes.is_empty() {
            println!("No changes since seq {}", delta.from_seq);
        } else {
            println!("Changes ({} found, seq {} → {}, est. {} tokens):",
                delta.changes.len(), delta.from_seq, delta.to_seq, delta.token_estimate);
            for c in &delta.changes {
                println!("  [seq={}] {} — {}", c.seq, c.change_type, c.summary);
            }
        }
    }
    if let Some(hr) = &response.hybrid_result {
        println!("Hybrid search results ({} items, {} vector, {} graph, {} paths, est. {} tokens):",
            hr.items.len(), hr.vector_hits, hr.graph_hits, hr.paths_found, hr.token_estimate);
        for (i, hit) in hr.items.iter().enumerate() {
            println!("  {}. [combined={:.2}] {}", i + 1, hit.combined_score, hit.cid);
            println!("     preview: {}", &hit.content_preview[..hit.content_preview.len().min(80)]);
            println!("     vector={:.2} graph={:.2}", hit.vector_score, hit.graph_score);
        }
    }
    if let Some(gr) = &response.growth_report {
        println!("Growth report for {} (period: {:?})", gr.agent_id, gr.period);
        println!("  Sessions: {}", gr.sessions_total);
        println!("  Token efficiency: {:.2} (first_5 avg vs last_5 avg)", gr.token_efficiency_ratio);
        println!("  Intent cache hit rate: {:.2}", gr.intent_cache_hit_rate);
        println!("  Memories: stored={}, shared={}", gr.memories_stored, gr.memories_shared);
        println!("  Procedures learned: {}", gr.procedures_learned);
        println!("  KG nodes/edges created: {}/{}", gr.kg_nodes_created, gr.kg_edges_created);
    }
    if let Some(total) = response.total_count {
        let shown = response.results.as_ref().map(|r| r.len())
            .or(response.nodes.as_ref().map(|n| n.len()))
            .or(response.edges.as_ref().map(|e| e.len()))
            .or(response.events.as_ref().map(|e| e.len()))
            .or(response.messages.as_ref().map(|m| m.len()))
            .unwrap_or(0);
        if response.has_more == Some(true) {
            println!("Showing {}/{} (use --offset/--limit for pagination)", shown, total);
        }
    }

    // Handle operation feedback message (L-1/F-47)
    if let Some(ref msg) = response.message {
        println!("{}", msg);
    }
    if let Some(hr) = &response.health_report {
        let status_str = if hr.healthy { "HEALTHY" } else { "DEGRADED" };
        println!("System Health: {} (at {}ms)", status_str, hr.timestamp_ms);
        println!("  CAS objects:    {}", hr.cas_objects);
        println!("  Agents:         {}", hr.agents);
        println!("  KG nodes:       {}", hr.kg_nodes);
        println!("  KG edges:       {}", hr.kg_edges);
        println!("  Active sessions: {}", hr.active_sessions);
        println!("  Embedding:      {}", hr.embedding_backend);
        println!("  Roundtrip:      {} ({}ms)", if hr.roundtrip_ok { "OK" } else { "FAILED" }, hr.roundtrip_ms);
        if !hr.degradations.is_empty() {
            println!("  Degradations:");
            for d in &hr.degradations {
                println!("    ⚠ [{}] {} — {}", d.component, d.severity, d.message);
            }
        }
    }

    // Error path: stderr + non-zero exit
    if !response.ok {
        if let Some(e) = &response.error {
            eprintln!("Error: {}", e);
        }
        return false;
    }

    response.ok
}
