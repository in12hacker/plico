#!/usr/bin/env python3
"""Verify a v53 WP2 candidate against its architecture-sealed checkpoint."""

from __future__ import annotations

import argparse
import collections
import fnmatch
import json
import os
import re
import resource
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from contextlib import contextmanager
from decimal import Decimal
from pathlib import Path, PurePosixPath

import verify
import authorize


TEST_DECLARATION = re.compile(
    r"(?m)^\s*(?:async\s+)?fn\s+(execution_observation_f(\d{2})_[A-Za-z0-9_]+)\s*\("
)
PURE_EXTERNAL_ROOTS = {
    "core",
    "serde",
    "serde_json",
    "serde_json_canonicalizer",
    "sha2",
    "std",
    "thiserror",
    "uuid",
}
PURE_STD_MODULES = {
    "borrow",
    "cmp",
    "collections",
    "convert",
    "fmt",
    "hash",
    "marker",
    "mem",
    "num",
    "ops",
    "option",
    "result",
    "slice",
    "str",
    "sync",
    "time",
}
OBSERVATION_MODULES = {
    "canonical",
    "error",
    "hash",
    "ids",
    "model",
    "store",
    "tests",
    "validation",
}
RUST_PRIMITIVE_PATH_ROOTS = {
    "bool",
    "char",
    "f32",
    "f64",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "str",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
}
RUST_INTEGER_PATH_ROOTS = {
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
}
MAX_CANDIDATE_OUTPUT_BYTES = 16 * 1024 * 1024
MAX_CANDIDATE_ADDRESS_SPACE_BYTES = 32 * 1024 * 1024 * 1024
WP1_EXTERNAL_TESTS = r"""
use std::num::NonZeroU32;

use super::canonical::parse_canonical;
use super::error::{
    CorruptionCategory, InvalidRequestCategory, LimitCategory,
    ObservationStoreError, TransitionConflictCategory,
};
use super::hash;
use super::model::{
    AppendStartedRequestV1, AppendTerminalRequestV1, FixtureAttemptViewV1,
    StoredStartedEventV1, STARTED_EVENT_SCHEMA,
};
use super::validation::{validate_started_transition, validate_terminal_transition};
use super::{validate_attempt_count, validate_event_count};

const STARTED: &[u8] = br#"{"attestation_state":"unverified_fixture","context_evidence_cids":["2222222222222222222222222222222222222222222222222222222222222222"],"fixture_origin":{"request_id":"123e4567-e89b-42d3-a456-426614174001","type":"public_request"},"fixture_role_ref":null,"fixture_session_ref":null,"input_evidence_cids":["0000000000000000000000000000000000000000000000000000000000000000","1111111111111111111111111111111111111111111111111111111111111111"],"key":{"attempt":1,"execution_id":"123e4567-e89b-42d3-a456-426614174000"},"operation_contract_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","policy_sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","runtime_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","schema":"plico.execution-observation.fixture-start-request/v1"}"#;
const TERMINAL: &[u8] = br#"{"attestation_state":"unverified_fixture","execution_elapsed_ms":null,"key":{"attempt":1,"execution_id":"123e4567-e89b-42d3-a456-426614174000"},"outcome":{"category":"tool_failed","type":"failure"},"output_evidence_cids":["3333333333333333333333333333333333333333333333333333333333333333"],"policy_sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","runtime_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","schema":"plico.execution-observation.fixture-terminal-request/v1"}"#;
const STARTED_SHA: &str = "160804b6003538aba7cf858993b2f3efdf830493875a9c03e5277db0225975ac";
const TERMINAL_SHA: &str = "f8dd59a4bdaeabe52b27b79f0f4c749e344f7483ec66588ef6f9efe55f9d5bf2";

fn started() -> AppendStartedRequestV1 {
    parse_canonical(STARTED).expect("architecture-owned started vector")
}

fn terminal() -> AppendTerminalRequestV1 {
    parse_canonical(TERMINAL).expect("architecture-owned terminal vector")
}

fn open_view(request: &AppendStartedRequestV1) -> FixtureAttemptViewV1 {
    FixtureAttemptViewV1 {
        key: request.key.clone(),
        attestation_state: "unverified_fixture".into(),
        started_request_sha256: hash::started_request_sha256(request).expect("hash"),
        started_event_sha256: "11".repeat(32),
        terminal_request_sha256: None,
        terminal_event_sha256: None,
    }
}

#[test]
fn architecture_wp1_contract_golden_hashes_are_external() {
    assert_eq!(hash::started_request_sha256(&started()).unwrap(), STARTED_SHA);
    assert_eq!(hash::terminal_request_sha256(&terminal()).unwrap(), TERMINAL_SHA);
}

#[test]
fn architecture_wp1_contract_jcs_precedes_semantics() {
    let noncanonical = [b" ".as_slice(), STARTED].concat();
    assert_eq!(
        parse_canonical::<AppendStartedRequestV1>(&noncanonical),
        Err(ObservationStoreError::invalid(
            InvalidRequestCategory::JcsCanonicalizationFailed,
        )),
    );
    let zero = String::from_utf8(STARTED.to_vec()).unwrap().replace(
        "\"attempt\":1",
        "\"attempt\":0",
    );
    assert_eq!(
        parse_canonical::<AppendStartedRequestV1>(zero.as_bytes()),
        Err(ObservationStoreError::invalid(InvalidRequestCategory::ZeroAttempt)),
    );
    let unknown = String::from_utf8(TERMINAL.to_vec()).unwrap().replace(
        "tool_failed",
        "not_known__",
    );
    assert_eq!(
        parse_canonical::<AppendTerminalRequestV1>(unknown.as_bytes()),
        Err(ObservationStoreError::invalid(
            InvalidRequestCategory::InvalidFailureCategory,
        )),
    );
}

#[test]
fn architecture_wp1_contract_retry_identity_is_body_derived() {
    let original = started();
    let view = open_view(&original);
    let mut modified = original.clone();
    modified.context_evidence_cids.push("44".repeat(32));
    assert_eq!(
        validate_started_transition(&modified, Some(&view)),
        Err(ObservationStoreError::conflict(
            TransitionConflictCategory::StartedAlreadyBound,
        )),
    );
}

#[test]
fn architecture_wp1_contract_terminal_binds_key_policy_and_total() {
    let original = started();
    let view = open_view(&original);
    let mut wrong_key = terminal();
    wrong_key.key.attempt = NonZeroU32::new(2).unwrap();
    assert_eq!(
        validate_terminal_transition(&wrong_key, Some(&view), Some(&original)),
        Err(ObservationStoreError::corrupt(
            CorruptionCategory::InvalidTransition,
        )),
    );

    let mut full = original.clone();
    full.input_evidence_cids = (0..256).map(|value| format!("{value:064x}")).collect();
    full.context_evidence_cids = (256..512).map(|value| format!("{value:064x}")).collect();
    let full_view = open_view(&full);
    assert_eq!(
        validate_terminal_transition(&terminal(), Some(&full_view), Some(&full)),
        Err(ObservationStoreError::limit(LimitCategory::EvidenceTotal)),
    );
}

#[test]
fn architecture_wp1_contract_caps_cover_stored_ordinals() {
    assert!(validate_attempt_count(10_000).is_ok());
    assert_eq!(
        validate_attempt_count(10_001),
        Err(ObservationStoreError::limit(LimitCategory::Attempt)),
    );
    assert!(validate_event_count(20_000).is_ok());
    assert_eq!(
        validate_event_count(20_001),
        Err(ObservationStoreError::limit(LimitCategory::Event)),
    );
    let request = started();
    let stored = StoredStartedEventV1 {
        schema: STARTED_EVENT_SCHEMA.into(),
        request_sha256: hash::started_request_sha256(&request).unwrap(),
        request,
        sequence: 20_001,
        root_generation: 20_001,
        recorded_at_ms: 1,
    };
    assert_eq!(
        stored.validate(),
        Err(ObservationStoreError::limit(LimitCategory::Event)),
    );
}
"""
WP1_EXTERNAL_TEST_NAMES = {
    "memory::execution_observation::architecture_contract_tests::architecture_wp1_contract_caps_cover_stored_ordinals",
    "memory::execution_observation::architecture_contract_tests::architecture_wp1_contract_golden_hashes_are_external",
    "memory::execution_observation::architecture_contract_tests::architecture_wp1_contract_jcs_precedes_semantics",
    "memory::execution_observation::architecture_contract_tests::architecture_wp1_contract_retry_identity_is_body_derived",
    "memory::execution_observation::architecture_contract_tests::architecture_wp1_contract_terminal_binds_key_policy_and_total",
}
WP2_EXTERNAL_TESTS = r"""
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;
use crate::cas::PersonalVaultStorage;

use super::canonical::{parse_canonical, to_canonical_vec};
use super::error::{CorruptionCategory, ObservationStoreError};
use super::hash;
use super::hash::tests::{
    GENESIS_ROOT_SHA, GENESIS_VIEW_SHA, STARTED_EVENT_SHA, STARTED_ROOT_SHA,
    STARTED_SEGMENT_SHA, TERMINAL_ROOT_SHA,
};
use super::model::{
    FixtureActivePointerV1, FixtureCurrentViewV1, FixtureLedgerRootV1,
    ATTESTATION_STATE, CURRENT_VIEW_SCHEMA, POINTER_SCHEMA, ROOT_SCHEMA,
    TRUST_CLASS,
};
use super::store::{
    FixtureObservationStoreV1, FixtureStoredEventV1, FixtureStructuralCommitV1,
};
use super::tests::{golden_chain, GoldenChain};
use super::{POINTER_MAX_BYTES, ROOT_MAX_BYTES, SEGMENT_MAX_BYTES};

const STORED_EVENT_MAX_BYTES: usize = 135_168;
const DIRECTORY: &str = "execution-observation-fixture-ledger";

fn open(vault: &Path) -> FixtureObservationStoreV1 {
    let storage = Arc::new(PersonalVaultStorage::open(vault, None).expect("open vault"));
    FixtureObservationStoreV1::open_fixture(storage).expect("open fixture store")
}

fn object_path(vault: &Path, hash: &str) -> std::path::PathBuf {
    vault.join(DIRECTORY).join("objects").join(hash)
}

fn active_path(vault: &Path) -> std::path::PathBuf {
    vault.join(DIRECTORY).join("roots/active")
}

fn candidate_path(vault: &Path) -> std::path::PathBuf {
    vault.join(DIRECTORY).join("roots/candidate")
}

fn assert_stored_limit(vault: &Path) {
    let storage = Arc::new(PersonalVaultStorage::open(vault, None).expect("reopen vault"));
    assert!(matches!(
        FixtureObservationStoreV1::open_fixture(storage),
        Err(ObservationStoreError::CorruptStore {
            category: CorruptionCategory::StoredResourceLimit,
        }),
    ));
}

fn started(chain: &GoldenChain) -> FixtureStructuralCommitV1 {
    FixtureStructuralCommitV1 {
        event: FixtureStoredEventV1::Started(chain.started_event.clone()),
        segment: chain.started_segment.clone(),
        current_view: chain.open_view.clone(),
        root: chain.started_root.clone(),
    }
}

fn terminal(chain: &GoldenChain) -> FixtureStructuralCommitV1 {
    FixtureStructuralCommitV1 {
        event: FixtureStoredEventV1::Terminal(chain.terminal_event.clone()),
        segment: chain.terminal_segment.clone(),
        current_view: chain.terminal_view.clone(),
        root: chain.terminal_root.clone(),
    }
}

#[test]
fn architecture_wp2_store_rebuilds_structural_head_after_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = directory.path().join("vault");
    let chain = golden_chain();
    let store = open(&vault);
    let genesis = store.structural_state().expect("genesis state");
    assert_eq!(genesis.root_sha256, GENESIS_ROOT_SHA);
    assert_eq!((genesis.generation, genesis.event_watermark), (0, 0));
    let committed = store.commit_structural(started(&chain)).expect("started");
    assert_eq!(committed.root_sha256, STARTED_ROOT_SHA);
    drop(store);
    let reopened = open(&vault);
    let rebuilt = reopened.structural_state().expect("rebuilt state");
    assert_eq!(rebuilt.root_sha256, STARTED_ROOT_SHA);
    assert_eq!((rebuilt.generation, rebuilt.event_watermark), (1, 1));
}

#[test]
fn architecture_wp2_store_never_promotes_pre_exchange_candidate() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = directory.path().join("vault");
    let chain = golden_chain();
    let store = open(&vault);
    store.commit_structural(started(&chain)).expect("started");
    store.inject_pre_exchange_failure_once();
    assert!(matches!(
        store.commit_structural(terminal(&chain)),
        Err(ObservationStoreError::StorageUnavailable),
    ));
    assert_eq!(
        store.structural_state().expect("authoritative active").root_sha256,
        STARTED_ROOT_SHA,
    );
    let active: FixtureActivePointerV1 =
        parse_canonical(&fs::read(active_path(&vault)).expect("active pointer"))
            .expect("parse active pointer");
    let candidate: FixtureActivePointerV1 =
        parse_canonical(&fs::read(candidate_path(&vault)).expect("candidate pointer"))
            .expect("parse candidate pointer");
    assert_eq!(active.root_sha256, STARTED_ROOT_SHA);
    assert_eq!(candidate.root_sha256, TERMINAL_ROOT_SHA);
    drop(store);
    assert_eq!(
        open(&vault).structural_state().expect("reopen").root_sha256,
        STARTED_ROOT_SHA,
    );
}

#[test]
fn architecture_wp2_store_retries_only_recomputed_exact_genesis() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = directory.path().join("vault");
    let owner = Arc::new(PersonalVaultStorage::open(&vault, None).expect("vault"));
    let storage = ExecutionObservationFixtureStorage::open(owner).expect("sealed storage");
    let view = FixtureCurrentViewV1 {
        schema: CURRENT_VIEW_SCHEMA.into(),
        attestation_state: ATTESTATION_STATE.into(),
        generation: 0,
        event_watermark: 0,
        attempts: Vec::new(),
    };
    let root = FixtureLedgerRootV1 {
        schema: ROOT_SCHEMA.into(),
        trust_class: TRUST_CLASS.into(),
        generation: 0,
        previous_root_sha256: None,
        event_segment_head_sha256: None,
        event_watermark: 0,
        current_view_sha256: GENESIS_VIEW_SHA.into(),
        committed_at_ms: 0,
    };
    let pointer = FixtureActivePointerV1 {
        schema: POINTER_SCHEMA.into(),
        root_sha256: GENESIS_ROOT_SHA.into(),
    };
    storage
        .put_immutable_bounded(
            GENESIS_VIEW_SHA,
            &to_canonical_vec(&view).expect("genesis view"),
            super::CURRENT_VIEW_MAX_BYTES as u64,
        )
        .expect("store genesis view");
    storage
        .put_immutable_bounded(
            GENESIS_ROOT_SHA,
            &to_canonical_vec(&root).expect("genesis root"),
            ROOT_MAX_BYTES as u64,
        )
        .expect("store genesis root");
    storage.inject_pre_exchange_failure_once();
    assert!(storage
        .publish_active(&to_canonical_vec(&pointer).expect("genesis pointer"))
        .is_err());
    assert_eq!(storage.read_active_bounded(POINTER_MAX_BYTES as u64).unwrap(), None);
    let candidate: FixtureActivePointerV1 = parse_canonical(
        &storage
            .read_candidate_bounded(POINTER_MAX_BYTES as u64)
            .unwrap()
            .expect("prepared genesis candidate"),
    )
    .expect("parse candidate");
    assert_eq!(candidate.root_sha256, GENESIS_ROOT_SHA);
    drop(storage);

    let reopened = open(&vault);
    assert_eq!(
        reopened.structural_state().expect("exact genesis retry").root_sha256,
        GENESIS_ROOT_SHA,
    );
    let active: FixtureActivePointerV1 =
        parse_canonical(&fs::read(active_path(&vault)).expect("active genesis"))
            .expect("parse active genesis");
    assert_eq!(active.root_sha256, GENESIS_ROOT_SHA);
}

#[test]
fn architecture_wp2_store_poison_follows_exchange_uncertainty() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = directory.path().join("vault");
    let chain = golden_chain();
    let store = open(&vault);
    store.commit_structural(started(&chain)).expect("started");
    store.inject_post_exchange_sync_failure_once();
    assert!(matches!(
        store.commit_structural(terminal(&chain)),
        Err(ObservationStoreError::CommitIndeterminate),
    ));
    assert!(matches!(
        store.structural_state(),
        Err(ObservationStoreError::Poisoned),
    ));
    drop(store);
    assert_eq!(
        open(&vault).structural_state().expect("reconcile").root_sha256,
        TERMINAL_ROOT_SHA,
    );
}

#[test]
fn architecture_wp2_store_rejects_oversized_pointer_before_parse() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = directory.path().join("vault");
    drop(open(&vault));
    fs::write(active_path(&vault), vec![b'x'; POINTER_MAX_BYTES + 1]).expect("oversize pointer");
    assert_stored_limit(&vault);
}

#[test]
fn architecture_wp2_store_rejects_oversized_root_before_parse() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = directory.path().join("vault");
    drop(open(&vault));
    fs::write(object_path(&vault, GENESIS_ROOT_SHA), vec![b'x'; ROOT_MAX_BYTES + 1])
        .expect("oversize root");
    assert_stored_limit(&vault);
}

#[test]
fn architecture_wp2_store_rejects_oversized_segment_before_parse() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = directory.path().join("vault");
    let chain = golden_chain();
    let store = open(&vault);
    store.commit_structural(started(&chain)).expect("started");
    drop(store);
    fs::write(object_path(&vault, STARTED_SEGMENT_SHA), vec![b'x'; SEGMENT_MAX_BYTES + 1])
        .expect("oversize segment");
    assert_stored_limit(&vault);
}

#[test]
fn architecture_wp2_store_rejects_oversized_event_before_parse() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = directory.path().join("vault");
    let chain = golden_chain();
    let store = open(&vault);
    store.commit_structural(started(&chain)).expect("started");
    drop(store);
    fs::write(
        object_path(&vault, STARTED_EVENT_SHA),
        vec![b'x'; STORED_EVENT_MAX_BYTES + 1],
    )
    .expect("oversize event");
    assert_stored_limit(&vault);
}

#[test]
fn architecture_wp2_store_maps_stored_semantic_limit_to_corruption() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = directory.path().join("vault");
    drop(open(&vault));
    let mut root: FixtureLedgerRootV1 = parse_canonical(
        &fs::read(object_path(&vault, GENESIS_ROOT_SHA)).expect("genesis root"),
    )
    .expect("parse genesis");
    root.generation = 20_001;
    root.event_watermark = 20_001;
    let root_bytes = to_canonical_vec(&root).expect("canonical root");
    let root_sha = hash::root_sha256(&root).expect("root hash");
    fs::write(object_path(&vault, &root_sha), root_bytes).expect("stored invalid root");
    let pointer = FixtureActivePointerV1 {
        schema: POINTER_SCHEMA.into(),
        root_sha256: root_sha,
    };
    fs::write(
        active_path(&vault),
        to_canonical_vec(&pointer).expect("canonical pointer"),
    )
    .expect("publish invalid pointer fixture");
    assert_stored_limit(&vault);
}

#[test]
fn architecture_wp2_store_maps_stored_schema_and_chain_failures_to_corruption() {
    let directory = tempfile::tempdir().expect("tempdir");
    let schema_vault = directory.path().join("schema-vault");
    drop(open(&schema_vault));
    let mut root: FixtureLedgerRootV1 = parse_canonical(
        &fs::read(object_path(&schema_vault, GENESIS_ROOT_SHA)).expect("genesis root"),
    )
    .expect("parse genesis");
    root.schema = "plico.execution-observation.unsupported/v1".into();
    let root_sha = hash::root_sha256(&root).expect("root hash");
    fs::write(
        object_path(&schema_vault, &root_sha),
        to_canonical_vec(&root).expect("canonical root"),
    )
    .expect("stored unsupported root");
    let pointer = FixtureActivePointerV1 {
        schema: POINTER_SCHEMA.into(),
        root_sha256: root_sha,
    };
    fs::write(
        active_path(&schema_vault),
        to_canonical_vec(&pointer).expect("canonical pointer"),
    )
    .expect("publish unsupported root fixture");
    let owner = Arc::new(PersonalVaultStorage::open(&schema_vault, None).expect("vault"));
    assert!(matches!(
        FixtureObservationStoreV1::open_fixture(owner),
        Err(ObservationStoreError::CorruptStore {
            category: CorruptionCategory::UnsupportedStoredSchema,
        }),
    ));

    let chain_vault = directory.path().join("chain-vault");
    let chain = golden_chain();
    let store = open(&chain_vault);
    store.commit_structural(started(&chain)).expect("started");
    drop(store);
    let mut root: FixtureLedgerRootV1 = parse_canonical(
        &fs::read(object_path(&chain_vault, STARTED_ROOT_SHA)).expect("started root"),
    )
    .expect("parse started root");
    root.previous_root_sha256 = None;
    let root_sha = hash::root_sha256(&root).expect("root hash");
    fs::write(
        object_path(&chain_vault, &root_sha),
        to_canonical_vec(&root).expect("canonical root"),
    )
    .expect("stored broken root");
    let pointer = FixtureActivePointerV1 {
        schema: POINTER_SCHEMA.into(),
        root_sha256: root_sha,
    };
    fs::write(
        active_path(&chain_vault),
        to_canonical_vec(&pointer).expect("canonical pointer"),
    )
    .expect("publish broken root fixture");
    let owner = Arc::new(PersonalVaultStorage::open(&chain_vault, None).expect("vault"));
    assert!(matches!(
        FixtureObservationStoreV1::open_fixture(owner),
        Err(ObservationStoreError::CorruptStore {
            category: CorruptionCategory::BrokenRootChain,
        }),
    ));
}

#[test]
fn architecture_wp2_store_rejects_invalid_candidate_without_promotion() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = directory.path().join("vault");
    drop(open(&vault));
    let active = fs::read(active_path(&vault)).expect("active bytes");
    fs::write(candidate_path(&vault), b"{}").expect("invalid candidate");
    let storage = Arc::new(PersonalVaultStorage::open(&vault, None).expect("reopen vault"));
    assert!(matches!(
        FixtureObservationStoreV1::open_fixture(storage),
        Err(ObservationStoreError::CorruptStore {
            category: CorruptionCategory::InvalidCandidateState,
        }),
    ));
    assert_eq!(fs::read(active_path(&vault)).expect("active remains"), active);
}

#[test]
fn architecture_wp2_store_rejects_non_private_topology_without_repair() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = directory.path().join("vault");
    drop(PersonalVaultStorage::open(&vault, None).expect("create vault"));
    let observation = vault.join(DIRECTORY);
    fs::create_dir(&observation).expect("observation directory");
    fs::create_dir(observation.join("objects")).expect("objects");
    fs::create_dir(observation.join("roots")).expect("roots");
    fs::write(observation.join("roots/active"), []).expect("active");
    fs::write(observation.join("roots/candidate"), []).expect("candidate");
    fs::set_permissions(&observation, fs::Permissions::from_mode(0o755)).expect("mode");
    let storage = Arc::new(PersonalVaultStorage::open(&vault, None).expect("reopen vault"));
    assert!(matches!(
        FixtureObservationStoreV1::open_fixture(storage),
        Err(ObservationStoreError::StorageUnavailable),
    ));
    assert_eq!(
        fs::metadata(&observation).expect("metadata").permissions().mode() & 0o777,
        0o755,
    );
}

#[test]
fn architecture_wp2_cas_collision_read_is_bounded_and_nonmutating() {
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = directory.path().join("vault");
    let owner = Arc::new(PersonalVaultStorage::open(&vault, None).expect("vault"));
    let storage = ExecutionObservationFixtureStorage::open(owner).expect("sealed storage");
    let hash = "a".repeat(64);
    storage.put_immutable_bounded(&hash, b"x", 1).expect("first put");
    fs::write(object_path(&vault, &hash), b"xx").expect("oversized collision fixture");
    assert_eq!(
        storage
            .put_immutable_bounded(&hash, b"x", 1)
            .expect_err("bounded collision read")
            .kind(),
        std::io::ErrorKind::InvalidData,
    );
    assert_eq!(fs::read(object_path(&vault, &hash)).expect("collision bytes"), b"xx");
}
"""
WP2_EXTERNAL_TEST_NAMES = {
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_cas_collision_read_is_bounded_and_nonmutating",
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_store_maps_stored_semantic_limit_to_corruption",
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_store_maps_stored_schema_and_chain_failures_to_corruption",
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_store_never_promotes_pre_exchange_candidate",
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_store_poison_follows_exchange_uncertainty",
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_store_rejects_invalid_candidate_without_promotion",
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_store_rejects_non_private_topology_without_repair",
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_store_rejects_oversized_event_before_parse",
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_store_rejects_oversized_pointer_before_parse",
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_store_rejects_oversized_root_before_parse",
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_store_rejects_oversized_segment_before_parse",
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_store_rebuilds_structural_head_after_restart",
    "memory::execution_observation::architecture_wp2_store_tests::architecture_wp2_store_retries_only_recomputed_exact_genesis",
}
LISTED_F_TEST = re.compile(
    r"(?m)^(?P<name>\S*execution_observation_f(?P<id>\d{2})_\S*): test$"
)
UUID_TEXT = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
)
SHA_TEXT = re.compile(r"^[0-9a-f]{64}$")
GIT_ENV_EXACT = {
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_REPLACE_REF_BASE",
    "GIT_WORK_TREE",
}
TOOLCHAIN_ENV_EXACT = {
    "CARGO",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_TARGET_DIR",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTDOC",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "RUSTUP_TOOLCHAIN",
}


def _rust_tokens(text: str) -> list[str]:
    """Return identifiers/punctuation while discarding Rust comments and literals.

    This is deliberately a small lexical scanner, not a Rust parser.  It handles
    nested block comments and every Rust string spelling needed to prevent
    whitespace/comment/literal tricks from bypassing the scope deny rules.
    """

    tokens: list[str] = []
    index = 0
    length = len(text)
    while index < length:
        character = text[index]
        if character.isspace():
            index += 1
            continue
        if text.startswith("//", index):
            newline = text.find("\n", index + 2)
            index = length if newline < 0 else newline + 1
            continue
        if text.startswith("/*", index):
            depth = 1
            index += 2
            while index < length and depth:
                if text.startswith("/*", index):
                    depth += 1
                    index += 2
                elif text.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
            if depth:
                raise verify.VerificationError("unterminated Rust block comment")
            continue

        raw_start = index
        if character == "b" and index + 1 < length and text[index + 1] == "r":
            raw_start = index + 1
        if text[raw_start] == "r":
            cursor = raw_start + 1
            while cursor < length and text[cursor] == "#":
                cursor += 1
            if cursor < length and text[cursor] == '"':
                hashes = cursor - raw_start - 1
                terminator = '"' + ("#" * hashes)
                end = text.find(terminator, cursor + 1)
                if end < 0:
                    raise verify.VerificationError("unterminated Rust raw string")
                index = end + len(terminator)
                continue

        if character == "b" and index + 1 < length and text[index + 1] in {'"', "'"}:
            index += 1
            character = text[index]
        if character == '"':
            index += 1
            escaped = False
            while index < length:
                current = text[index]
                index += 1
                if escaped:
                    escaped = False
                elif current == "\\":
                    escaped = True
                elif current == '"':
                    break
            else:
                raise verify.VerificationError("unterminated Rust string")
            continue
        if character == "'":
            # A lifetime is lexed as apostrophe + identifier.  A character
            # literal has a closing apostrophe before whitespace/punctuation.
            cursor = index + 1
            escaped = False
            while cursor < length and cursor - index <= 12:
                current = text[cursor]
                if escaped:
                    escaped = False
                elif current == "\\":
                    escaped = True
                elif current == "'":
                    index = cursor + 1
                    break
                elif current.isspace():
                    break
                cursor += 1
            else:
                cursor = length
            if index > cursor:
                continue
            tokens.append("'")
            index += 1
            continue
        if character.isalpha() or character == "_":
            cursor = index + 1
            while cursor < length and (text[cursor].isalnum() or text[cursor] == "_"):
                cursor += 1
            tokens.append(text[index:cursor])
            index = cursor
            continue
        if text.startswith("::", index):
            tokens.append("::")
            index += 2
            continue
        tokens.append(character)
        index += 1
    return tokens


def _matching_token(tokens: list[str], start: int, opening: str, closing: str) -> int:
    depth = 0
    for index in range(start, len(tokens)):
        if tokens[index] == opening:
            depth += 1
        elif tokens[index] == closing:
            depth -= 1
            if depth == 0:
                return index
    raise verify.VerificationError(f"unterminated Rust token group: {opening}")


def _is_rust_identifier(token: str) -> bool:
    return re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", token) is not None


def _module_depths(path: str, tokens: list[str]) -> list[int]:
    """Return each token's lexical module depth below execution_observation.

    A sibling file such as `canonical.rs` starts one module below the boundary;
    `mod.rs` starts at the boundary. Inline `mod name { ... }` blocks add one.
    This lets relative paths be checked by where they resolve instead of by the
    spelling `super`, which is safe in a child but an escape at the boundary.
    """

    observation_root = PurePosixPath("src/memory/execution_observation")
    try:
        relative = PurePosixPath(path).relative_to(observation_root)
    except ValueError as error:
        raise verify.VerificationError(
            f"Rust source is outside execution_observation: {path}"
        ) from error
    parent_depth = len(relative.parts) - 1
    base_depth = parent_depth if relative.name == "mod.rs" else parent_depth + 1
    module_braces = {
        index + 2
        for index in range(len(tokens) - 2)
        if tokens[index] == "mod"
        and _is_rust_identifier(tokens[index + 1])
        and tokens[index + 2] == "{"
    }
    depths: list[int] = []
    brace_stack: list[bool] = []
    depth = base_depth
    for index, token in enumerate(tokens):
        if token == "}":
            if not brace_stack:
                raise verify.VerificationError(f"unbalanced Rust braces: {path}")
            if brace_stack.pop():
                depth -= 1
        depths.append(depth)
        if token == "{":
            is_module = index in module_braces
            brace_stack.append(is_module)
            if is_module:
                depth += 1
    if brace_stack:
        raise verify.VerificationError(f"unbalanced Rust braces: {path}")
    return depths


def _split_use_branches(tokens: list[str]) -> list[list[str]]:
    branches: list[list[str]] = []
    start = 0
    depth = 0
    for index, token in enumerate(tokens):
        if token == "{":
            depth += 1
        elif token == "}":
            depth -= 1
            if depth < 0:
                raise verify.VerificationError("unbalanced Rust use tree")
        elif token == "," and depth == 0:
            if tokens[start:index]:
                branches.append(tokens[start:index])
            start = index + 1
    if depth:
        raise verify.VerificationError("unbalanced Rust use tree")
    if tokens[start:]:
        branches.append(tokens[start:])
    return branches


def _flat_use_path(tokens: list[str]) -> tuple[str, ...]:
    if not tokens:
        raise verify.VerificationError("empty Rust use path")
    segments: list[str] = []
    expect_segment = True
    for token in tokens:
        if expect_segment:
            if token == "*" or _is_rust_identifier(token):
                segments.append(token)
                expect_segment = False
            else:
                raise verify.VerificationError("unparseable Rust use path")
        elif token == "::":
            expect_segment = True
        else:
            raise verify.VerificationError("unparseable Rust use path")
    if expect_segment or "*" in segments[:-1]:
        raise verify.VerificationError("unparseable Rust use path")
    return tuple(segments)


def _expand_use_tree(
    tokens: list[str], prefix: tuple[str, ...] = ()
) -> list[tuple[str, ...]]:
    """Expand a Rust use tree into fully inherited paths.

    For example `serde::{de::DeserializeOwned, Serialize}` becomes
    `serde::de::DeserializeOwned` and `serde::Serialize`. Alias syntax is
    rejected before expansion so every introduced name retains provenance.
    """

    if "as" in tokens:
        raise verify.VerificationError("aliased Rust use item is forbidden")
    depth = 0
    group_start: int | None = None
    for index, token in enumerate(tokens):
        if token == "{":
            if depth == 0:
                group_start = index
                break
            depth += 1
        elif token == "}":
            depth -= 1
    if group_start is None:
        return [prefix + _flat_use_path(tokens)]
    group_end = _matching_token(tokens, group_start, "{", "}")
    if group_end != len(tokens) - 1:
        raise verify.VerificationError("tokens after Rust use group are forbidden")
    head = tokens[:group_start]
    if head and head[-1] == "::":
        head = head[:-1]
    inherited = prefix + (_flat_use_path(head) if head else ())
    branches = _split_use_branches(tokens[group_start + 1 : group_end])
    if not branches:
        raise verify.VerificationError("empty Rust use group")
    expanded: list[tuple[str, ...]] = []
    for branch in branches:
        expanded.extend(_expand_use_tree(branch, inherited))
    return expanded


def _validate_capability_path(
    path: tuple[str, ...], *, module_depth: int, source_path: str
) -> None:
    if not path:
        raise verify.VerificationError(f"empty Rust path: {source_path}")
    super_count = 0
    while super_count < len(path) and path[super_count] == "super":
        super_count += 1
    if super_count:
        if super_count > module_depth:
            raise verify.VerificationError(
                f"relative path escapes execution_observation: {source_path}"
            )
        return
    root = path[0]
    if root == "self" or root in OBSERVATION_MODULES:
        return
    is_store_test = source_path == "src/memory/execution_observation/store/tests.rs"
    if root == "tempfile" and is_store_test:
        return
    sealed_cas_capability = path == (
        "crate",
        "cas",
        "execution_observation_store",
        "ExecutionObservationFixtureStorage",
    )
    frozen_cas_types = (
        len(path) == 3
        and path[1] == "cas"
        and path[2]
        in {"LedgerStorageError", "LedgerStorageOpenError", "PersonalVaultStorage"}
    )
    if root == "crate" and (sealed_cas_capability or frozen_cas_types):
        return
    if root == "crate":
        raise verify.VerificationError(
            f"crate dependency escapes the WP2 observation boundary: {source_path}"
        )
    if root not in PURE_EXTERNAL_ROOTS:
        raise verify.VerificationError(
            f"unapproved dependency root {root!r}: {source_path}"
        )
    if root in {"std", "core"}:
        if len(path) < 2 or path[1] not in PURE_STD_MODULES:
            raise verify.VerificationError(
                f"non-pure {root} capability is forbidden: {source_path}"
            )
    if "*" in path:
        raise verify.VerificationError(
            f"external glob import is forbidden: {source_path}"
        )


def _qualified_path(tokens: list[str], start: int) -> tuple[str, ...]:
    segments = [tokens[start]]
    cursor = start + 1
    while cursor + 1 < len(tokens) and tokens[cursor] == "::":
        segment = tokens[cursor + 1]
        if not (_is_rust_identifier(segment) or segment == "*"):
            break
        segments.append(segment)
        cursor += 2
    return tuple(segments)


def _count_token_sequence(tokens: list[str], sequence: list[str]) -> int:
    if not sequence or len(sequence) > len(tokens):
        return 0
    return sum(
        tokens[index : index + len(sequence)] == sequence
        for index in range(len(tokens) - len(sequence) + 1)
    )


def _scan_rust_tokens(path: str, text: str, *, observation: bool) -> list[str]:
    tokens = _rust_tokens(text)
    module_depths = _module_depths(path, tokens)
    forbidden_macros = {
        "cfg",
        "env",
        "include",
        "include_bytes",
        "include_str",
        "macro_rules",
        "option_env",
    }
    use_ranges: set[int] = set()
    imported_bindings: set[str] = set()
    if observation:
        for index, token in enumerate(tokens):
            if token != "use":
                continue
            try:
                statement_end = tokens.index(";", index + 1)
            except ValueError as error:
                raise verify.VerificationError(
                    f"unterminated Rust use item: {path}"
                ) from error
            use_ranges.update(range(index, statement_end + 1))
            statement = tokens[index + 1 : statement_end]
            if statement[:2] == ["::", "crate"]:
                statement = statement[1:]
            expanded = _expand_use_tree(statement)
            for imported_path in expanded:
                _validate_capability_path(
                    imported_path,
                    module_depth=module_depths[index],
                    source_path=path,
                )
                binding = imported_path[-1]
                if binding not in {"*", "self", "super"}:
                    imported_bindings.add(binding)
    for index, token in enumerate(tokens):
        if (
            token in forbidden_macros
            and index + 1 < len(tokens)
            and tokens[index + 1] == "!"
        ):
            raise verify.VerificationError(
                f"macro/environment side door is forbidden: {path}"
            )
        if token == "#" and index + 1 < len(tokens) and tokens[index + 1] == "[":
            end = _matching_token(tokens, index + 1, "[", "]")
            attribute = tokens[index + 2 : end]
            if attribute and attribute[0] in {
                "cfg",
                "cfg_attr",
                "path",
                "macro_export",
            }:
                allowed_attributes = [["cfg", "(", "test", ")"]]
                if path == "src/memory/execution_observation/store/tests.rs":
                    allowed_attributes.extend(
                        [
                            ["cfg", "(", "unix", ")"],
                            ["cfg", "(", "not", "(", "unix", ")", ")"],
                        ]
                    )
                if attribute not in allowed_attributes:
                    raise verify.VerificationError(
                        f"conditional/path/macro attribute is forbidden: {path}"
                    )
        if not observation:
            continue
        if index in use_ranges:
            continue
        if (
            token == "extern"
            and index + 1 < len(tokens)
            and tokens[index + 1] == "crate"
        ):
            raise verify.VerificationError(
                f"extern crate is forbidden in observation source: {path}"
            )
        if token == "unsafe":
            raise verify.VerificationError(f"unsafe code is forbidden: {path}")
        if token in RUST_INTEGER_PATH_ROOTS and tokens[index + 1 : index + 3] == [
            "::",
            "MAX",
        ]:
            raise verify.VerificationError(
                f"unbounded {token}::MAX capability is forbidden: {path}"
            )
        if token == "pub":
            if index + 1 >= len(tokens) or tokens[index + 1] != "(":
                raise verify.VerificationError(
                    f"plain public export is forbidden in observation module: {path}"
                )
            end = _matching_token(tokens, index + 1, "(", ")")
            visibility = tokens[index + 2 : end]
            if visibility not in (["crate"], ["super"]):
                raise verify.VerificationError(
                    f"only pub(crate)/pub(super) visibility is allowed: {path}"
                )
            if path.startswith(
                "src/memory/execution_observation/store/"
            ) and visibility != ["super"]:
                raise verify.VerificationError(
                    f"WP2 store visibility must remain pub(super): {path}"
                )
        if not _is_rust_identifier(token) or index + 1 >= len(tokens):
            continue
        if tokens[index + 1] != "::":
            continue
        if index + 2 >= len(tokens) or not (
            _is_rust_identifier(tokens[index + 2]) or tokens[index + 2] == "*"
        ):
            # Turbofish (`function::<T>`) is not a module/capability path.
            continue
        if index > 0 and tokens[index - 1] == "::":
            if index > 1 and _is_rust_identifier(tokens[index - 2]):
                continue
        if not (token[0].islower() or token in {"self", "super", "crate"}):
            continue
        qualified = _qualified_path(tokens, index)
        root = qualified[0]
        if root in imported_bindings or root in RUST_PRIMITIVE_PATH_ROOTS:
            continue
        _validate_capability_path(
            qualified,
            module_depth=module_depths[index],
            source_path=path,
        )

    return tokens


def _verify_wp1_memory_module_anchor(repo: Path, base: str, candidate: str) -> None:
    """Require the sole memory module change to be one crate-private declaration."""

    module_path = "src/memory/mod.rs"
    base_mode, _, base_bytes = verify.git_object(repo, base, module_path)
    candidate_mode, _, candidate_bytes = verify.git_object(repo, candidate, module_path)
    if base_mode != "100644" or candidate_mode != "100644":
        raise verify.VerificationError(
            "WP1 memory module anchor must remain a regular 100644 Git blob"
        )
    anchor = b"pub(crate) mod execution_observation;\n"
    if anchor in base_bytes:
        raise verify.VerificationError(
            "WP1 memory module anchor already exists in the approved scope base"
        )
    candidate_lines = candidate_bytes.splitlines(keepends=True)
    if candidate_lines.count(anchor) != 1:
        raise verify.VerificationError(
            "WP1 must add exactly one crate-private execution_observation module anchor"
        )
    without_anchor = b"".join(line for line in candidate_lines if line != anchor)
    if without_anchor != base_bytes:
        raise verify.VerificationError(
            "WP1 src/memory/mod.rs changed beyond the exact crate-private module anchor"
        )


def _verify_wp2_module_anchor(repo: Path, base: str, candidate: str) -> None:
    """Permit only the checkpoint-frozen private observation-store activation."""

    memory_path = "src/memory/execution_observation/mod.rs"
    _, _, memory_base = verify.git_object(repo, base, memory_path)
    _, _, memory_candidate = verify.git_object(repo, candidate, memory_path)
    anchor_after = b"pub(crate) mod model;\n"
    store_anchor = b"mod store;\n"
    if memory_base.count(anchor_after) != 1 or store_anchor in memory_base:
        raise verify.VerificationError(
            "WP2 observation base cannot accept the frozen private store anchor"
        )
    memory_expected = memory_base.replace(
        anchor_after,
        anchor_after + store_anchor,
        1,
    )
    if memory_candidate != memory_expected:
        raise verify.VerificationError(
            "WP2 observation mod.rs differs from the exact private store anchor"
        )


@contextmanager
def _sanitized_git_environment():
    affected = {
        key: value for key, value in os.environ.items() if key.startswith("GIT_")
    }
    for key in list(os.environ):
        if key.startswith("GIT_"):
            os.environ.pop(key, None)
    os.environ.update({"GIT_NO_LAZY_FETCH": "1", "GIT_NO_REPLACE_OBJECTS": "1"})
    try:
        yield
    finally:
        for key in list(os.environ):
            if key.startswith("GIT_"):
                os.environ.pop(key, None)
        os.environ.update(affected)


def _scope_git_environment() -> dict[str, str]:
    return {
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_NO_LAZY_FETCH": "1",
        "GIT_NO_REPLACE_OBJECTS": "1",
        "GIT_OPTIONAL_LOCKS": "0",
        "GIT_PAGER": "cat",
        "GIT_TERMINAL_PROMPT": "0",
        "LANG": "C",
        "LC_ALL": "C",
        "PATH": os.defpath,
    }


@contextmanager
def _absolute_git_runner(git_path: Path):
    """Make every verifier Git call use the packet-bound absolute executable."""

    original = verify.run_git

    def run_git_absolute(
        repo: Path,
        args: list[str],
        *,
        input_bytes: bytes | None = None,
        git_executable: Path | None = None,
    ) -> bytes:
        if git_executable is not None:
            try:
                requested = Path(git_executable).resolve(strict=True)
                frozen = git_path.resolve(strict=True)
            except OSError as error:
                raise verify.VerificationError(
                    f"Git executable identity cannot be resolved: {error}"
                ) from error
            if requested != frozen:
                raise verify.VerificationError(
                    "Git executable differs from the packet-frozen scope tool"
                )
        try:
            result = subprocess.run(
                [
                    os.fspath(git_path),
                    "--no-pager",
                    "--no-replace-objects",
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.untrackedCache=false",
                    "-c",
                    "core.preloadIndex=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-C",
                    os.fspath(repo),
                    *args,
                ],
                env=_scope_git_environment(),
                input=input_bytes,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
                timeout=120,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise verify.VerificationError(
                f"cannot execute frozen git: {error}"
            ) from error
        if result.returncode != 0:
            detail = (
                result.stderr.decode("utf-8", errors="replace").strip().splitlines()
            )
            suffix = detail[-1] if detail else "unknown git failure"
            raise verify.VerificationError(f"git {' '.join(args[:2])} failed: {suffix}")
        return result.stdout

    verify.run_git = run_git_absolute
    try:
        yield
    finally:
        verify.run_git = original


def _hardened_tool_environment(cargo_path: Path) -> dict[str, str]:
    environment = os.environ.copy()
    for key in list(environment):
        if (
            key in TOOLCHAIN_ENV_EXACT
            or key.startswith("CARGO_ALIAS_")
            or key.startswith("CARGO_BUILD_")
            or key.startswith("DYLD_")
        ):
            environment.pop(key, None)
    environment["PATH"] = os.pathsep.join(
        (os.fspath(cargo_path.parent), "/usr/bin", "/bin")
    )
    environment.update({"EMBEDDING_BACKEND": "stub", "LLM_BACKEND": "stub"})
    return environment


@contextmanager
def _isolated_candidate_environment(base: dict[str, str]):
    """Use an alias-free Cargo home and private build/temp directories.

    `HOME` is intentionally replaced, but rustup's installed, packet-observed
    toolchain remains a read-only runtime input. Pin its existing home
    explicitly so rustup never interprets the private HOME as a request to
    install a missing toolchain (which would violate the offline gate).
    """

    with tempfile.TemporaryDirectory(prefix="plico-v53-execution-") as temporary:
        root = Path(temporary)
        home = root / "home"
        cargo_home = home / ".cargo"
        temp = root / "tmp"
        target = root / "target"
        for directory in (home, cargo_home, temp, target):
            directory.mkdir(mode=0o700, exist_ok=True)
        original_cargo_home = Path(
            os.environ.get("CARGO_HOME", os.fspath(Path.home() / ".cargo"))
        )
        original_rustup_home = Path(
            os.environ.get("RUSTUP_HOME", os.fspath(Path.home() / ".rustup"))
        ).resolve(strict=True)
        if not original_rustup_home.is_dir():
            raise verify.VerificationError("installed rustup home is unavailable")
        for cache_name in ("git", "registry"):
            cache = original_cargo_home / cache_name
            if cache.is_dir():
                (cargo_home / cache_name).symlink_to(cache, target_is_directory=True)
        environment = base.copy()
        environment.update(
            {
                "CARGO_HOME": os.fspath(cargo_home),
                "CARGO_NET_OFFLINE": "true",
                "CARGO_TARGET_DIR": os.fspath(target),
                "HOME": os.fspath(home),
                "PYTHONDONTWRITEBYTECODE": "1",
                "RUSTUP_HOME": os.fspath(original_rustup_home),
                "TMPDIR": os.fspath(temp),
            }
        )
        yield environment, root


def _resolve_frozen_cargo(
    spec: dict[str, object], observed: dict[str, object]
) -> dict[str, object]:
    located = shutil.which("cargo")
    if located is None:
        raise verify.VerificationError("cargo is absent from PATH")
    cargo_path = Path(located).absolute()
    realpath = cargo_path.resolve(strict=True)
    info = realpath.stat()
    if (
        not stat.S_ISREG(info.st_mode)
        or info.st_uid != os.geteuid()
        or info.st_mode & 0o022
    ):
        raise verify.VerificationError(
            "cargo target must be current-owner, regular, and non-writable by group/other"
        )
    cargo_bytes = realpath.read_bytes()
    cargo_digest = verify.sha256_bytes(cargo_bytes)
    sealed_cargo = observed["cargo"]
    if cargo_digest != sealed_cargo["launcher_sha256"]:
        raise verify.VerificationError("cargo launcher content differs from WP2 packet")
    environment = _hardened_tool_environment(cargo_path)
    for name in ("cargo", "cargo_llvm_cov", "rustc", "git"):
        current = verify._observe_tool(
            name,
            spec["toolchain"][name],
            None,
            environment=environment,
        )
        if current != observed[name]:
            raise verify.VerificationError(
                f"frozen logical version/content identity mismatch: {name}"
            )
    rustup = verify._tool_launcher("rustup", None)
    resolved = subprocess.run(
        [os.fspath(rustup), "which", "cargo", "--toolchain", "1.95.0"],
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
        timeout=30,
    )
    resolved_path = Path(resolved.stdout.decode("utf-8", errors="replace").strip())
    if resolved.returncode != 0 or not resolved_path.is_absolute():
        raise verify.VerificationError("resolved cargo 1.95.0 lookup failed")
    resolved_realpath = resolved_path.resolve(strict=True)
    sealed_resolved = sealed_cargo["resolved_tool"]
    if verify.sha256_bytes(resolved_realpath.read_bytes()) != sealed_resolved["sha256"]:
        raise verify.VerificationError(
            "resolved cargo 1.95.0 identity differs from WP2 packet"
        )
    if verify.sha256_bytes(realpath.read_bytes()) != cargo_digest:
        raise verify.VerificationError(
            "cargo executable changed during identity verification"
        )
    cov_located = shutil.which("cargo-llvm-cov", path=environment["PATH"])
    if cov_located is None:
        raise verify.VerificationError("cargo-llvm-cov executable is absent")
    cov_path = Path(cov_located).absolute()
    cov_realpath = cov_path.resolve(strict=True)
    cov_digest = verify.sha256_bytes(cov_realpath.read_bytes())
    sealed_cov = observed["cargo_llvm_cov"]["resolved_tool"]
    if cov_digest != sealed_cov["sha256"]:
        raise verify.VerificationError(
            "resolved cargo-llvm-cov identity differs from WP2 packet"
        )
    git_path = verify._tool_launcher(spec["toolchain"]["git"]["command"][0], None)
    git_realpath = git_path.resolve(strict=True)
    git_digest = verify.sha256_bytes(git_realpath.read_bytes())
    return {
        "cargo_path": cargo_path,
        "cargo_realpath": realpath,
        "cargo_sha256": cargo_digest,
        "environment": environment,
        "resolved_cargo_path": resolved_path,
        "resolved_cargo_realpath": resolved_realpath,
        "resolved_cargo_sha256": sealed_resolved["sha256"],
        "cargo_llvm_cov_path": cov_path,
        "cargo_llvm_cov_realpath": cov_realpath,
        "cargo_llvm_cov_sha256": cov_digest,
        "git_path": git_path,
        "git_realpath": git_realpath,
        "git_sha256": git_digest,
    }


def _assert_cargo_unchanged(toolchain: dict[str, object]) -> None:
    cargo_path = toolchain["cargo_path"]
    try:
        realpath = cargo_path.resolve(strict=True)
        digest = verify.sha256_bytes(realpath.read_bytes())
    except OSError as error:
        raise verify.VerificationError(
            f"cargo identity cannot be re-read: {error}"
        ) from error
    if realpath != toolchain["cargo_realpath"] or digest != toolchain["cargo_sha256"]:
        raise verify.VerificationError(
            "cargo realpath/digest changed after it was frozen"
        )
    try:
        resolved_realpath = toolchain["resolved_cargo_path"].resolve(strict=True)
        resolved_digest = verify.sha256_bytes(resolved_realpath.read_bytes())
    except OSError as error:
        raise verify.VerificationError(
            f"resolved cargo identity cannot be re-read: {error}"
        ) from error
    if (
        resolved_realpath != toolchain["resolved_cargo_realpath"]
        or resolved_digest != toolchain["resolved_cargo_sha256"]
    ):
        raise verify.VerificationError(
            "resolved cargo 1.95.0 digest changed after it was frozen"
        )
    try:
        cov_realpath = toolchain["cargo_llvm_cov_path"].resolve(strict=True)
        cov_digest = verify.sha256_bytes(cov_realpath.read_bytes())
    except OSError as error:
        raise verify.VerificationError(
            f"cargo-llvm-cov identity cannot be re-read: {error}"
        ) from error
    if (
        cov_realpath != toolchain["cargo_llvm_cov_realpath"]
        or cov_digest != toolchain["cargo_llvm_cov_sha256"]
    ):
        raise verify.VerificationError(
            "cargo-llvm-cov realpath/digest changed after it was frozen"
        )
    for name in ("git",):
        try:
            current_realpath = toolchain[f"{name}_path"].resolve(strict=True)
            current_digest = verify.sha256_bytes(current_realpath.read_bytes())
        except OSError as error:
            raise verify.VerificationError(
                f"{name} identity cannot be re-read: {error}"
            ) from error
        if (
            current_realpath != toolchain[f"{name}_realpath"]
            or current_digest != toolchain[f"{name}_sha256"]
        ):
            raise verify.VerificationError(
                f"{name} realpath/digest changed after it was frozen"
            )


def _run_bounded_process(
    command: list[str],
    *,
    cwd: Path,
    environment: dict[str, str],
    timeout: int,
) -> subprocess.CompletedProcess[bytes]:
    """Run candidate code with bounded time, output and basic OS resources."""

    def child_limits() -> None:
        os.umask(0o077)
        resource.setrlimit(
            resource.RLIMIT_AS,
            (
                MAX_CANDIDATE_ADDRESS_SPACE_BYTES,
                MAX_CANDIDATE_ADDRESS_SPACE_BYTES,
            ),
        )
        resource.setrlimit(resource.RLIMIT_CORE, (0, 0))

    try:
        with tempfile.TemporaryFile() as output:
            process = subprocess.Popen(
                command,
                cwd=cwd,
                env=environment,
                stdout=output,
                stderr=subprocess.STDOUT,
                start_new_session=True,
                preexec_fn=child_limits,
            )
            deadline = time.monotonic() + timeout
            while process.poll() is None:
                if os.fstat(output.fileno()).st_size > MAX_CANDIDATE_OUTPUT_BYTES:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                    raise verify.VerificationError(
                        "candidate command output exceeded "
                        f"{MAX_CANDIDATE_OUTPUT_BYTES} bytes"
                    )
                if time.monotonic() >= deadline:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                    raise verify.VerificationError(
                        f"candidate command exceeded {timeout} seconds: {command[0]}"
                    )
                time.sleep(0.05)
            output_size = os.fstat(output.fileno()).st_size
            if output_size > MAX_CANDIDATE_OUTPUT_BYTES:
                raise verify.VerificationError(
                    f"candidate command output exceeded {MAX_CANDIDATE_OUTPUT_BYTES} bytes"
                )
            output.seek(0)
            stdout = output.read(MAX_CANDIDATE_OUTPUT_BYTES + 1)
    except OSError as error:
        raise verify.VerificationError(
            f"candidate command could not execute: {error}"
        ) from error
    return subprocess.CompletedProcess(command, process.returncode, stdout, b"")


def _control_file_bytes(path: Path, label: str) -> bytes:
    try:
        info = path.lstat()
    except FileNotFoundError:
        return b""
    except OSError as error:
        raise verify.VerificationError(
            f"cannot inspect Git {label}: {error}"
        ) from error
    if not stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
        raise verify.VerificationError(f"Git {label} must be absent or regular")
    if info.st_size > 1024 * 1024:
        raise verify.VerificationError(f"Git {label} exceeds 1 MiB")
    try:
        data = path.read_bytes()
    except OSError as error:
        raise verify.VerificationError(f"cannot read Git {label}: {error}") from error
    if len(data) != info.st_size:
        raise verify.VerificationError(f"Git {label} changed while reading")
    return data


def _effective_control_lines(data: bytes, label: str) -> list[bytes]:
    lines = []
    for line in data.splitlines():
        stripped = line.strip()
        if stripped and not stripped.startswith(b"#"):
            lines.append(stripped)
    if lines:
        raise verify.VerificationError(f"Git {label} contains active local input")
    return lines


def _dangerous_untracked_path(path: str) -> bool:
    parts = Path(path).parts
    if not parts:
        return True
    if parts[0] in {"src", "tests", "benches", "examples", ".cargo"}:
        return True
    name = parts[-1]
    return (
        name in {"Cargo.lock", "Cargo.toml", "build.rs", "rust-toolchain.toml"}
        or name.endswith(".rs")
        or path.startswith("scripts/milestones/v53/")
    )


def _decode_git_paths(data: bytes, label: str) -> list[str]:
    paths = []
    for raw in data.split(b"\0"):
        if not raw:
            continue
        try:
            path = raw.decode("utf-8", errors="strict")
        except UnicodeDecodeError as error:
            raise verify.VerificationError(f"non-UTF-8 path in {label}") from error
        if any(ord(character) < 32 or ord(character) == 127 for character in path):
            raise verify.VerificationError(f"control character path in {label}")
        paths.append(path)
    return paths


def _audit_repository_metadata(repo: Path) -> str:
    """Reject Git metadata/worktree inputs that can change object interpretation."""

    replacement_refs = verify.run_git(
        repo, ["for-each-ref", "--format=%(refname)", "refs/replace"]
    )
    if replacement_refs.strip():
        raise verify.VerificationError(
            "Git replacement refs are forbidden during scope verification"
        )
    shallow = (
        verify.run_git(repo, ["rev-parse", "--is-shallow-repository"])
        .decode("ascii", errors="strict")
        .strip()
    )
    if shallow != "false":
        raise verify.VerificationError("shallow repositories are forbidden")
    sparse = (
        verify.run_git(
            repo, ["config", "--bool", "--default", "false", "core.sparseCheckout"]
        )
        .decode("ascii", errors="strict")
        .strip()
    )
    if sparse != "false":
        raise verify.VerificationError("sparse checkout is forbidden")

    config_names_raw = verify.run_git(
        repo, ["config", "--local", "--null", "--name-only", "--list"]
    )
    try:
        config_names = [
            name.decode("utf-8", errors="strict").lower()
            for name in config_names_raw.split(b"\0")
            if name
        ]
    except UnicodeDecodeError as error:
        raise verify.VerificationError(
            "local Git config contains non-UTF-8 keys"
        ) from error
    dangerous_exact = {
        "core.attributesfile",
        "core.excludesfile",
        "core.fsmonitor",
        "core.preloadindex",
        "core.untrackedcache",
        "extensions.partialclone",
    }
    for name in config_names:
        if (
            name in dangerous_exact
            or name.startswith("include.")
            or name.startswith("includeif.")
            or name.endswith(".promisor")
            or name.endswith(".partialclonefilter")
        ):
            raise verify.VerificationError(
                f"dangerous local Git config is forbidden: {name}"
            )
    config_bytes = verify.run_git(repo, ["config", "--local", "--null", "--list"])

    common_text = (
        verify.run_git(repo, ["rev-parse", "--git-common-dir"])
        .decode("utf-8", errors="strict")
        .strip()
    )
    common_path = Path(common_text)
    if not common_path.is_absolute():
        common_path = repo / common_path
    try:
        common_path = common_path.resolve(strict=True)
    except OSError as error:
        raise verify.VerificationError(
            f"Git common directory cannot be resolved: {error}"
        ) from error
    controls = {
        "grafts": _control_file_bytes(common_path / "info/grafts", "grafts"),
        "info/exclude": _control_file_bytes(
            common_path / "info/exclude", "info/exclude"
        ),
        "object alternates": _control_file_bytes(
            common_path / "objects/info/alternates", "object alternates"
        ),
    }
    for label, data in controls.items():
        _effective_control_lines(data, label)
    if any((common_path / "objects/pack").glob("*.promisor")):
        raise verify.VerificationError("promisor pack files are forbidden")

    flags = verify.run_git(repo, ["ls-files", "-v", "-z"])
    for record in flags.split(b"\0"):
        if not record:
            continue
        if len(record) < 3 or record[1:2] != b" ":
            raise verify.VerificationError("malformed Git index flag output")
        tag = record[:1]
        if tag == b"S" or (b"a" <= tag <= b"z"):
            raise verify.VerificationError(
                "assume-unchanged/skip-worktree index flags are forbidden"
            )

    try:
        (repo / ".cargo").lstat()
    except FileNotFoundError:
        pass
    except OSError as error:
        raise verify.VerificationError(
            f"cannot inspect repository .cargo: {error}"
        ) from error
    else:
        raise verify.VerificationError("repository-local .cargo input is forbidden")

    untracked = _decode_git_paths(
        verify.run_git(repo, ["ls-files", "--others", "--exclude-standard", "-z"]),
        "untracked inventory",
    )
    ignored = _decode_git_paths(
        verify.run_git(
            repo,
            ["ls-files", "--others", "--ignored", "--exclude-standard", "-z"],
        ),
        "ignored inventory",
    )
    dangerous = sorted(
        path for path in {*untracked, *ignored} if _dangerous_untracked_path(path)
    )
    if dangerous:
        raise verify.VerificationError(
            f"dangerous ignored/untracked repository input is present: {dangerous[0]}"
        )

    fingerprint_parts = [config_bytes]
    for label in sorted(controls):
        fingerprint_parts.extend((label.encode("utf-8"), controls[label]))
    return verify.sha256_bytes(b"\0".join(fingerprint_parts))


def _assert_execution_seal(
    repo: Path, candidate: str, toolchain: dict[str, object]
) -> None:
    _assert_cargo_unchanged(toolchain)
    fingerprint = _audit_repository_metadata(repo)
    if fingerprint != toolchain.get("repository_metadata_fingerprint"):
        raise verify.VerificationError(
            "repository metadata changed during candidate execution"
        )
    if verify.resolve_commit(repo, "HEAD") != candidate:
        raise verify.VerificationError("candidate HEAD changed during execution")
    verify.git_status_clean(repo)


def _parse_name_status(data: bytes) -> list[tuple[str, str]]:
    fields = data.split(b"\0")
    if fields and fields[-1] == b"":
        fields.pop()
    if len(fields) % 2:
        raise verify.VerificationError("malformed NUL-delimited Git name-status output")
    changes: list[tuple[str, str]] = []
    for index in range(0, len(fields), 2):
        try:
            status = fields[index].decode("ascii", errors="strict")
            path = fields[index + 1].decode("utf-8", errors="strict")
        except UnicodeDecodeError as error:
            raise verify.VerificationError(
                "non-canonical path/status in Git diff"
            ) from error
        if (
            not status
            or not path
            or any(ord(character) < 32 or ord(character) == 127 for character in path)
        ):
            raise verify.VerificationError("empty/control path in Git diff")
        changes.append((status, path))
    return changes


def _is_allowed(path: str, scope: dict[str, object]) -> bool:
    if path in scope["allowed_exact"]:
        return True
    if any(path.startswith(prefix) for prefix in scope["allowed_prefixes"]):
        return True
    return any(fnmatch.fnmatchcase(path, pattern) for pattern in scope["allowed_globs"])


def _check_repo_checkout(repo: Path, candidate: str, require_clean: bool) -> None:
    sparse = (
        verify.run_git(
            repo, ["config", "--bool", "--default", "false", "core.sparseCheckout"]
        )
        .decode("ascii", errors="strict")
        .strip()
    )
    if sparse == "true":
        raise verify.VerificationError(
            "sparse checkout is forbidden for scope verification"
        )
    if require_clean:
        if verify.resolve_commit(repo, "HEAD") != candidate:
            raise verify.VerificationError(
                "--require-clean requires candidate to be HEAD"
            )
        verify.git_status_clean(repo)
        ignored = verify.run_git(
            repo,
            [
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "-z",
                "--",
                "src/memory/execution_observation",
                "tests",
            ],
        )
        relevant = [
            path
            for path in ignored.split(b"\0")
            if path
            and (
                path.startswith(b"src/memory/execution_observation/")
                or path.startswith(b"tests/execution_observation_")
            )
        ]
        if relevant:
            raise verify.VerificationError(
                "ignored v53 implementation/test files are present"
            )


def _candidate_files(repo: Path, candidate: str) -> dict[str, bytes]:
    names_data = verify.run_git(
        repo,
        [
            "ls-tree",
            "-r",
            "--name-only",
            "-z",
            candidate,
            "--",
            "src/memory/execution_observation",
            "tests",
        ],
    )
    result: dict[str, bytes] = {}
    for raw in names_data.split(b"\0"):
        if not raw:
            continue
        path = raw.decode("utf-8", errors="strict")
        if path.endswith(".rs") and (
            path.startswith("src/memory/execution_observation/")
            or fnmatch.fnmatchcase(path, "tests/execution_observation_*.rs")
        ):
            _, _, data = verify.git_object(repo, candidate, path)
            result[path] = data
    return result


def _scan_observation_source(
    path: str,
    data: bytes,
    *,
    maximum_bytes: int,
    maximum_lines_exclusive: int,
) -> None:
    if len(data) > maximum_bytes:
        raise verify.VerificationError(f"observation source exceeds byte limit: {path}")
    try:
        text = data.decode("utf-8", errors="strict")
    except UnicodeDecodeError as error:
        raise verify.VerificationError(
            f"observation Rust source is not UTF-8: {path}"
        ) from error
    if len(text.splitlines()) >= maximum_lines_exclusive:
        raise verify.VerificationError(
            f"observation source must remain below 300 lines: {path}"
        )
    _scan_rust_tokens(path, text, observation=True)
    if path.startswith("src/memory/execution_observation/store/"):
        deferred = {
            "append_started",
            "append_terminal",
            "read_attempt",
            "FixtureAttemptObservationV1",
            "FixtureAttemptViewV1",
            "FixtureObservationLedgerV1",
            "ObservationReceiptV1",
            "validate_started_transition",
            "validate_terminal_transition",
        }
        tokens = set(_rust_tokens(text))
        used = deferred & tokens
        if used:
            raise verify.VerificationError(
                f"WP3 facade/receipt symbol is forbidden in WP2: {path}"
            )
        token_stream = _rust_tokens(text)
        if path != "src/memory/execution_observation/store/tests.rs":
            forbidden_output_macros = {
                "dbg",
                "eprint",
                "eprintln",
                "panic",
                "print",
                "println",
                "todo",
                "unimplemented",
            }
            for index, token in enumerate(token_stream[:-1]):
                if token in forbidden_output_macros and token_stream[index + 1] == "!":
                    raise verify.VerificationError(
                        f"production output/panic macro is forbidden in WP2: {path}"
                    )
        for index, token in enumerate(token_stream):
            if token != "PersonalVaultStorage":
                continue
            cursor = index + 1
            if cursor < len(token_stream) and token_stream[cursor] == ">":
                cursor += 1
            if (
                token_stream[cursor : cursor + 3] == ["::", "open", "("]
                and path != "src/memory/execution_observation/store/tests.rs"
            ):
                raise verify.VerificationError(
                    f"WP2 production store may not open a second vault: {path}"
                )


def _verify_wp2_store_surface(candidate_files: dict[str, bytes]) -> None:
    """Enforce the ADR-0008 seam as a closed crate-private declaration set."""

    production_sources = "\n".join(
        data.decode("utf-8", errors="strict")
        for path, data in sorted(candidate_files.items())
        if path.startswith("src/memory/execution_observation/store/")
        and path != "src/memory/execution_observation/store/tests.rs"
    )
    sources = "\n".join(
        data.decode("utf-8", errors="strict")
        for path, data in sorted(candidate_files.items())
        if path.startswith("src/memory/execution_observation/store/")
    )
    expected_types = {
        "FixtureObservationStoreV1",
        "FixtureStoredEventV1",
        "FixtureStructuralCommitV1",
        "FixtureStructuralStateV1",
    }
    expected_methods = {
        "commit_structural",
        "inject_post_exchange_sync_failure_once",
        "inject_pre_exchange_failure_once",
        "open_fixture",
        "structural_state",
    }
    expected_fields = {
        "current_view",
        "event",
        "event_watermark",
        "generation",
        "root",
        "root_sha256",
        "segment",
    }
    types = set(
        re.findall(
            r"pub\s*\(\s*super\s*\)\s*(?:enum|struct)\s+([A-Za-z_][A-Za-z0-9_]*)",
            sources,
        )
    )
    methods = set(
        re.findall(
            r"pub\s*\(\s*super\s*\)\s*fn\s+([A-Za-z_][A-Za-z0-9_]*)",
            sources,
        )
    )
    fields = set(
        re.findall(
            r"pub\s*\(\s*super\s*\)\s*([a-z_][A-Za-z0-9_]*)\s*:",
            sources,
        )
    )
    surface_tokens = _rust_tokens(sources)
    production_tokens = _rust_tokens(production_sources)
    if production_tokens.count("PersonalVaultStorage") != 2:
        raise verify.VerificationError(
            "PersonalVaultStorage may appear only in the frozen import and Arc parameter"
        )
    if production_tokens.count("vault") != 2:
        raise verify.VerificationError(
            "open_fixture vault may only be declared and consumed by the sealed CAS opener"
        )
    signature = ["vault", ":", "Arc", "<", "PersonalVaultStorage", ">"]
    delegation = [
        "ExecutionObservationFixtureStorage",
        "::",
        "open",
        "(",
        "vault",
        ")",
    ]
    if (
        _count_token_sequence(production_tokens, signature) != 1
        or _count_token_sequence(production_tokens, delegation) != 1
    ):
        raise verify.VerificationError(
            "open_fixture must consume its vault exactly once through the sealed CAS opener"
        )
    enum_starts = [
        index
        for index in range(len(surface_tokens) - 2)
        if surface_tokens[index : index + 2] == ["enum", "FixtureStoredEventV1"]
    ]
    if len(enum_starts) != 1 or surface_tokens[enum_starts[0] + 2] != "{":
        raise verify.VerificationError(
            "FixtureStoredEventV1 declaration differs from ADR-0008"
        )
    enum_end = _matching_token(surface_tokens, enum_starts[0] + 2, "{", "}")
    enum_body = surface_tokens[enum_starts[0] + 3 : enum_end]
    if enum_body and enum_body[-1] == ",":
        enum_body = enum_body[:-1]
    if enum_body != [
        "Started",
        "(",
        "StoredStartedEventV1",
        ")",
        ",",
        "Terminal",
        "(",
        "StoredTerminalEventV1",
        ")",
    ]:
        raise verify.VerificationError(
            "FixtureStoredEventV1 variants differ from ADR-0008"
        )
    cfg_test_prefix = [
        "#",
        "[",
        "cfg",
        "(",
        "test",
        ")",
        "]",
        "pub",
        "(",
        "super",
        ")",
        "fn",
    ]
    for method in {
        "inject_pre_exchange_failure_once",
        "inject_post_exchange_sync_failure_once",
    }:
        occurrences = [
            index
            for index, token in enumerate(surface_tokens)
            if token == method and index > 0 and surface_tokens[index - 1] == "fn"
        ]
        if (
            len(occurrences) != 1
            or surface_tokens[occurrences[0] - len(cfg_test_prefix) : occurrences[0]]
            != cfg_test_prefix
        ):
            raise verify.VerificationError(
                f"WP2 fault seam must remain cfg(test): {method}"
            )
    if (
        types != expected_types
        or methods != expected_methods
        or fields != expected_fields
        or len(re.findall(r"pub\s*\(\s*super\s*\)", sources)) != 16
    ):
        raise verify.VerificationError(
            "WP2 store crate-private surface differs from the frozen ADR-0008 seam"
        )


def _read_lcov(path: Path, repo: Path) -> dict[str, dict[int, int]]:
    flags = os.O_RDONLY | os.O_CLOEXEC | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except OSError as error:
        raise verify.VerificationError(f"cannot open coverage LCOV: {error}") from error
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid():
            raise verify.VerificationError(
                "coverage LCOV must be a current-owner regular file"
            )
        if info.st_size <= 0 or info.st_size > 64 * 1024 * 1024:
            raise verify.VerificationError("coverage LCOV is empty or exceeds 64 MiB")
        chunks: list[bytes] = []
        remaining = info.st_size
        while remaining:
            chunk = os.read(fd, min(remaining, 1024 * 1024))
            if not chunk:
                raise verify.VerificationError("coverage LCOV changed while reading")
            chunks.append(chunk)
            remaining -= len(chunk)
        data = b"".join(chunks)
    finally:
        os.close(fd)
    try:
        text = data.decode("utf-8", errors="strict")
    except UnicodeDecodeError as error:
        raise verify.VerificationError("coverage LCOV is not UTF-8") from error

    repo_real = os.path.realpath(repo)
    records: dict[str, dict[int, int]] = {}
    current: str | None = None
    ended = True
    for number, line in enumerate(text.splitlines(), 1):
        if line.startswith("SF:"):
            if not ended:
                raise verify.VerificationError(
                    f"LCOV nested SF record at line {number}"
                )
            source = line[3:]
            if not source:
                raise verify.VerificationError(
                    f"LCOV empty source path at line {number}"
                )
            if os.path.isabs(source):
                source_real = os.path.realpath(source)
                try:
                    if os.path.commonpath((repo_real, source_real)) != repo_real:
                        raise verify.VerificationError(
                            "LCOV source is outside the candidate repository"
                        )
                except ValueError as error:
                    raise verify.VerificationError(
                        "LCOV source path is not comparable to repository"
                    ) from error
                source = os.path.relpath(source_real, repo_real)
            else:
                normalized = os.path.normpath(source)
                if normalized == ".." or normalized.startswith("../"):
                    raise verify.VerificationError(
                        "LCOV relative source escapes the repository"
                    )
                source = normalized
            if source in records:
                raise verify.VerificationError(
                    f"LCOV duplicate source record: {source}"
                )
            records[source] = {}
            current = source
            ended = False
        elif line.startswith("DA:"):
            if current is None or ended:
                raise verify.VerificationError(
                    f"LCOV DA outside source record at line {number}"
                )
            parts = line[3:].split(",")
            if len(parts) < 2 or not parts[0].isdigit() or not parts[1].isdigit():
                raise verify.VerificationError(f"LCOV malformed DA at line {number}")
            source_line, hits = int(parts[0]), int(parts[1])
            if source_line <= 0 or source_line in records[current]:
                raise verify.VerificationError(
                    f"LCOV duplicate/invalid executable line at line {number}"
                )
            records[current][source_line] = hits
        elif line == "end_of_record":
            if current is None or ended:
                raise verify.VerificationError(
                    f"LCOV unmatched end_of_record at line {number}"
                )
            current = None
            ended = True
    if not ended:
        raise verify.VerificationError("LCOV final source record is not terminated")
    if not records:
        raise verify.VerificationError("LCOV contains no source records")
    return records


def _verify_coverage(
    lcov_path: Path,
    repo: Path,
    candidate: str,
    candidate_files: dict[str, bytes],
    contract: dict[str, object],
) -> dict[str, object]:
    records = _read_lcov(lcov_path, repo)
    for path, lines in records.items():
        if not path.endswith(".rs"):
            raise verify.VerificationError(f"LCOV source is not a Rust file: {path}")
        mode, _, git_bytes = verify.git_object(repo, candidate, path)
        worktree_path = repo / path
        try:
            info = worktree_path.lstat()
        except OSError as error:
            raise verify.VerificationError(
                f"LCOV source is absent from candidate worktree: {path}"
            ) from error
        if (
            mode != "100644"
            or not stat.S_ISREG(info.st_mode)
            or worktree_path.read_bytes() != git_bytes
        ):
            raise verify.VerificationError(
                f"LCOV source is not the candidate regular Git blob: {path}"
            )
        try:
            source_lines = len(git_bytes.decode("utf-8", errors="strict").splitlines())
        except UnicodeDecodeError as error:
            raise verify.VerificationError(
                f"LCOV Rust source is not UTF-8: {path}"
            ) from error
        if any(line > source_lines for line in lines):
            raise verify.VerificationError(
                f"LCOV DA line exceeds candidate source: {path}"
            )
    total_found = sum(len(lines) for lines in records.values())
    total_hit = sum(
        sum(1 for hits in lines.values() if hits > 0) for lines in records.values()
    )
    if total_found == 0:
        raise verify.VerificationError("LCOV reports zero executable lines")
    minimum_found = contract["baseline_global"]["executable_lines"]
    if total_found < minimum_found:
        raise verify.VerificationError(
            f"LCOV executable-line denominator is below frozen baseline: {total_found}/{minimum_found}"
        )
    global_minimum = Decimal(contract["global_minimum_percent"])
    if Decimal(total_hit * 100) < global_minimum * Decimal(total_found):
        raise verify.VerificationError(
            f"global line coverage is below {global_minimum}%: {total_hit}/{total_found}"
        )

    candidate_observation = {
        path
        for path in candidate_files
        if path.startswith("src/memory/execution_observation/")
    }
    observation_records = {
        path: lines
        for path, lines in records.items()
        if path.startswith("src/memory/execution_observation/")
    }
    if not observation_records or not (
        set(observation_records) & candidate_observation
    ):
        raise verify.VerificationError(
            "LCOV has no candidate observation module record"
        )
    observation_found = sum(len(lines) for lines in observation_records.values())
    observation_hit = sum(
        sum(1 for hits in lines.values() if hits > 0)
        for lines in observation_records.values()
    )
    if observation_found == 0:
        raise verify.VerificationError(
            "LCOV observation module has zero executable lines"
        )
    observation_minimum = Decimal(contract["observation_minimum_percent"])
    if Decimal(observation_hit * 100) < observation_minimum * Decimal(
        observation_found
    ):
        raise verify.VerificationError(
            f"observation line coverage is below {observation_minimum}%: "
            f"{observation_hit}/{observation_found}"
        )
    return {
        "global": f"{total_hit}/{total_found}",
        "observation": f"{observation_hit}/{observation_found}",
    }


def _run_and_verify_coverage(
    repo: Path,
    candidate: str,
    candidate_files: dict[str, bytes],
    contract: dict[str, object],
    toolchain: dict[str, object],
) -> dict[str, object]:
    _assert_cargo_unchanged(toolchain)
    if verify.resolve_commit(repo, "HEAD") != candidate:
        raise verify.VerificationError("coverage must run at candidate HEAD")
    verify.git_status_clean(repo)
    with _isolated_candidate_environment(toolchain["environment"]) as (
        environment,
        execution_root,
    ):
        output_name = os.fspath(execution_root / "coverage.lcov")
        command = [
            os.fspath(toolchain["cargo_path"]),
            "llvm-cov",
            "--locked",
            "--lib",
            "--all-features",
            "--lcov",
            "--output-path",
            output_name,
        ]
        environment.update(contract["environment"])
        result = _run_bounded_process(
            command,
            cwd=repo,
            environment=environment,
            timeout=contract["timeout_seconds"],
        )
        if result.returncode != 0:
            lines = result.stdout.decode("utf-8", errors="replace").strip().splitlines()
            detail = lines[-1] if lines else "no output"
            raise verify.VerificationError(
                f"frozen coverage command exited nonzero: {detail}"
            )
        _assert_execution_seal(repo, candidate, toolchain)
        return _verify_coverage(
            Path(output_name), repo, candidate, candidate_files, contract
        )


def _parse_listed_f_tests(output: str) -> dict[str, list[str]]:
    listed = {f"F{index:02d}": [] for index in range(1, 17)}
    seen: set[str] = set()
    for match in LISTED_F_TEST.finditer(output):
        test_id = f"F{match.group('id')}"
        name = match.group("name")
        if test_id not in listed:
            raise verify.VerificationError(f"cargo listed unknown F-test id: {test_id}")
        if name in seen:
            raise verify.VerificationError(
                f"cargo listed duplicate F-test identity: {name}"
            )
        seen.add(name)
        listed[test_id].append(name)
    return listed


def _parse_exact_f_test_execution(output: str, expected_name: str) -> None:
    escaped = re.escape(expected_name)
    ok_lines = re.findall(rf"(?m)^test\s+{escaped}\s+\.\.\.\s+ok\s*$", output)
    ignored_lines = re.findall(
        rf"(?m)^test\s+{escaped}\s+\.\.\.\s+ignored(?:,.*)?\s*$", output
    )
    one_pass_summaries = re.findall(
        r"(?m)^test result: ok\. 1 passed; 0 failed; 0 ignored; \d+ measured; \d+ filtered out;"
        r" finished in .+$",
        output,
    )
    if ignored_lines or len(ok_lines) != 1 or len(one_pass_summaries) != 1:
        raise verify.VerificationError(
            f"exact F-test did not prove one non-ignored execution: {expected_name}"
        )


def _run_required_f_tests(
    source_repo: Path,
    candidate: str,
    candidate_checkout: Path,
    candidate_manifest: dict[str, tuple[str, str]],
    required_test_ids: set[str],
    test_contract: dict[str, object],
    toolchain: dict[str, object],
) -> dict[str, list[str]]:
    _assert_execution_seal(source_repo, candidate, toolchain)
    _verify_materialized_tree(candidate_checkout, candidate_manifest)
    with _isolated_candidate_environment(toolchain["environment"]) as (
        environment,
        _,
    ):
        list_command = [
            os.fspath(toolchain["cargo_path"]),
            "test",
            "--locked",
            "--all-features",
            "execution_observation_f",
            "--",
            "--list",
        ]
        result = _run_bounded_process(
            list_command,
            cwd=candidate_checkout,
            environment=environment,
            timeout=1200,
        )
        output = result.stdout.decode("utf-8", errors="replace")
        if result.returncode != 0:
            detail = output.strip().splitlines()
            raise verify.VerificationError(
                f"F-test command exited nonzero: {detail[-1] if detail else 'no output'}"
            )
        _assert_execution_seal(source_repo, candidate, toolchain)
        _verify_materialized_tree(candidate_checkout, candidate_manifest)
        listed = _parse_listed_f_tests(output)
        executed = {test_id: [] for test_id in sorted(required_test_ids)}
        for test_id in sorted(required_test_ids):
            names = listed[test_id]
            minimum = test_contract[test_id]["minimum_tests"]
            if len(names) < minimum:
                raise verify.VerificationError(
                    f"cargo listed {len(names)} tests for {test_id}, required {minimum}"
                )
            for name in names:
                exact_command = [
                    os.fspath(toolchain["cargo_path"]),
                    "test",
                    "--locked",
                    "--all-features",
                    name,
                    "--",
                    "--exact",
                    "--nocapture",
                ]
                exact = _run_bounded_process(
                    exact_command,
                    cwd=candidate_checkout,
                    environment=environment,
                    timeout=1200,
                )
                exact_output = exact.stdout.decode("utf-8", errors="replace")
                if exact.returncode != 0:
                    detail = exact_output.strip().splitlines()
                    raise verify.VerificationError(
                        f"exact F-test exited nonzero: {detail[-1] if detail else 'no output'}"
                    )
                _parse_exact_f_test_execution(exact_output, name)
                _assert_execution_seal(source_repo, candidate, toolchain)
                _verify_materialized_tree(candidate_checkout, candidate_manifest)
                executed[test_id].append(name)
        return executed


def _run_wp1_external_corpus(
    source_repo: Path,
    candidate: str,
    candidate_checkout: Path,
    candidate_manifest: dict[str, tuple[str, str]],
    toolchain: dict[str, object],
) -> dict[str, object]:
    """Compile architecture-owned contract tests over immutable candidate bytes."""

    module = candidate_checkout / "src/memory/execution_observation/mod.rs"
    corpus = (
        candidate_checkout
        / "src/memory/execution_observation/architecture_contract_tests.rs"
    )
    module_bytes = module.read_bytes()
    anchor = b"\n#[cfg(test)]\nmod architecture_contract_tests;\n"
    if b"architecture_contract_tests" in module_bytes or corpus.exists():
        raise verify.VerificationError(
            "candidate predeclares the architecture-owned WP1 corpus"
        )
    module.chmod(0o600)
    module.write_bytes(module_bytes + anchor)
    module.chmod(0o400)
    corpus.write_text(WP1_EXTERNAL_TESTS, encoding="utf-8")
    corpus.chmod(0o400)

    overlay_manifest = candidate_manifest.copy()
    module_name = "src/memory/execution_observation/mod.rs"
    corpus_name = "src/memory/execution_observation/architecture_contract_tests.rs"
    overlay_manifest[module_name] = (
        "100644",
        verify.sha256_bytes(module.read_bytes()),
    )
    overlay_manifest[corpus_name] = (
        "100644",
        verify.sha256_bytes(corpus.read_bytes()),
    )
    _verify_materialized_tree(candidate_checkout, overlay_manifest)
    _assert_execution_seal(source_repo, candidate, toolchain)

    with _isolated_candidate_environment(toolchain["environment"]) as (
        environment,
        _,
    ):
        list_command = [
            os.fspath(toolchain["cargo_path"]),
            "test",
            "--locked",
            "--all-features",
            "architecture_wp1_contract_",
            "--",
            "--list",
        ]
        listed_result = _run_bounded_process(
            list_command,
            cwd=candidate_checkout,
            environment=environment,
            timeout=1200,
        )
        listed_output = listed_result.stdout.decode("utf-8", errors="replace")
        if listed_result.returncode != 0:
            detail = listed_output.strip().splitlines()
            raise verify.VerificationError(
                "architecture WP1 corpus list failed: "
                f"{detail[-1] if detail else 'no output'}"
            )
        listed = {
            line.removesuffix(": test")
            for line in listed_output.splitlines()
            if line.endswith(": test") and "architecture_wp1_contract_" in line
        }
        if listed != WP1_EXTERNAL_TEST_NAMES:
            raise verify.VerificationError(
                "architecture WP1 corpus inventory differs from the frozen oracle"
            )
        for name in sorted(WP1_EXTERNAL_TEST_NAMES):
            exact = _run_bounded_process(
                [
                    os.fspath(toolchain["cargo_path"]),
                    "test",
                    "--locked",
                    "--all-features",
                    name,
                    "--",
                    "--exact",
                    "--nocapture",
                ],
                cwd=candidate_checkout,
                environment=environment,
                timeout=1200,
            )
            output = exact.stdout.decode("utf-8", errors="replace")
            if exact.returncode != 0:
                detail = output.strip().splitlines()
                raise verify.VerificationError(
                    "architecture WP1 corpus test failed: "
                    f"{detail[-1] if detail else name}"
                )
            _parse_exact_f_test_execution(output, name)
            _assert_execution_seal(source_repo, candidate, toolchain)
            _verify_materialized_tree(candidate_checkout, overlay_manifest)
    return {
        "source_sha256": verify.sha256_bytes(WP1_EXTERNAL_TESTS.encode("utf-8")),
        "tests": sorted(WP1_EXTERNAL_TEST_NAMES),
    }


def _run_wp2_external_corpus(
    source_repo: Path,
    candidate: str,
    candidate_checkout: Path,
    candidate_manifest: dict[str, tuple[str, str]],
    toolchain: dict[str, object],
) -> dict[str, object]:
    """Compile the independent WP2 store oracle over immutable candidate bytes."""

    module = candidate_checkout / "src/memory/execution_observation/mod.rs"
    corpus = (
        candidate_checkout
        / "src/memory/execution_observation/architecture_wp2_store_tests.rs"
    )
    module_bytes = module.read_bytes()
    anchor = b"\n#[cfg(test)]\nmod architecture_wp2_store_tests;\n"
    if b"architecture_wp2_store_tests" in module_bytes or corpus.exists():
        raise verify.VerificationError(
            "candidate predeclares the architecture-owned WP2 corpus"
        )
    module.chmod(0o600)
    module.write_bytes(module_bytes + anchor)
    module.chmod(0o400)
    corpus.write_text(WP2_EXTERNAL_TESTS, encoding="utf-8")
    corpus.chmod(0o400)

    overlay_manifest = candidate_manifest.copy()
    module_name = "src/memory/execution_observation/mod.rs"
    corpus_name = "src/memory/execution_observation/architecture_wp2_store_tests.rs"
    overlay_manifest[module_name] = (
        "100644",
        verify.sha256_bytes(module.read_bytes()),
    )
    overlay_manifest[corpus_name] = (
        "100644",
        verify.sha256_bytes(corpus.read_bytes()),
    )
    _verify_materialized_tree(candidate_checkout, overlay_manifest)
    _assert_execution_seal(source_repo, candidate, toolchain)

    with _isolated_candidate_environment(toolchain["environment"]) as (
        environment,
        _,
    ):
        listed_result = _run_bounded_process(
            [
                os.fspath(toolchain["cargo_path"]),
                "test",
                "--locked",
                "--all-features",
                "architecture_wp2_store_",
                "--",
                "--list",
            ],
            cwd=candidate_checkout,
            environment=environment,
            timeout=1200,
        )
        listed_output = listed_result.stdout.decode("utf-8", errors="replace")
        if listed_result.returncode != 0:
            detail = listed_output.strip().splitlines()
            raise verify.VerificationError(
                "architecture WP2 corpus list failed: "
                f"{detail[-1] if detail else 'no output'}"
            )
        listed = {
            line.removesuffix(": test")
            for line in listed_output.splitlines()
            if line.endswith(": test") and "architecture_wp2_store_" in line
        }
        if listed != WP2_EXTERNAL_TEST_NAMES:
            raise verify.VerificationError(
                "architecture WP2 corpus inventory differs from the frozen oracle"
            )
        for name in sorted(WP2_EXTERNAL_TEST_NAMES):
            exact = _run_bounded_process(
                [
                    os.fspath(toolchain["cargo_path"]),
                    "test",
                    "--locked",
                    "--all-features",
                    name,
                    "--",
                    "--exact",
                    "--nocapture",
                ],
                cwd=candidate_checkout,
                environment=environment,
                timeout=1200,
            )
            output = exact.stdout.decode("utf-8", errors="replace")
            if exact.returncode != 0:
                detail = output.strip().splitlines()
                raise verify.VerificationError(
                    "architecture WP2 corpus test failed: "
                    f"{detail[-1] if detail else name}"
                )
            _parse_exact_f_test_execution(output, name)
            _assert_execution_seal(source_repo, candidate, toolchain)
            _verify_materialized_tree(candidate_checkout, overlay_manifest)
    return {
        "source_sha256": verify.sha256_bytes(WP2_EXTERNAL_TESTS.encode("utf-8")),
        "tests": sorted(WP2_EXTERNAL_TEST_NAMES),
    }


def _extract_git_archive(
    repo: Path, commit: str, destination: Path
) -> dict[str, tuple[str, str]]:
    """Materialize exact Git blobs without archive attributes or the worktree."""

    destination.mkdir(mode=0o700)
    tree = verify.run_git(repo, ["ls-tree", "-r", "-z", commit])
    manifest: dict[str, tuple[str, str]] = {}
    for record in tree.split(b"\0"):
        if not record:
            continue
        try:
            header, raw_path = record.split(b"\t", 1)
            mode, kind, object_id = header.decode("ascii", errors="strict").split()
            path = raw_path.decode("utf-8", errors="strict")
        except (UnicodeDecodeError, ValueError) as error:
            raise verify.VerificationError("malformed Git tree entry") from error
        parts = Path(path).parts
        if (
            not parts
            or path.startswith("/")
            or ".." in parts
            or "\\" in path
            or any(ord(character) < 32 or ord(character) == 127 for character in path)
        ):
            raise verify.VerificationError("Git tree contains an unsafe path")
        if ".git" in parts:
            raise verify.VerificationError(
                "Git object tree may not materialize repository-control paths"
            )
        if parts[0] == ".cargo":
            raise verify.VerificationError(
                "repository-local .cargo is forbidden in the Git object tree"
            )
        if mode not in {"100644", "100755"} or kind != "blob":
            raise verify.VerificationError(
                f"Git tree contains symlink/special/submodule entry: {path}"
            )
        if path in manifest:
            raise verify.VerificationError(f"duplicate Git tree path: {path}")
        data = verify.run_git(repo, ["cat-file", "blob", object_id])
        digest = verify.sha256_bytes(data)
        target = destination.joinpath(*parts)
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        try:
            target.write_bytes(data)
            target.chmod(0o500 if mode == "100755" else 0o400)
        except OSError as error:
            raise verify.VerificationError(
                f"cannot materialize Git object path {path}: {error}"
            ) from error
        manifest[path] = (mode, digest)
    if not manifest:
        raise verify.VerificationError("Git tree materialization produced no files")
    return manifest


def _verify_materialized_tree(
    checkout: Path, manifest: dict[str, tuple[str, str]]
) -> None:
    observed: set[str] = set()
    for path in checkout.rglob("*"):
        relative = path.relative_to(checkout).as_posix()
        try:
            info = path.lstat()
        except OSError as error:
            raise verify.VerificationError(
                f"cannot inspect materialized candidate path {relative}: {error}"
            ) from error
        if stat.S_ISDIR(info.st_mode):
            continue
        if not stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
            raise verify.VerificationError(
                f"materialized candidate contains special path: {relative}"
            )
        expected = manifest.get(relative)
        if expected is None:
            raise verify.VerificationError(
                f"candidate command created an unbound source path: {relative}"
            )
        executable = bool(info.st_mode & 0o111)
        if executable != (expected[0] == "100755"):
            raise verify.VerificationError(
                f"candidate command changed source mode: {relative}"
            )
        if verify.sha256_bytes(path.read_bytes()) != expected[1]:
            raise verify.VerificationError(
                f"candidate command changed Git object bytes: {relative}"
            )
        observed.add(relative)
    missing = set(manifest) - observed
    if missing:
        raise verify.VerificationError(
            f"materialized candidate lost Git object path: {sorted(missing)[0]}"
        )


def _verified_candidate_files_from_checkout(
    checkout: Path, candidate_files: dict[str, bytes]
) -> dict[str, bytes]:
    result: dict[str, bytes] = {}
    for relative, expected in candidate_files.items():
        path = checkout / relative
        try:
            info = path.lstat()
            data = path.read_bytes()
        except OSError as error:
            raise verify.VerificationError(
                f"cannot read materialized candidate source {relative}: {error}"
            ) from error
        if (
            not stat.S_ISREG(info.st_mode)
            or stat.S_ISLNK(info.st_mode)
            or data != expected
        ):
            raise verify.VerificationError(
                f"materialized source differs from candidate Git object: {relative}"
            )
        result[relative] = data
    return result


def _run_wp2_archive_gate(
    source_repo: Path,
    base: str,
    candidate: str,
    scope: dict[str, object],
    test_contract: dict[str, object],
    toolchain: dict[str, object],
) -> tuple[dict[str, bytes], dict[str, list[str]], dict[str, object]]:
    """Scan/build/test exact object materializations, never the developer worktree."""

    with tempfile.TemporaryDirectory(prefix="plico-v53-scope-objects-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        base_checkout = root / "base"
        candidate_checkout = root / "candidate"
        wp1_external_checkout = root / "wp1-external-corpus"
        wp2_external_checkout = root / "wp2-external-corpus"
        base_manifest = _extract_git_archive(source_repo, base, base_checkout)
        candidate_manifest = _extract_git_archive(
            source_repo, candidate, candidate_checkout
        )
        wp1_external_manifest = _extract_git_archive(
            source_repo, candidate, wp1_external_checkout
        )
        wp2_external_manifest = _extract_git_archive(
            source_repo, candidate, wp2_external_checkout
        )
        _verify_materialized_tree(base_checkout, base_manifest)
        _verify_materialized_tree(candidate_checkout, candidate_manifest)
        _verify_materialized_tree(wp1_external_checkout, wp1_external_manifest)
        _verify_materialized_tree(wp2_external_checkout, wp2_external_manifest)

        object_files = _candidate_files(source_repo, candidate)
        candidate_files = _verified_candidate_files_from_checkout(
            candidate_checkout, object_files
        )
        observation_sources = {
            path: data
            for path, data in candidate_files.items()
            if path.startswith("src/memory/execution_observation/store/")
        }
        if not observation_sources:
            raise verify.VerificationError(
                "candidate has no observation module Rust source"
            )
        for path, data in observation_sources.items():
            _scan_observation_source(
                path,
                data,
                maximum_bytes=scope["observation_file_max_bytes"],
                maximum_lines_exclusive=scope["observation_file_max_lines_exclusive"],
            )
        _verify_wp2_store_surface(candidate_files)

        declarations: dict[str, int] = {f"F{index:02d}": 0 for index in range(1, 17)}
        total_tests = 0
        for path, data in candidate_files.items():
            text = data.decode("utf-8", errors="strict")
            for match in TEST_DECLARATION.finditer(text):
                test_id = f"F{match.group(2)}"
                if test_id not in declarations:
                    raise verify.VerificationError(
                        f"unknown F-test id in {path}: {test_id}"
                    )
                declarations[test_id] += 1
                total_tests += 1
        if total_tests == 0:
            raise verify.VerificationError(
                "scope gate matched zero execution_observation_fNN tests"
            )

        required_test_ids = {
            test_id
            for test_id, contract in test_contract.items()
            if contract["work_package"] in {"WP1", "WP2"}
        }
        for test_id in sorted(required_test_ids):
            minimum = test_contract[test_id]["minimum_tests"]
            if declarations[test_id] < minimum:
                raise verify.VerificationError(
                    f"WP2 cumulative gate requires at least {minimum} source declaration for {test_id}"
                )

        candidate_self_evidence = _run_required_f_tests(
            source_repo,
            candidate,
            candidate_checkout,
            candidate_manifest,
            required_test_ids,
            test_contract,
            toolchain,
        )
        _verify_materialized_tree(candidate_checkout, candidate_manifest)
        wp1_external_evidence = _run_wp1_external_corpus(
            source_repo,
            candidate,
            wp1_external_checkout,
            wp1_external_manifest,
            toolchain,
        )
        wp2_external_evidence = _run_wp2_external_corpus(
            source_repo,
            candidate,
            wp2_external_checkout,
            wp2_external_manifest,
            toolchain,
        )
        return (
            candidate_files,
            candidate_self_evidence,
            {"WP1": wp1_external_evidence, "WP2": wp2_external_evidence},
        )


def _normalize_semantic(value: object, key: str = "") -> object:
    if isinstance(value, dict):
        return {
            name: _normalize_semantic(item, name)
            for name, item in sorted(value.items())
        }
    if isinstance(value, list):
        return [_normalize_semantic(item, key) for item in value]
    if isinstance(value, str):
        if UUID_TEXT.fullmatch(value):
            return "<uuid>"
        if SHA_TEXT.fullmatch(value):
            return "<sha256>"
        return value
    if isinstance(value, int) and any(
        token in key for token in ("time", "created", "updated", "recorded")
    ):
        return "<writer-time>"
    return value


def _run_lifecycle_cli(binary: Path, vault: Path, operation: list[str]) -> object:
    if not Path("/proc/self/task").is_dir() or not Path("/proc/self/fd").is_dir():
        raise verify.VerificationError(
            "lifecycle thread/handle proof requires Linux /proc"
        )
    command = [os.fspath(binary), "--embedded", "--root", os.fspath(vault), *operation]
    environment = os.environ.copy()
    environment.update(
        {"EMBEDDING_BACKEND": "stub", "LLM_BACKEND": "stub", "RUST_LOG": "off"}
    )
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=environment,
        start_new_session=True,
    )
    deadline = time.monotonic() + 120
    observation_resource = False
    samples = 0
    maximum_threads = 0
    maximum_handles = 0
    while process.poll() is None:
        if time.monotonic() >= deadline:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise verify.VerificationError("lifecycle CLI exceeded 120 seconds")
        proc = Path("/proc") / str(process.pid)
        tasks = list((proc / "task").glob("*/comm"))
        descriptors = list((proc / "fd").glob("*"))
        samples += 1
        maximum_threads = max(maximum_threads, len(tasks))
        maximum_handles = max(maximum_handles, len(descriptors))
        for task in tasks:
            try:
                observation_resource |= "execution-observation" in task.read_text(
                    encoding="utf-8", errors="replace"
                )
            except OSError:
                pass
        for descriptor in descriptors:
            try:
                observation_resource |= (
                    "execution-observation-fixture-ledger" in os.readlink(descriptor)
                )
            except OSError:
                pass
        time.sleep(0.005)
    stdout, stderr = process.communicate()
    if process.returncode != 0:
        detail = stderr.decode("utf-8", errors="replace").strip().splitlines()
        raise verify.VerificationError(
            f"lifecycle CLI failed: {detail[-1] if detail else 'no stderr'}"
        )
    if observation_resource:
        raise verify.VerificationError(
            "production lifecycle exposed an observation thread/handle"
        )
    if samples == 0:
        raise verify.VerificationError(
            "lifecycle process ended before thread/handle sampling"
        )
    return {
        "response": _normalize_semantic(
            verify.strict_json_loads(stdout, "lifecycle CLI response")
        ),
        "maximum_threads": maximum_threads,
        "maximum_handles": maximum_handles,
        "observation_resource_absent": True,
    }


def _normalize_inventory_path(relative: Path) -> str:
    parts = []
    for part in relative.parts:
        if SHA_TEXT.fullmatch(part):
            parts.append("<sha256>")
        elif UUID_TEXT.fullmatch(part):
            parts.append("<uuid>")
        else:
            parts.append(part)
    return "/".join(parts)


def _vault_inventory(vault: Path) -> list[tuple[object, ...]]:
    inventory: collections.Counter[tuple[object, ...]] = collections.Counter()
    for path in sorted(vault.rglob("*")):
        relative = path.relative_to(vault)
        normalized = _normalize_inventory_path(relative)
        if "execution-observation-fixture-ledger" in relative.parts:
            raise verify.VerificationError(
                "production lifecycle created observation namespace"
            )
        info = path.lstat()
        if stat.S_ISLNK(info.st_mode) or not (
            stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode)
        ):
            raise verify.VerificationError(
                "lifecycle vault contains symlink/special state"
            )
        if stat.S_ISDIR(info.st_mode):
            inventory[("dir", normalized, stat.S_IMODE(info.st_mode))] += 1
            continue
        data = path.read_bytes()
        schemas: set[str] = set()
        for line in data.splitlines() or [data]:
            try:
                parsed = json.loads(line)
            except (UnicodeDecodeError, json.JSONDecodeError):
                continue
            stack = [parsed]
            while stack:
                item = stack.pop()
                if isinstance(item, dict):
                    schema = item.get("schema")
                    if isinstance(schema, str):
                        schemas.add(schema)
                    stack.extend(item.values())
                elif isinstance(item, list):
                    stack.extend(item)
        inventory[
            (
                "file",
                normalized,
                stat.S_IMODE(info.st_mode),
                len(data),
                tuple(sorted(schemas)),
            )
        ] += 1
    return sorted((*key, count) for key, count in inventory.items())


def _run_lifecycle_checkout(
    checkout: Path, target: Path, toolchain: dict[str, object]
) -> dict[str, object]:
    _assert_cargo_unchanged(toolchain)
    environment = toolchain["environment"].copy()
    environment.update(
        {
            "CARGO_TARGET_DIR": os.fspath(target),
            "EMBEDDING_BACKEND": "stub",
            "LLM_BACKEND": "stub",
        }
    )
    result = _run_bounded_process(
        [os.fspath(toolchain["cargo_path"]), "build", "--locked", "--bin", "aicli"],
        cwd=checkout,
        environment=environment,
        timeout=1200,
    )
    if result.returncode != 0:
        output = result.stdout.decode("utf-8", errors="replace").strip().splitlines()
        raise verify.VerificationError(
            f"lifecycle aicli build failed: {output[-1] if output else 'no output'}"
        )
    _assert_cargo_unchanged(toolchain)
    binary = target / "debug/aicli"
    vault = checkout.parent / f"{checkout.name}-vault"
    responses = [
        _run_lifecycle_cli(binary, vault, ["capabilities.describe"]),
        _run_lifecycle_cli(
            binary,
            vault,
            [
                "memory.create",
                "--content",
                "plico-v53-deterministic-lifecycle-fixture",
                "--tag",
                "plico:v53:lifecycle",
            ],
        ),
        _run_lifecycle_cli(binary, vault, ["capabilities.describe"]),
        _run_lifecycle_cli(
            binary,
            vault,
            [
                "memory.recall",
                "--query",
                "plico-v53-deterministic-lifecycle-fixture",
                "--limit",
                "5",
            ],
        ),
    ]
    return {"responses": responses, "mutation_inventory": _vault_inventory(vault)}


def _run_lifecycle_differential(
    repo: Path, base: str, candidate: str, toolchain: dict[str, object]
) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="plico-v53-lifecycle-") as temporary:
        root = Path(temporary)
        base_checkout = root / "base"
        candidate_checkout = root / "candidate"
        _extract_git_archive(repo, base, base_checkout)
        _extract_git_archive(repo, candidate, candidate_checkout)
        target = root / "target"
        base_result = _run_lifecycle_checkout(base_checkout, target, toolchain)
        candidate_result = _run_lifecycle_checkout(
            candidate_checkout, target, toolchain
        )
        if base_result != candidate_result:
            raise verify.VerificationError(
                "base/candidate lifecycle semantic fixture or mutation inventory differs"
            )
        return {
            "schema": "plico.v53.lifecycle-differential-result/v1",
            "base": base,
            "candidate": candidate,
            "semantic_fixtures_equal": True,
            "mutation_inventories_equal": True,
            "observation_namespace_thread_handle_absent": True,
        }


def _verify_scope_sanitized(
    handoff_dir: Path,
    repo: Path,
    handoff: dict[str, object],
    toolchain: dict[str, object],
    authorization: dict[str, object],
    *,
    approval_revision: str,
    candidate_revision: str,
    work_package: str,
    require_clean: bool,
) -> dict[str, object]:
    toolchain["repository_metadata_fingerprint"] = _audit_repository_metadata(repo)
    verified_against_repo = verify.verify_handoff(handoff_dir, repo=repo)
    if verified_against_repo != handoff:
        raise verify.VerificationError("WP2 packet changed during scope verification")
    if (
        authorization.get("authorization") != "GO"
        or authorization.get("integrity") != "verified"
        or authorization.get("packet_id") != handoff["packet_id"]
    ):
        raise verify.VerificationError(
            "offline authorization result is not a verified GO for this packet"
        )
    base = authorization.get("candidate_scope_base_sha")
    if (
        not isinstance(base, str)
        or base != authorization.get("approval_commit_sha")
        or not verify.GIT_OBJECT_ID.fullmatch(base)
    ):
        raise verify.VerificationError(
            "offline authorization did not return one canonical approval scope base"
        )
    candidate = verify.resolve_commit(repo, candidate_revision)
    ancestor = verify.run_git(repo, ["merge-base", "--is-ancestor", base, candidate])
    if ancestor:
        raise verify.VerificationError("unexpected merge-base output")
    _check_repo_checkout(repo, candidate, require_clean)

    raw = verify.run_git(
        repo,
        ["diff", "--name-status", "-z", "--no-renames", base, candidate, "--"],
    )
    changes = _parse_name_status(raw)
    if not changes:
        raise verify.VerificationError("candidate has no implementation diff")
    scope = handoff["spec"]["developer_scope"]
    if scope["active_work_package"] != work_package:
        raise verify.VerificationError(
            "requested work package differs from the packet-frozen active work package"
        )
    work_package_scope = scope["work_packages"].get(work_package)
    if not isinstance(work_package_scope, dict):
        raise verify.VerificationError(
            "requested work package has no packet-frozen developer allowlist"
        )
    architecture_owned = set(scope["architecture_owned"])
    for status, path in changes:
        if status not in {"A", "M"}:
            raise verify.VerificationError(
                f"delete/type/merge change is forbidden: {status} {path}"
            )
        if path in architecture_owned:
            raise verify.VerificationError(f"architecture-owned file changed: {path}")
        if path in scope["forbidden_exact"] or any(
            path.startswith(prefix) for prefix in scope["forbidden_prefixes"]
        ):
            raise verify.VerificationError(f"forbidden path changed: {path}")
        if not _is_allowed(path, work_package_scope):
            raise verify.VerificationError(
                f"path is outside the frozen {work_package} developer allowlist: {path}"
            )
        mode, _, data = verify.git_object(repo, candidate, path)
        if mode != "100644":
            raise verify.VerificationError(
                f"developer file mode must remain 100644: {path}"
            )
        if path.endswith(".rs"):
            try:
                rust_text = data.decode("utf-8", errors="strict")
            except UnicodeDecodeError as error:
                raise verify.VerificationError(
                    f"changed Rust source is not UTF-8: {path}"
                ) from error
            _scan_rust_tokens(path, rust_text, observation=False)

    _verify_wp2_module_anchor(repo, base, candidate)

    candidate_files, candidate_self_evidence, external_evidence = _run_wp2_archive_gate(
        repo,
        base,
        candidate,
        scope,
        handoff["spec"]["test_contract"],
        toolchain,
    )

    lifecycle_result: dict[str, object] | None = None
    if work_package in {"WP2", "WP5", "WP6"}:
        lifecycle_result = _run_lifecycle_differential(repo, base, candidate, toolchain)

    coverage_result: dict[str, object] | None = None
    coverage_contract = handoff["spec"]["coverage_contract"]
    if work_package in coverage_contract["required_work_packages"]:
        coverage_result = _run_and_verify_coverage(
            repo, candidate, candidate_files, coverage_contract, toolchain
        )

    _assert_execution_seal(repo, candidate, toolchain)
    final_handoff = verify.verify_handoff(handoff_dir, repo=repo)
    if final_handoff != handoff:
        raise verify.VerificationError("WP2 packet changed during candidate execution")
    try:
        final_authorization = authorize.authorize(
            handoff_dir,
            repo,
            approval_revision=approval_revision,
        )
    except authorize.AuthorizationError as error:
        raise verify.VerificationError(
            f"offline approval changed during candidate execution: {error}"
        ) from error
    if final_authorization != authorization:
        raise verify.VerificationError(
            "offline approval ref/record changed during candidate execution"
        )
    return {
        "approval_commit": base,
        "authorization_source": authorization["authorization_source"],
        "base": base,
        "candidate": candidate,
        "changed_paths": len(changes),
        "coverage": coverage_result,
        "candidate_self_evidence_f_tests": candidate_self_evidence,
        "external_architecture_corpus": external_evidence,
        "lifecycle_differential": lifecycle_result,
        "toolchain": {
            "cargo_sha256": toolchain["cargo_sha256"],
            "resolved_cargo_sha256": toolchain["resolved_cargo_sha256"],
            "cargo_llvm_cov_sha256": toolchain["cargo_llvm_cov_sha256"],
            "git_sha256": toolchain["git_sha256"],
            "identity": "portable-logical-name-version-content-digest",
        },
        "work_package": work_package,
    }


def verify_scope(
    handoff_dir: Path,
    repo: Path,
    *,
    approval_revision: str,
    candidate_revision: str,
    work_package: str,
    require_clean: bool,
) -> dict[str, object]:
    if work_package != "WP2":
        raise verify.VerificationError(
            "the WP2 checkpoint only permits WP2; old WP1 and later work packages "
            "require their own architecture approval"
        )
    with _sanitized_git_environment():
        try:
            authorization = authorize.authorize(
                handoff_dir,
                repo,
                approval_revision=approval_revision,
            )
        except authorize.AuthorizationError as error:
            raise verify.VerificationError(
                f"offline WP2 authorization failed: {error}"
            ) from error
        handoff = verify.verify_handoff(handoff_dir, repo=None)
        toolchain = _resolve_frozen_cargo(
            handoff["spec"], handoff["toolchain_observed"]
        )
        with _absolute_git_runner(toolchain["git_path"]):
            return _verify_scope_sanitized(
                handoff_dir,
                repo,
                handoff,
                toolchain,
                authorization,
                approval_revision=approval_revision,
                candidate_revision=candidate_revision,
                work_package=work_package,
                require_clean=require_clean,
            )


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--handoff-dir", type=Path, required=True)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument(
        "--approval-commit",
        required=True,
        help="allowed v53 approval ref or exact approval commit object id",
    )
    parser.add_argument("--candidate", default="HEAD")
    parser.add_argument("--work-package", choices=["WP2"], required=True)
    parser.add_argument("--require-clean", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse_args(sys.argv[1:] if argv is None else argv)
    try:
        result = verify_scope(
            args.handoff_dir,
            args.repo,
            approval_revision=args.approval_commit,
            candidate_revision=args.candidate,
            work_package=args.work_package,
            require_clean=args.require_clean,
        )
    except verify.VerificationError as error:
        print(f"v53 scope verification failed: {error}", file=sys.stderr)
        return 1
    print(
        "v53 scope verified: "
        f"base={result['base']} candidate={result['candidate']} "
        f"changed_paths={result['changed_paths']} "
        "candidate_self_evidence_f_tests="
        f"{sum(len(names) for names in result['candidate_self_evidence_f_tests'].values())} "
        f"work_package={result['work_package']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
