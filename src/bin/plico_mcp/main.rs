//! `plico-mcp` — a thin MCP stdio adapter for `plico.personal.v2`.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use plico::client::{EmbeddedClient, KernelClient, RemoteClient};
use plico::kernel::{AIKernel, PublicTransport};
use tracing_subscriber::EnvFilter;

mod rpc;
mod tools;

const SERVER_NAME: &str = "plico-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const TCP_BEARER_ENV: &str = "PLICO_BEARER_TOKEN";

#[derive(Debug, Clone, PartialEq, Eq)]
enum LaunchMode {
    Embedded,
    Uds,
    Tcp(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupError {
    InvalidArguments,
    MissingTcpBearer,
    ClientInitialization,
    KernelInitialization,
}

impl StartupError {
    const fn category(self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::MissingTcpBearer => "missing_tcp_bearer",
            Self::ClientInitialization => "client_initialization",
            Self::KernelInitialization => "kernel_initialization",
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .init();

    let root = std::env::var("PLICO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_else(std::env::temp_dir).join(".plico"));
    let mode = match parse_launch_mode(std::env::args().skip(1)) {
        Ok(mode) => mode,
        Err(error) => {
            tracing::error!(error_category = error.category(), "MCP server startup failed");
            return;
        }
    };
    let client = match build_client(mode, root) {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(error_category = error.category(), "MCP server startup failed");
            return;
        }
    };

    serve_stdio(client.as_ref());
}

fn parse_launch_mode(args: impl IntoIterator<Item = String>) -> Result<LaunchMode, StartupError> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(LaunchMode::Embedded);
    };
    let mode = match first.as_str() {
        "--daemon" => LaunchMode::Uds,
        "--tcp" => LaunchMode::Tcp(args.next().ok_or(StartupError::InvalidArguments)?),
        _ => return Err(StartupError::InvalidArguments),
    };
    if args.next().is_some() {
        return Err(StartupError::InvalidArguments);
    }
    Ok(mode)
}

fn build_client(mode: LaunchMode, root: PathBuf) -> Result<Arc<dyn KernelClient>, StartupError> {
    match mode {
        LaunchMode::Embedded => {
            let kernel = AIKernel::new(root).map_err(|_| StartupError::KernelInitialization)?;
            kernel.start_workers();
            EmbeddedClient::new(kernel, PublicTransport::Mcp)
                .map(|client| Arc::new(client) as Arc<dyn KernelClient>)
                .map_err(|_| StartupError::ClientInitialization)
        }
        LaunchMode::Uds => Ok(Arc::new(RemoteClient::uds(root.join("plico.sock")))),
        LaunchMode::Tcp(address) => {
            let bearer = std::env::var(TCP_BEARER_ENV).map_err(|_| StartupError::MissingTcpBearer)?;
            RemoteClient::tcp(address, bearer)
                .map(|client| Arc::new(client) as Arc<dyn KernelClient>)
                .map_err(|_| StartupError::ClientInitialization)
        }
    }
}

fn serve_stdio(client: &dyn KernelClient) {
    let stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();

    for line in stdin.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => {
                tracing::warn!(error_category = "stdio_read", "MCP input stream failed");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let Some(response) = rpc::process_line(&line, client) else {
            continue;
        };
        if serde_json::to_writer(&mut stdout, &response).is_err()
            || stdout.write_all(b"\n").is_err()
            || stdout.flush().is_err()
        {
            tracing::warn!(error_category = "stdio_write", "MCP output stream failed");
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_modes_are_single_path() {
        assert_eq!(parse_launch_mode(Vec::new()), Ok(LaunchMode::Embedded));
        assert_eq!(parse_launch_mode(vec!["--daemon".to_string()]), Ok(LaunchMode::Uds));
        assert_eq!(
            parse_launch_mode(vec!["--tcp".to_string(), "127.0.0.1:7878".to_string()]),
            Ok(LaunchMode::Tcp("127.0.0.1:7878".to_string()))
        );
        assert_eq!(
            parse_launch_mode(vec!["--daemon".to_string(), "--tcp".to_string()]),
            Err(StartupError::InvalidArguments)
        );
    }
}
