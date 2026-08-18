//! WP3B.1 facade self-tests: the ADR-0010 corpus (C01-C12) bound to
//! concrete fixtures. Physical chains live on tempfile vaults; concurrency
//! uses scoped threads against one handle; zero-mutation claims are pinned
//! by whole-tree fingerprints (path/kind/mode/bytes).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;

use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;
use crate::cas::PersonalVaultStorage;

use super::super::super::canonical::to_canonical_vec;
use super::super::super::error::{
    CorruptionCategory, LimitCategory, ObservationStoreError, TransitionConflictCategory,
};
use super::super::super::hash;
use super::super::super::ids::{
    CanonicalUuid, EventKind, ExecutionAttemptKeyV1, FailureCategoryV1, FixtureOriginV1, TerminalOutcomeV1,
};
use super::super::super::model::{
    AppendStartedRequestV1, AppendTerminalRequestV1, FixtureActivePointerV1, FixtureAttemptViewV1,
    FixtureCurrentViewV1, FixtureEventSegmentV1, FixtureLedgerRootV1, ObservationReceiptV1, StoredStartedEventV1,
    ATTESTATION_STATE, CURRENT_VIEW_SCHEMA, POINTER_SCHEMA, ROOT_SCHEMA, SEGMENT_SCHEMA, STARTED_EVENT_SCHEMA,
    STARTED_REQUEST_SCHEMA, TERMINAL_REQUEST_SCHEMA, TRUST_CLASS,
};
use super::super::super::validation::CANONICAL_REQUEST_MAX_BYTES;
use super::super::super::{CURRENT_VIEW_MAX_BYTES, ROOT_MAX_BYTES};
use super::super::clock::JSON_SAFE_MAX_MS;
use super::FixtureObservationLedgerV1;

/// A distinct, well-formed lowercase-hex64 digest (distinct for all seeds
/// below 2^16).
fn digest(seed: u16) -> String {
    format!("{seed:064x}")
}

fn execution_id(seed: u8) -> CanonicalUuid {
    CanonicalUuid::from_canonical_str(&format!("00000000-0000-0000-0000-0000000000{seed:02x}"))
        .expect("non-nil canonical uuid")
}

fn attempt_key(seed: u8, attempt: u32) -> ExecutionAttemptKeyV1 {
    ExecutionAttemptKeyV1::from_parts(execution_id(seed), attempt).expect("valid key")
}

fn started_request(key: &ExecutionAttemptKeyV1, policy_seed: u16) -> AppendStartedRequestV1 {
    AppendStartedRequestV1 {
        schema: STARTED_REQUEST_SCHEMA.to_string(),
        key: *key,
        fixture_origin: FixtureOriginV1::InternalTask {
            task_id: execution_id(key.execution_id.as_bytes()[0] | 0x10),
        },
        attestation_state: ATTESTATION_STATE.to_string(),
        fixture_role_ref: None,
        fixture_session_ref: None,
        operation_contract_sha256: digest(0xA0),
        input_evidence_cids: vec![digest(0xB0)],
        context_evidence_cids: vec![digest(0xB1)],
        policy_sha256: digest(policy_seed),
        runtime_sha256: digest(0xC0),
    }
}

fn terminal_request(key: &ExecutionAttemptKeyV1, variant: u16) -> AppendTerminalRequestV1 {
    AppendTerminalRequestV1 {
        schema: TERMINAL_REQUEST_SCHEMA.to_string(),
        key: *key,
        attestation_state: ATTESTATION_STATE.to_string(),
        outcome: match variant % 5 {
            0 => TerminalOutcomeV1::Success,
            1 => TerminalOutcomeV1::Failure {
                category: FailureCategoryV1::Internal,
            },
            2 => TerminalOutcomeV1::Timeout,
            3 => TerminalOutcomeV1::Cancelled,
            _ => TerminalOutcomeV1::Indeterminate,
        },
        output_evidence_cids: vec![digest(0xD0)],
        execution_elapsed_ms: Some(u64::from(variant) * 1_000),
        policy_sha256: digest(1),
        runtime_sha256: digest(0xC0),
    }
}

fn open_facade(path: &Path) -> FixtureObservationLedgerV1 {
    let owner = Arc::new(PersonalVaultStorage::open(path, None).expect("vault"));
    FixtureObservationLedgerV1::open_fixture(owner).expect("facade opens")
}

fn observation_dir(path: &Path) -> PathBuf {
    path.join("execution-observation-fixture-ledger")
}

/// SHA-256 fingerprint over every entry under `root`: relative path, file
/// kind, mode, and — for regular files — the bytes.
fn vault_fingerprint(root: &Path) -> String {
    fn collect(root: &Path, directory: &Path, entries: &mut Vec<Vec<u8>>) {
        let mut children: Vec<PathBuf> = std::fs::read_dir(directory)
            .expect("read_dir")
            .map(|entry| entry.expect("entry").path())
            .collect();
        children.sort();
        for child in children {
            let metadata = std::fs::symlink_metadata(&child).expect("symlink_metadata");
            let mut record = format!(
                "{}|{:?}|{:o}|",
                child.strip_prefix(root).expect("prefix").display(),
                metadata.file_type(),
                metadata.permissions().mode(),
            )
            .into_bytes();
            if metadata.is_dir() {
                collect(root, &child, entries);
            } else if metadata.is_file() {
                record.extend(std::fs::read(&child).expect("read"));
            }
            entries.push(record);
        }
    }
    let mut entries = Vec::new();
    collect(root, root, &mut entries);
    entries.sort();
    let mut hasher = Sha256::new();
    hasher.update(format!("entries={}\n", entries.len()));
    for entry in &entries {
        hasher.update(entry);
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

/// C01: identical Started retries (same handle and reopened) return the
/// first receipt with a byte-identical tree.
#[test]
fn execution_observation_facade_started_idempotency_zero_mutation() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let key = attempt_key(1, 1);
    let request = started_request(&key, 1);
    let facade = open_facade(&path);
    let first = facade.append_started(request.clone()).expect("first started");

    let after_first = vault_fingerprint(&path);
    let retry = facade.append_started(request.clone()).expect("same-handle retry");
    assert_eq!(retry, first);
    assert_eq!(vault_fingerprint(&path), after_first);

    drop(facade);
    let reopened = open_facade(&path);
    let reopened_retry = reopened.append_started(request).expect("reopened retry");
    assert_eq!(reopened_retry, first);
    assert_eq!(vault_fingerprint(&path), after_first);
}

/// C02: exactly one Started per key; sequential and concurrent different
/// Started attempts are typed conflicts with zero mutation.
#[test]
fn execution_observation_facade_started_conflict_sequential_and_concurrent() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let key = attempt_key(2, 1);
    let facade = open_facade(&path);
    facade.append_started(started_request(&key, 1)).expect("first started");
    let fingerprint = vault_fingerprint(&path);

    let different = facade
        .append_started(started_request(&key, 2))
        .expect_err("rebind rejected");
    assert_eq!(
        different,
        ObservationStoreError::conflict(TransitionConflictCategory::StartedAlreadyBound)
    );
    assert_eq!(vault_fingerprint(&path), fingerprint);

    let second_key = attempt_key(3, 1);
    let loser = started_request(&second_key, 7);
    let winner = started_request(&second_key, 8);
    let barrier = Barrier::new(2);
    thread::scope(|scope| {
        let left = scope.spawn(|| {
            barrier.wait();
            facade.append_started(loser.clone())
        });
        let right = scope.spawn(|| {
            barrier.wait();
            facade.append_started(winner.clone())
        });
        let results = [left.join().expect("thread"), right.join().expect("thread")];
        let accepted = results.iter().filter(|result| result.is_ok()).count();
        assert_eq!(accepted, 1, "exactly one Started accepted per key");
        for result in results {
            if let Err(error) = result {
                assert_eq!(
                    error,
                    ObservationStoreError::conflict(TransitionConflictCategory::StartedAlreadyBound)
                );
            }
        }
    });
}

/// C03: concurrent identical Terminals and a reopened retry all converge
/// on the first receipt.
#[test]
fn execution_observation_facade_terminal_idempotency_concurrent() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let key = attempt_key(4, 1);
    let facade = open_facade(&path);
    facade.append_started(started_request(&key, 1)).expect("started");
    let request = terminal_request(&key, 0);
    let first = facade.append_terminal(request.clone()).expect("terminal");
    let fingerprint = vault_fingerprint(&path);

    let barrier = Barrier::new(8);
    thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    facade.append_terminal(request.clone())
                })
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.join().expect("thread").expect("retry ok"), first);
        }
    });
    assert_eq!(vault_fingerprint(&path), fingerprint);

    drop(facade);
    let reopened = open_facade(&path);
    assert_eq!(reopened.append_terminal(request).expect("reopened retry"), first);
    assert_eq!(vault_fingerprint(&path), fingerprint);
}

/// C04: every rebind dimension of a bound terminal is a typed conflict
/// with zero mutation; terminal-without-started keeps its category.
#[test]
fn execution_observation_facade_terminal_conflict_rebind_matrix() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let key = attempt_key(5, 1);
    let facade = open_facade(&path);
    facade.append_started(started_request(&key, 1)).expect("started");
    facade.append_terminal(terminal_request(&key, 0)).expect("terminal");

    let mut different_outcome = terminal_request(&key, 0);
    different_outcome.outcome = TerminalOutcomeV1::Timeout;
    assert_eq!(
        facade.append_terminal(different_outcome).expect_err("outcome rebind"),
        ObservationStoreError::conflict(TransitionConflictCategory::TerminalAlreadyBound)
    );

    let mut different_evidence = terminal_request(&key, 0);
    different_evidence.output_evidence_cids = vec![digest(0xEE)];
    assert_eq!(
        facade.append_terminal(different_evidence).expect_err("evidence rebind"),
        ObservationStoreError::conflict(TransitionConflictCategory::TerminalAlreadyBound)
    );

    let mut different_elapsed = terminal_request(&key, 0);
    different_elapsed.execution_elapsed_ms = Some(999);
    assert_eq!(
        facade.append_terminal(different_elapsed).expect_err("elapsed rebind"),
        ObservationStoreError::conflict(TransitionConflictCategory::TerminalAlreadyBound)
    );

    let mut different_policy = terminal_request(&key, 0);
    different_policy.policy_sha256 = digest(0x99);
    assert_eq!(
        facade.append_terminal(different_policy).expect_err("policy rebind"),
        ObservationStoreError::conflict(TransitionConflictCategory::TerminalPolicyRebind)
    );

    let mut different_runtime = terminal_request(&key, 0);
    different_runtime.runtime_sha256 = digest(0x98);
    assert_eq!(
        facade.append_terminal(different_runtime).expect_err("runtime rebind"),
        ObservationStoreError::conflict(TransitionConflictCategory::TerminalRuntimeRebind)
    );

    assert_eq!(
        facade
            .append_terminal(terminal_request(&attempt_key(6, 1), 0))
            .expect_err("terminal without started"),
        ObservationStoreError::conflict(TransitionConflictCategory::TerminalWithoutStarted)
    );

    // First-Terminal rebinds against the bound Started keep their typed
    // categories too (frozen classifier), not a corruption signal.
    let open_key = attempt_key(21, 1);
    facade
        .append_started(started_request(&open_key, 1))
        .expect("open started");

    // The three-list shared evidence budget (started input + context +
    // terminal output <= 512) is enforced by the same frozen classifier.
    let heavy_key = attempt_key(22, 1);
    let mut heavy = started_request(&heavy_key, 1);
    heavy.input_evidence_cids = (0..256).map(digest).collect();
    heavy.context_evidence_cids = (256..512).map(digest).collect();
    facade
        .append_started(heavy)
        .expect("saturating started at the per-request budget");
    let fingerprint = vault_fingerprint(&path);

    let mut wrong_policy = terminal_request(&open_key, 0);
    wrong_policy.policy_sha256 = digest(0x99);
    assert_eq!(
        facade
            .append_terminal(wrong_policy)
            .expect_err("first-terminal policy rebind"),
        ObservationStoreError::conflict(TransitionConflictCategory::TerminalPolicyRebind)
    );
    let mut wrong_runtime = terminal_request(&open_key, 0);
    wrong_runtime.runtime_sha256 = digest(0x98);
    assert_eq!(
        facade
            .append_terminal(wrong_runtime)
            .expect_err("first-terminal runtime rebind"),
        ObservationStoreError::conflict(TransitionConflictCategory::TerminalRuntimeRebind)
    );
    assert_eq!(
        facade
            .append_terminal(terminal_request(&heavy_key, 0))
            .expect_err("three-list budget"),
        ObservationStoreError::limit(LimitCategory::EvidenceTotal)
    );
    assert_eq!(vault_fingerprint(&path), fingerprint);
}

/// C05: after restart, reads and idempotent retries are field-identical
/// to the first-commit results.
#[test]
fn execution_observation_facade_restart_equality() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let key = attempt_key(7, 1);
    let started = started_request(&key, 1);
    let terminal = terminal_request(&key, 1);

    let facade = open_facade(&path);
    let started_receipt = facade.append_started(started.clone()).expect("started");
    let terminal_receipt = facade.append_terminal(terminal.clone()).expect("terminal");
    let observation = facade.read_attempt(&key).expect("read").expect("present");
    drop(facade);

    let reopened = open_facade(&path);
    let reopened_observation = reopened.read_attempt(&key).expect("read").expect("present");
    assert_eq!(reopened_observation, observation);
    assert_eq!(reopened_observation.attestation_state, ATTESTATION_STATE);
    assert_eq!(
        reopened.append_started(started).expect("started retry"),
        started_receipt
    );
    assert_eq!(
        reopened.append_terminal(terminal).expect("terminal retry"),
        terminal_receipt
    );
}

/// C06: all five outcome classes (failure across its categories) restart
/// identically with round-tripping receipts.
#[test]
fn execution_observation_facade_terminal_outcomes_restart_matrix() {
    let parent = TempDir::new().expect("temp parent");
    let outcomes = [
        TerminalOutcomeV1::Success,
        TerminalOutcomeV1::Failure {
            category: FailureCategoryV1::InvalidInput,
        },
        TerminalOutcomeV1::Failure {
            category: FailureCategoryV1::PolicyDenied,
        },
        TerminalOutcomeV1::Failure {
            category: FailureCategoryV1::DependencyUnavailable,
        },
        TerminalOutcomeV1::Failure {
            category: FailureCategoryV1::ExecutorRejected,
        },
        TerminalOutcomeV1::Failure {
            category: FailureCategoryV1::ExecutorFailed,
        },
        TerminalOutcomeV1::Failure {
            category: FailureCategoryV1::ExecutorPanicked,
        },
        TerminalOutcomeV1::Failure {
            category: FailureCategoryV1::ToolFailed,
        },
        TerminalOutcomeV1::Failure {
            category: FailureCategoryV1::Internal,
        },
        TerminalOutcomeV1::Timeout,
        TerminalOutcomeV1::Cancelled,
        TerminalOutcomeV1::Indeterminate,
    ];
    for (index, outcome) in outcomes.iter().enumerate() {
        let path = parent.path().join(format!("vault-{index}"));
        let key = attempt_key(8, u32::try_from(index + 1).expect("attempt"));
        let mut terminal = terminal_request(&key, 0);
        terminal.outcome = *outcome;
        let facade = open_facade(&path);
        let started_receipt = facade.append_started(started_request(&key, 1)).expect("started");
        let terminal_receipt = facade.append_terminal(terminal).expect("terminal");
        let observation = facade.read_attempt(&key).expect("read").expect("present");
        drop(facade);

        let reopened = open_facade(&path);
        assert_eq!(
            reopened.read_attempt(&key).expect("read"),
            Some(observation),
            "outcome {outcome:?}"
        );
        assert_eq!(
            reopened.append_terminal(clone_terminal(&key, *outcome)).expect("retry"),
            terminal_receipt
        );
        assert_eq!(
            reopened.append_started(started_request(&key, 1)).expect("retry"),
            started_receipt
        );
    }
}

fn clone_terminal(key: &ExecutionAttemptKeyV1, outcome: TerminalOutcomeV1) -> AppendTerminalRequestV1 {
    let mut request = terminal_request(key, 0);
    request.outcome = outcome;
    request
}

/// C07: a pre-exchange failure returns a storage error with no receipt and
/// leaves the old active intact; the un-injected retry succeeds exactly once.
#[test]
fn execution_observation_facade_pre_exchange_failure_retry() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let key = attempt_key(9, 1);
    let facade = open_facade(&path);
    facade.store.inject_pre_exchange_failure_once();
    let failure = facade
        .append_started(started_request(&key, 1))
        .expect_err("pre-exchange failure surfaces");
    assert_eq!(failure, ObservationStoreError::StorageUnavailable);
    assert!(
        facade.read_attempt(&key).expect("read after failure").is_none(),
        "no partial state after a pre-exchange failure"
    );

    let accepted = facade.append_started(started_request(&key, 1)).expect("retry succeeds");
    assert_eq!(accepted.sequence, 1);
    assert!(facade
        .read_attempt(&key)
        .expect("read")
        .expect("present")
        .terminal_receipt
        .is_none());
}

/// C08: post-exchange uncertainty poisons the handle for reads and writes;
/// reopen reconciles strictly from the authoritative active.
#[test]
fn execution_observation_facade_post_exchange_poison_and_reopen() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let key = attempt_key(10, 1);
    let facade = open_facade(&path);
    facade.append_started(started_request(&key, 1)).expect("started");
    facade.store.inject_post_exchange_sync_failure_once();
    assert_eq!(
        facade
            .append_terminal(terminal_request(&key, 0))
            .expect_err("indeterminate"),
        ObservationStoreError::CommitIndeterminate
    );
    assert_eq!(
        facade.read_attempt(&key).expect_err("read poisoned"),
        ObservationStoreError::Poisoned
    );
    assert_eq!(
        facade
            .append_started(started_request(&attempt_key(11, 1), 1))
            .expect_err("write poisoned"),
        ObservationStoreError::Poisoned
    );
    drop(facade);

    let reopened = open_facade(&path);
    let observation = reopened.read_attempt(&key).expect("read").expect("present");
    assert!(
        observation.terminal_receipt.is_some(),
        "exchanged root reconciled from active"
    );
    assert_eq!(
        reopened
            .append_terminal(terminal_request(&key, 0))
            .expect("retry after reopen"),
        observation.terminal_receipt.expect("terminal receipt")
    );
}

/// C10: rollback is absorbed by the max rule, equal milliseconds still
/// advance ordinals, beyond-JSON-safe input is a typed limit with zero
/// mutation, and idempotent hits never consume the clock.
#[test]
fn execution_observation_facade_clock_matrix() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let facade = open_facade(&path);

    facade.set_clock_for_test(Some(1_000)).expect("seam");
    let first = facade
        .append_started(started_request(&attempt_key(12, 1), 1))
        .expect("started at 1000");
    assert_eq!(first.recorded_at_ms, 1_000);

    facade.set_clock_for_test(Some(500)).expect("seam");
    let second = facade
        .append_started(started_request(&attempt_key(13, 1), 1))
        .expect("rollback absorbed");
    assert_eq!(second.recorded_at_ms, 1_000, "the clock never regresses");

    facade.set_clock_for_test(Some(1_000)).expect("seam");
    let third = facade
        .append_started(started_request(&attempt_key(14, 1), 1))
        .expect("same millisecond");
    assert_eq!(third.recorded_at_ms, 1_000);
    assert_eq!(third.sequence, 3, "equal milliseconds still advance ordinals");

    facade.set_clock_for_test(Some(JSON_SAFE_MAX_MS + 1)).expect("seam");
    let fingerprint = vault_fingerprint(&path);
    assert_eq!(
        facade
            .append_started(started_request(&attempt_key(15, 1), 1))
            .expect_err("beyond JSON-safe"),
        ObservationStoreError::limit(LimitCategory::Event)
    );
    assert_eq!(
        vault_fingerprint(&path),
        fingerprint,
        "overflow rejects with zero mutation"
    );

    // The idempotency decision precedes the clock read: with the seam still
    // parked beyond the JSON-safe ceiling, an identical retry must return
    // the first receipt instead of the clock's limit rejection.
    assert_eq!(
        facade
            .append_started(started_request(&attempt_key(12, 1), 1))
            .expect("idempotent retry at an overflowing clock"),
        first
    );

    // Idempotent hits do not consume the clock: the retry keeps the first
    // recorded_at even with a much later clock setting.
    facade.set_clock_for_test(Some(9_000)).expect("seam");
    assert_eq!(
        facade
            .append_started(started_request(&attempt_key(12, 1), 1))
            .expect("idempotent retry"),
        first
    );
    facade.set_clock_for_test(Some(2_000)).expect("seam");
    let fourth = facade
        .append_started(started_request(&attempt_key(15, 1), 1))
        .expect("append after overflow failure");
    assert_eq!(fourth.recorded_at_ms, 2_000);
}

/// C11: barriered races against one handle linearize — identical requests
/// converge on one receipt, distinct ones leave typed losers, and a final
/// reopen replays exactly the facade's state.
#[test]
fn execution_observation_facade_concurrency_single_linearization() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let key = attempt_key(16, 1);
    let facade = open_facade(&path);

    // (a) same-Started x8: one commit, eight identical receipts.
    let started = started_request(&key, 1);
    let barrier = Barrier::new(8);
    thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    facade.append_started(started.clone())
                })
            })
            .collect();
        let receipts: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread").expect("identical started ok"))
            .collect();
        assert!(receipts.windows(2).all(|pair| pair[0] == pair[1]));
    });

    // (b) same-Terminal x8: same convergence.
    let terminal = terminal_request(&key, 0);
    let barrier = Barrier::new(8);
    thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    facade.append_terminal(terminal.clone())
                })
            })
            .collect();
        let receipts: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread").expect("identical terminal ok"))
            .collect();
        assert!(receipts.windows(2).all(|pair| pair[0] == pair[1]));
    });

    // (c) distinct-Terminal x2 on a fresh open attempt: exactly one success.
    let race_key = attempt_key(20, 1);
    facade
        .append_started(started_request(&race_key, 1))
        .expect("started for the terminal race");
    let barrier = Barrier::new(2);
    thread::scope(|scope| {
        let left = scope.spawn(|| {
            barrier.wait();
            facade.append_terminal(terminal_request(&race_key, 2))
        });
        let right = scope.spawn(|| {
            barrier.wait();
            facade.append_terminal(terminal_request(&race_key, 3))
        });
        let results = [left.join().expect("thread"), right.join().expect("thread")];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    });

    // (d) Started(K2) interleaved with Terminal(K2): either order is a
    // legal linearization; both must leave a consistent chain.
    let second_key = attempt_key(17, 1);
    let barrier = Barrier::new(2);
    thread::scope(|scope| {
        let left = scope.spawn(|| {
            barrier.wait();
            facade.append_started(started_request(&second_key, 1))
        });
        let right = scope.spawn(|| {
            barrier.wait();
            facade.append_terminal(terminal_request(&second_key, 1))
        });
        let _ = left.join().expect("thread");
        let _ = right.join().expect("thread");
    });

    let observed = facade.read_attempt(&key).expect("read").expect("present");
    assert!(observed.terminal_receipt.is_some());
    drop(facade);
    let reopened = open_facade(&path);
    assert_eq!(reopened.read_attempt(&key).expect("read"), Some(observed));
}

/// C09: a fully valid child chain left in the candidate slot is never
/// promoted or merged — opens read the active chain only and the next
/// append builds on the active head.
#[test]
fn execution_observation_facade_candidate_never_promoted() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let key = attempt_key(18, 1);
    let forged_key = attempt_key(19, 1);
    let facade = open_facade(&path);
    let first = facade.append_started(started_request(&key, 1)).expect("started");
    drop(facade);

    // Forge a valid child of the ACTIVE head for a different key and park
    // it in the candidate slot (the sealed writer seam places objects; the
    // pointer bytes are written directly, exactly like the architecture
    // corpus stages candidates).
    let forged = forged_child_bundle(&first, &key, &forged_key);
    let owner = Arc::new(PersonalVaultStorage::open(&path, None).expect("vault"));
    let capability = ExecutionObservationFixtureStorage::open(owner).expect("capability");
    capability
        .put_immutable_bounded(
            &forged.event_sha256,
            &to_canonical_vec(&forged.event).expect("event"),
            (CANONICAL_REQUEST_MAX_BYTES + 4_096) as u64,
        )
        .expect("put forged event");
    capability
        .put_immutable_bounded(
            &forged.segment_sha256,
            &to_canonical_vec(&forged.segment).expect("segment"),
            65_536,
        )
        .expect("put forged segment");
    capability
        .put_immutable_bounded(
            &forged.view_sha256,
            &to_canonical_vec(&forged.view).expect("view"),
            CURRENT_VIEW_MAX_BYTES as u64,
        )
        .expect("put forged view");
    capability
        .put_immutable_bounded(
            &forged.root_sha256,
            &to_canonical_vec(&forged.root).expect("root"),
            ROOT_MAX_BYTES as u64,
        )
        .expect("put forged root");
    let pointer = FixtureActivePointerV1 {
        schema: POINTER_SCHEMA.to_string(),
        root_sha256: forged.root_sha256.clone(),
    };
    let candidate = observation_dir(&path).join("roots").join("candidate");
    std::fs::write(&candidate, to_canonical_vec(&pointer).expect("pointer")).expect("stage candidate");
    std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o600)).expect("private mode");
    drop(capability);

    let facade = open_facade(&path);
    assert!(
        facade.read_attempt(&forged_key).expect("read").is_none(),
        "candidate data is never merged into reads"
    );
    assert!(facade.read_attempt(&key).expect("read").is_some());
    let terminal = facade
        .append_terminal(terminal_request(&key, 1))
        .expect("append builds on the active head");
    assert_ne!(
        terminal.root_sha256, forged.root_sha256,
        "the append child is its own root, never the parked candidate"
    );
    assert_eq!(terminal.sequence, 2);

    // The contradiction variant: a candidate root that is neither the
    // direct child nor the direct parent of the active head (a two-step
    // descendant) fails closed through the existing slot classification
    // and is never auto-repaired.
    let contradict_path = parent.path().join("vault-contradiction");
    let contradict_key = attempt_key(23, 1);
    let facade = open_facade(&contradict_path);
    let accepted = facade
        .append_started(started_request(&contradict_key, 1))
        .expect("started");
    drop(facade);
    let skipping_root = FixtureLedgerRootV1 {
        schema: ROOT_SCHEMA.to_string(),
        trust_class: TRUST_CLASS.to_string(),
        generation: 3,
        previous_root_sha256: Some(accepted.root_sha256.clone()),
        event_segment_head_sha256: Some(digest(1)),
        event_watermark: 3,
        current_view_sha256: digest(2),
        committed_at_ms: accepted.recorded_at_ms,
    };
    let skipping_root_sha256 = hash::root_sha256(&skipping_root).expect("root hash");
    let owner = Arc::new(PersonalVaultStorage::open(&contradict_path, None).expect("vault"));
    let capability = ExecutionObservationFixtureStorage::open(owner).expect("capability");
    capability
        .put_immutable_bounded(
            &skipping_root_sha256,
            &to_canonical_vec(&skipping_root).expect("root"),
            ROOT_MAX_BYTES as u64,
        )
        .expect("put skipping root");
    drop(capability);
    let candidate = observation_dir(&contradict_path).join("roots").join("candidate");
    let pointer = FixtureActivePointerV1 {
        schema: POINTER_SCHEMA.to_string(),
        root_sha256: skipping_root_sha256.clone(),
    };
    std::fs::write(&candidate, to_canonical_vec(&pointer).expect("pointer")).expect("stage contradiction");
    std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o600)).expect("private mode");
    let owner = Arc::new(PersonalVaultStorage::open(&contradict_path, None).expect("vault"));
    let failure = match FixtureObservationLedgerV1::open_fixture(owner) {
        Ok(_) => unreachable!("a contradicting candidate must fail closed"),
        Err(error) => error,
    };
    assert_eq!(
        failure,
        ObservationStoreError::corrupt(CorruptionCategory::InvalidCandidateState)
    );
}

struct ForgedChild {
    event: StoredStartedEventV1,
    segment: FixtureEventSegmentV1,
    view: FixtureCurrentViewV1,
    root: FixtureLedgerRootV1,
    event_sha256: String,
    segment_sha256: String,
    view_sha256: String,
    root_sha256: String,
}

/// Rebuilds the accepted generation-1 view entry and segment identity from
/// the receipt, then derives a valid generation-2 child for `forged_key`
/// (same rehash order as the publisher).
fn forged_child_bundle(
    accepted: &ObservationReceiptV1,
    key: &ExecutionAttemptKeyV1,
    forged_key: &ExecutionAttemptKeyV1,
) -> ForgedChild {
    let request = started_request(forged_key, 1);
    let request_sha256 = hash::started_request_sha256(&request).expect("request hash");
    let event = StoredStartedEventV1 {
        schema: STARTED_EVENT_SCHEMA.to_string(),
        request,
        request_sha256,
        sequence: 2,
        root_generation: 2,
        recorded_at_ms: accepted.recorded_at_ms,
    };
    let event_sha256 = hash::started_event_sha256(&event).expect("event hash");
    // The generation-1 segment is deterministic: one Started event whose
    // parent is the genesis root (no previous segment).
    let accepted_segment = FixtureEventSegmentV1 {
        schema: SEGMENT_SCHEMA.to_string(),
        first_sequence: 1,
        last_sequence: 1,
        previous_segment_sha256: None,
        event_kind: EventKind::Started,
        event_sha256: accepted.event_sha256.clone(),
    };
    let accepted_segment_sha256 = hash::segment_sha256(&accepted_segment).expect("accepted segment hash");
    let segment = FixtureEventSegmentV1 {
        schema: SEGMENT_SCHEMA.to_string(),
        first_sequence: 2,
        last_sequence: 2,
        previous_segment_sha256: Some(accepted_segment_sha256),
        event_kind: EventKind::Started,
        event_sha256: event_sha256.clone(),
    };
    let segment_sha256 = hash::segment_sha256(&segment).expect("segment hash");
    let original = FixtureAttemptViewV1 {
        key: *key,
        attestation_state: ATTESTATION_STATE.to_string(),
        started_request_sha256: accepted.request_sha256.clone(),
        started_event_sha256: accepted.event_sha256.clone(),
        terminal_request_sha256: None,
        terminal_event_sha256: None,
    };
    let forged_view_entry = FixtureAttemptViewV1 {
        key: *forged_key,
        attestation_state: ATTESTATION_STATE.to_string(),
        started_request_sha256: event.request_sha256.clone(),
        started_event_sha256: event_sha256.clone(),
        terminal_request_sha256: None,
        terminal_event_sha256: None,
    };
    let mut attempts = vec![original, forged_view_entry];
    attempts.sort_by(|left, right| {
        (left.key.execution_id.as_bytes(), left.key.attempt)
            .cmp(&(right.key.execution_id.as_bytes(), right.key.attempt))
    });
    let view = FixtureCurrentViewV1 {
        schema: CURRENT_VIEW_SCHEMA.to_string(),
        attestation_state: ATTESTATION_STATE.to_string(),
        generation: 2,
        event_watermark: 2,
        attempts,
    };
    let view_sha256 = hash::current_view_sha256(&view).expect("view hash");
    let root = FixtureLedgerRootV1 {
        schema: ROOT_SCHEMA.to_string(),
        trust_class: TRUST_CLASS.to_string(),
        generation: 2,
        previous_root_sha256: Some(accepted.root_sha256.clone()),
        event_segment_head_sha256: Some(segment_sha256.clone()),
        event_watermark: 2,
        current_view_sha256: view_sha256.clone(),
        committed_at_ms: accepted.recorded_at_ms,
    };
    let root_sha256 = hash::root_sha256(&root).expect("root hash");
    ForgedChild {
        event,
        segment,
        view,
        root,
        event_sha256,
        segment_sha256,
        view_sha256,
        root_sha256,
    }
}

/// C12: default-off boundary — module inclusion alone creates nothing, and
/// the public catalog keeps its exact-14 shape.
#[test]
fn execution_observation_facade_default_off_boundary() {
    use crate::api::public::PUBLIC_OPERATIONS;

    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    drop(PersonalVaultStorage::open(&path, None).expect("vault"));
    assert!(
        !observation_dir(&path).exists(),
        "no observation namespace without facade use"
    );
    assert_eq!(PUBLIC_OPERATIONS.len(), 14, "public exact-14 catalog unchanged");
}
