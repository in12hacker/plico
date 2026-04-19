//! aicli — AI-Friendly CLI for Plico
//!
//! Command-line interface for AI agents. Every operation is semantic —
//! no paths, no filenames. Just content, tags, and intent.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use std::path::PathBuf;
use std::io::Write;
use std::net::TcpStream;
use tracing_subscriber::util::SubscriberInitExt;
use std::time::Duration;

mod commands;

fn main() {
    let env = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(&env)
        .with_writer(std::io::stderr)
        .finish()
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return;
    }

    let mode = args.iter().position(|a| a == "--tcp")
        .map(|tcp_idx| {
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
    std::process::exit(0);
}

enum Mode {
    Local,
    Tcp(String),
}

fn run_local(args: &[String]) {
    let mut filtered = Vec::with_capacity(args.len());
    let mut i = 0;
    let mut root = PathBuf::from("/tmp/plico");

    while i < args.len() {
        match args[i].as_str() {
            "--root" if i + 1 < args.len() => {
                root = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--" => { i += 1; }
            other => {
                filtered.push(other.to_string());
                i += 1;
            }
        }
    }

    let kernel = AIKernel::new(root).expect("Failed to initialize kernel");
    let result = commands::execute_local(&kernel, &filtered);
    commands::print_result(&result);
}

fn run_tcp(args: &[String], addr: &str) {
    let mut i = 0;
    let mut filtered = Vec::with_capacity(args.len());
    while i < args.len() {
        match args[i].as_str() {
            "--tcp" | "--addr" => { i += 2; }
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

    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("Failed to read response");
    let line = line.trim();

    let response: ApiResponse = serde_json::from_str(line).expect("Failed to parse response");
    commands::print_result(&response);
}

fn build_request(args: &[String]) -> Option<ApiRequest> {
    match args.first().map(|s| s.as_str()) {
        Some("put") | Some("create") => {
            let content = commands::extract_arg(args, "--content").unwrap_or_default();
            let tags = commands::extract_tags(args, "--tags");
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Create { api_version: None, content, content_encoding: Default::default(), tags, agent_id, tenant_id: None, agent_token: None, intent: commands::extract_arg(args, "--intent") })
        }
        Some("get") | Some("read") => {
            let cid = args.get(1).cloned().unwrap_or_default();
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Read { cid, agent_id, tenant_id: None, agent_token: None })
        }
        Some("search") => {
            let query = commands::extract_arg(args, "--query")
                .or_else(|| args.get(1).cloned())
                .unwrap_or_default();
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let limit = commands::extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            let require_tags = commands::extract_tags_opt(args, "--require-tags")
                .unwrap_or_else(|| commands::extract_tags_opt(args, "-t").unwrap_or_default());
            let exclude_tags = commands::extract_tags_opt(args, "--exclude-tags").unwrap_or_default();
            let since = commands::extract_arg(args, "--since").and_then(|s| s.parse::<i64>().ok());
            let until = commands::extract_arg(args, "--until").and_then(|s| s.parse::<i64>().ok());
            let offset = commands::extract_arg(args, "--offset").and_then(|s| s.parse().ok());
            Some(ApiRequest::Search { query, agent_id, tenant_id: None, agent_token: None, limit, offset, require_tags, exclude_tags, since, until })
        }
        Some("update") => {
            let cid = commands::extract_arg(args, "--cid").unwrap_or_default();
            let content = commands::extract_arg(args, "--content").unwrap_or_default();
            let new_tags = commands::extract_tags_opt(args, "--tags");
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Update { cid, content, content_encoding: Default::default(), new_tags, agent_id, tenant_id: None, agent_token: None })
        }
        Some("delete") => {
            let cid = commands::extract_arg(args, "--cid").unwrap_or_default();
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Delete { cid, agent_id, tenant_id: None, agent_token: None })
        }
        Some("agent") => {
            let name = commands::extract_arg(args, "--register").unwrap_or_else(|| "unnamed".to_string());
            Some(ApiRequest::RegisterAgent { name })
        }
        Some("agents") => Some(ApiRequest::ListAgents),
        Some("remember") => {
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let content = commands::extract_arg(args, "--content").unwrap_or_default();
            Some(ApiRequest::Remember { agent_id, tenant_id: None, content })
        }
        Some("recall") => {
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Recall { agent_id })
        }
        Some("explore") => {
            let cid = commands::extract_arg(args, "--cid").unwrap_or_default();
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let edge_type = commands::extract_arg(args, "--edge-type");
            let depth = commands::extract_arg(args, "--depth").and_then(|s| s.parse().ok());
            Some(ApiRequest::Explore { cid, edge_type, depth, agent_id })
        }
        Some("deleted") => {
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::ListDeleted { agent_id })
        }
        Some("restore") => {
            let cid = commands::extract_arg(args, "--cid").unwrap_or_default();
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Restore { cid, agent_id })
        }
        Some("history") => {
            let cid = commands::extract_arg(args, "--cid").unwrap_or_else(|| args.get(1).cloned().unwrap_or_default());
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::History { cid, agent_id })
        }
        Some("rollback") => {
            let cid = commands::extract_arg(args, "--cid").unwrap_or_else(|| args.get(1).cloned().unwrap_or_default());
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::Rollback { cid, agent_id })
        }
        Some("node") => {
            let label = commands::extract_arg(args, "--label").unwrap_or_default();
            let node_type = commands::parse_node_type(&commands::extract_arg(args, "--type").unwrap_or_else(|| "entity".to_string()));
            let props = commands::extract_arg(args, "--props")
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Null);
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::AddNode { label, node_type, properties: props, agent_id, tenant_id: None })
        }
        Some("edge") => {
            let src_id = commands::extract_arg(args, "--src").unwrap_or_default();
            let dst_id = commands::extract_arg(args, "--dst").unwrap_or_default();
            let edge_type = commands::parse_edge_type(&commands::extract_arg(args, "--type").unwrap_or_else(|| "related_to".to_string()));
            let weight = commands::extract_arg(args, "--weight").and_then(|s| s.parse().ok());
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::AddEdge { src_id, dst_id, edge_type, weight, agent_id, tenant_id: None })
        }
        Some("nodes") => {
            let node_type = commands::extract_arg(args, "--type").map(|s| commands::parse_node_type(&s));
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let limit = commands::extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            let offset = commands::extract_arg(args, "--offset").and_then(|s| s.parse().ok());
            if let Some(at_str) = commands::extract_arg(args, "--at-time") {
                let t: u64 = at_str.parse().unwrap_or(0);
                Some(ApiRequest::ListNodesAtTime { node_type, agent_id, tenant_id: None, t })
            } else {
                Some(ApiRequest::ListNodes { node_type, agent_id, tenant_id: None, limit, offset })
            }
        }
        Some("paths") => {
            let src_id = commands::extract_arg(args, "--src").unwrap_or_default();
            let dst_id = commands::extract_arg(args, "--dst").unwrap_or_default();
            let max_depth = commands::extract_arg(args, "--depth").and_then(|s| s.parse().ok());
            let weighted = args.iter().any(|a| a == "--weighted");
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::FindPaths { src_id, dst_id, max_depth, weighted, agent_id, tenant_id: None })
        }
        Some("intent") => {
            if commands::extract_arg(args, "--description").is_some() {
                let description = commands::extract_arg(args, "--description").unwrap_or_default();
                let priority = commands::extract_arg(args, "--priority").unwrap_or_else(|| "medium".to_string());
                let action = commands::extract_arg(args, "--action");
                let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
                Some(ApiRequest::SubmitIntent { description, priority, action, agent_id })
            } else {
                None
            }
        }
        Some("status") => {
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::AgentStatus { agent_id })
        }
        Some("suspend") => {
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::AgentSuspend { agent_id })
        }
        Some("resume") => {
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::AgentResume { agent_id })
        }
        Some("terminate") => {
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            Some(ApiRequest::AgentTerminate { agent_id })
        }
        Some("send") => {
            let from = commands::extract_arg(args, "--from").unwrap_or_else(|| "cli".to_string());
            let to = commands::extract_arg(args, "--to").unwrap_or_default();
            let payload_str = commands::extract_arg(args, "--payload").unwrap_or_else(|| "{}".to_string());
            let payload: serde_json::Value = serde_json::from_str(&payload_str).unwrap_or_default();
            Some(ApiRequest::SendMessage { from, to, payload })
        }
        Some("messages") => {
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let unread_only = args.iter().any(|a| a == "--unread");
            let limit = commands::extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            let offset = commands::extract_arg(args, "--offset").and_then(|s| s.parse().ok());
            Some(ApiRequest::ReadMessages { agent_id, unread_only, limit, offset })
        }
        Some("ack") => {
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let message_id = args.get(1).cloned().unwrap_or_default();
            Some(ApiRequest::AckMessage { agent_id, message_id })
        }
        Some("tool") => {
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            match args.get(1).map(|s| s.as_str()) {
                Some("list") => Some(ApiRequest::ToolList { agent_id }),
                Some("describe") => {
                    let name = args.get(2).cloned().unwrap_or_default();
                    Some(ApiRequest::ToolDescribe { tool: name, agent_id })
                }
                Some("call") => {
                    let name = args.get(2).cloned().unwrap_or_default();
                    let params_str = commands::extract_arg(args, "--params").unwrap_or_else(|| "{}".to_string());
                    let params: serde_json::Value = serde_json::from_str(&params_str).unwrap_or_default();
                    Some(ApiRequest::ToolCall { tool: name, params, agent_id })
                }
                _ => None,
            }
        }
        Some("events") => {
            let agent_id = commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
            let tags: Vec<String> = commands::extract_tags(args, "--tags");
            match args.get(1).map(|s| s.as_str()) {
                // events list [--since TS] [--until TS] [--tags TAGS]
                Some("list") => {
                    let since = commands::extract_arg(args, "--since")
                        .and_then(|s| s.parse().ok());
                    let until = commands::extract_arg(args, "--until")
                        .and_then(|s| s.parse().ok());
                    Some(ApiRequest::ListEvents { since, until, tags, event_type: None, agent_id, limit: None, offset: None })
                }
                // events by-time "last week" [--tags TAGS]
                Some("by-time") | Some("text") => {
                    let time_expression = args.get(2..)
                        .map(|v| v.iter().take_while(|s| !s.starts_with("--")).cloned().collect::<Vec<_>>().join(" "))
                        .unwrap_or_default();
                    Some(ApiRequest::ListEventsText { time_expression, tags, event_type: None, agent_id })
                }
                _ => {
                    // Default: show help hint
                    println!("Usage: events <list|by-time> [options]");
                    println!("  list    --since TS --until TS --tags TAGS");
                    println!("  by-time \"last week\" --tags TAGS");
                    println!();
                    println!("Examples:");
                    println!("  events list --since 1713000000000 --until 1713100000000");
                    println!("  events by-time \"上周\" --tags work");
                    None
                }
            }
        }
        _ => None,
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
    --agent ID        Requesting agent (default: cli)

  search       Semantic search with optional tag/time filtering
    --query TEXT      Natural language query
    <text>            Positional query (alternative to --query)
    --require-tags T  Only return files with ALL these tags (comma-sep)
    --exclude-tags T  Exclude files with any of these tags
    -t               Short for --require-tags
    --since MS        Inclusive lower bound (Unix ms)
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
    --at-time MS     Unix timestamp (ms) to query temporal validity (optional)
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

  tool list     List all available tools
  tool describe NAME  Describe a specific tool's schema
  tool call NAME --params JSON  Call a tool with JSON parameters
    --agent ID       Agent ID

NOTES:
  • delete/restore require Delete permission
  • tags command is local-only (not available via TCP)
  • TCP mode connects to plicod at --tcp [addr] for persistent storage
"#);
}
