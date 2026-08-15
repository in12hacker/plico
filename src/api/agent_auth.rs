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
//! - `Optional`: token optional, allow unauthenticated requests (development/testing)
//! - `Required`: all requests must carry valid token (production)

use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use time::OffsetDateTime;

type HmacSha256 = Hmac<Sha256>;

/// Stable identity represented by the daemon's local owner credential.
pub use crate::PERSONAL_OWNER_ROLE_ID;

/// Whether daemon bootstrap created the personal owner credential or reused it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonalOwnerCredentialState {
    Created,
    Existing,
}

/// Agent authentication mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentAuthMode {
    /// Token optional — unauthenticated requests allowed (default for development).
    #[default]
    Optional,
    /// Token required — all requests with agent_id must carry valid token.
    Required,
}

impl AgentAuthMode {
    /// Resolve the daemon authentication mode from `PLICO_AGENT_AUTH_MODE`.
    ///
    /// Unknown configured values fail closed to `Required` and emit a warning.
    pub fn from_env(default: Self) -> Self {
        let Ok(value) = std::env::var("PLICO_AGENT_AUTH_MODE") else {
            return default;
        };
        Self::from_config_value(&value).unwrap_or_else(|| {
            tracing::warn!(value, "Invalid PLICO_AGENT_AUTH_MODE; requiring authentication");
            Self::Required
        })
    }

    fn from_config_value(value: &str) -> Option<Self> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "required" | "require" | "1" | "true" => Self::Required,
            "optional" | "0" | "false" => Self::Optional,
            _ => return None,
        })
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

/// In-kernel token store backed by owner-only persistent auth material.
#[derive(Debug)]
pub struct AgentKeyStore {
    secret: [u8; 32],
    tokens: RwLock<HashMap<String, AgentToken>>,
    mode: RwLock<AgentAuthMode>,
}

impl AgentKeyStore {
    /// Create a new keystore with a randomly generated secret (for testing only).
    pub fn new() -> Self {
        let secret = rand::random::<[u8; 32]>();
        Self {
            secret,
            tokens: RwLock::new(HashMap::new()),
            mode: RwLock::new(AgentAuthMode::Optional),
        }
    }

    /// Open or create a keystore at the given root.
    ///
    /// If `agent_secret.key` exists, reuses it; otherwise generates a new one.
    /// Tokens are restored from `agent_tokens.json` if present.
    pub fn open(root: &Path) -> Self {
        let secret_path = Self::secret_path(root);
        let secret = if secret_path.exists() {
            let _ = Self::restrict_private_permissions(&secret_path);
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
            mode: RwLock::new(AgentAuthMode::from_env(AgentAuthMode::Optional)),
        }
    }

    fn secret_path(root: &Path) -> PathBuf {
        root.join("agent_secret.key")
    }

    fn tokens_path(root: &Path) -> PathBuf {
        root.join("agent_tokens.json")
    }

    /// The sole file containing bearer credentials for explicit local
    /// distribution. The separate HMAC secret is kernel signing material and
    /// is never a client credential.
    pub fn credential_path(root: &Path) -> PathBuf {
        Self::tokens_path(root)
    }

    fn write_secret(path: &Path, secret: &[u8; 32]) -> std::io::Result<()> {
        Self::write_private_file(path, secret)
    }

    fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        Self::restrict_private_permissions(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }

    fn restrict_private_permissions(path: &Path) -> std::io::Result<()> {
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
        let _ = Self::restrict_private_permissions(&path);
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

    fn persist_snapshot(root: &Path, tokens: &HashMap<String, AgentToken>) -> std::io::Result<()> {
        let path = Self::tokens_path(root);
        let tmp = path.with_extension("json.tmp");
        let result = serde_json::to_vec_pretty(tokens)
            .map_err(std::io::Error::other)
            .and_then(|json| Self::write_private_file(&tmp, &json))
            .and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(error) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
        Self::restrict_private_permissions(&path)
    }

    /// Persist tokens to disk.
    pub fn persist(&self, root: &Path) {
        let tokens = self.tokens.read().unwrap();
        if Self::persist_snapshot(root, &tokens).is_err() {
            tracing::warn!(
                error_category = "credential_persistence",
                "Failed to persist agent credentials"
            );
        }
    }

    /// Ensure the daemon has exactly one stable bootstrap credential for the
    /// personal owner. Persistence happens before the credential is published
    /// to the live store, so a failed write cannot create an ephemeral bearer.
    ///
    /// Rotation should replace this same map entry atomically through the same
    /// private file; it must not add a second runtime credential source.
    pub fn ensure_personal_owner_credential(&self, root: &Path) -> std::io::Result<PersonalOwnerCredentialState> {
        let mut tokens = self.tokens.write().unwrap();
        let state = if tokens.contains_key(PERSONAL_OWNER_ROLE_ID) {
            PersonalOwnerCredentialState::Existing
        } else {
            PersonalOwnerCredentialState::Created
        };
        let mut candidate = tokens.clone();
        candidate
            .entry(PERSONAL_OWNER_ROLE_ID.to_string())
            .or_insert_with(|| self.create_token(PERSONAL_OWNER_ROLE_ID));
        Self::persist_snapshot(root, &candidate)?;
        *tokens = candidate;
        Ok(state)
    }

    /// Set auth mode.
    pub fn set_mode(&self, mode: AgentAuthMode) {
        *self.mode.write().unwrap() = mode;
    }

    /// Generate a new token for an agent.
    pub fn generate_token(&self, agent_id: &str) -> AgentToken {
        let token = self.create_token(agent_id);
        self.store_token(&token);
        token
    }

    fn create_token(&self, agent_id: &str) -> AgentToken {
        let nonce: u64 = rand::random();
        let timestamp = now_secs();
        let input = format!("{}:{}:{}", agent_id, nonce, timestamp);

        let mut mac = HmacSha256::new_from_slice(&self.secret).expect("HMAC accepts fixed key size");
        mac.update(input.as_bytes());
        let sig = mac.finalize().into_bytes();
        let token_b64 = base64::engine::general_purpose::STANDARD.encode(sig);

        AgentToken {
            agent_id: agent_id.to_string(),
            token: token_b64,
            issued_at: timestamp,
            expires_at: None, // daemon tokens don't expire
            capabilities: Vec::new(),
        }
    }

    /// Store a token for an agent.
    pub fn store_token(&self, token: &AgentToken) {
        self.tokens
            .write()
            .unwrap()
            .insert(token.agent_id.clone(), token.clone());
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

        use subtle::ConstantTimeEq;
        let a = token.token.as_bytes();
        let b = token_str.as_bytes();
        if a.len() != b.len() {
            return false;
        }
        a.ct_eq(b).into()
    }

    /// Resolve a bearer credential to its authenticated local role.
    ///
    /// Public transports must derive identity from the credential instead of
    /// accepting an `agent_id` asserted by the request payload. The error is
    /// intentionally generic so callers cannot enumerate stored roles.
    pub fn authenticate_bearer(&self, token_str: &str) -> Result<String, String> {
        if token_str.is_empty() {
            return Err("invalid bearer credential".to_string());
        }

        use subtle::ConstantTimeEq;
        let now = now_secs();
        let tokens = self.tokens.read().unwrap();
        for token in tokens.values() {
            if token.expires_at.is_some_and(|expires_at| now > expires_at) {
                continue;
            }
            let expected = token.token.as_bytes();
            let actual = token_str.as_bytes();
            if expected.len() == actual.len() && bool::from(expected.ct_eq(actual)) {
                return Ok(token.agent_id.clone());
            }
        }

        Err("invalid bearer credential".to_string())
    }

    /// Get a token for an agent (returns clone).
    pub fn get_token(&self, agent_id: &str) -> Option<AgentToken> {
        self.tokens.read().unwrap().get(agent_id).cloned()
    }

    /// Check if auth mode requires token.
    pub fn requires_token(&self) -> bool {
        *self.mode.read().unwrap() == AgentAuthMode::Required
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
        let mode = *self.mode.read().unwrap();
        match (mode, token_opt) {
            // Required mode: token must be present and valid
            (AgentAuthMode::Required, None) => Err(format!("Agent '{}': token required but none provided", agent_id)),
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
    OffsetDateTime::now_utc().unix_timestamp().unsigned_abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct CapturedLogs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedLogs {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedLogs {
        type Writer = Self;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

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
    fn bearer_resolves_authenticated_role_without_claimed_identity() {
        let store = AgentKeyStore::new();
        let token = store.generate_token("local-research-role");

        assert_eq!(store.authenticate_bearer(&token.token).unwrap(), "local-research-role");
        assert_eq!(
            store.authenticate_bearer("wrong").unwrap_err(),
            "invalid bearer credential"
        );
        assert_eq!(store.authenticate_bearer("").unwrap_err(), "invalid bearer credential");
    }

    #[test]
    fn optional_mode_allows_no_token() {
        let store = AgentKeyStore::new(); // default = Optional
        assert!(store.verify_agent_token("agent1", None).is_ok());
    }

    #[test]
    fn parses_production_auth_mode_values() {
        assert_eq!(
            AgentAuthMode::from_config_value("required"),
            Some(AgentAuthMode::Required)
        );
        assert_eq!(AgentAuthMode::from_config_value("TRUE"), Some(AgentAuthMode::Required));
        assert_eq!(
            AgentAuthMode::from_config_value("optional"),
            Some(AgentAuthMode::Optional)
        );
        assert_eq!(AgentAuthMode::from_config_value("invalid"), None);
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
        let store = AgentKeyStore::new();
        store.set_mode(AgentAuthMode::Required);

        let result = store.verify_agent_token("agent1", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("token required"));
    }

    #[test]
    fn required_mode_accepts_valid_token() {
        let store = AgentKeyStore::new();
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

    #[cfg(unix)]
    #[test]
    fn persisted_auth_material_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let store = AgentKeyStore::open(dir.path());
        store.generate_token("agent1");
        store.persist(dir.path());

        for path in [
            AgentKeyStore::secret_path(dir.path()),
            AgentKeyStore::tokens_path(dir.path()),
        ] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn personal_owner_bootstrap_is_stable_private_and_never_logged() {
        use std::os::unix::fs::PermissionsExt;

        let _trace_guard = crate::TRACE_CAPTURE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let directory = tempfile::tempdir().unwrap();
        let captured = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(captured.clone())
            .finish();

        let first_token = tracing::subscriber::with_default(subscriber, || {
            tracing::callsite::rebuild_interest_cache();
            let first = AgentKeyStore::open(directory.path());
            assert_eq!(
                first.ensure_personal_owner_credential(directory.path()).unwrap(),
                PersonalOwnerCredentialState::Created
            );
            let token = first.get_token(PERSONAL_OWNER_ROLE_ID).unwrap().token;
            assert_eq!(first.authenticate_bearer(&token).unwrap(), PERSONAL_OWNER_ROLE_ID);
            token
        });

        let restarted = AgentKeyStore::open(directory.path());
        assert_eq!(
            restarted.ensure_personal_owner_credential(directory.path()).unwrap(),
            PersonalOwnerCredentialState::Existing
        );
        let restarted_token = restarted.get_token(PERSONAL_OWNER_ROLE_ID).unwrap().token;
        assert_eq!(restarted_token, first_token);

        let credential_path = AgentKeyStore::credential_path(directory.path());
        let persisted: HashMap<String, AgentToken> =
            serde_json::from_slice(&std::fs::read(&credential_path).unwrap()).unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[PERSONAL_OWNER_ROLE_ID].token, first_token);
        let mode = std::fs::metadata(credential_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        assert!(!logs.contains(&first_token));
    }

    #[test]
    fn failed_personal_owner_bootstrap_does_not_publish_ephemeral_credential() {
        let directory = tempfile::tempdir().unwrap();
        let invalid_root = directory.path().join("not-a-directory");
        std::fs::write(&invalid_root, b"occupied").unwrap();
        let store = AgentKeyStore::new();

        assert!(store.ensure_personal_owner_credential(&invalid_root).is_err());
        assert!(store.get_token(PERSONAL_OWNER_ROLE_ID).is_none());
    }
}
