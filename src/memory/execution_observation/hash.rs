//! Domain-separated identity hashes for observation objects (ADR-0007 §8).
//!
//! Every digest is `sha256(domain || RFC8785_JCS(value))` with lowercase hex
//! output. The seven domains are frozen; the active pointer has no identity
//! hash because its canonical bytes carry only the root hash. This module is an
//! independent implementation and never imports `memory/ledger/hash.rs`.

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::canonical::to_canonical_vec;
use super::error::ObservationStoreError;
use super::model::{
    AppendStartedRequestV1, AppendTerminalRequestV1, FixtureCurrentViewV1, FixtureEventSegmentV1, FixtureLedgerRootV1,
    StoredStartedEventV1, StoredTerminalEventV1,
};

const STARTED_REQUEST_DOMAIN: &[u8] = b"plico.execution-observation.fixture.started-request.v1\0";
const TERMINAL_REQUEST_DOMAIN: &[u8] = b"plico.execution-observation.fixture.terminal-request.v1\0";
const STARTED_EVENT_DOMAIN: &[u8] = b"plico.execution-observation.fixture.started-event.v1\0";
const TERMINAL_EVENT_DOMAIN: &[u8] = b"plico.execution-observation.fixture.terminal-event.v1\0";
const SEGMENT_DOMAIN: &[u8] = b"plico.execution-observation.fixture.segment.v1\0";
const CURRENT_VIEW_DOMAIN: &[u8] = b"plico.execution-observation.fixture.current-view.v1\0";
const ROOT_DOMAIN: &[u8] = b"plico.execution-observation.fixture.root.v1\0";

pub(crate) fn started_request_sha256(value: &AppendStartedRequestV1) -> Result<String, ObservationStoreError> {
    domain_hash(STARTED_REQUEST_DOMAIN, value)
}

pub(crate) fn terminal_request_sha256(value: &AppendTerminalRequestV1) -> Result<String, ObservationStoreError> {
    domain_hash(TERMINAL_REQUEST_DOMAIN, value)
}

pub(crate) fn started_event_sha256(value: &StoredStartedEventV1) -> Result<String, ObservationStoreError> {
    domain_hash(STARTED_EVENT_DOMAIN, value)
}

pub(crate) fn terminal_event_sha256(value: &StoredTerminalEventV1) -> Result<String, ObservationStoreError> {
    domain_hash(TERMINAL_EVENT_DOMAIN, value)
}

pub(crate) fn segment_sha256(value: &FixtureEventSegmentV1) -> Result<String, ObservationStoreError> {
    domain_hash(SEGMENT_DOMAIN, value)
}

pub(crate) fn current_view_sha256(value: &FixtureCurrentViewV1) -> Result<String, ObservationStoreError> {
    domain_hash(CURRENT_VIEW_DOMAIN, value)
}

pub(crate) fn root_sha256(value: &FixtureLedgerRootV1) -> Result<String, ObservationStoreError> {
    domain_hash(ROOT_DOMAIN, value)
}

fn domain_hash<T: Serialize>(domain: &[u8], value: &T) -> Result<String, ObservationStoreError> {
    let canonical = to_canonical_vec(value)?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(&canonical);
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
pub(crate) mod tests;
