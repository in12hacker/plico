//! Unified Configuration — single entry point for all Plico settings.
//!
//! Three-layer cascade (lowest → highest priority):
//!   1. Built-in defaults (always present, zero-config works)
//!   2. Config file (`$PLICO_ROOT/config.json`)
//!   3. Environment variables (`PLICO_*`, `LLAMA_*`, `EMBEDDING_*`, etc.)
//!
//! CLI flags are applied *after* `PlicoConfig::load()` by each binary.
//!
//! # Example config.json
//!
//! ```json
//! {
//!   "network": { "host": "127.0.0.1", "daemon_port": 7878 },
//!   "inference": { "embedding_backend": "openai", "llm_backend": "llama" },
//!   "tuning": { "persist_interval_secs": 300 }
//! }
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── Root-level Config ──────────────────────────────────────────────────

/// Unified configuration for all Plico binaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlicoConfig {
    /// Storage root for all data (CAS, indexes, PID files, sockets).
    #[serde(default = "default_root")]
    pub root: PathBuf,

    /// Network settings for daemon and adapters.
    #[serde(default)]
    pub network: NetworkConfig,

    /// Inference backend settings (embedding + LLM).
    #[serde(default)]
    pub inference: InferenceConfig,

    /// Runtime tuning knobs.
    #[serde(default)]
    pub tuning: TuningConfig,
}

// ── Network ────────────────────────────────────────────────────────────

/// Network-related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Bind address for TCP listeners (default: `127.0.0.1`).
    #[serde(default = "default_host")]
    pub host: String,

    /// `plicod` TCP port (default: 7878).
    #[serde(default = "default_daemon_port")]
    pub daemon_port: u16,

    /// SSE adapter port (default: 7879).
    #[serde(default = "default_sse_port")]
    pub sse_port: u16,

    /// Whether to disable UDS (Unix Domain Socket) in `plicod`.
    #[serde(default)]
    pub disable_uds: bool,
}

// ── Inference ──────────────────────────────────────────────────────────

/// Embedding and LLM backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceConfig {
    /// Embedding backend: `"openai"` | `"ollama"` | `"local"` | `"ort"` | `"stub"`.
    #[serde(default = "default_embedding_backend")]
    pub embedding_backend: String,

    /// OpenAI-compatible embedding endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_api_base: Option<String>,

    /// Model name for the embedding backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,

    /// HuggingFace model ID for `"local"` backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model_id: Option<String>,

    /// Python interpreter for `"local"` backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_python: Option<String>,

    /// LLM backend: `"llama"` | `"ollama"` | `"openai"` | `"stub"`.
    #[serde(default = "default_llm_backend")]
    pub llm_backend: String,

    /// llama.cpp / OpenAI-compatible server URL (auto-detected if unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llama_url: Option<String>,

    /// Model name for the llama/OpenAI backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llama_model: Option<String>,

    /// Ollama daemon URL (default: `http://localhost:11434`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ollama_url: Option<String>,

    /// OpenAI-compatible base URL (for generic providers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_api_base: Option<String>,

    /// API key for authenticated endpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

// ── Tuning ─────────────────────────────────────────────────────────────

/// Runtime tuning knobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuningConfig {
    /// Periodic persist interval in seconds (default: 300).
    #[serde(default = "default_persist_interval")]
    pub persist_interval_secs: u64,

    /// RRF rank fusion constant (default: 60).
    #[serde(default = "default_rrf_k")]
    pub rrf_k: u64,

    /// Static BM25 weight override (adaptive if unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rrf_bm25_weight: Option<f64>,

    /// Static vector weight override (adaptive if unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rrf_vector_weight: Option<f64>,

    /// Enable async KG extraction on writes.
    #[serde(default)]
    pub kg_auto_extract: bool,

    /// KG extraction batch size (default: 5).
    #[serde(default = "default_kg_batch_size")]
    pub kg_extract_batch_size: usize,

    /// KG extraction batch timeout in ms (default: 3000).
    #[serde(default = "default_kg_extract_timeout")]
    pub kg_extract_timeout_ms: u64,

    /// Log level filter (default: `"info"`). Overridden by `RUST_LOG`.
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

// ── Default value functions ────────────────────────────────────────────

fn default_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(".plico")
}

fn default_host() -> String { "127.0.0.1".into() }
fn default_daemon_port() -> u16 { 7878 }
fn default_sse_port() -> u16 { 7879 }
fn default_embedding_backend() -> String { "openai".into() }
fn default_llm_backend() -> String { "llama".into() }
fn default_persist_interval() -> u64 { 300 }
fn default_rrf_k() -> u64 { 60 }
fn default_kg_batch_size() -> usize { 5 }
fn default_kg_extract_timeout() -> u64 { 3000 }
fn default_log_level() -> String { "info".into() }

// ── Default trait impls ────────────────────────────────────────────────

impl Default for PlicoConfig {
    fn default() -> Self {
        Self {
            root: default_root(),
            network: NetworkConfig::default(),
            inference: InferenceConfig::default(),
            tuning: TuningConfig::default(),
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            daemon_port: default_daemon_port(),
            sse_port: default_sse_port(),
            disable_uds: false,
        }
    }
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            embedding_backend: default_embedding_backend(),
            embedding_api_base: None,
            embedding_model: None,
            embedding_model_id: None,
            embedding_python: None,
            llm_backend: default_llm_backend(),
            llama_url: None,
            llama_model: None,
            ollama_url: None,
            openai_api_base: None,
            api_key: None,
        }
    }
}

impl Default for TuningConfig {
    fn default() -> Self {
        Self {
            persist_interval_secs: default_persist_interval(),
            rrf_k: default_rrf_k(),
            rrf_bm25_weight: None,
            rrf_vector_weight: None,
            kg_auto_extract: false,
            kg_extract_batch_size: default_kg_batch_size(),
            kg_extract_timeout_ms: default_kg_extract_timeout(),
            log_level: default_log_level(),
        }
    }
}

// ── Loading ────────────────────────────────────────────────────────────

impl PlicoConfig {
    /// Load configuration with three-layer cascade:
    ///   built-in defaults → config file → environment variables.
    ///
    /// The `root_override` parameter (from `--root` CLI flag or `PLICO_ROOT` env)
    /// takes highest priority for the storage root path.
    pub fn load(root_override: Option<PathBuf>) -> Self {
        let mut config = Self::default();

        // Resolve root first (needed to find the config file).
        if let Some(root) = root_override {
            config.root = root;
        } else if let Ok(root) = std::env::var("PLICO_ROOT") {
            config.root = PathBuf::from(root);
        }

        // Layer 2: config file (merges on top of defaults).
        let config_path = config.root.join("config.json");
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            match serde_json::from_str::<PlicoConfig>(&contents) {
                Ok(file_config) => config.merge_from(file_config),
                Err(e) => tracing::warn!(
                    "Failed to parse {}: {e}",
                    config_path.display(),
                ),
            }
        }

        // Layer 3: environment variables (highest priority for each field).
        config.apply_env_overrides();

        config
    }

    /// Merge values from `other`, overwriting only non-default fields.
    fn merge_from(&mut self, other: PlicoConfig) {
        // Network
        if other.network.host != default_host() {
            self.network.host = other.network.host;
        }
        if other.network.daemon_port != default_daemon_port() {
            self.network.daemon_port = other.network.daemon_port;
        }
        if other.network.sse_port != default_sse_port() {
            self.network.sse_port = other.network.sse_port;
        }
        if other.network.disable_uds {
            self.network.disable_uds = true;
        }

        // Inference — Option fields: take if present in file
        if other.inference.embedding_backend != default_embedding_backend() {
            self.inference.embedding_backend = other.inference.embedding_backend;
        }
        macro_rules! merge_opt {
            ($field:ident) => {
                if other.inference.$field.is_some() {
                    self.inference.$field = other.inference.$field;
                }
            };
        }
        merge_opt!(embedding_api_base);
        merge_opt!(embedding_model);
        merge_opt!(embedding_model_id);
        merge_opt!(embedding_python);
        if other.inference.llm_backend != default_llm_backend() {
            self.inference.llm_backend = other.inference.llm_backend;
        }
        merge_opt!(llama_url);
        merge_opt!(llama_model);
        merge_opt!(ollama_url);
        merge_opt!(openai_api_base);
        merge_opt!(api_key);

        // Tuning
        if other.tuning.persist_interval_secs != default_persist_interval() {
            self.tuning.persist_interval_secs = other.tuning.persist_interval_secs;
        }
        if other.tuning.rrf_k != default_rrf_k() {
            self.tuning.rrf_k = other.tuning.rrf_k;
        }
        if other.tuning.rrf_bm25_weight.is_some() {
            self.tuning.rrf_bm25_weight = other.tuning.rrf_bm25_weight;
        }
        if other.tuning.rrf_vector_weight.is_some() {
            self.tuning.rrf_vector_weight = other.tuning.rrf_vector_weight;
        }
        if other.tuning.kg_auto_extract {
            self.tuning.kg_auto_extract = true;
        }
        if other.tuning.kg_extract_batch_size != default_kg_batch_size() {
            self.tuning.kg_extract_batch_size = other.tuning.kg_extract_batch_size;
        }
        if other.tuning.kg_extract_timeout_ms != default_kg_extract_timeout() {
            self.tuning.kg_extract_timeout_ms = other.tuning.kg_extract_timeout_ms;
        }
        if other.tuning.log_level != default_log_level() {
            self.tuning.log_level = other.tuning.log_level;
        }
    }

    /// Apply environment variable overrides (layer 3, highest priority).
    fn apply_env_overrides(&mut self) {
        macro_rules! env_str {
            ($var:expr, $field:expr) => {
                if let Ok(val) = std::env::var($var) {
                    $field = val;
                }
            };
        }
        macro_rules! env_opt {
            ($var:expr, $field:expr) => {
                if let Ok(val) = std::env::var($var) {
                    $field = Some(val);
                }
            };
        }
        macro_rules! env_parse {
            ($var:expr, $field:expr) => {
                if let Ok(val) = std::env::var($var) {
                    if let Ok(parsed) = val.parse() {
                        $field = parsed;
                    }
                }
            };
        }

        // Network
        env_str!("PLICO_HOST", self.network.host);
        env_parse!("PLICO_DAEMON_PORT", self.network.daemon_port);
        env_parse!("PLICO_SSE_PORT", self.network.sse_port);

        // Inference
        env_str!("EMBEDDING_BACKEND", self.inference.embedding_backend);
        env_opt!("EMBEDDING_API_BASE", self.inference.embedding_api_base);
        env_opt!("EMBEDDING_MODEL", self.inference.embedding_model);
        env_opt!("EMBEDDING_MODEL_ID", self.inference.embedding_model_id);
        env_opt!("EMBEDDING_PYTHON", self.inference.embedding_python);
        env_str!("LLM_BACKEND", self.inference.llm_backend);
        env_opt!("LLAMA_URL", self.inference.llama_url);
        env_opt!("LLAMA_MODEL", self.inference.llama_model);
        env_opt!("OLLAMA_URL", self.inference.ollama_url);
        env_opt!("OPENAI_API_BASE", self.inference.openai_api_base);
        env_opt!("OPENAI_API_KEY", self.inference.api_key);

        // Tuning
        env_parse!("PLICO_PERSIST_INTERVAL_SECS", self.tuning.persist_interval_secs);
        env_parse!("PLICO_RRF_K", self.tuning.rrf_k);
        if let Ok(val) = std::env::var("PLICO_RRF_BM25_WEIGHT") {
            self.tuning.rrf_bm25_weight = val.parse().ok();
        }
        if let Ok(val) = std::env::var("PLICO_RRF_VECTOR_WEIGHT") {
            self.tuning.rrf_vector_weight = val.parse().ok();
        }
        if let Ok(val) = std::env::var("PLICO_KG_AUTO_EXTRACT") {
            self.tuning.kg_auto_extract = matches!(val.as_str(), "1" | "true");
        }
        env_parse!("PLICO_KG_EXTRACT_BATCH_SIZE", self.tuning.kg_extract_batch_size);
        env_parse!("PLICO_KG_EXTRACT_TIMEOUT_MS", self.tuning.kg_extract_timeout_ms);

        // Log level — RUST_LOG takes priority over config
        if let Ok(val) = std::env::var("RUST_LOG") {
            self.tuning.log_level = val;
        }
    }

    /// Resolve the llama.cpp / OpenAI-compatible server URL.
    ///
    /// Priority: config `llama_url` > `openai_api_base` > auto-detect > fallback.
    pub fn resolve_llama_url(&self) -> String {
        if let Some(ref url) = self.inference.llama_url {
            return ensure_v1_suffix(url);
        }
        if let Some(ref url) = self.inference.openai_api_base {
            return ensure_v1_suffix(url);
        }
        let config_path = self.root.join("llama.url");
        if let Ok(url) = std::fs::read_to_string(&config_path) {
            let url = url.trim();
            if !url.is_empty() {
                return ensure_v1_suffix(url);
            }
        }
        if let Some(port) = detect_llama_server_port() {
            return format!("http://127.0.0.1:{port}/v1");
        }
        "http://127.0.0.1:8080/v1".into()
    }

    /// Resolve Ollama URL (config > default).
    pub fn resolve_ollama_url(&self) -> String {
        self.inference.ollama_url
            .clone()
            .unwrap_or_else(|| "http://localhost:11434".into())
    }

    /// Helper: construct the agent card URL for A2A protocol.
    pub fn agent_card_url(&self) -> String {
        format!("http://{}:{}", self.network.host, self.network.sse_port)
    }

    /// Helper: construct the daemon bind address.
    pub fn daemon_bind_addr(&self) -> String {
        format!("{}:{}", self.network.host, self.network.daemon_port)
    }

    /// Helper: PID file path.
    pub fn pid_path(&self) -> PathBuf { self.root.join("plicod.pid") }

    /// Helper: UDS socket path.
    pub fn sock_path(&self) -> PathBuf { self.root.join("plico.sock") }
}

pub fn ensure_v1_suffix(url: &str) -> String {
    if url.contains("/v1") {
        url.to_string()
    } else {
        format!("{}/v1", url.trim_end_matches('/'))
    }
}

/// Detect port of a running llama-server process (cross-platform).
///
/// Works on Linux, macOS, and any POSIX system with `ps`.
/// Returns `None` on Windows or when no llama-server is found.
fn detect_llama_server_port() -> Option<u16> {
    let output = std::process::Command::new("ps")
        .args(["aux"])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if !line.contains("llama-server") || line.contains("grep") {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        for (i, token) in tokens.iter().enumerate() {
            if *token == "--port" {
                if let Some(port_str) = tokens.get(i + 1) {
                    if let Ok(port) = port_str.parse::<u16>() {
                        return Some(port);
                    }
                }
            }
        }
    }
    None
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sane_values() {
        let config = PlicoConfig::default();
        assert_eq!(config.network.host, "127.0.0.1");
        assert_eq!(config.network.daemon_port, 7878);
        assert_eq!(config.network.sse_port, 7879);
        assert!(!config.network.disable_uds);
        assert_eq!(config.inference.embedding_backend, "openai");
        assert_eq!(config.inference.llm_backend, "llama");
        assert_eq!(config.tuning.persist_interval_secs, 300);
        assert_eq!(config.tuning.rrf_k, 60);
        assert_eq!(config.tuning.log_level, "info");
    }

    #[test]
    fn config_serialization_roundtrip() {
        let config = PlicoConfig::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: PlicoConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.network.host, config.network.host);
        assert_eq!(parsed.network.daemon_port, config.network.daemon_port);
        assert_eq!(parsed.inference.embedding_backend, config.inference.embedding_backend);
    }

    #[test]
    fn config_partial_json_uses_defaults() {
        let json = r#"{"network": {"daemon_port": 9999}}"#;
        let config: PlicoConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.network.daemon_port, 9999);
        assert_eq!(config.network.host, "127.0.0.1");
        assert_eq!(config.network.sse_port, 7879);
        assert_eq!(config.inference.llm_backend, "llama");
    }

    #[test]
    fn agent_card_url_reflects_config() {
        let mut config = PlicoConfig::default();
        assert_eq!(config.agent_card_url(), "http://127.0.0.1:7879");
        config.network.host = "0.0.0.0".into();
        config.network.sse_port = 8080;
        assert_eq!(config.agent_card_url(), "http://0.0.0.0:8080");
    }

    #[test]
    fn daemon_bind_addr_reflects_config() {
        let config = PlicoConfig::default();
        assert_eq!(config.daemon_bind_addr(), "127.0.0.1:7878");
    }

    #[test]
    fn ensure_v1_suffix_works() {
        assert_eq!(ensure_v1_suffix("http://localhost:8080"), "http://localhost:8080/v1");
        assert_eq!(ensure_v1_suffix("http://localhost:8080/v1"), "http://localhost:8080/v1");
        assert_eq!(ensure_v1_suffix("http://localhost:8080/"), "http://localhost:8080/v1");
    }

    #[test]
    fn merge_preserves_existing_when_other_is_default() {
        let mut config = PlicoConfig::default();
        config.network.daemon_port = 9999;
        config.merge_from(PlicoConfig::default());
        assert_eq!(config.network.daemon_port, 9999);
    }

    #[test]
    fn merge_overwrites_with_non_default() {
        let mut config = PlicoConfig::default();
        let mut other = PlicoConfig::default();
        other.network.daemon_port = 9999;
        config.merge_from(other);
        assert_eq!(config.network.daemon_port, 9999);
    }
}
