use std::collections::BTreeMap;

use super::hash::projection_id;
use super::model::{
    ActiveBuilderSpec, CanonicalWatermark, ManifestEvent, ManifestRecord, ProjectionCurrentView, ProjectionEntry,
    ProjectionError, ProjectionKind, ProjectionState, QueueReason, CURRENT_VIEW_SCHEMA, MANIFEST_RECORD_SCHEMA,
};
use super::validate::{invalid, validate_builder_hash, validate_source, validate_state_shape, validate_watermark};

const PROJECTION_WORKER_ROLE: &str = "projection-worker";
const MAX_JCS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub(super) fn rebuild_current_view(
    generation: u64,
    genesis_source: &CanonicalWatermark,
    records: &[ManifestRecord],
) -> Result<ProjectionCurrentView, ProjectionError> {
    validate_watermark(genesis_source)?;
    let mut active_builders = BTreeMap::new();
    let mut entries = BTreeMap::new();
    let mut reconciled_source = genesis_source.clone();

    for (offset, record) in records.iter().enumerate() {
        if record.schema != MANIFEST_RECORD_SCHEMA
            || record.sequence != offset as u64 + 1
            || record.committed_at == 0
            || record.committed_at > MAX_JCS_SAFE_INTEGER
            || record.committed_by_role.trim().is_empty()
        {
            return Err(invalid("invalid_manifest_record"));
        }
        match &record.event {
            ManifestEvent::BuilderActivated {
                projection_kind,
                builder_spec,
                builder_spec_hash,
                previous_builder_spec_hash,
            } => {
                if *projection_kind != ProjectionKind::MemoryEmbedding
                    || builder_spec.projection_kind != *projection_kind
                    || record.committed_by_role != crate::PERSONAL_OWNER_ROLE_ID
                    || previous_builder_spec_hash.as_ref() == Some(builder_spec_hash)
                    || active_builders
                        .get(projection_kind)
                        .map(|active: &ActiveBuilderSpec| &active.builder_spec_hash)
                        != previous_builder_spec_hash.as_ref()
                {
                    return Err(invalid("invalid_builder_activation"));
                }
                validate_builder_hash(builder_spec, builder_spec_hash)?;
                active_builders.insert(
                    *projection_kind,
                    ActiveBuilderSpec {
                        projection_kind: *projection_kind,
                        builder_spec_hash: builder_spec_hash.clone(),
                        builder_spec: builder_spec.clone(),
                    },
                );
            }
            ManifestEvent::ProjectionTransition {
                projection_id: event_projection_id,
                projection_kind,
                projection_version,
                previous_sequence,
                source,
                desired_builder_spec_hash,
                state,
            } => {
                validate_source(source)?;
                validate_state_shape(state, record.committed_at)?;
                if *projection_kind != ProjectionKind::MemoryEmbedding
                    || *projection_version > MAX_JCS_SAFE_INTEGER
                    || projection_id(&source.revision_id)? != *event_projection_id
                    || active_builders
                        .get(projection_kind)
                        .map(|builder| builder.builder_spec_hash.as_str())
                        != Some(desired_builder_spec_hash)
                {
                    return Err(invalid("invalid_projection_transition"));
                }
                let previous = entries.get(event_projection_id);
                if previous.map(|entry: &ProjectionEntry| entry.last_transition_sequence) != *previous_sequence
                    || previous.map_or(*projection_version != 1, |entry| {
                        entry.projection_version.checked_add(1) != Some(*projection_version)
                            || entry.projection_kind != *projection_kind
                            || entry.source != *source
                    })
                {
                    return Err(invalid("broken_projection_transition_chain"));
                }
                validate_state_binding(
                    previous,
                    state,
                    source,
                    desired_builder_spec_hash,
                    active_builders
                        .get(projection_kind)
                        .ok_or_else(|| invalid("missing_active_builder"))?,
                )?;
                validate_actor_and_transition(previous, state, record)?;
                let attempt_count = next_attempt_count(previous, state)?;
                entries.insert(
                    *event_projection_id,
                    ProjectionEntry {
                        projection_id: *event_projection_id,
                        projection_kind: *projection_kind,
                        projection_version: *projection_version,
                        last_transition_sequence: record.sequence,
                        attempt_count,
                        source: source.clone(),
                        desired_builder_spec_hash: desired_builder_spec_hash.clone(),
                        state: state.clone(),
                    },
                );
            }
            ManifestEvent::ReconciliationAdvanced {
                previous_source,
                reconciled_source: next_source,
                classified_revision_count,
            } => {
                if record.committed_by_role != PROJECTION_WORKER_ROLE
                    || previous_source != &reconciled_source
                    || next_source == previous_source
                    || *classified_revision_count != next_source.revision_watermark
                    || next_source.generation < previous_source.generation
                    || next_source.revision_watermark < previous_source.revision_watermark
                    || next_source.policy_watermark < previous_source.policy_watermark
                    || next_source.relation_watermark < previous_source.relation_watermark
                {
                    return Err(invalid("invalid_reconciliation_advance"));
                }
                validate_watermark(next_source)?;
                reconciled_source = next_source.clone();
            }
        }
    }

    let mut active_builder_specs: Vec<_> = active_builders.into_values().collect();
    active_builder_specs.sort_by_key(|builder| builder.projection_kind);
    if generation > 0
        && (active_builder_specs.len() != 1
            || active_builder_specs[0].projection_kind != ProjectionKind::MemoryEmbedding)
    {
        return Err(invalid("missing_active_memory_embedding_builder"));
    }
    let mut entries: Vec<_> = entries.into_values().collect();
    entries.sort_by(|left, right| {
        (left.projection_kind, left.source.revision_id.as_str())
            .cmp(&(right.projection_kind, right.source.revision_id.as_str()))
    });
    if entries.iter().any(|entry| {
        !matches!(entry.state, ProjectionState::AbsentByPolicy { .. })
            && active_builder_specs
                .iter()
                .find(|builder| builder.projection_kind == entry.projection_kind)
                .is_none_or(|builder| builder.builder_spec_hash != entry.desired_builder_spec_hash)
    }) {
        return Err(invalid("projection_entry_uses_inactive_builder"));
    }
    Ok(ProjectionCurrentView {
        schema: CURRENT_VIEW_SCHEMA.to_string(),
        generation,
        event_watermark: records.len() as u64,
        reconciled_source,
        active_builder_specs,
        entries,
    })
}

fn validate_actor_and_transition(
    previous: Option<&ProjectionEntry>,
    next: &ProjectionState,
    record: &ManifestRecord,
) -> Result<(), ProjectionError> {
    let owner_action = matches!(
        next,
        ProjectionState::Queued {
            reason: QueueReason::OwnerRebuild | QueueReason::BuilderChanged
        } | ProjectionState::Stale {
            reason: super::model::StaleReason::OwnerRebuild | super::model::StaleReason::BuilderSpecChanged,
            ..
        }
    );
    let expected_actor = if owner_action {
        crate::PERSONAL_OWNER_ROLE_ID
    } else {
        PROJECTION_WORKER_ROLE
    };
    if record.committed_by_role != expected_actor || !transition_allowed(previous.map(|entry| &entry.state), next) {
        return Err(invalid("illegal_projection_transition"));
    }
    if let (
        Some(previous),
        ProjectionState::Ready {
            attempt, attempt_id, ..
        },
    )
    | (
        Some(previous),
        ProjectionState::Failed {
            attempt, attempt_id, ..
        },
    ) = (previous, next)
    {
        match &previous.state {
            ProjectionState::Building {
                attempt: expected_attempt,
                attempt_id: expected_id,
                ..
            } if expected_attempt == attempt && expected_id == attempt_id => {}
            _ => return Err(invalid("projection_attempt_mismatch")),
        }
    }
    match (previous.map(|entry| &entry.state), next) {
        (
            Some(ProjectionState::AbsentByPolicy { .. }),
            ProjectionState::Queued {
                reason: QueueReason::OwnerRebuild,
            },
        ) => return Err(invalid("owner_rebuild_cannot_revive_absent_projection")),
        (
            Some(ProjectionState::Building { lease_expires_at, .. }),
            ProjectionState::Queued {
                reason: QueueReason::LeaseExpired,
            },
        ) if record.committed_at >= *lease_expires_at => {}
        (
            Some(ProjectionState::Failed {
                retryable: true,
                retry_not_before: Some(retry_at),
                ..
            }),
            ProjectionState::Queued {
                reason: QueueReason::Retry,
            },
        ) if record.committed_at >= *retry_at => {}
        (
            Some(ProjectionState::Failed { .. }),
            ProjectionState::Queued {
                reason: QueueReason::BuilderChanged | QueueReason::OwnerRebuild,
            },
        ) => {}
        (Some(ProjectionState::Failed { .. }), ProjectionState::Queued { .. }) => {
            return Err(invalid("invalid_projection_retry_transition"))
        }
        (
            Some(ProjectionState::Building { .. }),
            ProjectionState::Queued {
                reason: QueueReason::BuilderChanged | QueueReason::OwnerRebuild,
            },
        ) => {}
        (Some(ProjectionState::Building { .. }), ProjectionState::Queued { .. }) => {
            return Err(invalid("invalid_projection_retry_transition"))
        }
        _ => {}
    }
    Ok(())
}

fn validate_state_binding(
    previous: Option<&ProjectionEntry>,
    next: &ProjectionState,
    source: &super::model::CanonicalSourceIdentity,
    desired_builder_spec_hash: &str,
    active_builder: &ActiveBuilderSpec,
) -> Result<(), ProjectionError> {
    match next {
        ProjectionState::Ready { artifact, .. }
            if artifact.source_revision_id != source.revision_id
                || artifact.source_content_hash != source.content_hash
                || artifact.builder_spec_hash != desired_builder_spec_hash
                || artifact.dimension != active_builder.builder_spec.dimension =>
        {
            return Err(invalid("ready_artifact_binding_mismatch"));
        }
        ProjectionState::Ready { .. } => {}
        ProjectionState::Stale { artifact, .. } => {
            let Some(ProjectionState::Ready {
                artifact: previous_artifact,
                ..
            }) = previous.map(|entry| &entry.state)
            else {
                return Err(invalid("stale_artifact_has_no_ready_source"));
            };
            if artifact != previous_artifact
                || artifact.source_revision_id != source.revision_id
                || artifact.source_content_hash != source.content_hash
            {
                return Err(invalid("stale_artifact_binding_mismatch"));
            }
        }
        _ => {}
    }
    Ok(())
}

fn transition_allowed(previous: Option<&ProjectionState>, next: &ProjectionState) -> bool {
    matches!(
        (previous, next),
        (
            None,
            ProjectionState::AbsentByPolicy { .. } | ProjectionState::Queued { .. }
        ) | (
            Some(ProjectionState::AbsentByPolicy { .. }),
            ProjectionState::Queued { .. }
        ) | (
            Some(ProjectionState::Queued { .. }),
            ProjectionState::Queued {
                reason: QueueReason::BuilderChanged | QueueReason::OwnerRebuild,
            } | ProjectionState::Building { .. }
                | ProjectionState::AbsentByPolicy { .. },
        ) | (
            Some(ProjectionState::Building { .. }),
            ProjectionState::Ready { .. }
                | ProjectionState::Failed { .. }
                | ProjectionState::Queued { .. }
                | ProjectionState::AbsentByPolicy { .. },
        ) | (
            Some(ProjectionState::Ready { .. }),
            ProjectionState::Stale { .. } | ProjectionState::AbsentByPolicy { .. },
        ) | (
            Some(ProjectionState::Failed { .. }),
            ProjectionState::Queued { .. } | ProjectionState::AbsentByPolicy { .. },
        ) | (
            Some(ProjectionState::Stale { .. }),
            ProjectionState::Queued { .. } | ProjectionState::AbsentByPolicy { .. },
        )
    )
}

fn next_attempt_count(previous: Option<&ProjectionEntry>, next: &ProjectionState) -> Result<u32, ProjectionError> {
    let current = previous.map_or(0, |entry| entry.attempt_count);
    match next {
        ProjectionState::Building { attempt, .. } if current.checked_add(1) == Some(*attempt) => Ok(*attempt),
        ProjectionState::Building { .. } => Err(invalid("non_monotonic_projection_attempt")),
        ProjectionState::Ready { attempt, .. } | ProjectionState::Failed { attempt, .. } if *attempt == current => {
            Ok(current)
        }
        ProjectionState::Ready { .. } | ProjectionState::Failed { .. } => Err(invalid("projection_attempt_mismatch")),
        _ => Ok(current),
    }
}
