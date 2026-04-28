//! aicli — AI-Friendly CLI for Plico
//!
//! Command-line interface for AI agents. Every operation is semantic —
//! no paths, no filenames. Just content, tags, and intent.
//!
//! # Connection Modes (Daemon-First)
//!
//! Default behavior: connect to `plicod` daemon via UDS (`~/.plico/plico.sock`).
//! If daemon is not running, auto-start it and wait for readiness.
//!
//! - `--embedded`: bypass daemon, embed kernel directly (for testing/debugging)
//! - `--tcp [addr]`: connect to remote daemon via TCP

use plico::kernel::AIKernel;
use plico::client::{KernelClient, RemoteClient};
use plico::api::semantic::{ApiRequest, ApiResponse};
use std::path::PathBuf;
use tracing_subscriber::util::SubscriberInitExt;

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

    let root = resolve_root(&args);
    let mode = resolve_mode(&args);

    let exit_ok = match mode {
        Mode::Embedded => run_embedded(&args, &root),
        Mode::Daemon => run_daemon(&args, &root),
        Mode::Tcp(addr) => run_tcp(&args, &addr),
    };
    std::process::exit(if exit_ok { 0 } else { 1 });
}

enum Mode {
    Daemon,
    Embedded,
    Tcp(String),
}

fn resolve_root(args: &[String]) -> PathBuf {
    if let Some(r) = extract_opt(args, "--root") {
        return PathBuf::from(r);
    }
    std::env::var("PLICO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join(".plico")
        })
}

fn resolve_mode(args: &[String]) -> Mode {
    if args.iter().any(|a| a == "--embedded") {
        return Mode::Embedded;
    }
    if let Some(tcp_idx) = args.iter().position(|a| a == "--tcp") {
        let addr = extract_opt(args, "--addr")
            .or_else(|| args.get(tcp_idx + 1).filter(|s| !s.starts_with("--")).cloned())
            .unwrap_or_else(|| "127.0.0.1:7878".to_string());
        return Mode::Tcp(addr);
    }
    Mode::Daemon
}

/// Filter out mode/root flags, leaving only command + command flags.
fn filter_args(args: &[String]) -> Vec<String> {
    let mut filtered = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" | "--addr" => { i += 2; }
            "--tcp" => {
                i += 1;
                if i < args.len() && !args[i].starts_with("--") { i += 1; }
            }
            "--embedded" | "--" => { i += 1; }
            other => {
                filtered.push(other.to_string());
                i += 1;
            }
        }
    }
    filtered
}

fn extract_opt(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

// ── Daemon mode (default) ───────────────────────────────────────────

fn run_daemon(args: &[String], root: &PathBuf) -> bool {
    let sock_path = root.join("plico.sock");
    let filtered = filter_args(args);

    // Handle --help locally in daemon mode
    if filtered.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return true;
    }

    let client = RemoteClient::uds(sock_path.clone());
    if client.is_reachable() {
        return execute_via_client(&client, &filtered);
    }

    // Daemon not running — try to auto-start
    if !try_auto_start_daemon(root, &sock_path) {
        eprintln!("Error: daemon not running and auto-start failed.");
        eprintln!("  Start manually: plicod --root {:?}", root);
        eprintln!("  Or use: aicli --embedded <command>");
        return false;
    }

    let client = RemoteClient::uds(sock_path);
    execute_via_client(&client, &filtered)
}

/// Fork plicod in the background and wait for the socket to become reachable.
fn try_auto_start_daemon(root: &std::path::Path, sock_path: &std::path::Path) -> bool {
    let plicod = which_plicod();
    let Some(plicod_bin) = plicod else {
        eprintln!("Warning: plicod binary not found in PATH");
        return false;
    };

    eprintln!("Starting plicod daemon...");
    let child = std::process::Command::new(&plicod_bin)
        .arg("--root")
        .arg(root.as_os_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    match child {
        Ok(_) => {
            wait_for_socket(sock_path, std::time::Duration::from_secs(10))
        }
        Err(e) => {
            eprintln!("Failed to spawn plicod: {}", e);
            false
        }
    }
}

fn which_plicod() -> Option<PathBuf> {
    // Check adjacent to current exe first
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("plicod");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    // Fallback: search PATH
    std::env::var("PATH").ok().and_then(|path| {
        path.split(':')
            .map(|dir| PathBuf::from(dir).join("plicod"))
            .find(|p| p.exists())
    })
}

fn wait_for_socket(sock_path: &std::path::Path, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    let poll = std::time::Duration::from_millis(100);
    while start.elapsed() < timeout {
        if std::os::unix::net::UnixStream::connect(sock_path).is_ok() {
            eprintln!("plicod ready.");
            return true;
        }
        std::thread::sleep(poll);
    }
    eprintln!("Timeout waiting for plicod socket at {:?}", sock_path);
    false
}

// ── Embedded mode (--embedded) ──────────────────────────────────────

fn run_embedded(args: &[String], root: &std::path::Path) -> bool {
    let filtered = filter_args(args);
    let kernel = AIKernel::new(root.to_path_buf()).expect("Failed to initialize kernel");
    let result = commands::execute_local(&kernel, &filtered);
    commands::print_result(&result)
}

// ── TCP mode (--tcp) ────────────────────────────────────────────────

fn run_tcp(args: &[String], addr: &str) -> bool {
    let filtered = filter_args(args);
    let client = RemoteClient::tcp(addr.to_string());
    execute_via_client(&client, &filtered)
}

// ── Shared remote execution ─────────────────────────────────────────

fn execute_via_client(client: &dyn KernelClient, args: &[String]) -> bool {
    let result = execute_remote(client, args);
    commands::print_result(&result)
}

/// Execute a command via KernelClient. Constructs ApiRequest from args and sends it.
fn execute_remote(client: &dyn KernelClient, args: &[String]) -> ApiResponse {
    let req = match build_remote_request(args) {
        Some(r) => r,
        None => {
            return ApiResponse::error(format!(
                "Command '{}' not recognized. Run: aicli --help",
                args.first().map(|s| s.as_str()).unwrap_or("?")
            ));
        }
    };
    client.request(req)
}

/// Build an ApiRequest from CLI args for remote execution.
/// This covers the full API surface — all commands that handle_api_request supports.
fn build_remote_request(args: &[String]) -> Option<ApiRequest> {
    let agent_id = || commands::extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match args.first().map(|s| s.as_str()) {
        Some("put") | Some("create") => {
            let file_path = commands::extract_arg(args, "--file");
            let dir_path = commands::extract_arg(args, "--dir");
            if file_path.is_some() || dir_path.is_some() {
                let glob_pattern = commands::extract_arg(args, "--glob").unwrap_or_else(|| "*.md".to_string());
                let mut paths = Vec::new();
                if let Some(ref fp) = file_path {
                    let p = std::path::Path::new(fp);
                    paths.push(std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_string());
                }
                if let Some(ref dp) = dir_path {
                    if let Ok(found) = commands::handlers::crud::collect_files(std::path::Path::new(dp), &glob_pattern) {
                        paths.extend(found);
                    }
                }
                Some(ApiRequest::ImportFiles {
                    paths,
                    agent_id: agent_id(),
                    tags: commands::extract_tags(args, "--tags"),
                    chunking: commands::extract_arg(args, "--chunking"),
                    tenant_id: None,
                })
            } else {
                let content = commands::extract_arg(args, "--content")
                    .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
                    .unwrap_or_default();
                let tags = commands::extract_tags(args, "--tags");
                Some(ApiRequest::Create { api_version: None, content, content_encoding: Default::default(), tags, agent_id: agent_id(), tenant_id: None, agent_token: None, intent: commands::extract_arg(args, "--intent") })
            }
        }
        Some("get") | Some("read") => {
            let cid = args.get(1).cloned().unwrap_or_default();
            Some(ApiRequest::Read { cid, agent_id: agent_id(), tenant_id: None, agent_token: None })
        }
        Some("search") => {
            let query = commands::extract_arg(args, "--query")
                .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
                .unwrap_or_default();
            let limit = commands::extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            let require_tags = commands::extract_tags_opt(args, "--require-tags")
                .unwrap_or_else(|| commands::extract_tags_opt(args, "-t").unwrap_or_default());
            let exclude_tags = commands::extract_tags_opt(args, "--exclude-tags").unwrap_or_default();
            let since = commands::extract_arg(args, "--since").and_then(|s| s.parse::<i64>().ok());
            let until = commands::extract_arg(args, "--until").and_then(|s| s.parse::<i64>().ok());
            let offset = commands::extract_arg(args, "--offset").and_then(|s| s.parse().ok());
            Some(ApiRequest::Search { query, agent_id: agent_id(), tenant_id: None, agent_token: None, limit, offset, require_tags, exclude_tags, since, until, intent_context: None })
        }
        Some("update") => {
            let cid = commands::extract_arg(args, "--cid").unwrap_or_default();
            let content = commands::extract_arg(args, "--content").unwrap_or_default();
            let new_tags = commands::extract_tags_opt(args, "--tags");
            Some(ApiRequest::Update { cid, content, content_encoding: Default::default(), new_tags, agent_id: agent_id(), tenant_id: None, agent_token: None })
        }
        Some("delete") => {
            let cid = commands::extract_arg(args, "--cid").unwrap_or_default();
            Some(ApiRequest::Delete { cid, agent_id: agent_id(), tenant_id: None, agent_token: None })
        }
        Some("agent") => {
            if let Some(name) = commands::extract_arg(args, "--register") {
                Some(ApiRequest::RegisterAgent { name })
            } else if commands::extract_arg(args, "--set-resources").is_some() {
                let name = commands::extract_arg(args, "--name").unwrap_or_else(agent_id);
                Some(ApiRequest::AgentSetResources {
                    agent_id: name,
                    memory_quota: None,
                    cpu_time_quota: None,
                    allowed_tools: None,
                    caller_agent_id: agent_id(),
                })
            } else {
                None
            }
        }
        Some("agents") => Some(ApiRequest::ListAgents),
        Some("remember") => {
            let content = commands::extract_arg(args, "--content").unwrap_or_default();
            let tier = commands::extract_arg(args, "--tier").unwrap_or_else(|| "ephemeral".to_string());
            match tier.to_lowercase().as_str() {
                "long-term" | "longterm" | "lt" => {
                    Some(ApiRequest::RememberLongTerm {
                        agent_id: agent_id(), content,
                        tags: commands::extract_tags(args, "--tags"),
                        importance: 50, scope: None, tenant_id: None,
                    })
                }
                _ => {
                    Some(ApiRequest::Remember { agent_id: agent_id(), tenant_id: None, content })
                }
            }
        }
        Some("recall") => {
            let scope = commands::extract_arg(args, "--scope");
            let query = commands::extract_arg(args, "--query");
            let limit = commands::extract_arg(args, "--limit").and_then(|l| l.parse().ok());
            let tier = commands::extract_arg(args, "--tier");
            Some(ApiRequest::Recall { agent_id: agent_id(), scope, query, limit, tier })
        }
        Some("explore") => {
            let cid = commands::extract_arg(args, "--cid").unwrap_or_default();
            let edge_type = commands::extract_arg(args, "--edge-type");
            let depth = commands::extract_arg(args, "--depth").and_then(|s| s.parse().ok());
            Some(ApiRequest::Explore { cid, edge_type, depth, agent_id: agent_id() })
        }
        Some("deleted") => Some(ApiRequest::ListDeleted { agent_id: agent_id() }),
        Some("restore") => {
            let cid = commands::extract_arg(args, "--cid").unwrap_or_default();
            Some(ApiRequest::Restore { cid, agent_id: agent_id() })
        }
        Some("history") => {
            let cid = commands::extract_arg(args, "--cid").unwrap_or_else(|| args.get(1).cloned().unwrap_or_default());
            Some(ApiRequest::History { cid, agent_id: agent_id() })
        }
        Some("rollback") => {
            let cid = commands::extract_arg(args, "--cid").unwrap_or_else(|| args.get(1).cloned().unwrap_or_default());
            Some(ApiRequest::Rollback { cid, agent_id: agent_id() })
        }
        Some("node") => {
            let label = commands::extract_arg(args, "--label").unwrap_or_default();
            let node_type = commands::parse_node_type(&commands::extract_arg(args, "--type").unwrap_or_else(|| "entity".to_string()));
            let props = commands::extract_arg(args, "--props")
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Null);
            Some(ApiRequest::AddNode { label, node_type, properties: props, agent_id: agent_id(), tenant_id: None })
        }
        Some("edge") => {
            let src_id = commands::extract_arg(args, "--src").unwrap_or_default();
            let dst_id = commands::extract_arg(args, "--dst").unwrap_or_default();
            let edge_type = match commands::parse_edge_type(&commands::extract_arg(args, "--type").unwrap_or_else(|| "related_to".to_string())) {
                Ok(t) => t,
                Err(_) => return None,
            };
            let weight = commands::extract_arg(args, "--weight").and_then(|s| s.parse().ok());
            Some(ApiRequest::AddEdge { src_id, dst_id, edge_type, weight, agent_id: agent_id(), tenant_id: None })
        }
        Some("nodes") => {
            let node_type = commands::extract_arg(args, "--type").map(|s| commands::parse_node_type(&s));
            let limit = commands::extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            let offset = commands::extract_arg(args, "--offset").and_then(|s| s.parse().ok());
            if let Some(at_str) = commands::extract_arg(args, "--at-time") {
                let t: u64 = at_str.parse().unwrap_or(0);
                Some(ApiRequest::ListNodesAtTime { node_type, agent_id: agent_id(), tenant_id: None, t })
            } else {
                Some(ApiRequest::ListNodes { node_type, agent_id: agent_id(), tenant_id: None, limit, offset })
            }
        }
        Some("get-node") => {
            let node_id = args.get(1).cloned().unwrap_or_default();
            Some(ApiRequest::GetNode { node_id, agent_id: agent_id(), tenant_id: None })
        }
        Some("edges") => {
            let node_id = commands::extract_arg(args, "--node");
            let limit = commands::extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            let offset = commands::extract_arg(args, "--offset").and_then(|s| s.parse().ok());
            Some(ApiRequest::ListEdges { node_id, agent_id: agent_id(), tenant_id: None, limit, offset })
        }
        Some("rm-node") => {
            let node_id = args.get(1).cloned().unwrap_or_default();
            Some(ApiRequest::RemoveNode { node_id, agent_id: agent_id(), tenant_id: None })
        }
        Some("rm-edge") => {
            let src_id = commands::extract_arg(args, "--src").unwrap_or_default();
            let dst_id = commands::extract_arg(args, "--dst").unwrap_or_default();
            let edge_type = commands::extract_arg(args, "--type")
                .and_then(|s| commands::parse_edge_type(&s).ok());
            Some(ApiRequest::RemoveEdge { src_id, dst_id, edge_type, agent_id: agent_id(), tenant_id: None })
        }
        Some("update-node") => {
            let node_id = commands::extract_arg(args, "--id").unwrap_or_default();
            let label = commands::extract_arg(args, "--label");
            let properties = commands::extract_arg(args, "--props")
                .and_then(|s| serde_json::from_str(&s).ok());
            Some(ApiRequest::UpdateNode { node_id, label, properties, agent_id: agent_id(), tenant_id: None })
        }
        Some("edge-history") => {
            let src_id = commands::extract_arg(args, "--src").unwrap_or_default();
            let dst_id = commands::extract_arg(args, "--dst").unwrap_or_default();
            Some(ApiRequest::EdgeHistory { src_id, dst_id, edge_type: None, agent_id: agent_id(), tenant_id: None })
        }
        Some("paths") => {
            let src_id = commands::extract_arg(args, "--src").unwrap_or_default();
            let dst_id = commands::extract_arg(args, "--dst").unwrap_or_default();
            let max_depth = commands::extract_arg(args, "--depth").and_then(|s| s.parse().ok());
            let weighted = args.iter().any(|a| a == "--weighted");
            Some(ApiRequest::FindPaths { src_id, dst_id, max_depth, weighted, agent_id: agent_id(), tenant_id: None })
        }
        Some("intent") => {
            let description = commands::extract_arg(args, "--description")?;
            let priority = commands::extract_arg(args, "--priority").unwrap_or_else(|| "medium".to_string());
            let action = commands::extract_arg(args, "--action");
            Some(ApiRequest::SubmitIntent { description, priority, action, agent_id: agent_id() })
        }
        Some("status") => Some(ApiRequest::AgentStatus { agent_id: agent_id() }),
        Some("suspend") => Some(ApiRequest::AgentSuspend { agent_id: agent_id() }),
        Some("resume") => Some(ApiRequest::AgentResume { agent_id: agent_id() }),
        Some("terminate") => Some(ApiRequest::AgentTerminate { agent_id: agent_id() }),
        Some("complete") => {
            Some(ApiRequest::AgentComplete { agent_id: agent_id() })
        }
        Some("fail") => {
            let reason = commands::extract_arg(args, "--reason").unwrap_or_default();
            Some(ApiRequest::AgentFail { agent_id: agent_id(), reason })
        }
        Some("checkpoint") => Some(ApiRequest::AgentCheckpoint { agent_id: agent_id() }),
        Some("restore-checkpoint") => {
            let checkpoint_cid = commands::extract_arg(args, "--checkpoint-id").unwrap_or_default();
            Some(ApiRequest::AgentRestore { agent_id: agent_id(), checkpoint_cid })
        }
        Some("memmove") => {
            let entry_id = commands::extract_arg(args, "--id").unwrap_or_default();
            let target_tier = commands::extract_arg(args, "--to").unwrap_or_default();
            Some(ApiRequest::MemoryMove { agent_id: agent_id(), entry_id, target_tier, tenant_id: None })
        }
        Some("memdelete") => {
            let entry_id = commands::extract_arg(args, "--id").unwrap_or_default();
            Some(ApiRequest::MemoryDeleteEntry { agent_id: agent_id(), entry_id, tenant_id: None })
        }
        Some("quota") => Some(ApiRequest::AgentUsage { agent_id: agent_id() }),
        Some("discover") => Some(ApiRequest::DiscoverAgents { agent_id: agent_id(), state_filter: None, tool_filter: None }),
        Some("delegate") => {
            let task_id = commands::extract_arg(args, "--task-id")
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let to_agent = commands::extract_arg(args, "--to").unwrap_or_default();
            let intent = commands::extract_arg(args, "--description").unwrap_or_default();
            Some(ApiRequest::DelegateTask {
                task_id,
                from_agent: agent_id(),
                to_agent,
                intent,
                context_cids: Vec::new(),
                deadline_ms: None,
            })
        }
        Some("tool") => {
            match args.get(1).map(|s| s.as_str()) {
                Some("list") => Some(ApiRequest::ToolList { agent_id: agent_id() }),
                Some("describe") => {
                    let name = args.get(2).cloned().unwrap_or_default();
                    Some(ApiRequest::ToolDescribe { tool: name, agent_id: agent_id() })
                }
                Some("call") => {
                    let name = args.get(2).cloned().unwrap_or_default();
                    let params_str = commands::extract_arg(args, "--params").unwrap_or_else(|| "{}".to_string());
                    let params: serde_json::Value = serde_json::from_str(&params_str).unwrap_or_default();
                    Some(ApiRequest::ToolCall { tool: name, params, agent_id: agent_id() })
                }
                _ => None,
            }
        }
        Some("send") => {
            let from = commands::extract_arg(args, "--from").unwrap_or_else(|| "cli".to_string());
            let to = commands::extract_arg(args, "--to").unwrap_or_default();
            let payload_str = commands::extract_arg(args, "--payload").unwrap_or_else(|| "{}".to_string());
            let payload: serde_json::Value = serde_json::from_str(&payload_str).unwrap_or_default();
            Some(ApiRequest::SendMessage { from, to, payload })
        }
        Some("messages") => {
            let unread_only = args.iter().any(|a| a == "--unread");
            let limit = commands::extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            let offset = commands::extract_arg(args, "--offset").and_then(|s| s.parse().ok());
            Some(ApiRequest::ReadMessages { agent_id: agent_id(), unread_only, limit, offset })
        }
        Some("ack") => {
            let message_id = args.get(1).cloned().unwrap_or_default();
            Some(ApiRequest::AckMessage { agent_id: agent_id(), message_id })
        }
        Some("events") => {
            let tags: Vec<String> = commands::extract_tags(args, "--tags");
            match args.get(1).map(|s| s.as_str()) {
                Some("list") => {
                    let since = commands::extract_arg(args, "--since").and_then(|s| s.parse().ok());
                    let until = commands::extract_arg(args, "--until").and_then(|s| s.parse().ok());
                    Some(ApiRequest::ListEvents { since, until, tags, event_type: None, agent_id: agent_id(), limit: None, offset: None })
                }
                Some("by-time") | Some("text") => {
                    let time_expression = args.get(2..)
                        .map(|v| v.iter().take_while(|s| !s.starts_with("--")).cloned().collect::<Vec<_>>().join(" "))
                        .unwrap_or_default();
                    Some(ApiRequest::ListEventsText { time_expression, tags, event_type: None, agent_id: agent_id() })
                }
                Some("subscribe") => {
                    Some(ApiRequest::EventSubscribe {
                        agent_id: agent_id(),
                        event_types: if tags.is_empty() { None } else { Some(tags) },
                        agent_ids: None,
                    })
                }
                Some("poll") => {
                    let sub_id = commands::extract_arg(args, "--sub-id").unwrap_or_default();
                    Some(ApiRequest::EventPoll { subscription_id: sub_id })
                }
                Some("unsubscribe") => {
                    let sub_id = commands::extract_arg(args, "--sub-id").unwrap_or_default();
                    Some(ApiRequest::EventUnsubscribe { subscription_id: sub_id })
                }
                Some("history") => {
                    let since_seq = commands::extract_arg(args, "--since").and_then(|s| s.parse().ok());
                    let limit = commands::extract_arg(args, "--limit").and_then(|s| s.parse().ok());
                    Some(ApiRequest::EventHistory { since_seq, limit, agent_id_filter: Some(agent_id()) })
                }
                _ => None,
            }
        }
        Some("context") => {
            match args.get(1).map(|s| s.as_str()) {
                Some("load") => {
                    let cid = commands::extract_arg(args, "--cid").unwrap_or_default();
                    let layer = commands::extract_arg(args, "--layer").unwrap_or_else(|| "L0".to_string());
                    Some(ApiRequest::LoadContext { cid, layer, agent_id: agent_id(), tenant_id: None })
                }
                Some("assemble") => {
                    let budget_tokens = commands::extract_arg(args, "--budget")
                        .and_then(|s| s.parse().ok()).unwrap_or(4096);
                    let cids = Vec::new();
                    Some(ApiRequest::ContextAssemble { agent_id: agent_id(), cids, budget_tokens })
                }
                _ => None,
            }
        }
        Some("hook") => {
            match args.get(1).map(|s| s.as_str()) {
                Some("list") => Some(ApiRequest::HookList),
                Some("register") => Some(ApiRequest::HookRegister {
                    point: commands::extract_arg(args, "--point").unwrap_or_else(|| "PreToolCall".to_string()),
                    action: commands::extract_arg(args, "--action").unwrap_or_else(|| "block".to_string()),
                    tool_pattern: commands::extract_arg(args, "--tool"),
                    reason: commands::extract_arg(args, "--reason"),
                    priority: commands::extract_arg(args, "--priority").and_then(|s| s.parse().ok()),
                }),
                _ => None,
            }
        }
        Some("permission") | Some("perm") => {
            match args.get(1).map(|s| s.as_str()) {
                Some("grant") => {
                    let action_str = commands::extract_arg(args, "--action").unwrap_or_default();
                    let scope = commands::extract_arg(args, "--scope");
                    Some(ApiRequest::GrantPermission { agent_id: agent_id(), action: action_str, scope, expires_at: None })
                }
                Some("revoke") => {
                    let action_str = commands::extract_arg(args, "--action").unwrap_or_default();
                    Some(ApiRequest::RevokePermission { agent_id: agent_id(), action: action_str })
                }
                Some("list") => Some(ApiRequest::ListPermissions { agent_id: agent_id() }),
                _ => None,
            }
        }
        Some("skills") => {
            match args.get(1).map(|s| s.as_str()) {
                Some("register") => {
                    let name = commands::extract_arg(args, "--name").unwrap_or_default();
                    let description = commands::extract_arg(args, "--description").unwrap_or_default();
                    let tags = commands::extract_tags(args, "--tags");
                    Some(ApiRequest::RegisterSkill { name, description, tags, agent_id: agent_id() })
                }
                _ => Some(ApiRequest::DiscoverSkills {
                    query: commands::extract_arg(args, "--query"),
                    agent_id_filter: Some(agent_id()),
                    tag_filter: None,
                }),
            }
        }
        Some("session-start") => {
            let intent_hint = commands::extract_arg(args, "--intent");
            let last_seen_seq = commands::extract_arg(args, "--last-seq").and_then(|s| s.parse().ok());
            Some(ApiRequest::StartSession { agent_id: agent_id(), agent_token: None, intent_hint, load_tiers: vec![], last_seen_seq })
        }
        Some("session-end") => {
            let session_id = commands::extract_arg(args, "--session").unwrap_or_default();
            let auto_checkpoint = !args.iter().any(|a| a == "--no-checkpoint");
            Some(ApiRequest::EndSession { agent_id: agent_id(), session_id, auto_checkpoint })
        }
        Some("delta") => {
            let since_seq = commands::extract_arg(args, "--since").and_then(|s| s.parse().ok()).unwrap_or(0);
            let watch_cids: Vec<String> = commands::extract_arg(args, "--watch-cids")
                .map(|s| s.split(',').map(String::from).collect())
                .unwrap_or_default();
            let watch_tags: Vec<String> = commands::extract_arg(args, "--watch-tags")
                .map(|s| s.split(',').map(String::from).collect())
                .unwrap_or_default();
            let limit = commands::extract_arg(args, "--limit").and_then(|s| s.parse().ok());
            Some(ApiRequest::DeltaSince { agent_id: agent_id(), since_seq, watch_cids, watch_tags, limit })
        }
        Some("growth") => {
            let period_str = commands::extract_arg(args, "--period").unwrap_or_else(|| "last7days".to_string());
            let period = match period_str.to_lowercase().as_str() {
                "last7days" | "7d" => plico::api::semantic::GrowthPeriod::Last7Days,
                "last30days" | "30d" => plico::api::semantic::GrowthPeriod::Last30Days,
                "alltime" | "all" => plico::api::semantic::GrowthPeriod::AllTime,
                _ => return None,
            };
            Some(ApiRequest::QueryGrowthReport { agent_id: agent_id(), period })
        }
        Some("hybrid") => {
            let query_text = commands::extract_arg(args, "--query")
                .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
                .unwrap_or_default();
            let seed_tags: Vec<String> = commands::extract_arg(args, "--seed-tags")
                .map(|s| s.split(',').map(String::from).collect())
                .unwrap_or_default();
            let graph_depth = commands::extract_arg(args, "--depth")
                .and_then(|s| s.parse().ok())
                .unwrap_or(2);
            let edge_types: Vec<String> = commands::extract_arg(args, "--edge-types")
                .map(|s| s.split(',').map(String::from).collect())
                .unwrap_or_default();
            let max_results = commands::extract_arg(args, "--limit")
                .and_then(|s| s.parse().ok())
                .unwrap_or(20);
            let token_budget = commands::extract_arg(args, "--budget").and_then(|s| s.parse().ok());
            Some(ApiRequest::HybridRetrieve { query_text, seed_tags, graph_depth, edge_types, max_results, token_budget, agent_id: agent_id(), tenant_id: None })
        }
        Some("cost") => {
            match args.get(1).map(|s| s.as_str()) {
                Some("session") => {
                    let session_id = commands::extract_arg(args, "--session").unwrap_or_default();
                    Some(ApiRequest::CostSessionSummary { session_id })
                }
                Some("agent") => {
                    let agent_id = commands::extract_arg(args, "--agent").unwrap_or_default();
                    let last = commands::extract_arg(args, "--last")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(10);
                    Some(ApiRequest::CostAgentTrend { agent_id, last_n_sessions: last })
                }
                Some("anomaly") => {
                    let agent_id = commands::extract_arg(args, "--agent").unwrap_or_default();
                    Some(ApiRequest::CostAnomalyCheck { agent_id })
                }
                _ => None,
            }
        }
        Some("system-status") => Some(ApiRequest::SystemStatus),
        Some("health") => Some(ApiRequest::HealthReport),
        _ => None,
    }
}

fn print_help() {
    println!(r#"
Plico AI-Native OS — AI-Friendly CLI (Daemon-First)

USAGE:
  aicli [MODE] <command> [flags]

MODE (default: daemon via UDS):
  --embedded         Direct kernel access (no daemon, for testing/debugging)
  --tcp [addr]       Connect to remote daemon via TCP (default: 127.0.0.1:7878)
  --root PATH        Storage root directory (default: ~/.plico)

COMMANDS:
  put/create   Store content with semantic tags
  get/read     Retrieve object by CID
  search       Semantic search with optional tag/time filtering
  update       Update object content
  delete       Logical delete (soft, requires Delete permission)
  agent        Register a new agent / set-resources
  agents       List active agents
  remember     Store memory (--tier ephemeral|working|long-term|procedural)
  recall       Retrieve agent memories
  tags         List all tags (embedded mode only)
  explore      Graph neighbors of a CID
  deleted      List logically deleted objects (recycle bin)
  restore      Restore a deleted object
  node/edge    Create KG nodes and edges
  nodes/edges  List KG nodes and edges
  get-node     Get a specific KG node
  rm-node      Remove a KG node
  rm-edge      Remove a KG edge
  update-node  Update a KG node
  edge-history Edge version history
  paths        Find paths between two KG nodes
  intent       Submit an intent for agent execution
  status       Query agent state
  suspend      Suspend a running agent
  resume       Resume a suspended agent
  terminate    Permanently terminate an agent
  complete     Mark agent task as completed
  fail         Mark agent task as failed
  checkpoint   Create agent checkpoint
  restore-checkpoint  Restore agent from checkpoint
  tool         List/describe/call tools
  send         Send inter-agent message
  messages     Read agent messages
  ack          Acknowledge a message
  memmove      Move a memory entry between tiers
  memdelete    Delete a memory entry
  quota        Query agent resource usage
  discover     Discover agents
  delegate     Delegate task to another agent
  events       List/subscribe/poll/unsubscribe/history events
  context      Load context (load/assemble)
  history      Version history for a CID
  rollback     Rollback to previous version
  skills       Register/discover skills
  permission   Grant/revoke/list permissions
  hook         List/register lifecycle hooks
  session-start / session-end  Session lifecycle
  delta        Query changes since sequence number
  growth       Query agent growth statistics
  hybrid       Graph-RAG hybrid retrieval
  system-status  System status
  health       System health report

NOTES:
  • Default mode connects to plicod daemon (auto-started if needed)
  • Use --embedded for testing without daemon
  • tags command only available in --embedded mode
"#);
}
