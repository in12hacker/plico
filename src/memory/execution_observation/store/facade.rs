//! Single-writer append facade (ADR-0010; milestone v53 WP3B.1).
//!
//! Pure orchestration over the accepted structural store and the single
//! reducer: one facade mutex linearizes poison check → head equality →
//! transition/idempotency → clock → bundle → commit → state update, with
//! the lock order facade-state → structural transaction. Idempotent
//! retries return the first receipt rebuilt from accepted identity with
//! zero durable change; conflicting rebinds are typed rejections. The
//! candidate slot is never read or written here, and restart rebuilds
//! strictly from the authoritative active chain.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::cas::{ExistingExecutionObservationReadOnly, PersonalVaultStorage};

use super::super::canonical::parse_canonical;
use super::super::error::{CorruptionCategory, InvalidRequestCategory, ObservationStoreError};
use super::super::hash;
use super::super::ids::{EventKind, ExecutionAttemptKeyV1};
use super::super::model::{
    AppendStartedRequestV1, AppendTerminalRequestV1, FixtureAttemptObservationV1, FixtureAttemptViewV1,
    FixtureCurrentViewV1, FixtureEventSegmentV1, FixtureLedgerRootV1, ObservationReceiptV1, StoredStartedEventV1,
    StoredTerminalEventV1, ATTESTATION_STATE, CURRENT_VIEW_SCHEMA, ROOT_SCHEMA, SEGMENT_SCHEMA, STARTED_EVENT_SCHEMA,
    TERMINAL_EVENT_SCHEMA, TRUST_CLASS,
};
use super::super::validation::{validate_started_transition, validate_terminal_transition};
use super::super::{EVENTS_MAX, ROOT_MAX_BYTES, SEGMENT_MAX_BYTES};
use super::clock;
use super::reducer::{
    attempt_ordering, reduce, ReducibleAttemptV1, ReducibleEventV1, ReducibleKindV1, ReducibleReceiptV1,
};
use super::{
    FixtureObservationStoreV1, FixtureStoredEventV1, FixtureStructuralCommitV1, FixtureStructuralStateV1,
    STORED_EVENT_MAX_BYTES,
};

#[cfg(test)]
mod tests;

/// Head-identity cache plus the linearized event log the reducer consumes.
/// `started_requests` retains the accepted Started bodies the frozen WP1
/// transition validator needs; `clock_override_ms` is only ever written by
/// the test-only clock seam.
struct FacadeState {
    head: FixtureStructuralStateV1,
    head_segment_sha256: Option<String>,
    events: Vec<ReducibleEventV1>,
    started_requests: Vec<AppendStartedRequestV1>,
    poisoned: bool,
    clock_override_ms: Option<u64>,
}

impl FacadeState {
    fn system_now_ms(&self) -> u64 {
        #[cfg(test)]
        if let Some(fixed) = self.clock_override_ms {
            return fixed;
        }
        clock::system_now_ms()
    }
}

pub(crate) struct FixtureObservationLedgerV1 {
    vault: Arc<PersonalVaultStorage>,
    store: FixtureObservationStoreV1,
    state: Mutex<FacadeState>,
}

impl FixtureObservationLedgerV1 {
    /// Opens the ledger and rebuilds the linearized event log strictly from
    /// the authoritative active chain (never the candidate slot).
    pub(crate) fn open_fixture(vault: Arc<PersonalVaultStorage>) -> Result<Self, ObservationStoreError> {
        let store = FixtureObservationStoreV1::open_fixture(vault.clone())?;
        let head = store.structural_state()?;
        let RebuiltLedger {
            events,
            started_requests,
            head_segment_sha256,
        } = rebuild_from_active(&vault, &head)?;
        Ok(Self {
            vault,
            store,
            state: Mutex::new(FacadeState {
                head,
                head_segment_sha256,
                events,
                started_requests,
                poisoned: false,
                clock_override_ms: None,
            }),
        })
    }

    /// Appends Started; a canonical-identical retry returns the first
    /// receipt with zero durable change, a different Started on the same
    /// key is a typed conflict (ADR-0010 §4).
    pub(crate) fn append_started(
        &self,
        request: AppendStartedRequestV1,
    ) -> Result<ObservationReceiptV1, ObservationStoreError> {
        let mut state = self.lock()?;
        if state.poisoned {
            return Err(ObservationStoreError::Poisoned);
        }
        self.reconcile_head(&mut state)?;
        let attempts = reduce(state.events.clone())?;
        let existing = find_attempt(&attempts, &request.key).map(attempt_view);
        // Frozen WP1 transition classifier: absent -> Ok, canonical-identical
        // retry -> Ok (idempotent), any other Started for this key ->
        // started_already_bound.
        validate_started_transition(&request, existing.as_ref())?;
        if let Some(attempt) = find_attempt(&attempts, &request.key) {
            // The validator only returns Ok here for a canonical-identical
            // retry: the first receipt, rebuilt from accepted identity,
            // before any clock read or object construction.
            return Ok(receipt_from(&attempt.started));
        }
        let request_sha256 = hash::started_request_sha256(&request)?;
        let recorded_at_ms = clock::advance(state.system_now_ms(), previous_accepted_ms(&state.events))?;
        let sequence = state.head.event_watermark + 1;
        let event = StoredStartedEventV1 {
            schema: STARTED_EVENT_SCHEMA.to_string(),
            request: request.clone(),
            request_sha256,
            sequence,
            root_generation: sequence,
            recorded_at_ms,
        };
        let event_sha256 = hash::started_event_sha256(&event)?;
        self.append_event(
            &mut state,
            FixtureStoredEventV1::Started(event),
            event_sha256,
            recorded_at_ms,
            Some(request),
        )
    }

    /// Appends Terminal; identical retries converge on the first receipt,
    /// rebinds of a bound terminal are typed conflicts, and Terminal for an
    /// unknown attempt keeps its existing category (ADR-0010 §4).
    pub(crate) fn append_terminal(
        &self,
        request: AppendTerminalRequestV1,
    ) -> Result<ObservationReceiptV1, ObservationStoreError> {
        let mut state = self.lock()?;
        if state.poisoned {
            return Err(ObservationStoreError::Poisoned);
        }
        self.reconcile_head(&mut state)?;
        let attempts = reduce(state.events.clone())?;
        let existing = find_attempt(&attempts, &request.key).map(attempt_view);
        let bound_started = find_started_request(&state.started_requests, &request.key);
        // Frozen WP1 transition classifier: terminal-without-started,
        // first-Terminal and rebind policy/runtime mismatch, the three-list
        // evidence budget, and bound-terminal identity are all classified
        // here — the facade never re-derives these rules.
        validate_terminal_transition(&request, existing.as_ref(), bound_started)?;
        if let Some(attempt) = find_attempt(&attempts, &request.key) {
            if let Some(first) = &attempt.terminal {
                // The validator only returns Ok for a bound terminal when
                // the canonical digest matches: the idempotent first receipt.
                return Ok(receipt_from(first));
            }
        }
        let request_sha256 = hash::terminal_request_sha256(&request)?;
        let recorded_at_ms = clock::advance(state.system_now_ms(), previous_accepted_ms(&state.events))?;
        let sequence = state.head.event_watermark + 1;
        let event = StoredTerminalEventV1 {
            schema: TERMINAL_EVENT_SCHEMA.to_string(),
            request,
            request_sha256,
            sequence,
            root_generation: sequence,
            recorded_at_ms,
        };
        let event_sha256 = hash::terminal_event_sha256(&event)?;
        self.append_event(
            &mut state,
            FixtureStoredEventV1::Terminal(event),
            event_sha256,
            recorded_at_ms,
            None,
        )
    }

    /// Reads one attempt from the reducer's verified state; a poisoned
    /// handle fails closed.
    pub(crate) fn read_attempt(
        &self,
        key: &ExecutionAttemptKeyV1,
    ) -> Result<Option<FixtureAttemptObservationV1>, ObservationStoreError> {
        let state = self.lock()?;
        if state.poisoned {
            return Err(ObservationStoreError::Poisoned);
        }
        let attempts = reduce(state.events.clone())?;
        Ok(find_attempt(&attempts, key).map(observation_from))
    }

    /// Test-only clock seam (ADR-0010 §5): fixes the system-time input the
    /// next non-idempotent appends observe. No injector exists in
    /// production builds.
    #[cfg(test)]
    fn set_clock_for_test(&self, fixed_system_now_ms: Option<u64>) -> Result<(), ObservationStoreError> {
        let mut state = self.lock()?;
        state.clock_override_ms = fixed_system_now_ms;
        Ok(())
    }

    /// Shared append tail: derives the view through the single reducer,
    /// builds segment/root, commits, and updates the facade state from the
    /// accepted identity only (ADR-0010 §6). The reducer ignores the root
    /// digest, so the provisional event carries an empty one that the
    /// accepted root fills in afterwards.
    fn append_event(
        &self,
        state: &mut FacadeState,
        event: FixtureStoredEventV1,
        event_sha256: String,
        recorded_at_ms: u64,
        started_request: Option<AppendStartedRequestV1>,
    ) -> Result<ObservationReceiptV1, ObservationStoreError> {
        let (key, kind, request_sha256, sequence) = match &event {
            FixtureStoredEventV1::Started(event) => (
                event.request.key,
                ReducibleKindV1::Started {
                    policy_sha256: event.request.policy_sha256.clone(),
                    runtime_sha256: event.request.runtime_sha256.clone(),
                },
                event.request_sha256.clone(),
                event.sequence,
            ),
            FixtureStoredEventV1::Terminal(event) => (
                event.request.key,
                ReducibleKindV1::Terminal {
                    policy_sha256: event.request.policy_sha256.clone(),
                    runtime_sha256: event.request.runtime_sha256.clone(),
                },
                event.request_sha256.clone(),
                event.sequence,
            ),
        };
        let mut next_events = state.events.clone();
        next_events.push(ReducibleEventV1 {
            sequence,
            root_generation: sequence,
            root_sha256: String::new(),
            recorded_at_ms,
            event_sha256: event_sha256.clone(),
            request_sha256: request_sha256.clone(),
            key,
            kind,
        });
        let attempts = reduce(next_events.clone())?;
        let segment = FixtureEventSegmentV1 {
            schema: SEGMENT_SCHEMA.to_string(),
            first_sequence: sequence,
            last_sequence: sequence,
            previous_segment_sha256: state.head_segment_sha256.clone(),
            event_kind: event_kind_of(&event),
            event_sha256: event_sha256.clone(),
        };
        let segment_sha256 = hash::segment_sha256(&segment)?;
        let current_view = FixtureCurrentViewV1 {
            schema: CURRENT_VIEW_SCHEMA.to_string(),
            attestation_state: ATTESTATION_STATE.to_string(),
            generation: sequence,
            event_watermark: sequence,
            attempts: attempts.iter().map(attempt_view).collect(),
        };
        let view_sha256 = hash::current_view_sha256(&current_view)?;
        let root = FixtureLedgerRootV1 {
            schema: ROOT_SCHEMA.to_string(),
            trust_class: TRUST_CLASS.to_string(),
            generation: sequence,
            previous_root_sha256: Some(state.head.root_sha256.clone()),
            event_segment_head_sha256: Some(segment_sha256.clone()),
            event_watermark: sequence,
            current_view_sha256: view_sha256,
            committed_at_ms: recorded_at_ms,
        };
        let commit = FixtureStructuralCommitV1 {
            event,
            segment,
            current_view,
            root,
        };
        match self.store.commit_structural(commit) {
            Ok(accepted) => {
                next_events.last_mut().expect("pushed above").root_sha256 = accepted.root_sha256.clone();
                state.head = accepted.clone();
                state.head_segment_sha256 = Some(segment_sha256);
                state.events = next_events;
                if let Some(request) = started_request {
                    state.started_requests.push(request);
                }
                Ok(ObservationReceiptV1 {
                    request_sha256,
                    event_sha256,
                    sequence,
                    root_generation: sequence,
                    root_sha256: accepted.root_sha256,
                    recorded_at_ms,
                })
            }
            Err(ObservationStoreError::CommitIndeterminate) => {
                state.poisoned = true;
                Err(ObservationStoreError::CommitIndeterminate)
            }
            Err(other) => Err(other),
        }
    }

    /// Head-equality guard: the structural store is authoritative, and a
    /// diverging cache rebuilds strictly from the active chain.
    fn reconcile_head(&self, state: &mut FacadeState) -> Result<(), ObservationStoreError> {
        let store_head = self.store.structural_state()?;
        if store_head.root_sha256 != state.head.root_sha256
            || store_head.generation != state.head.generation
            || store_head.event_watermark != state.head.event_watermark
        {
            let RebuiltLedger {
                events,
                started_requests,
                head_segment_sha256,
            } = rebuild_from_active(&self.vault, &store_head)?;
            state.head = store_head;
            state.events = events;
            state.started_requests = started_requests;
            state.head_segment_sha256 = head_segment_sha256;
        }
        Ok(())
    }

    /// Typed fail-closed lock acquisition: a poisoned mutex reports
    /// `Poisoned` instead of panicking here.
    fn lock(&self) -> Result<MutexGuard<'_, FacadeState>, ObservationStoreError> {
        self.state.lock().map_err(|_| ObservationStoreError::Poisoned)
    }
}

/// The rebuild payload from one authoritative-active walk: the event log,
/// the accepted Started bodies the frozen transition validator needs, and
/// the head segment identity.
struct RebuiltLedger {
    events: Vec<ReducibleEventV1>,
    started_requests: Vec<AppendStartedRequestV1>,
    head_segment_sha256: Option<String>,
}

/// Rebuilds the event log (plus the accepted Started bodies) by walking
/// the authoritative active chain from the head root down to the exact
/// genesis; the store's own open-time typestate validation has already
/// verified the chain.
fn rebuild_from_active(
    vault: &Arc<PersonalVaultStorage>,
    head: &FixtureStructuralStateV1,
) -> Result<RebuiltLedger, ObservationStoreError> {
    vault
        .with_existing_execution_observation_readonly(|view| -> Result<RebuiltLedger, ObservationStoreError> {
            let Some(view) = view else {
                return Err(ObservationStoreError::StorageUnavailable);
            };
            let mut collected = Vec::new();
            let mut started_requests = Vec::new();
            let mut head_segment = None;
            let mut root_sha256 = head.root_sha256.clone();
            for _ in 0..=EVENTS_MAX {
                let root = load_root(&view, &root_sha256)?;
                let segment_head = root.event_segment_head_sha256.clone();
                if root_sha256 == head.root_sha256 {
                    head_segment = segment_head.clone();
                }
                if let Some(segment_sha256) = segment_head {
                    let (event, started_request) = load_event(&view, &root, &root_sha256, &segment_sha256)?;
                    collected.push(event);
                    if let Some(request) = started_request {
                        started_requests.push(request);
                    }
                }
                match root.previous_root_sha256.clone() {
                    Some(parent) => root_sha256 = parent,
                    None => {
                        collected.reverse();
                        // Continuity validation through the single
                        // reducer; the facade keeps the event log itself.
                        reduce(collected.clone())?;
                        return Ok(RebuiltLedger {
                            events: collected,
                            started_requests,
                            head_segment_sha256: head_segment,
                        });
                    }
                }
            }
            Err(ObservationStoreError::corrupt(CorruptionCategory::BrokenRootChain))
        })
        .map_err(|_| ObservationStoreError::StorageUnavailable)?
}

fn load_root(
    view: &ExistingExecutionObservationReadOnly,
    root_sha256: &str,
) -> Result<FixtureLedgerRootV1, ObservationStoreError> {
    let bytes = read_object(view, root_sha256, ROOT_MAX_BYTES, CorruptionCategory::BrokenRootChain)?;
    let root: FixtureLedgerRootV1 = parse_canonical(&bytes).map_err(map_stored)?;
    if hash::root_sha256(&root).map_err(map_stored)? != root_sha256 {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    root.validate(&root.current_view_sha256).map_err(map_stored)?;
    Ok(root)
}

fn load_event(
    view: &ExistingExecutionObservationReadOnly,
    root: &FixtureLedgerRootV1,
    root_sha256: &str,
    segment_sha256: &str,
) -> Result<(ReducibleEventV1, Option<AppendStartedRequestV1>), ObservationStoreError> {
    let segment_bytes = read_object(
        view,
        segment_sha256,
        SEGMENT_MAX_BYTES,
        CorruptionCategory::BrokenSegmentChain,
    )?;
    let segment: FixtureEventSegmentV1 = parse_canonical(&segment_bytes).map_err(map_stored)?;
    if hash::segment_sha256(&segment).map_err(map_stored)? != segment_sha256 {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    let event_sha256 = segment.event_sha256.clone();
    let event_bytes = read_object(
        view,
        &event_sha256,
        STORED_EVENT_MAX_BYTES,
        CorruptionCategory::BrokenSegmentChain,
    )?;
    let (sequence, root_generation, recorded_at_ms, request_sha256, key, kind, started_request) =
        match segment.event_kind {
            EventKind::Started => {
                let event: StoredStartedEventV1 = parse_canonical(&event_bytes).map_err(map_stored)?;
                event.validate().map_err(map_stored)?;
                if hash::started_event_sha256(&event).map_err(map_stored)? != event_sha256 {
                    return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
                }
                (
                    event.sequence,
                    event.root_generation,
                    event.recorded_at_ms,
                    event.request_sha256,
                    event.request.key,
                    ReducibleKindV1::Started {
                        policy_sha256: event.request.policy_sha256.clone(),
                        runtime_sha256: event.request.runtime_sha256.clone(),
                    },
                    Some(event.request),
                )
            }
            EventKind::Terminal => {
                let event: StoredTerminalEventV1 = parse_canonical(&event_bytes).map_err(map_stored)?;
                event.validate().map_err(map_stored)?;
                if hash::terminal_event_sha256(&event).map_err(map_stored)? != event_sha256 {
                    return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
                }
                (
                    event.sequence,
                    event.root_generation,
                    event.recorded_at_ms,
                    event.request_sha256,
                    event.request.key,
                    ReducibleKindV1::Terminal {
                        policy_sha256: event.request.policy_sha256,
                        runtime_sha256: event.request.runtime_sha256,
                    },
                    None,
                )
            }
        };
    // Persisted-stamp binding: the event's own stamps must equal the
    // generation that accepted them.
    if root_generation != root.generation || sequence != root.event_watermark {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::GenerationMismatch));
    }
    Ok((
        ReducibleEventV1 {
            sequence,
            root_generation,
            root_sha256: root_sha256.to_string(),
            recorded_at_ms,
            event_sha256,
            request_sha256,
            key,
            kind,
        },
        started_request,
    ))
}

fn read_object(
    view: &ExistingExecutionObservationReadOnly,
    sha256: &str,
    maximum_bytes: usize,
    missing: CorruptionCategory,
) -> Result<Vec<u8>, ObservationStoreError> {
    match view.get_immutable_bounded(sha256, maximum_bytes as u64) {
        Ok(bytes) => Ok(bytes),
        Err(error) => Err(match error.kind() {
            std::io::ErrorKind::NotFound => ObservationStoreError::corrupt(missing),
            std::io::ErrorKind::InvalidData => ObservationStoreError::corrupt(CorruptionCategory::StoredResourceLimit),
            _ => ObservationStoreError::StorageUnavailable,
        }),
    }
}

/// Stored-parse failures collapse into stable corruption categories; caller
/// input categories never leak out of a stored-object read.
fn map_stored(error: ObservationStoreError) -> ObservationStoreError {
    match error {
        ObservationStoreError::InvalidRequest { category } => ObservationStoreError::corrupt(match category {
            InvalidRequestCategory::UnsupportedSchema | InvalidRequestCategory::InvalidAttestation => {
                CorruptionCategory::UnsupportedStoredSchema
            }
            _ => CorruptionCategory::ObjectHashMismatch,
        }),
        ObservationStoreError::LimitExceeded { .. } => {
            ObservationStoreError::corrupt(CorruptionCategory::StoredResourceLimit)
        }
        ObservationStoreError::TransitionConflict { .. } => {
            ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition)
        }
        stored => stored,
    }
}

fn event_kind_of(event: &FixtureStoredEventV1) -> EventKind {
    match event {
        FixtureStoredEventV1::Started(_) => EventKind::Started,
        FixtureStoredEventV1::Terminal(_) => EventKind::Terminal,
    }
}

fn find_attempt<'a>(attempts: &'a [ReducibleAttemptV1], key: &ExecutionAttemptKeyV1) -> Option<&'a ReducibleAttemptV1> {
    let needle = (key.execution_id.as_bytes(), key.attempt.get());
    attempts
        .binary_search_by(|attempt| attempt_ordering(attempt, needle))
        .ok()
        .map(|index| &attempts[index])
}

fn find_started_request<'a>(
    started_requests: &'a [AppendStartedRequestV1],
    key: &ExecutionAttemptKeyV1,
) -> Option<&'a AppendStartedRequestV1> {
    started_requests.iter().rev().find(|request| request.key == *key)
}

fn previous_accepted_ms(events: &[ReducibleEventV1]) -> u64 {
    events.iter().map(|event| event.recorded_at_ms).max().unwrap_or(0)
}

fn receipt_from(receipt: &ReducibleReceiptV1) -> ObservationReceiptV1 {
    ObservationReceiptV1 {
        request_sha256: receipt.request_sha256.clone(),
        event_sha256: receipt.event_sha256.clone(),
        sequence: receipt.sequence,
        root_generation: receipt.root_generation,
        root_sha256: receipt.root_sha256.clone(),
        recorded_at_ms: receipt.recorded_at_ms,
    }
}

fn attempt_view(attempt: &ReducibleAttemptV1) -> FixtureAttemptViewV1 {
    FixtureAttemptViewV1 {
        key: attempt.key,
        attestation_state: ATTESTATION_STATE.to_string(),
        started_request_sha256: attempt.started.request_sha256.clone(),
        started_event_sha256: attempt.started.event_sha256.clone(),
        terminal_request_sha256: attempt
            .terminal
            .as_ref()
            .map(|terminal| terminal.request_sha256.clone()),
        terminal_event_sha256: attempt.terminal.as_ref().map(|terminal| terminal.event_sha256.clone()),
    }
}

fn observation_from(attempt: &ReducibleAttemptV1) -> FixtureAttemptObservationV1 {
    FixtureAttemptObservationV1 {
        key: attempt.key,
        attestation_state: ATTESTATION_STATE.to_string(),
        started_receipt: receipt_from(&attempt.started),
        terminal_receipt: attempt.terminal.as_ref().map(receipt_from),
    }
}
