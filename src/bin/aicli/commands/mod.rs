//! CLI command handlers — extracted from aicli.rs for module compliance.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use plico::fs::{KGNodeType, KGEdgeType};

/// Execute a command locally (direct kernel access).
pub fn execute_local(kernel: &AIKernel, args: &[String]) -> ApiResponse {
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
        _ => ApiResponse::error("Unknown command. Run: aicli --help"),
    }
}

// ─── Object CRUD ────────────────────────────────────────────────────

fn cmd_create(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let content = extract_arg(args, "--content").unwrap_or_default();
    let tags = extract_tags(args, "--tags");
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let intent = extract_arg(args, "--intent");

    match kernel.semantic_create(content.into_bytes(), tags, &agent_id, intent) {
        Ok(cid) => ApiResponse::with_cid(cid),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

fn cmd_read(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = args.get(1).cloned().unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.get_object(&cid, &agent_id) {
        Ok(obj) => {
            println!("CID: {}", obj.cid);
            println!("Tags: {:?}", obj.meta.tags);
            println!("Type: {}", obj.meta.content_type);
            if let Some(intent) = obj.meta.intent {
                println!("Intent: {}", intent);
            }
            println!("---");
            println!("{}", String::from_utf8_lossy(&obj.data));
            ApiResponse::ok()
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

fn cmd_search(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let query = extract_arg(args, "--query")
        .or_else(|| args.get(1).cloned())
        .unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let limit = extract_arg(args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let require_tags = extract_tags_opt(args, "--require-tags")
        .or_else(|| extract_tags_opt(args, "-t"))
        .unwrap_or_default();
    let exclude_tags = extract_tags_opt(args, "--exclude-tags").unwrap_or_default();
    let since = extract_arg(args, "--since").and_then(|s| s.parse::<i64>().ok());
    let until = extract_arg(args, "--until").and_then(|s| s.parse::<i64>().ok());

    if query.is_empty() {
        eprintln!("Error: search requires a query. Use: search --query <text> or: search <text>");
        return ApiResponse::error("empty query");
    }

    let results = kernel.semantic_search_with_time(
        &query, &agent_id, limit, require_tags, exclude_tags, since, until,
    );

    if results.is_empty() {
        println!("No results for: {}", query);
    } else {
        for (i, r) in results.iter().enumerate() {
            println!("{}. [relevance={:.2}] {}", i + 1, r.relevance, r.cid);
            println!("   Tags: {:?}", r.meta.tags);
            if let Some(intent) = &r.meta.intent {
                println!("   Intent: {}", intent);
            }
        }
    }
    ApiResponse::ok()
}

fn cmd_update(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let content = extract_arg(args, "--content").unwrap_or_default();
    let new_tags = extract_tags_opt(args, "--tags");
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.semantic_update(&cid, content.into_bytes(), new_tags, &agent_id) {
        Ok(new_cid) => {
            println!("Updated. Old CID: {}", cid);
            println!("New CID: {}", new_cid);
            ApiResponse::with_cid(new_cid)
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

fn cmd_delete(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.semantic_delete(&cid, &agent_id) {
        Ok(()) => {
            println!("Deleted (logical): {}", cid);
            ApiResponse::ok()
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

// ─── Agent Management ───────────────────────────────────────────────

fn cmd_agent(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    if args.get(1).map(|s| s.as_str()) == Some("set-resources") {
        let target = args.get(2).cloned().unwrap_or_default();
        let mq = extract_arg(args, "--memory-quota").and_then(|s| s.parse().ok());
        let cq = extract_arg(args, "--cpu-time-quota").and_then(|s| s.parse().ok());
        let at = extract_arg(args, "--allowed-tools")
            .map(|s| s.split(',').map(String::from).collect::<Vec<_>>());
        return match kernel.agent_set_resources(&target, mq, cq, at) {
            Ok(()) => {
                println!("Resources updated for agent: {}", target);
                ApiResponse::ok()
            }
            Err(e) => ApiResponse::error(e.to_string()),
        };
    }

    let name = extract_arg(args, "--register").unwrap_or_else(|| "unnamed".to_string());
    let id = kernel.register_agent(name.clone());
    println!("Agent registered: {} (ID: {})", name, id);
    let mut r = ApiResponse::ok();
    r.agent_id = Some(id);
    r
}

fn cmd_agents(kernel: &AIKernel, _args: &[String]) -> ApiResponse {
    let agents = kernel.list_agents();
    if agents.is_empty() {
        println!("No active agents.");
    } else {
        for a in &agents {
            println!("- {} ({}) [{:?}]", a.name, a.id, a.state);
        }
    }
    ApiResponse::ok()
}

fn cmd_agent_status(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_status(&agent_id) {
        Some((_id, state, pending)) => {
            println!("Agent: {}", agent_id);
            println!("State: {}", state);
            println!("Pending intents: {}", pending);
            let mut r = ApiResponse::ok();
            r.agent_state = Some(state);
            r.pending_intents = Some(pending);
            r
        }
        None => {
            println!("Agent not found: {}", agent_id);
            ApiResponse::error(format!("Agent not found: {}", agent_id))
        }
    }
}

fn cmd_agent_suspend(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_suspend(&agent_id) {
        Ok(()) => { println!("Agent {} suspended", agent_id); ApiResponse::ok() }
        Err(e) => { println!("Error: {}", e); ApiResponse::error(e.to_string()) }
    }
}

fn cmd_agent_resume(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_resume(&agent_id) {
        Ok(()) => { println!("Agent {} resumed", agent_id); ApiResponse::ok() }
        Err(e) => { println!("Error: {}", e); ApiResponse::error(e.to_string()) }
    }
}

fn cmd_agent_terminate(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_terminate(&agent_id) {
        Ok(()) => { println!("Agent {} terminated", agent_id); ApiResponse::ok() }
        Err(e) => { println!("Error: {}", e); ApiResponse::error(e.to_string()) }
    }
}

// ─── Memory ────────────────────────────────────────────────────────

fn cmd_remember(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let content = extract_arg(args, "--content").unwrap_or_default();
    kernel.remember(&agent_id, content);
    println!("Remembered for agent: {}", agent_id);
    ApiResponse::ok()
}

fn cmd_recall(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let memories = kernel.recall(&agent_id);
    if memories.is_empty() {
        println!("No memories for agent: {}", agent_id);
    } else {
        for m in &memories {
            println!("[{:?}] {}", m.tier, m.content.display());
        }
    }
    ApiResponse::ok()
}

fn cmd_tags(kernel: &AIKernel, _args: &[String]) -> ApiResponse {
    let tags = kernel.list_tags();
    if tags.is_empty() {
        println!("No tags in filesystem.");
    } else {
        println!("All tags ({} total):", tags.len());
        for tag in &tags {
            println!("  - {}", tag);
        }
    }
    ApiResponse::ok()
}

// ─── Graph / KG ────────────────────────────────────────────────────

fn cmd_explore(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let edge_type = extract_arg(args, "--edge-type");
    let depth: u8 = extract_arg(args, "--depth")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_u8)
        .min(3);

    let neighbors = kernel.graph_explore_raw(&cid, edge_type.as_deref(), depth);

    if neighbors.is_empty() {
        println!("No graph neighbors for: {}", cid);
    } else {
        println!("Graph neighbors of {} (depth {}):", cid, depth);
        for (i, (node_id, label, node_type, edge_str, auth)) in neighbors.iter().enumerate() {
            println!("{}. [auth={:.3}] {} ({}) {} \"{}\"", i + 1, auth, node_id, node_type, edge_str, label);
        }
    }
    ApiResponse::ok()
}

fn cmd_add_node(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let label = extract_arg(args, "--label").unwrap_or_default();
    let node_type = parse_node_type(&extract_arg(args, "--type").unwrap_or_else(|| "entity".to_string()));
    let props = extract_arg(args, "--props")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.kg_add_node(&label, node_type, props, &agent_id) {
        Ok(id) => {
            println!("Node created: {} (type={}, label=\"{}\")", id, node_type, label);
            ApiResponse::with_node_id(id)
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

fn cmd_add_edge(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let src = extract_arg(args, "--src").unwrap_or_default();
    let dst = extract_arg(args, "--dst").unwrap_or_default();
    let edge_type = parse_edge_type(&extract_arg(args, "--type").unwrap_or_else(|| "related_to".to_string()));
    let weight = extract_arg(args, "--weight").and_then(|s| s.parse().ok());
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.kg_add_edge(&src, &dst, edge_type, weight, &agent_id) {
        Ok(()) => {
            println!("Edge created: {} --[{}]--> {}", src, edge_type, dst);
            ApiResponse::ok()
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

fn cmd_list_nodes(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let node_type = extract_arg(args, "--type").map(|s| parse_node_type(&s));
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    let nodes = kernel.kg_list_nodes(node_type, &agent_id);
    if nodes.is_empty() {
        println!("No KG nodes found.");
    } else {
        println!("KG nodes ({} total):", nodes.len());
        for n in &nodes {
            println!("  {} [{}] \"{}\"", n.id, n.node_type, n.label);
        }
    }
    ApiResponse::ok()
}

fn cmd_find_paths(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let src = extract_arg(args, "--src").unwrap_or_default();
    let dst = extract_arg(args, "--dst").unwrap_or_default();
    let depth: u8 = extract_arg(args, "--depth")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
        .min(5);

    let paths = kernel.kg_find_paths(&src, &dst, depth);
    if paths.is_empty() {
        println!("No paths from {} to {} (depth {})", src, dst, depth);
    } else {
        println!("Paths from {} to {} ({} found):", src, dst, paths.len());
        for (i, path) in paths.iter().enumerate() {
            let labels: Vec<&str> = path.iter().map(|n| n.label.as_str()).collect();
            println!("  {}: {}", i + 1, labels.join(" → "));
        }
    }
    ApiResponse::ok()
}

// ─── Deleted / Recycle Bin ─────────────────────────────────────────

fn cmd_deleted(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let entries = kernel.list_deleted(&agent_id);
    if entries.is_empty() {
        println!("Recycle bin is empty.");
    } else {
        println!("Recycle bin ({} items):", entries.len());
        for entry in &entries {
            println!("  CID: {}", entry.cid);
            println!("    Tags: {:?}", entry.original_meta.tags);
            println!("    Deleted at: {}", entry.deleted_at);
        }
    }
    ApiResponse::ok()
}

fn cmd_restore(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.restore_deleted(&cid, &agent_id) {
        Ok(()) => {
            println!("Restored: {}", cid);
            ApiResponse::ok()
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

// ─── Intent ────────────────────────────────────────────────────────

fn cmd_intent(kernel: &AIKernel, args: &[String]) -> ApiResponse {
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

        let id = kernel.submit_intent(priority, description, action, Some(agent_id));
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
        return ApiResponse::error("Usage: intent \"<natural language text>\" or intent --description \"...\"");
    }

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

// ─── Messaging ────────────────────────────────────────────────────

fn cmd_send_message(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let from = extract_arg(args, "--from").unwrap_or_else(|| "cli".to_string());
    let to = extract_arg(args, "--to").unwrap_or_default();
    let payload_str = extract_arg(args, "--payload").unwrap_or_else(|| "{}".to_string());
    let payload: serde_json::Value = serde_json::from_str(&payload_str).unwrap_or_default();

    match kernel.send_message(&from, &to, payload) {
        Ok(msg_id) => {
            println!("Message sent: {}", msg_id);
            let mut r = ApiResponse::ok();
            r.data = Some(msg_id);
            r
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

fn cmd_read_messages(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let unread_only = args.iter().any(|a| a == "--unread");

    let msgs = kernel.read_messages(&agent_id, unread_only);
    if msgs.is_empty() {
        println!("No messages for agent: {}", agent_id);
    } else {
        println!("Messages for {} ({} total):", agent_id, msgs.len());
        for m in &msgs {
            let status = if m.read { "read" } else { "unread" };
            println!("  [{}] from={} id={}", status, m.from, m.id);
            println!("    payload: {}", serde_json::to_string(&m.payload).unwrap_or_default());
        }
    }
    let mut r = ApiResponse::ok();
    r.messages = Some(msgs);
    r
}

fn cmd_ack_message(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let message_id = args.get(1).cloned().unwrap_or_default();

    if kernel.ack_message(&agent_id, &message_id) {
        println!("Message acknowledged: {}", message_id);
        ApiResponse::ok()
    } else {
        ApiResponse::error(format!("Message not found: {}", message_id))
    }
}

// ─── Tool ─────────────────────────────────────────────────────────

fn cmd_tool(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    use plico::api::semantic::ApiRequest;

    match args.get(1).map(|s| s.as_str()) {
        Some("list") => {
            let req = ApiRequest::ToolList { agent_id: "cli".to_string() };
            let resp = kernel.handle_api_request(req);
            if let Some(ref tools) = resp.tools {
                println!("Available tools ({} total):", tools.len());
                for t in tools {
                    println!("  {} — {}", t.name, t.description);
                }
            }
            resp
        }
        Some("describe") => {
            let name = args.get(2).cloned().unwrap_or_default();
            let req = ApiRequest::ToolDescribe { tool: name, agent_id: "cli".to_string() };
            let resp = kernel.handle_api_request(req);
            if let Some(ref tools) = resp.tools {
                if let Some(t) = tools.first() {
                    println!("Tool: {}", t.name);
                    println!("Description: {}", t.description);
                    println!("Schema: {}", serde_json::to_string_pretty(&t.schema).unwrap_or_default());
                }
            }
            resp
        }
        Some("call") => {
            let name = args.get(2).cloned().unwrap_or_default();
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let params_str = extract_arg(args, "--params").unwrap_or_else(|| "{}".to_string());
            let params: serde_json::Value = serde_json::from_str(&params_str).unwrap_or_default();
            let req = ApiRequest::ToolCall { tool: name, params, agent_id };
            let resp = kernel.handle_api_request(req);
            if let Some(ref result) = resp.tool_result {
                if result.success {
                    println!("{}", serde_json::to_string_pretty(&result.output).unwrap_or_default());
                } else {
                    eprintln!("Tool error: {}", result.error.as_deref().unwrap_or("unknown"));
                }
            }
            resp
        }
        _ => {
            println!("Usage: tool <list|describe|call> ...");
            println!("  tool list                  — list all available tools");
            println!("  tool describe <name>       — describe a specific tool");
            println!("  tool call <name> --params JSON --agent ID  — call a tool");
            ApiResponse::error("unknown tool subcommand")
        }
    }
}

// ─── Utilities ─────────────────────────────────────────────────────

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

pub fn parse_node_type(s: &str) -> KGNodeType {
    match s {
        "entity" => KGNodeType::Entity,
        "fact" => KGNodeType::Fact,
        "document" => KGNodeType::Document,
        "agent" => KGNodeType::Agent,
        "memory" => KGNodeType::Memory,
        _ => KGNodeType::Entity,
    }
}

pub fn parse_edge_type(s: &str) -> KGEdgeType {
    match s {
        "associates_with" => KGEdgeType::AssociatesWith,
        "follows" => KGEdgeType::Follows,
        "mentions" => KGEdgeType::Mentions,
        "causes" => KGEdgeType::Causes,
        "reminds" => KGEdgeType::Reminds,
        "part_of" => KGEdgeType::PartOf,
        "similar_to" => KGEdgeType::SimilarTo,
        "related_to" => KGEdgeType::RelatedTo,
        "has_attendee" => KGEdgeType::HasAttendee,
        "has_document" => KGEdgeType::HasDocument,
        "has_media" => KGEdgeType::HasMedia,
        "has_decision" => KGEdgeType::HasDecision,
        "has_preference" => KGEdgeType::HasPreference,
        "suggests_action" => KGEdgeType::SuggestsAction,
        "motivated_by" => KGEdgeType::MotivatedBy,
        _ => KGEdgeType::RelatedTo,
    }
}

// ─── Output Formatting ─────────────────────────────────────────────

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
