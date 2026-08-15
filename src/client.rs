//! Typed public-protocol transport for daemon-first consumers.
//!
//! Both implementations accept only [`PublicRequest`] and return only
//! [`PublicResponse`]. Messages use a four-byte big-endian length prefix
//! followed by one JSON document.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use crate::api::public::{EmptyInput, PublicAuth, PublicCommand, PublicRequest, PublicResponse, MAX_AUTH_BYTES};
use crate::kernel::{AIKernel, PublicRequestContext, PublicTransport};

const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Transport and protocol failures are distinct from typed domain failures in
/// [`PublicResponse`].
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("failed to connect to {endpoint}: {source}")]
    Connect {
        endpoint: String,
        #[source]
        source: std::io::Error,
    },
    #[error("daemon {phase} timed out: {source}")]
    Timeout {
        phase: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid daemon frame during {phase}: {source}")]
    Frame {
        phase: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to encode public request: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("failed to decode public response: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("public protocol violation: {0}")]
    Protocol(String),
}

/// Unified typed interface for local and remote public requests.
pub trait KernelClient: Send + Sync {
    fn request(&self, request: PublicRequest) -> Result<PublicResponse, ClientError>;
}

/// In-process client with a host-selected, trusted local transport context.
pub struct EmbeddedClient {
    kernel: Arc<AIKernel>,
    context: PublicRequestContext,
}

impl EmbeddedClient {
    pub fn new(kernel: Arc<AIKernel>, transport: PublicTransport) -> Result<Self, ClientError> {
        if !matches!(transport, PublicTransport::Embedded | PublicTransport::Mcp) {
            return Err(ClientError::Protocol(
                "embedded clients require the embedded or mcp transport context".to_string(),
            ));
        }
        Ok(Self {
            kernel,
            context: PublicRequestContext::local_owner(transport),
        })
    }
}

impl KernelClient for EmbeddedClient {
    fn request(&self, request: PublicRequest) -> Result<PublicResponse, ClientError> {
        reject_payload_auth(&request)?;
        request
            .validate()
            .map_err(|error| ClientError::Protocol(error.message))?;
        let response = self.kernel.handle_public_request(&self.context, request.clone());
        response
            .validate_for(&request)
            .map_err(|error| ClientError::Protocol(error.message))?;
        Ok(response)
    }
}

/// Remote client. The transport determines whether a bearer is injected.
pub struct RemoteClient {
    addr: RemoteAddr,
}

enum RemoteAddr {
    Uds(PathBuf),
    Tcp { addr: String, bearer: String },
}

impl RemoteClient {
    pub fn uds(path: PathBuf) -> Self {
        Self {
            addr: RemoteAddr::Uds(path),
        }
    }

    pub fn tcp(addr: String, bearer: String) -> Result<Self, ClientError> {
        validate_bearer(&bearer)?;
        Ok(Self {
            addr: RemoteAddr::Tcp { addr, bearer },
        })
    }

    /// Address description suitable for diagnostics. Credentials are never
    /// included.
    pub fn addr_display(&self) -> String {
        match &self.addr {
            RemoteAddr::Tcp { addr, .. } => format!("tcp://{addr}"),
            RemoteAddr::Uds(path) => format!("unix://{}", path.display()),
        }
    }

    pub fn is_reachable(&self) -> bool {
        let request = PublicRequest::new(
            uuid::Uuid::new_v4(),
            None,
            PublicCommand::RuntimeReadiness(EmptyInput::default()),
        );
        self.request(request).is_ok_and(|response| response.ok)
    }

    fn send_request(&self, mut request: PublicRequest) -> Result<PublicResponse, ClientError> {
        let payload = self.prepare_request(&mut request)?;

        let raw = match &self.addr {
            RemoteAddr::Tcp { addr, .. } => {
                let mut stream = std::net::TcpStream::connect(addr)
                    .map_err(|source| map_connect_error(self.addr_display(), source))?;
                stream
                    .set_nodelay(true)
                    .map_err(|source| map_frame_error("configure", source))?;
                configure_tcp_timeouts(&stream)?;
                write_frame(&mut stream, &payload)?;
                read_frame(&mut stream)?
            }
            #[cfg(unix)]
            RemoteAddr::Uds(path) => {
                let mut stream = std::os::unix::net::UnixStream::connect(path)
                    .map_err(|source| map_connect_error(self.addr_display(), source))?;
                stream
                    .set_read_timeout(Some(IO_TIMEOUT))
                    .map_err(|source| map_frame_error("configure", source))?;
                stream
                    .set_write_timeout(Some(IO_TIMEOUT))
                    .map_err(|source| map_frame_error("configure", source))?;
                write_frame(&mut stream, &payload)?;
                read_frame(&mut stream)?
            }
            #[cfg(not(unix))]
            RemoteAddr::Uds(_) => {
                return Err(ClientError::Connect {
                    endpoint: self.addr_display(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "Unix domain sockets are unavailable on this platform",
                    ),
                });
            }
        };

        decode_response(&request, &raw)
    }

    fn prepare_request(&self, request: &mut PublicRequest) -> Result<Vec<u8>, ClientError> {
        reject_payload_auth(request)?;
        request
            .validate()
            .map_err(|error| ClientError::Protocol(error.message))?;

        if let RemoteAddr::Tcp { bearer, .. } = &self.addr {
            request.auth = Some(PublicAuth { bearer: bearer.clone() });
        }

        let payload = serde_json::to_vec(&request).map_err(ClientError::Encode)?;
        if payload.is_empty() || payload.len() > MAX_FRAME_SIZE {
            return Err(ClientError::Protocol(format!(
                "encoded request exceeds the {MAX_FRAME_SIZE}-byte frame limit"
            )));
        }
        Ok(payload)
    }
}

impl KernelClient for RemoteClient {
    fn request(&self, request: PublicRequest) -> Result<PublicResponse, ClientError> {
        self.send_request(request)
    }
}

fn reject_payload_auth(request: &PublicRequest) -> Result<(), ClientError> {
    if request.auth.is_some() {
        return Err(ClientError::Protocol(
            "request auth is transport-owned and must not be supplied by callers".to_string(),
        ));
    }
    Ok(())
}

fn validate_bearer(bearer: &str) -> Result<(), ClientError> {
    if bearer.is_empty() || bearer.len() > MAX_AUTH_BYTES {
        return Err(ClientError::Protocol(format!(
            "TCP bearer must contain 1..={MAX_AUTH_BYTES} bytes"
        )));
    }
    Ok(())
}

fn configure_tcp_timeouts(stream: &std::net::TcpStream) -> Result<(), ClientError> {
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|source| map_frame_error("configure", source))?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|source| map_frame_error("configure", source))
}

fn map_connect_error(endpoint: String, source: std::io::Error) -> ClientError {
    if is_timeout(&source) {
        ClientError::Timeout {
            phase: "connect",
            source,
        }
    } else {
        ClientError::Connect { endpoint, source }
    }
}

fn map_frame_error(phase: &'static str, source: std::io::Error) -> ClientError {
    if is_timeout(&source) {
        ClientError::Timeout { phase, source }
    } else {
        ClientError::Frame { phase, source }
    }
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), ClientError> {
    let length = u32::try_from(payload.len())
        .map_err(|_| ClientError::Protocol("encoded request length does not fit the wire frame".to_string()))?;
    writer
        .write_all(&length.to_be_bytes())
        .map_err(|source| map_frame_error("write", source))?;
    writer
        .write_all(payload)
        .map_err(|source| map_frame_error("write", source))?;
    writer.flush().map_err(|source| map_frame_error("write", source))
}

fn read_frame<R: Read>(reader: &mut R) -> Result<Vec<u8>, ClientError> {
    let mut header = [0u8; 4];
    reader
        .read_exact(&mut header)
        .map_err(|source| map_frame_error("read_header", source))?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_SIZE {
        return Err(ClientError::Frame {
            phase: "read_header",
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("response frame length {length} is outside 1..={MAX_FRAME_SIZE}"),
            ),
        });
    }
    let mut payload = vec![0u8; length];
    reader
        .read_exact(&mut payload)
        .map_err(|source| map_frame_error("read_body", source))?;
    Ok(payload)
}

fn decode_response(request: &PublicRequest, raw: &[u8]) -> Result<PublicResponse, ClientError> {
    let response: PublicResponse = serde_json::from_slice(raw).map_err(ClientError::Decode)?;
    response
        .validate_for(request)
        .map_err(|error| ClientError::Protocol(error.message))?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::public::{CapabilityCatalog, PublicData};

    fn request() -> PublicRequest {
        PublicRequest::new(
            uuid::Uuid::new_v4(),
            None,
            PublicCommand::CapabilitiesDescribe(EmptyInput::default()),
        )
    }

    #[test]
    fn response_request_id_mismatch_is_a_protocol_error() {
        let request = request();
        let response = PublicResponse::success(
            uuid::Uuid::new_v4(),
            PublicData::CapabilitiesDescribe(CapabilityCatalog::default()),
        );
        let raw = serde_json::to_vec(&response).unwrap();

        assert!(matches!(
            decode_response(&request, &raw),
            Err(ClientError::Protocol(message)) if message.contains("request_id")
        ));
    }

    #[test]
    fn response_operation_mismatch_is_a_protocol_error() {
        let request = PublicRequest::new(
            uuid::Uuid::new_v4(),
            None,
            PublicCommand::RuntimeReadiness(EmptyInput::default()),
        );
        let response = PublicResponse::success(
            request.request_id,
            PublicData::CapabilitiesDescribe(CapabilityCatalog::default()),
        );
        let raw = serde_json::to_vec(&response).unwrap();

        assert!(matches!(
            decode_response(&request, &raw),
            Err(ClientError::Protocol(message)) if message.contains("operation")
        ));
    }

    #[test]
    fn caller_cannot_supply_transport_auth() {
        let mut request = request();
        request.auth = Some(PublicAuth {
            bearer: "caller-controlled".to_string(),
        });
        let client = RemoteClient::uds(PathBuf::from("/tmp/not-used.sock"));

        assert!(matches!(client.request(request), Err(ClientError::Protocol(_))));
    }

    #[test]
    fn tcp_constructor_rejects_missing_bearer() {
        assert!(matches!(
            RemoteClient::tcp("127.0.0.1:1".to_string(), String::new()),
            Err(ClientError::Protocol(_))
        ));
    }

    #[test]
    fn connect_failure_remains_a_transport_error() {
        let error = map_connect_error(
            "tcp://127.0.0.1:1".to_string(),
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused"),
        );

        assert!(matches!(error, ClientError::Connect { .. }));
    }

    #[test]
    fn tcp_injects_bearer_without_changing_command_input() {
        let mut request = request();
        let client = RemoteClient::tcp("127.0.0.1:1".to_string(), "test-bearer".to_string()).unwrap();

        let raw = client.prepare_request(&mut request).unwrap();
        let received: PublicRequest = serde_json::from_slice(&raw).unwrap();
        assert_eq!(received.command.operation(), "capabilities.describe");
        assert_eq!(received.auth.unwrap().bearer, "test-bearer");
    }

    #[test]
    fn partial_response_body_is_a_frame_error() {
        let payload = b"{}";
        let mut frame = Vec::new();
        frame.extend_from_slice(&(payload.len() as u32 + 2).to_be_bytes());
        frame.extend_from_slice(payload);

        assert!(matches!(
            read_frame(&mut frame.as_slice()),
            Err(ClientError::Frame { phase: "read_body", .. })
        ));
    }
}
