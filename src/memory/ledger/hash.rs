use serde::Serialize;
use sha2::{Digest, Sha256};

use super::model::LedgerError;

const SEGMENT_DOMAIN: &[u8] = b"plico.memory.segment.v1\0";
const ROOT_DOMAIN: &[u8] = b"plico.memory.root.v1\0";
const VIEW_DOMAIN: &[u8] = b"plico.memory.current-view.v1\0";

pub(super) fn segment_bytes_and_hash<T: Serialize>(value: &T) -> Result<(Vec<u8>, String), LedgerError> {
    canonical_bytes_and_hash(SEGMENT_DOMAIN, value)
}

pub(super) fn root_bytes_and_hash<T: Serialize>(value: &T) -> Result<(Vec<u8>, String), LedgerError> {
    canonical_bytes_and_hash(ROOT_DOMAIN, value)
}

pub(super) fn view_bytes_and_hash<T: Serialize>(value: &T) -> Result<(Vec<u8>, String), LedgerError> {
    canonical_bytes_and_hash(VIEW_DOMAIN, value)
}

pub(super) fn verify_segment_hash<T: Serialize>(value: &T, expected: &str) -> Result<(), LedgerError> {
    verify(SEGMENT_DOMAIN, value, expected, "segment_hash_mismatch")
}

pub(super) fn verify_root_hash<T: Serialize>(value: &T, expected: &str) -> Result<(), LedgerError> {
    verify(ROOT_DOMAIN, value, expected, "root_hash_mismatch")
}

pub(super) fn verify_view_hash<T: Serialize>(value: &T, expected: &str) -> Result<(), LedgerError> {
    verify(VIEW_DOMAIN, value, expected, "current_view_hash_mismatch")
}

fn canonical_bytes_and_hash<T: Serialize>(domain: &[u8], value: &T) -> Result<(Vec<u8>, String), LedgerError> {
    let bytes = serde_json_canonicalizer::to_vec(value).map_err(|_| LedgerError::Invalid {
        category: "jcs_canonicalization_failed",
    })?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(&bytes);
    Ok((bytes, format!("{:x}", hasher.finalize())))
}

fn verify<T: Serialize>(domain: &[u8], value: &T, expected: &str, category: &'static str) -> Result<(), LedgerError> {
    let (_, actual) = canonical_bytes_and_hash(domain, value)?;
    if actual == expected {
        Ok(())
    } else {
        Err(LedgerError::Invalid { category })
    }
}
