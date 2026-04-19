//! Agent Identity Authentication
//!
//! Provides cryptographic agent tokens (HMAC-SHA256) for agent identity verification.
//!
//! # Design
//!
//! Similar to Unix UID/GID — the kernel issues tokens that cannot be forged.
//! Tokens are scoped to a specific agent_id and validated on every API call.
//!
//! # Token Format
//!
//! `HMAC-SHA256(secret, agent_id || nonce || timestamp)`
//!
//! # Auth Modes
//!
//! - `Optional` (POC): token optional, allow unauthenticated requests
//! - `Required` (Production): all authenticated requests must carry valid token

use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use time::OffsetDateTime;

type HmacSha256 = Hmac<Sha256>;

/// Agent authentication mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentAuthMode {
    /// Token optional — backward compatible, no token requests allowed.
    Optional,
    /// Token required — all requests with agent_id must carry valid token.
    Required,
}

impl Default for AgentAuthMode {
    fn default() -> Self {
        Self::Optional
    }
}

/// Agent token issued by the kernel on registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToken {
    pub agent_id: String,
    /// Base64-encoded HMAC-SHA256 signature.
    pub token: String,
    pub issued_at: u64,
    /// None = never expires (daemon tokens).
    pub expires_at: Option<u64>,
    /// Declared capabilities (future use).
    pub capabilities: Vec<String>,
}

/// In-kernel token store — holds secret key and issued tokens.
/// Secret is generated at kernel startup (not persisted).
#[derive(Debug)]
pub struct AgentKeyStore {
    secret: [u8; 32],
    tokens: RwLock<HashMap<String, AgentToken>>,
    mode: AgentAuthMode,
}

impl AgentKeyStore {
    /// Create a new keystore with a randomly generated secret (for testing only).
    pub fn new() -> Self {
        let secret = rand::random::<[u8; 32]>();
        Self {
            secret,
            tokens: RwLock::new(HashMap::new()),
            mode: AgentAuthMode::Optional,
        }
    }

    /// Open or create a keystore at the given root.
    ///
    /// If `agent_secret.key` exists, reuses it; otherwise generates a new one.
    /// Tokens are restored from `agent_tokens.json` if present.
    pub fn open(root: &Path) -> Self {
        let secret_path = Self::secret_path(root);
        let secret = if secret_path.exists() {
            match std::fs::read(&secret_path) {
                Ok(bytes) if bytes.len() == 32 => {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&bytes);
                    arr
                }
                _ => {
                    tracing::warn!("Invalid secret file, generating new secret");
                    let s = rand::random::<[u8; 32]>();
                    let _ = Self::write_secret(&secret_path, &s);
                    s
                }
            }
        } else {
            let s = rand::random::<[u8; 32]>();
            let _ = Self::write_secret(&secret_path, &s);
            s
        };

        let tokens = Self::load_tokens(root);

        Self {
            secret,
            tokens: RwLock::new(tokens),
            mode: AgentAuthMode::Optional,
        }
    }

    fn secret_path(root: &Path) -> PathBuf {
        root.join("agent_secret.key")
    }

    fn tokens_path(root: &Path) -> PathBuf {
        root.join("agent_tokens.json")
    }

    fn write_secret(path: &Path, secret: &[u8; 32]) -> std::io::Result<()> {
        std::fs::write(path, secret)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    fn load_tokens(root: &Path) -> HashMap<String, AgentToken> {
        let path = Self::tokens_path(root);
        if !path.exists() {
            return HashMap::new();
        }
        match std::fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str::<HashMap<String, AgentToken>>(&json) {
                Ok(tokens) => {
                    let count = tokens.len();
                    if count > 0 {
                        tracing::info!("Restored {count} agent tokens from persistent storage");
                    }
                    return tokens;
                }
                Err(e) => tracing::warn!("Failed to parse agent tokens: {e}"),
            },
            Err(e) => tracing::warn!("Failed to read agent tokens: {e}"),
        }
        HashMap::new()
    }

    /// Persist tokens to disk.
    pub fn persist(&self, root: &Path) {
        let tokens = self.tokens.read().unwrap();
        crate::kernel::persistence::atomic_write_json(&Self::tokens_path(root), &*tokens);
    }

    /// Set auth mode.
    pub fn set_mode(&mut self, mode: AgentAuthMode) {
        self.mode = mode;
    }

    /// Generate a new token for an agent.
    pub fn generate_token(&self, agent_id: &str) -> AgentToken {
        let nonce: u64 = rand::random();
        let timestamp = now_secs();
        let input = format!("{}:{}:{}", agent_id, nonce, timestamp);

        let mut mac = HmacSha256::new_from_slice(&self.secret).expect("HMAC accepts fixed key size");
        mac.update(input.as_bytes());
        let sig = mac.finalize().into_bytes();
        let token_b64 = base64::engine::general_purpose::STANDARD.encode(sig);

        let token = AgentToken {
            agent_id: agent_id.to_string(),
            token: token_b64,
            issued_at: timestamp,
            expires_at: None, // daemon tokens don't expire
            capabilities: Vec::new(),
        };

        // Auto-store the token so it can be verified later
        self.store_token(&token);
        token
    }

    /// Store a token for an agent.
    pub fn store_token(&self, token: &AgentToken) {
        self.tokens.write().unwrap().insert(token.agent_id.clone(), token.clone());
    }

    /// Verify a token for an agent.
    ///
    /// Returns `true` if:
    /// - Token is present and valid for the given agent_id
    /// - Token has not expired
    ///
    /// Returns `false` if:
    /// - Token not found
    /// - Token mismatch
    /// - Token expired
    pub fn verify_token(&self, agent_id: &str, token_str: &str) -> bool {
        let tokens = self.tokens.read().unwrap();
        let Some(token) = tokens.get(agent_id) else {
            return false;
        };

        // Check expiry
        if let Some(expires_at) = token.expires_at {
            if now_secs() > expires_at {
                return false;
            }
        }

        // Token string comparison (constant-time would be better but this is POC)
        token.token == token_str
    }

    /// Get a token for an agent (returns clone).
    pub fn get_token(&self, agent_id: &str) -> Option<AgentToken> {
        self.tokens.read().unwrap().get(agent_id).cloned()
    }

    /// Check if auth mode requires token.
    pub fn requires_token(&self) -> bool {
        self.mode == AgentAuthMode::Required
    }

    /// Verify agent token in Optional mode.
    ///
    /// In Optional mode:
    /// - If no token provided → Ok (allow)
    /// - If token provided → verify it (reject if invalid)
    ///
    /// In Required mode:
    /// - Token must be present and valid
    pub fn verify_agent_token(&self, agent_id: &str, token_opt: Option<&str>) -> Result<(), String> {
        match (self.mode, token_opt) {
            // Required mode: token must be present and valid
            (AgentAuthMode::Required, None) => {
                Err(format!("Agent '{}': token required but none provided", agent_id))
            }
            (AgentAuthMode::Required, Some(token)) => {
                if self.verify_token(agent_id, token) {
                    Ok(())
                } else {
                    Err(format!("Agent '{}': invalid token", agent_id))
                }
            }
            // Optional mode: no token → allow, invalid token → reject
            (AgentAuthMode::Optional, None) => Ok(()),
            (AgentAuthMode::Optional, Some(token)) => {
                if self.verify_token(agent_id, token) {
                    Ok(())
                } else {
                    Err(format!("Agent '{}': invalid token", agent_id))
                }
            }
        }
    }
}

impl Default for AgentKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

fn now_secs() -> u64 {
    OffsetDateTime::now_utc()
        .unix_timestamp()
        .unsigned_abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_verify_token() {
        let store = AgentKeyStore::new();
        let token = store.generate_token("agent1");

        assert_eq!(token.agent_id, "agent1");
        assert!(!token.token.is_empty());
        assert!(token.expires_at.is_none());

        // Verify valid token
        assert!(store.verify_token("agent1", &token.token));

        // Verify wrong token
        assert!(!store.verify_token("agent1", "wrong_token"));
    }

    #[test]
    fn verify_token_unknown_agent() {
        let store = AgentKeyStore::new();
        assert!(!store.verify_token("unknown", "any_token"));
    }

    #[test]
    fn verify_token_with_stored_token() {
        let store = AgentKeyStore::new();
        let token = store.generate_token("agent1");
        store.store_token(&token);

        assert!(store.verify_token("agent1", &token.token));
        assert!(!store.verify_token("agent1", "wrong"));
        assert!(!store.verify_token("agent2", &token.token));
    }

    #[test]
    fn optional_mode_allows_no_token() {
        let store = AgentKeyStore::new(); // default = Optional
        assert!(store.verify_agent_token("agent1", None).is_ok());
    }

    #[test]
    fn optional_mode_rejects_invalid_token() {
        let store = AgentKeyStore::new();
        let result = store.verify_agent_token("agent1", Some("invalid"));
        assert!(result.is_err());
    }

    #[test]
    fn optional_mode_accepts_valid_token() {
        let store = AgentKeyStore::new();
        let token = store.generate_token("agent1");
        store.store_token(&token);

        assert!(store.verify_agent_token("agent1", Some(&token.token)).is_ok());
    }

    #[test]
    fn required_mode_rejects_no_token() {
        let mut store = AgentKeyStore::new();
        store.set_mode(AgentAuthMode::Required);

        let result = store.verify_agent_token("agent1", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("token required"));
    }

    #[test]
    fn required_mode_accepts_valid_token() {
        let mut store = AgentKeyStore::new();
        store.set_mode(AgentAuthMode::Required);
        let token = store.generate_token("agent1");
        store.store_token(&token);

        assert!(store.verify_agent_token("agent1", Some(&token.token)).is_ok());
    }

    #[test]
    fn token_is_different_each_time() {
        let store = AgentKeyStore::new();
        let t1 = store.generate_token("agent1");
        let t2 = store.generate_token("agent1");

        // Same agent_id, different nonce → different tokens
        assert_ne!(t1.token, t2.token);
    }
}
