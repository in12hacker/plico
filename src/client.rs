//! KernelClient trait — transport abstraction for Daemon-First architecture.
//!
//! All consumers (CLI, MCP, SSE adapters) interact with the kernel through this trait.
//! Two implementations:
//! - `EmbeddedClient`: wraps `AIKernel` directly (for tests and `--embedded` mode)
//! - `RemoteClient`: communicates over Unix Domain Socket or TCP with `plicod`
//!
//! # Framing Protocol
//!
//! Messages use 4-byte big-endian length prefix + JSON payload:
//!   `[u32 length][JSON body]`

use crate::api::semantic::{ApiRequest, ApiResponse};

/// Unified interface for interacting with the Plico kernel.
pub trait KernelClient: Send + Sync + std::any::Any {
    fn request(&self, req: ApiRequest) -> ApiResponse;
}

/// Embedded client — wraps `AIKernel` directly. Used for tests and `--embedded` mode.
pub struct EmbeddedClient {
    pub kernel: crate::kernel::AIKernel,
}

impl KernelClient for EmbeddedClient {
    fn request(&self, req: ApiRequest) -> ApiResponse {
        self.kernel.handle_api_request(req)
    }
}

/// Remote client — connects to `plicod` via UDS or TCP.
pub struct RemoteClient {
    addr: RemoteAddr,
}

/// Remote address variants for plicod connection.
pub enum RemoteAddr {
    Uds(std::path::PathBuf),
    Tcp(String),
}

impl RemoteClient {
    pub fn uds(path: std::path::PathBuf) -> Self {
        Self { addr: RemoteAddr::Uds(path) }
    }

    pub fn tcp(addr: String) -> Self {
        Self { addr: RemoteAddr::Tcp(addr) }
    }

    /// Return the address description (for error messages).
    pub fn addr_display(&self) -> String {
        match &self.addr {
            RemoteAddr::Tcp(addr) => format!("tcp://{}", addr),
            RemoteAddr::Uds(path) => format!("unix://{}", path.display()),
        }
    }

    /// Check if the daemon is reachable (quick health probe).
    pub fn is_reachable(&self) -> bool {
        self.send_request(&ApiRequest::HealthReport).is_ok()
    }

    fn send_request(&self, req: &ApiRequest) -> std::io::Result<ApiResponse> {
        let payload = serde_json::to_vec(req)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let raw = match &self.addr {
            RemoteAddr::Tcp(addr) => {
                let mut stream = std::net::TcpStream::connect(addr)?;
                stream.set_nodelay(true)?;
                stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
                write_frame(&mut stream, &payload)?;
                read_frame(&mut stream)?
            }
            #[cfg(unix)]
            RemoteAddr::Uds(path) => {
                let mut stream = std::os::unix::net::UnixStream::connect(path)?;
                stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
                write_frame(&mut stream, &payload)?;
                read_frame(&mut stream)?
            }
            #[cfg(not(unix))]
            RemoteAddr::Uds(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "Unix domain sockets are not available on this platform; use TCP mode",
                ));
            }
        };
        serde_json::from_slice(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

impl KernelClient for RemoteClient {
    fn request(&self, req: ApiRequest) -> ApiResponse {
        match self.send_request(&req) {
            Ok(resp) => resp,
            Err(e) => ApiResponse::error(format!("daemon connection failed ({}): {}", self.addr_display(), e)),
        }
    }
}

// ── Length-Prefixed Framing (synchronous, for client side) ───────────

fn write_frame<W: std::io::Write>(w: &mut W, payload: &[u8]) -> std::io::Result<()> {
    let len = payload.len() as u32;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

fn read_frame<R: std::io::Read>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    r.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header);
    if len == 0 || len > 16 * 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid response frame length: {} bytes", len),
        ));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}
