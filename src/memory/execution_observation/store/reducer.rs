//! The single pure attempt-state reducer (ADR-0009/ADR-0010).
//!
//! Home: `store::reducer` since WP3B.1-A (convergence); the reader re-exports
//! from here so exactly one reducer implementation exists crate-wide.
//!
//! Input: stored events that have already passed the structural chain's
//! schema/hash/ordinal validation, ordered by strictly contiguous sequence
//! starting at 1. Output: attempts ordered by canonical key (execution UUID
//! bytes, then attempt number). Duplicate Started, Terminal without Started,
//! cross-attempt policy/runtime rebind, a second different Terminal, sequence
//! gaps/repeats, ordinal inconsistency, and ledger-cap breaches are all
//! stable CorruptStore rejects.

use std::cmp::Ordering;

use super::super::error::{CorruptionCategory, ObservationStoreError};
use super::super::ids::ExecutionAttemptKeyV1;
use super::super::EVENTS_MAX;

/// The event kind plus the caller-binding fields the rebind check needs.
#[derive(Debug, Clone)]
pub(crate) enum ReducibleKindV1 {
    Started {
        policy_sha256: String,
        runtime_sha256: String,
    },
    Terminal {
        policy_sha256: String,
        runtime_sha256: String,
    },
}

/// One validated stored event, flattened for the reducer.
#[derive(Debug, Clone)]
pub(crate) struct ReducibleEventV1 {
    pub(crate) sequence: u64,
    pub(crate) root_generation: u64,
    pub(crate) root_sha256: String,
    pub(crate) recorded_at_ms: u64,
    pub(crate) event_sha256: String,
    pub(crate) request_sha256: String,
    pub(crate) key: ExecutionAttemptKeyV1,
    pub(crate) kind: ReducibleKindV1,
}

/// The receipt-bearing facts one stored event contributes to an attempt.
#[derive(Debug)]
pub(crate) struct ReducibleReceiptV1 {
    pub(crate) request_sha256: String,
    pub(crate) event_sha256: String,
    pub(crate) sequence: u64,
    pub(crate) root_generation: u64,
    pub(crate) root_sha256: String,
    pub(crate) recorded_at_ms: u64,
}

struct AttemptState {
    key: ExecutionAttemptKeyV1,
    started: ReducibleReceiptV1,
    started_policy: String,
    started_runtime: String,
    terminal: Option<ReducibleReceiptV1>,
}

/// Reducer output: one attempt in canonical key order.
#[derive(Debug)]
pub(crate) struct ReducibleAttemptV1 {
    pub(crate) key: ExecutionAttemptKeyV1,
    pub(crate) started: ReducibleReceiptV1,
    pub(crate) terminal: Option<ReducibleReceiptV1>,
}

fn corrupt(category: CorruptionCategory) -> ObservationStoreError {
    ObservationStoreError::corrupt(category)
}

/// Canonical ordering: execution UUID bytes, then attempt number.
pub(crate) fn attempt_ordering(attempt: &ReducibleAttemptV1, needle: ([u8; 16], u32)) -> Ordering {
    let key = (attempt.key.execution_id.as_bytes(), attempt.key.attempt.get());
    key.cmp(&needle)
}

/// Folds validated events, in the given order, into attempt state. The
/// sequencing contract (1, 2, 3, ... — no gaps, no repeats) is enforced here
/// so every consumer inherits the same fail-closed behavior.
pub(crate) fn reduce(events: Vec<ReducibleEventV1>) -> Result<Vec<ReducibleAttemptV1>, ObservationStoreError> {
    if events.len() as u64 > EVENTS_MAX {
        return Err(corrupt(CorruptionCategory::StoredResourceLimit));
    }
    let mut attempts: Vec<AttemptState> = Vec::new();
    for (index, event) in events.iter().enumerate() {
        if event.sequence != index as u64 + 1 {
            return Err(corrupt(CorruptionCategory::SequenceGap));
        }
        if event.root_generation != event.sequence {
            return Err(corrupt(CorruptionCategory::GenerationMismatch));
        }
        let receipt = ReducibleReceiptV1 {
            request_sha256: event.request_sha256.clone(),
            event_sha256: event.event_sha256.clone(),
            sequence: event.sequence,
            root_generation: event.root_generation,
            root_sha256: event.root_sha256.clone(),
            recorded_at_ms: event.recorded_at_ms,
        };
        match &event.kind {
            ReducibleKindV1::Started {
                policy_sha256,
                runtime_sha256,
            } => {
                if find(&attempts, &event.key).is_some() {
                    return Err(corrupt(CorruptionCategory::DuplicateStarted));
                }
                attempts.push(AttemptState {
                    key: event.key,
                    started: receipt,
                    started_policy: policy_sha256.clone(),
                    started_runtime: runtime_sha256.clone(),
                    terminal: None,
                });
            }
            ReducibleKindV1::Terminal {
                policy_sha256,
                runtime_sha256,
            } => {
                let Some(position) = find(&attempts, &event.key) else {
                    return Err(corrupt(CorruptionCategory::InvalidTransition));
                };
                let state = &mut attempts[position];
                if state.terminal.is_some() {
                    return Err(corrupt(CorruptionCategory::DuplicateTerminal));
                }
                if *policy_sha256 != state.started_policy || *runtime_sha256 != state.started_runtime {
                    return Err(corrupt(CorruptionCategory::InvalidTransition));
                }
                state.terminal = Some(receipt);
            }
        }
    }
    attempts.sort_by(|left, right| {
        let left_key = (left.key.execution_id.as_bytes(), left.key.attempt.get());
        let right_key = (right.key.execution_id.as_bytes(), right.key.attempt.get());
        left_key.cmp(&right_key)
    });
    Ok(attempts
        .into_iter()
        .map(|state| ReducibleAttemptV1 {
            key: state.key,
            started: state.started,
            terminal: state.terminal,
        })
        .collect())
}

fn find(attempts: &[AttemptState], key: &ExecutionAttemptKeyV1) -> Option<usize> {
    attempts.iter().position(|state| &state.key == key)
}
