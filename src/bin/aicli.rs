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
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return;
    }

    // Determine mode: local (direct kernel) or tcp (remote daemon)
    let mode = args.iter().position(|a| a == "--tcp")
        .map(|_| {
            let addr = args.iter().position(|a| a == "--addr")
                .and_then(|i| args.get(i + 1))
                .unwrap_or(&"127.0.0.1:7878".to_string())
                .clone();
            Mode::Tcp(addr)
        })
        .unwrap_or(Mode::Local);

    match mode {
        Mode::Local => run_local(&args),
        Mode::Tcp(addr) => run_tcp(&args, &addr),
    }
}

enum Mode {
    Local,
    Tcp(String),
}

fn run_local(args: &[String]) {
    let root = args.iter().position(|a| a == "--root")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/plico"));

    let kernel = AIKernel::new(root).expect("Failed to initialize kernel");

    let result = execute_local(&kernel, args);
    print_result(&result);
}

fn run_tcp(args: &[String], addr: &str) {
    let mut stream = TcpStream::connect_timeout(
        &addr.parse().unwrap_or_else(|_| "127.0.0.1:7878".parse().unwrap()),
        Duration::from_secs(5),
    ).expect("Failed to connect to daemon");
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok();

    let req = build_request(args).expect("Failed to build request");
    let json = serde_json::to_vec(&req).expect("Failed to serialize request");

    stream.write_all(&json).expect("Failed to send request");
    stream.write_all(b"\n").expect("Failed to send newline");

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).expect("Failed to read response");

    let response: ApiResponse = serde_json::from_slice(&buf).expect("Failed to parse response");
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
        _ => ApiResponse::error("Unknown command. Run: aicli --help"),
    }
}

fn build_request(args: &[String]) -> Option<ApiRequest> {
    match args.first().map(|s| s.as_str()) {
        Some("put") | Some("create") => {
            let content = extract_arg(args, "--content").unwrap_or_default();
            let tags = extract_tags(args, "--tags");
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Create { content, tags, agent_id, intent: extract_arg(args, "--intent") })
        }
        Some("get") | Some("read") => {
            let cid = args.get(1).cloned().unwrap_or_default();
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Read { cid, agent_id })
        }
        Some("search") => {
            let query = extract_arg(args, "--query").unwrap_or_default();
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let limit = extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            Some(ApiRequest::Search { query, agent_id, limit })
        }
        Some("update") => {
            let cid = extract_arg(args, "--cid").unwrap_or_default();
            let content = extract_arg(args, "--content").unwrap_or_default();
            let new_tags = extract_tags_opt(args, "--tags");
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Update { cid, content, new_tags, agent_id })
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
        Some("agents") => {
            if args.contains(&"--list".to_string()) {
                Some(ApiRequest::ListAgents)
            } else {
                Some(ApiRequest::ListAgents)
            }
        }
        Some("remember") => {
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let content = extract_arg(args, "--content").unwrap_or_default();
            Some(ApiRequest::Remember { agent_id, content })
        }
        Some("recall") => {
            let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Recall { agent_id })
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
    let query = extract_arg(args, "--query").unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let limit = extract_arg(args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

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
    ApiResponse { ok: true, cid: None, data: None, results: None, agent_id: Some(id), agents: None, memory: None, tags: None, error: None }
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
  aicli <command> [flags]

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
    --agent ID       Agent ID

  update       Update object content
    --cid CID        Object CID to update
    --content TEXT   New content
    --tags TEXT      Optional new tags

  delete       Logical delete (soft)
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

EXAMPLES:
  aicli put --content "Project X kickoff" --tags "meeting,project-x"
  aicli get 3a4b5c...
  aicli search --query "meeting notes about project x"
  aicli agent --register Summarizer
"#);
}
