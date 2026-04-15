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
            Some(ApiRequest::Search { query, agent_id, limit })
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
    match kernel.get_object(&cid, "cli") {
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

    if query.is_empty() {
        eprintln!("Error: search requires a query. Use: search --query <text> or: search <text>");
        return ApiResponse::error("empty query");
    }

    let results = kernel.semantic_search(&query, &agent_id, limit);

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
    let name = extract_arg(args, "--register").unwrap_or_else(|| "unnamed".to_string());
    let id = kernel.register_agent(name.clone());
    println!("Agent registered: {} (ID: {})", name, id);
    ApiResponse { ok: true, cid: None, data: None, results: None, agent_id: Some(id), agents: None, memory: None, tags: None, neighbors: None, deleted: None, error: None }
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

fn cmd_deleted(kernel: &AIKernel, _args: &[String]) -> ApiResponse {
    let entries = kernel.list_deleted();
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

  search       Semantic search
    --query TEXT      Natural language query
    <text>            Positional query (alternative to --query)
    --agent ID       Agent ID

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

NOTES:
  • delete/restore require Delete permission (use --agent kernel, or grant first)
  • tags command is local-only (not available via TCP)
  • TCP mode connects to plicod at --tcp [addr] for persistent storage

EXAMPLES:
  aicli --root /tmp/plico put --content "Meeting notes" --tags "meeting,project-x"
  aicli --tcp 127.0.0.1:7879 put --content "..." --tags "..."
  aicli search "meeting notes about project x"
  aicli --tcp 127.0.0.1:7879 agent --register MyAgent
"#);
}
