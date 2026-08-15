//! Pure credential evidence parser for the offline memory migrator.
//!
//! This module never opens files, mutates the runtime key store, or exposes
//! stored bearer material. Bytes must come from the CAS offline I/O boundary.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::PERSONAL_OWNER_ROLE_ID;

pub struct OfflineCredentialSet {
    credentials: BTreeMap<String, OfflineCredential>,
}

#[derive(Deserialize, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct OfflineCredential {
    agent_id: String,
    token: String,
    issued_at: u64,
    expires_at: Option<u64>,
    capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid offline migration authorization")]
pub struct OfflineCredentialError;

impl OfflineCredentialSet {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, OfflineCredentialError> {
        let credentials: BTreeMap<String, OfflineCredential> =
            serde_json::from_slice(bytes).map_err(|_| OfflineCredentialError)?;
        if credentials.is_empty()
            || credentials.iter().any(|(role, credential)| {
                role.trim().is_empty()
                    || credential.agent_id != *role
                    || credential.token.is_empty()
                    || credential.issued_at == 0
                    || !credential.capabilities.is_empty()
            })
        {
            return Err(OfflineCredentialError);
        }
        Ok(Self { credentials })
    }

    /// Constant-time compare against the stored active personal-owner bearer.
    /// All failure modes intentionally return one opaque error.
    pub fn verify_owner(&self, bearer: &str, now_secs: u64) -> Result<(), OfflineCredentialError> {
        let owner = self.credentials.get(PERSONAL_OWNER_ROLE_ID);
        let expected = Sha256::digest(owner.map_or(&[][..], |credential| credential.token.as_bytes()));
        let actual = Sha256::digest(bearer.as_bytes());
        let token_matches = expected.as_slice().ct_eq(actual.as_slice());
        let owner_is_active =
            owner.is_some_and(|credential| credential.expires_at.is_none_or(|expires_at| now_secs <= expires_at));
        if !bool::from(token_matches) || !owner_is_active {
            return Err(OfflineCredentialError);
        }
        Ok(())
    }

    /// Return only active local role identities for access-policy validation.
    pub fn active_role_ids(&self, now_secs: u64) -> Vec<String> {
        self.credentials
            .iter()
            .filter(|(_, credential)| credential.expires_at.is_none_or(|expires_at| now_secs <= expires_at))
            .map(|(role, _)| role.clone())
            .collect()
    }

    /// Hash only active role IDs and their expiry cutoff. Credential bytes and
    /// bearer hashes are never included.
    pub fn active_role_cutoff_hash(&self, now_secs: u64) -> Result<String, OfflineCredentialError> {
        let cutoff: Vec<_> = self
            .credentials
            .iter()
            .filter(|(_, credential)| credential.expires_at.is_none_or(|expires_at| now_secs <= expires_at))
            .map(|(role_id, credential)| ActiveRoleCutoff {
                role_id,
                expires_at: credential.expires_at,
            })
            .collect();
        let bytes = serde_json_canonicalizer::to_vec(&cutoff).map_err(|_| OfflineCredentialError)?;
        let mut hasher = Sha256::new();
        hasher.update(b"plico.memory.migration-credential-role-cutoff.v1\0");
        hasher.update(bytes);
        Ok(format!("{:x}", hasher.finalize()))
    }
}

#[derive(Serialize)]
struct ActiveRoleCutoff<'a> {
    role_id: &'a str,
    expires_at: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(expires_at: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "personal-owner": {
                "agent_id": "personal-owner", "token": "owner-secret", "issued_at": 1,
                "expires_at": null, "capabilities": []
            },
            "expired-role": {
                "agent_id": "expired-role", "token": "expired", "issued_at": 1,
                "expires_at": expires_at, "capabilities": []
            }
        }))
        .unwrap()
    }

    #[test]
    fn verifies_owner_and_filters_expired_roles() {
        let set = OfflineCredentialSet::from_bytes(&bytes(serde_json::json!(9))).unwrap();
        set.verify_owner("owner-secret", 10).unwrap();
        assert_eq!(set.active_role_ids(10), vec![PERSONAL_OWNER_ROLE_ID]);
        assert_eq!(set.active_role_cutoff_hash(10).unwrap().len(), 64);
    }

    #[test]
    fn all_owner_failures_are_opaque() {
        let set = OfflineCredentialSet::from_bytes(&bytes(serde_json::Value::Null)).unwrap();
        assert_eq!(set.verify_owner("wrong", 10).unwrap_err(), OfflineCredentialError);
        let missing = OfflineCredentialSet::from_bytes(
            br#"{"other":{"agent_id":"other","token":"x","issued_at":1,"expires_at":null,"capabilities":[]}}"#,
        )
        .unwrap();
        assert_eq!(missing.verify_owner("x", 10).unwrap_err(), OfflineCredentialError);
    }

    #[test]
    fn rejects_unknown_or_inconsistent_schema() {
        assert!(OfflineCredentialSet::from_bytes(
            br#"{"role":{"agent_id":"different","token":"x","issued_at":1,"expires_at":null,"capabilities":[]}}"#,
        )
        .is_err());
        assert!(OfflineCredentialSet::from_bytes(
            br#"{"role":{"agent_id":"role","token":"x","issued_at":1,"expires_at":null,"capabilities":[],"extra":true}}"#,
        )
        .is_err());
    }
}
