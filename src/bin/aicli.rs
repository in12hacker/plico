//! aicli — AI-Friendly CLI for Plico
//!
//! Command-line interface for AI agents. Every operation is semantic —
//! no paths, no filenames. Just content, tags, and intent.
//!
//! # Usage
//!
//! ```bash
//! # Store content
//! aicli put --content "Project X meeting notes" --tags "meeting,project-x"
//!
//! # Retrieve by CID
//! aicli get <CID>
//!
//! # Semantic search
//! aicli search --query "meeting notes about project x"
//!
//! # Update
//! aicli update --cid <CID> --content "Updated notes..."
//!
//! # Delete (soft delete)
//! aicli delete --cid <CID>
//!
//! # Agent management
//! aicli agent --register "MyAgent"
//! aicli agents --list
//!
//! # Memory
//! aicli remember --agent agent1 --content "Don't forget to check the logs"
//! aicli recall --agent agent1
//! ```

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use std::path::PathBuf;
use std::io::Write;
use std::net::TcpStream;
use tracing_subscriber::util::SubscriberInitExt;
use std::time::Duration;

fn main() {
    // Initialize structured logging (reads RUST_LOG env var; defaults to INFO)
    // Use fmt().finish() instead of fmt::init() to avoid background worker
    // threads that prevent the process from exiting cleanly.
    let env = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(&env)
        .finish()
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return;
    }

    // Determine mode: local (direct kernel) or tcp (remote daemon)
    // --tcp may be followed by an optional address: --tcp [addr]
    // --addr addr is also accepted as an explicit alternative
    let mode = args.iter().position(|a| a == "--tcp")
        .map(|tcp_idx| {
            // Prefer --addr if present, otherwise use the arg immediately after --tcp
            let addr = args.iter().position(|a| a == "--addr")
                .and_then(|i| args.get(i + 1))
                .cloned()
                .or_else(|| args.get(tcp_idx + 1).filter(|s| !s.starts_with("--")).cloned())
                .unwrap_or_else(|| "127.0.0.1:7878".to_string());
            Mode::Tcp(addr)
        })
        .unwrap_or(Mode::Local);

    match mode {
        Mode::Local => run_local(&args),
        Mode::Tcp(addr) => run_tcp(&args, &addr),
    }
    // Explicit exit to bypass any tokio runtime or tracing worker threads that
    // may not shut down cleanly on process exit.
    std::process::exit(0);
}

enum Mode {
    Local,
    Tcp(String),
}

fn run_local(args: &[String]) {
    // Parse --root flag and skip bare "--" separators so remaining args
    // start with the command even when invoked as:
    //   cargo run -- aicli --root /tmp -- put ...
    let mut filtered = Vec::with_capacity(args.len());
    let mut i = 0;
    let mut root = PathBuf::from("/tmp/plico");

    while i < args.len() {
        match args[i].as_str() {
            "--root" if i + 1 < args.len() => {
                root = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            // Skip bare "--" separators (e.g. "cargo run -- aicli ...")
            "--" => {
                i += 1;
            }
            other => {
                filtered.push(other.to_string());
                i += 1;
            }
        }
    }

    let kernel = AIKernel::new(root).expect("Failed to initialize kernel");
    let result = execute_local(&kernel, &filtered);
    print_result(&result);
}

fn run_tcp(args: &[String], addr: &str) {
    // Filter out --tcp and --addr (and their values) before building request
    let mut i = 0;
    let mut filtered = Vec::with_capacity(args.len());
    while i < args.len() {
        match args[i].as_str() {
            "--tcp" | "--addr" => {
                i += 2; // skip flag and its value
            }
            a => {
                filtered.push(a.to_string());
                i += 1;
            }
        }
    }

    let mut stream = TcpStream::connect_timeout(
        &addr.parse().unwrap_or_else(|_| "127.0.0.1:7878".parse().unwrap()),
        Duration::from_secs(5),
    ).expect("Failed to connect to daemon");
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok();

    let req = build_request(&filtered).expect("Failed to build request");
    let json = serde_json::to_vec(&req).expect("Failed to serialize request");

    stream.write_all(&json).expect("Failed to send request");
    stream.write_all(b"\n").expect("Failed to send newline");
    stream.flush().expect("Failed to flush");

    // Read one line of response (daemon sends JSON + "\n", then keeps connection open)
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("Failed to read response");
    let line = line.trim();

    let response: ApiResponse = serde_json::from_str(line).expect("Failed to parse response");
    print_result(&response);
}

fn execute_local(kernel: &AIKernel, args: &[String]) -> ApiResponse {
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

fn build_request(args: &[String]) -> Option<ApiRequest> {
    match args.first().map(|s| s.as_str()) {
        Some("put") | Some("create") => {
            let content = extract_arg(args, "--content").unwrap_or_default();
            let tags = extract_tags(args, "--tags");
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Create { content, content_encoding: Default::default(), tags, agent_id, intent: extract_arg(args, "--intent") })
        }
        Some("get") | Some("read") => {
            let cid = args.get(1).cloned().unwrap_or_default();
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Read { cid, agent_id })
        }
        Some("search") => {
            let query = extract_arg(args, "--query")
                .or_else(|| args.get(1).cloned())
                .unwrap_or_default();
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let limit = extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            let require_tags = extract_tags_opt(args, "--require-tags")
                .unwrap_or_else(|| extract_tags_opt(args, "-t").unwrap_or_default());
            let exclude_tags = extract_tags_opt(args, "--exclude-tags").unwrap_or_default();
            let since = extract_arg(args, "--since").and_then(|s| s.parse::<i64>().ok());
            let until = extract_arg(args, "--until").and_then(|s| s.parse::<i64>().ok());
            Some(ApiRequest::Search { query, agent_id, limit, require_tags, exclude_tags, since, until })
        }
        Some("update") => {
            let cid = extract_arg(args, "--cid").unwrap_or_default();
            let content = extract_arg(args, "--content").unwrap_or_default();
            let new_tags = extract_tags_opt(args, "--tags");
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Update { cid, content, content_encoding: Default::default(), new_tags, agent_id })
        }
        Some("delete") => {
            let cid = extract_arg(args, "--cid").unwrap_or_default();
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Delete { cid, agent_id })
        }
        Some("agent") => {
            let name = extract_arg(args, "--register").unwrap_or_else(|| "unnamed".to_string());
            Some(ApiRequest::RegisterAgent { name })
        }
        Some("agents") => Some(ApiRequest::ListAgents),
        Some("remember") => {
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let content = extract_arg(args, "--content").unwrap_or_default();
            Some(ApiRequest::Remember { agent_id, content })
        }
        Some("recall") => {
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Recall { agent_id })
        }
        Some("explore") => {
            let cid = extract_arg(args, "--cid").unwrap_or_default();
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let edge_type = extract_arg(args, "--edge-type");
            let depth = extract_arg(args, "--depth").and_then(|s| s.parse().ok());
            Some(ApiRequest::Explore { cid, edge_type, depth, agent_id })
        }
        Some("deleted") => {
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::ListDeleted { agent_id })
        }
        Some("restore") => {
            let cid = extract_arg(args, "--cid").unwrap_or_default();
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Restore { cid, agent_id })
        }
        Some("node") => {
            let label = extract_arg(args, "--label").unwrap_or_default();
            let node_type = parse_node_type(&extract_arg(args, "--type").unwrap_or_else(|| "entity".to_string()));
            let props = extract_arg(args, "--props")
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Null);
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::AddNode { label, node_type, properties: props, agent_id })
        }
        Some("edge") => {
            let src_id = extract_arg(args, "--src").unwrap_or_default();
            let dst_id = extract_arg(args, "--dst").unwrap_or_default();
            let edge_type = parse_edge_type(&extract_arg(args, "--type").unwrap_or_else(|| "related_to".to_string()));
            let weight = extract_arg(args, "--weight").and_then(|s| s.parse().ok());
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::AddEdge { src_id, dst_id, edge_type, weight, agent_id })
        }
        Some("nodes") => {
            let node_type = extract_arg(args, "--type").map(|s| parse_node_type(&s));
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::ListNodes { node_type, agent_id })
        }
        Some("paths") => {
            let src_id = extract_arg(args, "--src").unwrap_or_default();
            let dst_id = extract_arg(args, "--dst").unwrap_or_default();
            let max_depth = extract_arg(args, "--depth").and_then(|s| s.parse().ok());
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::FindPaths { src_id, dst_id, max_depth, agent_id })
        }
        Some("intent") => {
            if extract_arg(args, "--description").is_some() {
                let description = extract_arg(args, "--description").unwrap_or_default();
                let priority = extract_arg(args, "--priority").unwrap_or_else(|| "medium".to_string());
                let action = extract_arg(args, "--action");
                let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
                Some(ApiRequest::SubmitIntent { description, priority, action, agent_id })
            } else {
                let text = args.iter().skip(1)
                    .filter(|a| !a.starts_with("--"))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
                Some(ApiRequest::IntentResolve { text, agent_id })
            }
        }
        Some("status") => {
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::AgentStatus { agent_id })
        }
        Some("suspend") => {
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::AgentSuspend { agent_id })
        }
        Some("resume") => {
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::AgentResume { agent_id })
        }
        Some("terminate") => {
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::AgentTerminate { agent_id })
        }
        Some("send") => {
            let from = extract_arg(args, "--from").unwrap_or_else(|| "cli".to_string());
            let to = extract_arg(args, "--to").unwrap_or_default();
            let payload_str = extract_arg(args, "--payload").unwrap_or_else(|| "{}".to_string());
            let payload: serde_json::Value = serde_json::from_str(&payload_str).unwrap_or_default();
            Some(ApiRequest::SendMessage { from, to, payload })
        }
        Some("messages") => {
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let unread_only = args.iter().any(|a| a == "--unread");
            Some(ApiRequest::ReadMessages { agent_id, unread_only })
        }
        Some("ack") => {
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let message_id = args.get(1).cloned().unwrap_or_default();
            Some(ApiRequest::AckMessage { agent_id, message_id })
        }
        Some("tool") => {
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            match args.get(1).map(|s| s.as_str()) {
                Some("list") => Some(ApiRequest::ToolList { agent_id }),
                Some("describe") => {
                    let name = args.get(2).cloned().unwrap_or_default();
                    Some(ApiRequest::ToolDescribe { tool: name, agent_id })
                }
                Some("call") => {
                    let name = args.get(2).cloned().unwrap_or_default();
                    let params_str = extract_arg(args, "--params").unwrap_or_else(|| "{}".to_string());
                    let params: serde_json::Value = serde_json::from_str(&params_str).unwrap_or_default();
                    Some(ApiRequest::ToolCall { tool: name, params, agent_id })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

// ─── Command handlers ────────────────────────────────────────────────

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
    // Accept either --query <text> or a positional argument
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

fn cmd_agent(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    // agent set-resources <agent-id> --memory-quota N --cpu-time-quota N --allowed-tools "a,b"
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

// ─── KG Command Handlers ─────────────────────────────────────────────

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

// ─── Agent Lifecycle CLI Commands ──────────────────────────────────

fn cmd_intent(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    // If --description is present, this is a submit intent (legacy behavior).
    // Otherwise, the positional text is NL to resolve.
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

    // NL intent resolution mode
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

// ─── Tool CLI Commands ──────────────────────────────────────────────

fn cmd_tool(kernel: &AIKernel, args: &[String]) -> ApiResponse {
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

fn parse_node_type(s: &str) -> plico::fs::KGNodeType {
    use plico::fs::KGNodeType;
    match s {
        "entity" => KGNodeType::Entity,
        "fact" => KGNodeType::Fact,
        "document" => KGNodeType::Document,
        "agent" => KGNodeType::Agent,
        "memory" => KGNodeType::Memory,
        _ => KGNodeType::Entity,
    }
}

fn parse_edge_type(s: &str) -> plico::fs::KGEdgeType {
    use plico::fs::KGEdgeType;
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

// ─── Utilities ───────────────────────────────────────────────────────

fn extract_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn extract_tags(args: &[String], flag: &str) -> Vec<String> {
    extract_tags_opt(args, flag).unwrap_or_default()
}

fn extract_tags_opt(args: &[String], flag: &str) -> Option<Vec<String>> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(|s| s.split(',').map(String::from).collect())
}

fn print_result(response: &ApiResponse) {
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

fn print_help() {
    println!(r#"
Plico AI-Native OS — AI-Friendly CLI

USAGE:
  aicli [MODE] <command> [flags]

MODE:
  --tcp [addr]       Connect to plicod daemon (default: 127.0.0.1:7878)
  --root PATH        Storage root directory (default: /tmp/plico)
  (default: direct kernel access, no daemon)

COMMANDS:
  put/create   Store content with semantic tags
    --content TEXT   Content to store
    --tags TEXT      Comma-separated tags
    --intent TEXT    Optional intent description
    --agent ID       Agent ID (default: cli)

  get/read     Retrieve object by CID
    <CID>             Object CID to retrieve
    --agent ID        Requesting agent (default: cli); must match ownership / grants for read

  search       Semantic search with optional tag/time filtering
    --query TEXT      Natural language query
    <text>            Positional query (alternative to --query)
    --require-tags T  Only return files with ALL these tags (comma-sep)
    --exclude-tags T  Exclude files with any of these tags (comma-sep)
    -t               Short for --require-tags
    --since MS        Inclusive lower bound (Unix ms) — e.g. "几天前" resolved
    --until MS        Inclusive upper bound (Unix ms)
    --agent ID        Agent ID

  update       Update object content
    --cid CID        Object CID to update
    --content TEXT   New content
    --tags TEXT      Optional new tags

  delete       Logical delete (soft, requires Delete permission)
    --cid CID        Object CID to delete

  agent        Register a new agent
    --register NAME  Agent name

  agents        List active agents
    --list

  remember      Store ephemeral memory
    --agent ID       Agent ID
    --content TEXT   Memory content

  recall        Retrieve agent memories
    --agent ID       Agent ID

  explore       Graph neighbors of a CID
    --cid CID        Starting node CID
    --depth N        Traversal depth (default: 1, max: 3)
    --agent ID       Agent ID

  deleted       List logically deleted objects (recycle bin)

  restore       Restore a deleted object
    --cid CID        Object CID to restore

  node          Create a KG node (Entity/Fact/Document/Agent/Memory)
    --label TEXT     Node label
    --type TYPE      Node type: entity|fact|document|agent|memory (default: entity)
    --props JSON     JSON properties (optional)
    --agent ID       Agent ID

  edge          Create a KG edge between two nodes
    --src ID         Source node ID
    --dst ID         Destination node ID
    --type TYPE      Edge type: related_to|part_of|mentions|causes|... (default: related_to)
    --weight N       Edge weight 0.0-1.0 (optional, default: 1.0)
    --agent ID       Agent ID

  nodes         List KG nodes
    --type TYPE      Filter by node type (optional)
    --agent ID       Agent ID

  paths         Find paths between two KG nodes
    --src ID         Source node ID
    --dst ID         Destination node ID
    --depth N        Max traversal depth (default: 3, max: 5)

  intent        Submit an intent for agent execution
    --description TEXT  Intent description
    --priority P     Priority: critical|high|medium|low (default: medium)
    --action JSON    Optional JSON-encoded ApiRequest to execute
    --agent ID       Agent ID

  status        Query agent state
    --agent ID       Agent ID

  suspend       Suspend a running agent
    --agent ID       Agent ID

  resume        Resume a suspended agent
    --agent ID       Agent ID

  terminate     Permanently terminate an agent
    --agent ID       Agent ID

  tool list     List all available tools (Everything is a Tool)
  tool describe NAME  Describe a specific tool's schema
  tool call NAME --params JSON  Call a tool with JSON parameters
    --agent ID       Agent ID

NOTES:
  • delete/restore require Delete permission (use --agent kernel, or grant first)
  • tags command is local-only (not available via TCP)
  • TCP mode connects to plicod at --tcp [addr] for persistent storage

EXAMPLES:
  aicli --root /tmp/plico put --content "agent output data" --tags "embedding,batch-result"
  aicli --tcp 127.0.0.1:7879 put --content "..." --tags "..."
  aicli search "meeting notes about project x"
  aicli --tcp 127.0.0.1:7879 agent --register MyAgent
"#);
}
