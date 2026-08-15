//! Pure legacy-to-ledger record planning.
//!
//! This module constructs canonical records only through the feature-gated
//! ledger migration factories. Publishing, locking, staging and filesystem
//! exchange remain exclusively in the CAS offline-migration I/O boundary.

use std::collections::{BTreeMap, BTreeSet};

use plico::memory::layered::{KnowledgePiece, Procedure, ProcedureStep};
use plico::memory::ledger::{
    validate_migration_record_sets, GroupMapping, LegacyCreatedAtMapping, ManifestSourceScope, ManifestTargetMode,
    MigrationDispositionCounters, MigrationManifest, MigrationManifestInput, MigrationPolicyInput,
    MigrationRevisionInput, PolicyMapping, RoleMapping, SourceManifest, SourceManifestInput, SourceSnapshot,
    StreamMapping,
};
use plico::memory::{
    CanonicalRevision, LedgerError, MemoryContent, MemoryId, MemoryRevisionId, MemoryTier, MemoryType, PolicyRecord,
};
use plico::{memory, PERSONAL_OWNER_ROLE_ID};

use super::legacy::{
    LegacyMemoryContent, LegacyMemoryEntry, LegacyMemoryScope, LegacyMemoryTier, LegacyMemoryType, LegacyNamespace,
    LegacyPreflightReport,
};
#[derive(Debug)]
pub struct MigrationRecordPlan {
    source_manifest: SourceManifest,
    migration_manifest: MigrationManifest,
    revisions: Vec<CanonicalRevision>,
    policies: Vec<PolicyRecord>,
}

impl MigrationRecordPlan {
    pub fn source_manifest(&self) -> &SourceManifest {
        &self.source_manifest
    }

    pub fn migration_manifest(&self) -> &MigrationManifest {
        &self.migration_manifest
    }

    pub fn revisions(&self) -> &[CanonicalRevision] {
        &self.revisions
    }

    pub fn policies(&self) -> &[PolicyRecord] {
        &self.policies
    }
}

pub struct MigrationPlanInput<'a> {
    pub preflight: &'a LegacyPreflightReport,
    /// Whether CAS had to create the parent-level vault lock before inspect.
    pub lock_created: bool,
    /// SHA-256 of the exact legacy index bytes observed by the CAS I/O layer.
    pub source_index_hash: String,
    /// Domain-separated JCS hash of active role IDs plus expiry cutoffs.
    pub credential_role_cutoff_hash: String,
    pub imported_at: u64,
    pub imported_by_role: String,
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationPlanError {
    #[error("migration plan rejected: {0}")]
    Rejected(&'static str),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
}

impl MigrationPlanError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::Rejected(category) => category,
            Self::Ledger(_) => "migration_plan_invalid",
        }
    }
}

/// Build and validate canonical revision/policy record sets without I/O.
pub fn build_migration_record_plan(input: MigrationPlanInput<'_>) -> Result<MigrationRecordPlan, MigrationPlanError> {
    validate_plan_input(&input)?;
    let source_manifest = build_source_manifest(&input)?;
    let (_, source_manifest_hash) = source_manifest.canonical_bytes_and_hash()?;
    let role_by_agent: BTreeMap<_, _> = input
        .preflight
        .role_mappings()
        .iter()
        .map(|mapping| (mapping.legacy_agent_id.as_str(), mapping.target_role_id.as_str()))
        .collect();
    let group_roles: BTreeMap<_, _> = input
        .preflight
        .group_mappings()
        .iter()
        .map(|mapping| (mapping.legacy_group_id.as_str(), mapping.target_role_ids.as_slice()))
        .collect();
    let shared_audience = shared_audience(input.preflight);

    let mut revisions = Vec::new();
    let mut policies = Vec::new();
    let mut stream_mappings = Vec::new();
    let mut dispositions = MigrationDispositionCounters::default();

    for stream in input.preflight.streams() {
        let mut parent_revision_id = None;
        let mut previous_scope: Option<&LegacyMemoryScope> = None;
        let mut policy_mappings = Vec::new();
        for revision_id in &stream.revision_ids {
            let entry = input
                .preflight
                .entry(revision_id)
                .ok_or(MigrationPlanError::Rejected("preflight_record_missing"))?;
            let source_role = role_by_agent
                .get(entry.agent_id.as_str())
                .ok_or(MigrationPlanError::Rejected("preflight_role_mapping_missing"))?;
            let revision = canonical_revision(RevisionPlanInput {
                entry,
                memory_id: stream.memory_id.as_str(),
                revision_id: &entry.id,
                parent_revision_id: parent_revision_id.clone(),
                sequence: next_sequence(revisions.len())?,
                imported_at: input.imported_at,
                actor: &input.imported_by_role,
                tombstone: false,
            })?;
            parent_revision_id = Some(revision.revision_id.clone());
            revisions.push(revision);

            if previous_scope != Some(&entry.scope) {
                let sequence = next_sequence(policies.len())?;
                let (policy, mapping) = migration_policy(PolicyPlanInput {
                    entry,
                    memory_id: stream.memory_id.as_str(),
                    source_role,
                    group_roles: group_roles.get(entry_group_id(entry)).copied(),
                    shared_roles: &shared_audience,
                    source_manifest_hash: &source_manifest_hash,
                    sequence,
                    imported_at: input.imported_at,
                    actor: &input.imported_by_role,
                })?;
                policies.push(policy);
                policy_mappings.push(mapping);
                previous_scope = Some(&entry.scope);
            }
            count_dispositions(entry, &mut dispositions);
        }

        let legacy_head = input
            .preflight
            .entry(
                stream
                    .revision_ids
                    .last()
                    .ok_or(MigrationPlanError::Rejected("empty_preflight_stream"))?,
            )
            .ok_or(MigrationPlanError::Rejected("preflight_record_missing"))?;
        let tombstone_revision_id = if legacy_head.deleted_at.is_some() {
            let tombstone_id = memory::ledger::deterministic_migration_revision_id(
                &source_manifest_hash,
                &legacy_head.id,
                "legacy-deleted-head-tombstone",
            );
            revisions.push(canonical_revision(RevisionPlanInput {
                entry: legacy_head,
                memory_id: stream.memory_id.as_str(),
                revision_id: &tombstone_id,
                parent_revision_id: Some(MemoryRevisionId::from(legacy_head.id.clone())),
                sequence: next_sequence(revisions.len())?,
                imported_at: input.imported_at,
                actor: &input.imported_by_role,
                tombstone: true,
            })?);
            dispositions.deleted_heads_converted_to_tombstones += 1;
            Some(tombstone_id)
        } else {
            dispositions.projection_rebuilds_required += 1;
            None
        };
        stream_mappings.push(StreamMapping {
            legacy_agent_id: legacy_head.agent_id.clone(),
            memory_id: stream.memory_id.clone(),
            legacy_revision_ids: stream.revision_ids.clone(),
            legacy_created_at_by_revision: stream
                .revision_ids
                .iter()
                .map(|revision_id| {
                    input
                        .preflight
                        .entry(revision_id)
                        .map(|entry| LegacyCreatedAtMapping {
                            revision_id: revision_id.clone(),
                            legacy_created_at: entry.created_at,
                        })
                        .ok_or(MigrationPlanError::Rejected("preflight_record_missing"))
                })
                .collect::<Result<_, _>>()?,
            target_head_revision_id: tombstone_revision_id.clone().unwrap_or_else(|| legacy_head.id.clone()),
            tombstone_revision_id,
            legacy_deleted_at: legacy_head.deleted_at,
            policy_mappings,
        });
    }

    validate_migration_record_sets(&revisions, &policies)?;
    let migration_manifest = MigrationManifest::new(MigrationManifestInput {
        source_manifest_hash: source_manifest_hash.clone(),
        stream_mappings,
        dispositions,
        target_revision_count: u64::try_from(revisions.len())
            .map_err(|_| MigrationPlanError::Rejected("target_count_overflow"))?,
        target_policy_count: u64::try_from(policies.len())
            .map_err(|_| MigrationPlanError::Rejected("target_count_overflow"))?,
        imported_at: input.imported_at,
        imported_by_role: input.imported_by_role,
    })?;
    migration_manifest.canonical_bytes_and_hash()?;
    validate_plan_against_preflight(input.preflight, &revisions, &policies, input.imported_at)?;
    Ok(MigrationRecordPlan {
        source_manifest,
        migration_manifest,
        revisions,
        policies,
    })
}

fn validate_plan_against_preflight(
    preflight: &LegacyPreflightReport,
    revisions: &[CanonicalRevision],
    policies: &[PolicyRecord],
    imported_at: u64,
) -> Result<(), MigrationPlanError> {
    for stream in preflight.streams() {
        for (position, revision_id) in stream.revision_ids.iter().enumerate() {
            let entry = preflight
                .entry(revision_id)
                .ok_or(MigrationPlanError::Rejected("preflight_record_missing"))?;
            let revision = revisions
                .iter()
                .find(|candidate| candidate.revision_id.as_str() == revision_id)
                .ok_or(MigrationPlanError::Rejected("target_revision_missing"))?;
            let expected_parent = position
                .checked_sub(1)
                .and_then(|previous| stream.revision_ids.get(previous))
                .map(String::as_str);
            let expected_content = convert_content(&entry.content);
            let expected_content_hash = expected_content
                .canonical_content_hash()
                .map_err(|_| MigrationPlanError::Rejected("target_revision_semantic_mismatch"))?;
            let actual_content_bytes = serde_json_canonicalizer::to_vec(&revision.content)
                .map_err(|_| MigrationPlanError::Rejected("target_revision_semantic_mismatch"))?;
            let expected_content_bytes = serde_json_canonicalizer::to_vec(&expected_content)
                .map_err(|_| MigrationPlanError::Rejected("target_revision_semantic_mismatch"))?;
            if revision.memory_id.as_str() != stream.memory_id
                || revision.parent_revision_id.as_ref().map(|parent| parent.as_str()) != expected_parent
                || actual_content_bytes != expected_content_bytes
                || revision.content_hash != expected_content_hash
                || revision.tags != entry.tags
                || revision.cognitive_tier != convert_tier(entry.tier)
                || revision.memory_type != convert_type(entry.memory_type)
                || revision.deleted_at.is_some()
                || revision.committed_at != imported_at
            {
                return Err(MigrationPlanError::Rejected("target_revision_semantic_mismatch"));
            }
        }
        let expected_policy_count = stream
            .revision_ids
            .iter()
            .filter_map(|revision_id| preflight.entry(revision_id))
            .scan(None::<LegacyMemoryScope>, |previous, entry| {
                let changed = previous.as_ref() != Some(&entry.scope);
                *previous = Some(entry.scope.clone());
                Some(usize::from(changed))
            })
            .sum::<usize>();
        if policies
            .iter()
            .filter(|policy| policy.memory_id.as_str() == stream.memory_id)
            .count()
            != expected_policy_count
        {
            return Err(MigrationPlanError::Rejected("target_policy_semantic_mismatch"));
        }
    }
    Ok(())
}

fn build_source_manifest(input: &MigrationPlanInput<'_>) -> Result<SourceManifest, MigrationPlanError> {
    Ok(SourceManifest::new(SourceManifestInput {
        lock_created: input.lock_created,
        source_index_hash: input.source_index_hash.clone(),
        legacy_namespace: match input.preflight.namespace() {
            LegacyNamespace::PreNamespace => None,
            LegacyNamespace::Named(value) => Some(value.clone()),
        },
        source_snapshots: input
            .preflight
            .source_snapshots()
            .iter()
            .map(|snapshot| SourceSnapshot {
                legacy_agent_id: snapshot.legacy_agent_id.clone(),
                legacy_tier: snapshot.legacy_tier.clone(),
                cid: snapshot.cid.clone(),
                object_envelope_hash: snapshot.object_envelope_hash.clone(),
                entry_count: snapshot.entry_count as u64,
            })
            .collect(),
        source_entry_count: u64::try_from(input.preflight.entry_count())
            .map_err(|_| MigrationPlanError::Rejected("source_count_overflow"))?,
        source_stream_count: u64::try_from(input.preflight.stream_count())
            .map_err(|_| MigrationPlanError::Rejected("source_count_overflow"))?,
        source_ttl_field_count: u64::try_from(input.preflight.ttl_field_count())
            .map_err(|_| MigrationPlanError::Rejected("source_count_overflow"))?,
        source_embedded_vector_count: u64::try_from(input.preflight.embedded_vector_count())
            .map_err(|_| MigrationPlanError::Rejected("source_count_overflow"))?,
        credential_role_cutoff_hash: input.credential_role_cutoff_hash.clone(),
        authorized_role_ids_at_cutoff: input.preflight.authorized_role_ids().to_vec(),
        role_mappings: input
            .preflight
            .role_mappings()
            .iter()
            .map(|mapping| RoleMapping {
                legacy_agent_id: mapping.legacy_agent_id.clone(),
                target_role_id: mapping.target_role_id.clone(),
            })
            .collect(),
        group_mappings: input
            .preflight
            .group_mappings()
            .iter()
            .map(|mapping| GroupMapping {
                legacy_group_id: mapping.legacy_group_id.clone(),
                target_role_ids: mapping.target_role_ids.clone(),
            })
            .collect(),
    })?)
}

fn validate_plan_input(input: &MigrationPlanInput<'_>) -> Result<(), MigrationPlanError> {
    if !is_lower_hex_sha256(&input.source_index_hash) {
        return Err(MigrationPlanError::Rejected("invalid_source_index_hash"));
    }
    if input.imported_at == 0 || input.imported_by_role != PERSONAL_OWNER_ROLE_ID {
        return Err(MigrationPlanError::Rejected("invalid_migration_commit_identity"));
    }
    if !input
        .preflight
        .authorized_role_ids()
        .iter()
        .any(|role| role == PERSONAL_OWNER_ROLE_ID)
    {
        return Err(MigrationPlanError::Rejected("personal_owner_not_authorized"));
    }
    Ok(())
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn next_sequence(current_len: usize) -> Result<u64, MigrationPlanError> {
    u64::try_from(current_len)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or(MigrationPlanError::Rejected("target_sequence_overflow"))
}

fn shared_audience(preflight: &LegacyPreflightReport) -> Vec<String> {
    preflight.authorized_role_ids().to_vec()
}

fn entry_group_id(entry: &LegacyMemoryEntry) -> &str {
    match &entry.scope {
        LegacyMemoryScope::Group(group_id) => group_id,
        LegacyMemoryScope::Private | LegacyMemoryScope::Shared => "",
    }
}

struct RevisionPlanInput<'a> {
    entry: &'a LegacyMemoryEntry,
    memory_id: &'a str,
    revision_id: &'a str,
    parent_revision_id: Option<MemoryRevisionId>,
    sequence: u64,
    imported_at: u64,
    actor: &'a str,
    tombstone: bool,
}

fn canonical_revision(input: RevisionPlanInput<'_>) -> Result<CanonicalRevision, MigrationPlanError> {
    Ok(CanonicalRevision::migration_import(MigrationRevisionInput {
        sequence: input.sequence,
        memory_id: MemoryId::from(input.memory_id),
        revision_id: MemoryRevisionId::from(input.revision_id),
        parent_revision_id: input.parent_revision_id,
        content: convert_content(&input.entry.content),
        tags: input.entry.tags.clone(),
        memory_type: convert_type(input.entry.memory_type),
        cognitive_tier: convert_tier(input.entry.tier),
        deleted_at: input.tombstone.then_some(input.imported_at),
        committed_at: input.imported_at,
        actor: input.actor.to_string(),
    })?)
}

struct PolicyPlanInput<'a> {
    entry: &'a LegacyMemoryEntry,
    memory_id: &'a str,
    source_role: &'a str,
    group_roles: Option<&'a [String]>,
    shared_roles: &'a [String],
    source_manifest_hash: &'a str,
    sequence: u64,
    imported_at: u64,
    actor: &'a str,
}

fn migration_policy(plan: PolicyPlanInput<'_>) -> Result<(PolicyRecord, PolicyMapping), MigrationPlanError> {
    let input = MigrationPolicyInput {
        sequence: plan.sequence,
        policy_id: memory::ledger::deterministic_migration_policy_id(
            plan.source_manifest_hash,
            plan.memory_id,
            plan.sequence,
        ),
        memory_id: MemoryId::from(plan.memory_id),
        effective_from_revision_id: MemoryRevisionId::from(plan.entry.id.clone()),
        source_role: plan.source_role.to_string(),
        committed_at: plan.imported_at,
        actor: plan.actor.to_string(),
    };
    let (policy, source_scope, target_mode) = match &plan.entry.scope {
        LegacyMemoryScope::Private => (
            PolicyRecord::migration_private(input)?,
            ManifestSourceScope::Private,
            ManifestTargetMode::Private,
        ),
        LegacyMemoryScope::Shared => (
            PolicyRecord::migration_explicit_role_set(input, plan.shared_roles.to_vec())?,
            ManifestSourceScope::Shared,
            ManifestTargetMode::ExplicitRoleSet,
        ),
        LegacyMemoryScope::Group(group_id) => {
            let mut readers: BTreeSet<_> = plan
                .group_roles
                .ok_or(MigrationPlanError::Rejected("unresolved_group_audience"))?
                .iter()
                .cloned()
                .collect();
            readers.insert(plan.source_role.to_string());
            readers.insert(PERSONAL_OWNER_ROLE_ID.to_string());
            (
                PolicyRecord::migration_explicit_role_set(input, readers.into_iter().collect())?,
                ManifestSourceScope::Group {
                    legacy_group_id: group_id.clone(),
                },
                ManifestTargetMode::ExplicitRoleSet,
            )
        }
    };
    let mapping = PolicyMapping {
        effective_from_revision_id: plan.entry.id.clone(),
        source_scope,
        target_mode,
        audience_cutoff_roles: policy.reader_roles.clone(),
    };
    Ok((policy, mapping))
}

fn count_dispositions(entry: &LegacyMemoryEntry, counters: &mut MigrationDispositionCounters) {
    counters.importance_values_discarded += 1;
    counters.access_statistic_fields_discarded += 2;
    counters.legacy_created_at_fields_discarded += 1;
    counters.ttl_fields_discarded += u64::from(entry.ttl_ms.is_some()) + u64::from(entry.original_ttl_ms.is_some());
    counters.embedded_vectors_discarded += u64::from(entry.embedding.is_some());
}

fn convert_tier(tier: LegacyMemoryTier) -> MemoryTier {
    match tier {
        LegacyMemoryTier::Ephemeral => MemoryTier::Ephemeral,
        LegacyMemoryTier::Working => MemoryTier::Working,
        LegacyMemoryTier::LongTerm => MemoryTier::LongTerm,
        LegacyMemoryTier::Procedural => MemoryTier::Procedural,
    }
}

fn convert_type(memory_type: LegacyMemoryType) -> MemoryType {
    match memory_type {
        LegacyMemoryType::Episodic => MemoryType::Episodic,
        LegacyMemoryType::Semantic => MemoryType::Semantic,
        LegacyMemoryType::Procedural => MemoryType::Procedural,
        LegacyMemoryType::Untyped => MemoryType::Untyped,
    }
}

fn convert_content(content: &LegacyMemoryContent) -> MemoryContent {
    match content {
        LegacyMemoryContent::Text(value) => MemoryContent::Text(value.clone()),
        LegacyMemoryContent::ObjectRef(value) => MemoryContent::ObjectRef(value.clone()),
        LegacyMemoryContent::Structured(value) => MemoryContent::Structured(value.clone()),
        LegacyMemoryContent::Procedure(value) => MemoryContent::Procedure(Procedure {
            name: value.name.clone(),
            description: value.description.clone(),
            steps: value
                .steps
                .iter()
                .map(|step| ProcedureStep {
                    step_number: step.step_number,
                    description: step.description.clone(),
                    action: step.action.clone(),
                    expected_outcome: step.expected_outcome.clone(),
                })
                .collect(),
            learned_from: value.learned_from.clone(),
        }),
        LegacyMemoryContent::Knowledge(value) => MemoryContent::Knowledge(KnowledgePiece {
            subject: value.subject.clone(),
            statement: value.statement.clone(),
            confidence: value.confidence,
            source: value.source.clone(),
        }),
    }
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
