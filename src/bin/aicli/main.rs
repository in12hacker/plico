//! `aicli` — typed command-line adapter for `plico.personal.v2`.

use std::path::PathBuf;
use std::sync::Arc;

use plico::api::public::PublicRequest;
use plico::client::{EmbeddedClient, KernelClient, RemoteClient};
use plico::kernel::{AIKernel, PublicTransport};
use tracing_subscriber::util::SubscriberInitExt;

mod input;

use input::parse_command;

enum Mode {
    Uds,
    Embedded,
    Tcp(String),
}

struct Invocation {
    mode: Mode,
    root: PathBuf,
    operation_args: Vec<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()))
        .with_writer(std::io::stderr)
        .finish()
        .try_init()
        .ok();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.is_empty() || matches!(raw.as_slice(), [arg] if matches!(arg.as_str(), "--help" | "-h")) {
        print_help();
        return;
    }

    let result = run(raw);
    match result {
        Ok(ok) => std::process::exit(if ok { 0 } else { 1 }),
        Err(error) => {
            eprintln!("aicli: {error}");
            std::process::exit(2);
        }
    }
}

fn run(raw: Vec<String>) -> Result<bool, String> {
    let invocation = parse_invocation(raw)?;
    let command = parse_command(invocation.operation_args)?;
    let request = PublicRequest::new(uuid::Uuid::new_v4(), None, command);
    let client: Box<dyn KernelClient> = match invocation.mode {
        Mode::Embedded => {
            let kernel = AIKernel::new(invocation.root).map_err(|_| "kernel initialization failed".to_string())?;
            kernel.start_workers();
            Box::new(
                EmbeddedClient::new(Arc::clone(&kernel), PublicTransport::Embedded)
                    .map_err(|error| error.to_string())?,
            )
        }
        Mode::Uds => Box::new(RemoteClient::uds(invocation.root.join("plico.sock"))),
        Mode::Tcp(addr) => {
            let bearer =
                std::env::var("PLICO_BEARER_TOKEN").map_err(|_| "TCP mode requires PLICO_BEARER_TOKEN".to_string())?;
            Box::new(RemoteClient::tcp(addr, bearer).map_err(|error| error.to_string())?)
        }
    };

    let response = client.request(request).map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response).map_err(|_| "failed to encode response".to_string())?
    );
    Ok(response.ok)
}

fn parse_invocation(raw: Vec<String>) -> Result<Invocation, String> {
    let mut root = std::env::var("PLICO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_else(std::env::temp_dir).join(".plico"));
    let mut mode = Mode::Uds;
    let mut selected_mode = false;
    let mut operation_args = Vec::new();
    let mut index = 0;

    while index < raw.len() {
        match raw[index].as_str() {
            "--root" => {
                let value = raw.get(index + 1).ok_or("--root requires a path")?;
                root = PathBuf::from(value);
                index += 2;
            }
            "--embedded" => {
                if selected_mode {
                    return Err("choose exactly one of --embedded or --tcp".to_string());
                }
                mode = Mode::Embedded;
                selected_mode = true;
                index += 1;
            }
            "--tcp" => {
                if selected_mode {
                    return Err("choose exactly one of --embedded or --tcp".to_string());
                }
                let addr = raw.get(index + 1).ok_or("--tcp requires host:port")?.clone();
                mode = Mode::Tcp(addr);
                selected_mode = true;
                index += 2;
            }
            _ => {
                operation_args.push(raw[index].clone());
                index += 1;
            }
        }
    }
    if operation_args.is_empty() {
        return Err("missing operation; run aicli --help".to_string());
    }
    Ok(Invocation {
        mode,
        root,
        operation_args,
    })
}

fn print_help() {
    println!(
        "aicli [--root PATH] [--embedded | --tcp HOST:PORT] OPERATION [OPTIONS]\n\
         TCP mode reads PLICO_BEARER_TOKEN. Public operations:\n\
         capabilities.describe\n\
         runtime.readiness\n\
         object.put --content TEXT [--encoding utf8|base64] [--tag TAG]...\n\
         object.get --cid CID\n\
         object.search --query TEXT [--limit N] [--require-tag TAG]... [--exclude-tag TAG]...\n\
         memory.create --content TEXT [--tag TAG]...\n\
         memory.get --entry-id UUID\n\
         memory.recall --query TEXT [--limit N]\n\
         projection.status --revision-id UUID\n\
         projection.rebuild (--revision-id UUID | --all-eligible)\n\
         memory.update --entry-id UUID --content TEXT\n\
         memory.delete --entry-id UUID\n\
         session.start [--last-seen-seq N]\n\
         session.end --session-id UUID"
    );
}
