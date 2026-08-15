use std::collections::HashMap;

use super::model::{CanonicalRevision, CurrentStream, CurrentView, LedgerError, PolicyRecord, CURRENT_VIEW_SCHEMA};
use crate::memory::{MemoryId, MemoryRevisionId};

pub(super) fn rebuild_current_view(
    generation: u64,
    revisions: &[CanonicalRevision],
    policies: &[PolicyRecord],
    relation_watermark: u64,
) -> Result<CurrentView, LedgerError> {
    let mut policy_by_memory: HashMap<&MemoryId, &PolicyRecord> = HashMap::new();
    for policy in policies {
        policy_by_memory.insert(&policy.memory_id, policy);
    }
    let mut head_by_memory: HashMap<&MemoryId, &CanonicalRevision> = HashMap::new();
    for revision in revisions {
        head_by_memory.insert(&revision.memory_id, revision);
    }
    let mut streams = Vec::with_capacity(head_by_memory.len());
    for (memory_id, head) in head_by_memory {
        let policy = policy_by_memory.get(memory_id).ok_or(LedgerError::Invalid {
            category: "missing_stream_policy",
        })?;
        streams.push(CurrentStream {
            memory_id: memory_id.clone(),
            head_revision_id: head.revision_id.clone(),
            deleted: head.deleted_at.is_some(),
            policy_id: policy.policy_id.clone(),
        });
    }
    streams.sort_by(|left, right| left.memory_id.as_str().cmp(right.memory_id.as_str()));
    Ok(CurrentView {
        schema: CURRENT_VIEW_SCHEMA.to_string(),
        generation,
        revision_watermark: revisions.last().map_or(0, |record| record.sequence),
        policy_watermark: policies.last().map_or(0, |record| record.sequence),
        relation_watermark,
        streams,
    })
}

pub(super) fn head_for(view: &CurrentView, memory_id: &MemoryId) -> Option<MemoryRevisionId> {
    view.streams
        .iter()
        .find(|stream| &stream.memory_id == memory_id)
        .map(|stream| stream.head_revision_id.clone())
}
