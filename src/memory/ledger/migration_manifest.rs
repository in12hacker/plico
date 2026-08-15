//! Canonical evidence manifests shared by the offline migrator and runtime verifier.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{CanonicalRevision, LedgerError, PolicyMode, PolicyRecord};

const SOURCE_SCHEMA: &str = "plico.memory.migration-source-manifest/v1";
const SOURCE_DOMAIN: &[u8] = b"plico.memory.migration-source-manifest.v1\0";
const TARGET_SCHEMA: &str = "plico.memory.migration-target-manifest/v1";
const TARGET_DOMAIN: &[u8] = b"plico.memory.migration-target-manifest.v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceManifest {
    pub schema: String,
    pub source_index_hash: String,
    pub credential_role_cutoff_hash: String,
    pub lock_created: bool,
    pub legacy_namespace: Option<String>,
    pub source_snapshots: Vec<SourceSnapshot>,
    pub source_entry_count: u64,
    pub source_stream_count: u64,
    pub source_ttl_field_count: u64,
    pub source_embedded_vector_count: u64,
    pub authorized_role_ids_at_cutoff: Vec<String>,
    pub role_mappings: Vec<RoleMapping>,
    pub group_mappings: Vec<GroupMapping>,
}

#[cfg(feature = "offline-migration")]
impl SourceManifest {
    pub fn new(input: SourceManifestInput) -> Result<Self, LedgerError> {
        let manifest = Self {
            schema: SOURCE_SCHEMA.to_string(),
            source_index_hash: input.source_index_hash,
            credential_role_cutoff_hash: input.credential_role_cutoff_hash,
            lock_created: input.lock_created,
            legacy_namespace: input.legacy_namespace,
            source_snapshots: input.source_snapshots,
            source_entry_count: input.source_entry_count,
            source_stream_count: input.source_stream_count,
            source_ttl_field_count: input.source_ttl_field_count,
            source_embedded_vector_count: input.source_embedded_vector_count,
            authorized_role_ids_at_cutoff: input.authorized_role_ids_at_cutoff,
            role_mappings: input.role_mappings,
            group_mappings: input.group_mappings,
        };
        manifest.validate()?;
        Ok(manifest)
    }
}

impl SourceManifest {
    pub fn canonical_bytes_and_hash(&self) -> Result<(Vec<u8>, String), LedgerError> {
        self.validate()?;
        canonical_bytes_and_hash(self, SOURCE_DOMAIN)
    }

    pub(crate) fn verify_hash(&self, expected: &str) -> Result<(), LedgerError> {
        let (_, actual) = self.canonical_bytes_and_hash()?;
        if actual == expected {
            Ok(())
        } else {
            Err(LedgerError::Invalid {
                category: "source_manifest_hash_mismatch",
            })
        }
    }

    fn validate(&self) -> Result<(), LedgerError> {
        if self.schema != SOURCE_SCHEMA
            || !is_lower_hash(&self.source_index_hash)
            || !is_lower_hash(&self.credential_role_cutoff_hash)
            || !strict_roles(&self.authorized_role_ids_at_cutoff)
            || !self
                .authorized_role_ids_at_cutoff
                .iter()
                .any(|role| role == crate::PERSONAL_OWNER_ROLE_ID)
            || self
                .legacy_namespace
                .as_ref()
                .is_some_and(|namespace| namespace.trim().is_empty())
            || self.source_snapshots.iter().any(|snapshot| {
                snapshot.legacy_agent_id.trim().is_empty()
                    || !matches!(
                        snapshot.legacy_tier.as_str(),
                        "ephemeral" | "working" | "long_term" | "procedural"
                    )
                    || !is_lower_hash(&snapshot.cid)
                    || !is_lower_hash(&snapshot.object_envelope_hash)
            })
            || !self.source_snapshots.windows(2).all(|pair| {
                (&pair[0].legacy_agent_id, &pair[0].legacy_tier, &pair[0].cid)
                    < (&pair[1].legacy_agent_id, &pair[1].legacy_tier, &pair[1].cid)
            })
            || !self
                .role_mappings
                .windows(2)
                .all(|pair| pair[0].legacy_agent_id < pair[1].legacy_agent_id)
            || self.role_mappings.iter().any(|mapping| {
                mapping.legacy_agent_id.trim().is_empty()
                    || mapping.target_role_id.trim().is_empty()
                    || !self.authorized_role_ids_at_cutoff.contains(&mapping.target_role_id)
            })
            || !self
                .group_mappings
                .windows(2)
                .all(|pair| pair[0].legacy_group_id < pair[1].legacy_group_id)
            || self.group_mappings.iter().any(|mapping| {
                mapping.legacy_group_id.trim().is_empty()
                    || !strict_roles(&mapping.target_role_ids)
                    || mapping
                        .target_role_ids
                        .iter()
                        .any(|role| !self.authorized_role_ids_at_cutoff.contains(role))
            })
            || self
                .source_snapshots
                .iter()
                .map(|snapshot| snapshot.entry_count)
                .sum::<u64>()
                != self.source_entry_count
            || self.source_stream_count > self.source_entry_count
            || (self.source_stream_count == 0) != (self.source_entry_count == 0)
            || self.source_ttl_field_count > self.source_entry_count.saturating_mul(2)
            || self.source_embedded_vector_count > self.source_entry_count
            || self
                .source_snapshots
                .iter()
                .map(|snapshot| &snapshot.legacy_agent_id)
                .collect::<HashSet<_>>()
                != self
                    .role_mappings
                    .iter()
                    .map(|mapping| &mapping.legacy_agent_id)
                    .collect::<HashSet<_>>()
        {
            return Err(LedgerError::Invalid {
                category: "invalid_source_manifest",
            });
        }
        Ok(())
    }
}

#[cfg(feature = "offline-migration")]
pub struct SourceManifestInput {
    pub source_index_hash: String,
    pub credential_role_cutoff_hash: String,
    pub lock_created: bool,
    pub legacy_namespace: Option<String>,
    pub source_snapshots: Vec<SourceSnapshot>,
    pub source_entry_count: u64,
    pub source_stream_count: u64,
    pub source_ttl_field_count: u64,
    pub source_embedded_vector_count: u64,
    pub authorized_role_ids_at_cutoff: Vec<String>,
    pub role_mappings: Vec<RoleMapping>,
    pub group_mappings: Vec<GroupMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationManifest {
    pub schema: String,
    pub source_manifest_hash: String,
    pub stream_mappings: Vec<StreamMapping>,
    pub dispositions: MigrationDispositionCounters,
    pub target_revision_count: u64,
    pub target_policy_count: u64,
    pub target_relation_count: u64,
    pub imported_at: u64,
    pub imported_by_role: String,
}

#[cfg(feature = "offline-migration")]
impl MigrationManifest {
    pub fn new(input: MigrationManifestInput) -> Result<Self, LedgerError> {
        let manifest = Self {
            schema: TARGET_SCHEMA.to_string(),
            source_manifest_hash: input.source_manifest_hash,
            stream_mappings: input.stream_mappings,
            dispositions: input.dispositions,
            target_revision_count: input.target_revision_count,
            target_policy_count: input.target_policy_count,
            target_relation_count: 0,
            imported_at: input.imported_at,
            imported_by_role: input.imported_by_role,
        };
        manifest.validate_shape()?;
        Ok(manifest)
    }
}

impl MigrationManifest {
    pub fn canonical_bytes_and_hash(&self) -> Result<(Vec<u8>, String), LedgerError> {
        self.validate_shape()?;
        canonical_bytes_and_hash(self, TARGET_DOMAIN)
    }

    pub(crate) fn verify_hash(&self, expected: &str) -> Result<(), LedgerError> {
        let (_, actual) = self.canonical_bytes_and_hash()?;
        if actual == expected {
            Ok(())
        } else {
            Err(LedgerError::Invalid {
                category: "migration_manifest_hash_mismatch",
            })
        }
    }

    pub(crate) fn validate_target_records(
        &self,
        source: &SourceManifest,
        revisions: &[CanonicalRevision],
        policies: &[PolicyRecord],
        relation_count: usize,
    ) -> Result<(), LedgerError> {
        if self.stream_mappings.len() as u64 != source.source_stream_count
            || self.target_revision_count != revisions.len() as u64
            || self.target_policy_count != policies.len() as u64
            || self.target_relation_count != relation_count as u64
        {
            return Err(LedgerError::Invalid {
                category: "migration_manifest_count_mismatch",
            });
        }
        let revisions_by_id: std::collections::HashMap<_, _> = revisions
            .iter()
            .map(|revision| (revision.revision_id.as_str(), revision))
            .collect();
        let mut covered_revisions = HashSet::new();
        let mut covered_policies = HashSet::new();
        let mut legacy_revision_ids = HashSet::new();
        let mut deleted_streams = 0_u64;
        for stream in &self.stream_mappings {
            if !source
                .source_snapshots
                .iter()
                .any(|snapshot| snapshot.legacy_agent_id == stream.legacy_agent_id)
            {
                return Err(LedgerError::Invalid {
                    category: "migration_manifest_unknown_legacy_agent",
                });
            }
            let mapped_origin_role = source
                .role_mappings
                .iter()
                .find(|mapping| mapping.legacy_agent_id == stream.legacy_agent_id)
                .map(|mapping| mapping.target_role_id.as_str())
                .ok_or(LedgerError::Invalid {
                    category: "migration_manifest_missing_origin_mapping",
                })?;
            if stream
                .legacy_revision_ids
                .iter()
                .any(|revision_id| !legacy_revision_ids.insert(revision_id))
            {
                return Err(LedgerError::Invalid {
                    category: "migration_manifest_duplicate_legacy_revision",
                });
            }
            let stream_revisions: Vec<_> = revisions
                .iter()
                .filter(|revision| revision.memory_id.as_str() == stream.memory_id)
                .collect();
            let expected_revision_count =
                stream.legacy_revision_ids.len() + usize::from(stream.tombstone_revision_id.is_some());
            if stream_revisions.len() != expected_revision_count
                || stream_revisions
                    .iter()
                    .any(|revision| !covered_revisions.insert(revision.revision_id.as_str()))
            {
                return Err(LedgerError::Invalid {
                    category: "migration_manifest_revision_bijection_mismatch",
                });
            }
            for (position, legacy_revision_id) in stream.legacy_revision_ids.iter().enumerate() {
                let revision = revisions_by_id
                    .get(legacy_revision_id.as_str())
                    .ok_or(LedgerError::Invalid {
                        category: "migration_manifest_missing_legacy_revision",
                    })?;
                let expected_parent = position
                    .checked_sub(1)
                    .and_then(|previous| stream.legacy_revision_ids.get(previous))
                    .map(String::as_str);
                if revision.memory_id.as_str() != stream.memory_id
                    || revision.deleted_at.is_some()
                    || revision.parent_revision_id.as_ref().map(|parent| parent.as_str()) != expected_parent
                    || revision.committed_at != self.imported_at
                    || revision.committed_by_role != self.imported_by_role
                {
                    return Err(LedgerError::Invalid {
                        category: "migration_manifest_legacy_revision_mismatch",
                    });
                }
            }
            if let Some(tombstone_id) = &stream.tombstone_revision_id {
                let tombstone = revisions_by_id.get(tombstone_id.as_str()).ok_or(LedgerError::Invalid {
                    category: "migration_manifest_missing_tombstone",
                })?;
                if tombstone.parent_revision_id.as_ref().map(|parent| parent.as_str())
                    != stream.legacy_revision_ids.last().map(String::as_str)
                    || tombstone.committed_at != self.imported_at
                    || tombstone.committed_by_role != self.imported_by_role
                {
                    return Err(LedgerError::Invalid {
                        category: "migration_manifest_tombstone_mismatch",
                    });
                }
            }
            let head = revisions_by_id
                .get(stream.target_head_revision_id.as_str())
                .ok_or(LedgerError::Invalid {
                    category: "migration_manifest_missing_head",
                })?;
            let actual_head =
                stream_revisions
                    .iter()
                    .max_by_key(|revision| revision.sequence)
                    .ok_or(LedgerError::Invalid {
                        category: "migration_manifest_empty_stream",
                    })?;
            if head.memory_id.as_str() != stream.memory_id {
                return Err(LedgerError::Invalid {
                    category: "migration_manifest_stream_mismatch",
                });
            }
            if actual_head.revision_id != head.revision_id
                || stream.tombstone_revision_id.is_some() != head.deleted_at.is_some()
                || stream
                    .tombstone_revision_id
                    .as_deref()
                    .is_some_and(|id| id != head.revision_id.as_str())
                || stream.legacy_created_at_by_revision.len() != stream.legacy_revision_ids.len()
                || stream
                    .legacy_created_at_by_revision
                    .iter()
                    .zip(&stream.legacy_revision_ids)
                    .any(|(created, revision_id)| &created.revision_id != revision_id)
            {
                return Err(LedgerError::Invalid {
                    category: "migration_manifest_disposition_mismatch",
                });
            }
            deleted_streams += u64::from(stream.tombstone_revision_id.is_some());
            let stream_policies: Vec<_> = policies
                .iter()
                .filter(|policy| policy.memory_id.as_str() == stream.memory_id)
                .collect();
            if stream_policies.len() != stream.policy_mappings.len() {
                return Err(LedgerError::Invalid {
                    category: "migration_manifest_policy_bijection_mismatch",
                });
            }
            for mapping in &stream.policy_mappings {
                let policy = policies
                    .iter()
                    .find(|policy| {
                        policy.memory_id.as_str() == stream.memory_id
                            && policy.effective_from_revision_id.as_str() == mapping.effective_from_revision_id
                    })
                    .ok_or(LedgerError::Invalid {
                        category: "migration_manifest_missing_policy",
                    })?;
                if !covered_policies.insert(policy.policy_id.as_str()) {
                    return Err(LedgerError::Invalid {
                        category: "migration_manifest_policy_bijection_mismatch",
                    });
                }
                let expected_mode = match mapping.target_mode {
                    ManifestTargetMode::Private => PolicyMode::Private,
                    ManifestTargetMode::ExplicitRoleSet => PolicyMode::ExplicitRoleSet,
                };
                let scope_mode_matches = matches!(
                    (&mapping.source_scope, mapping.target_mode),
                    (ManifestSourceScope::Private, ManifestTargetMode::Private)
                        | (
                            ManifestSourceScope::Shared | ManifestSourceScope::Group { .. },
                            ManifestTargetMode::ExplicitRoleSet
                        )
                );
                let mut private_roles = vec![policy.origin_role_id.clone(), crate::PERSONAL_OWNER_ROLE_ID.to_string()];
                private_roles.sort();
                private_roles.dedup();
                let expected_readers = match &mapping.source_scope {
                    ManifestSourceScope::Private => private_roles.clone(),
                    ManifestSourceScope::Shared => source.authorized_role_ids_at_cutoff.clone(),
                    ManifestSourceScope::Group { legacy_group_id } => {
                        let mut roles = source
                            .group_mappings
                            .iter()
                            .find(|group| group.legacy_group_id == *legacy_group_id)
                            .map(|group| group.target_role_ids.clone())
                            .ok_or(LedgerError::Invalid {
                                category: "migration_manifest_unknown_group",
                            })?;
                        roles.extend(private_roles.iter().cloned());
                        roles.sort();
                        roles.dedup();
                        roles
                    }
                };
                if !scope_mode_matches
                    || policy.origin_role_id != mapped_origin_role
                    || policy.mode != expected_mode
                    || policy.reader_roles != mapping.audience_cutoff_roles
                    || mapping.audience_cutoff_roles != expected_readers
                    || policy.writer_roles != private_roles
                {
                    return Err(LedgerError::Invalid {
                        category: "migration_manifest_policy_mismatch",
                    });
                }
                if policy.committed_at != self.imported_at || policy.committed_by_role != self.imported_by_role {
                    return Err(LedgerError::Invalid {
                        category: "migration_manifest_policy_commit_mismatch",
                    });
                }
            }
        }
        let dispositions = &self.dispositions;
        if covered_revisions.len() != revisions.len()
            || covered_policies.len() != policies.len()
            || legacy_revision_ids.len() as u64 != source.source_entry_count
            || dispositions.importance_values_discarded != source.source_entry_count
            || dispositions.access_statistic_fields_discarded != source.source_entry_count.saturating_mul(2)
            || dispositions.legacy_created_at_fields_discarded != source.source_entry_count
            || dispositions.deleted_heads_converted_to_tombstones != deleted_streams
            || dispositions.projection_rebuilds_required != source.source_stream_count.saturating_sub(deleted_streams)
            || dispositions.ttl_fields_discarded != source.source_ttl_field_count
            || dispositions.embedded_vectors_discarded != source.source_embedded_vector_count
            || self.target_revision_count != source.source_entry_count.saturating_add(deleted_streams)
            || self.target_policy_count
                != self
                    .stream_mappings
                    .iter()
                    .map(|stream| stream.policy_mappings.len() as u64)
                    .sum::<u64>()
            || revisions
                .iter()
                .map(|revision| revision.memory_id.as_str())
                .collect::<HashSet<_>>()
                != self
                    .stream_mappings
                    .iter()
                    .map(|stream| stream.memory_id.as_str())
                    .collect::<HashSet<_>>()
        {
            return Err(LedgerError::Invalid {
                category: "migration_manifest_disposition_count_mismatch",
            });
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), LedgerError> {
        let mut memory_ids = HashSet::new();
        if self.schema != TARGET_SCHEMA
            || !is_lower_hash(&self.source_manifest_hash)
            || self.imported_by_role != crate::PERSONAL_OWNER_ROLE_ID
            || self.imported_at == 0
            || self.target_relation_count != 0
            || self.stream_mappings.iter().any(|stream| {
                stream.legacy_agent_id.trim().is_empty()
                    || uuid::Uuid::parse_str(&stream.memory_id).is_err()
                    || !memory_ids.insert(&stream.memory_id)
                    || stream.legacy_revision_ids.is_empty()
                    || !unique_nonempty(&stream.legacy_revision_ids)
                    || uuid::Uuid::parse_str(&stream.target_head_revision_id).is_err()
                    || stream
                        .tombstone_revision_id
                        .as_ref()
                        .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
                    || stream.policy_mappings.is_empty()
                    || stream.policy_mappings.iter().any(|mapping| {
                        uuid::Uuid::parse_str(&mapping.effective_from_revision_id).is_err()
                            || !strict_roles(&mapping.audience_cutoff_roles)
                    })
            })
        {
            return Err(LedgerError::Invalid {
                category: "invalid_migration_manifest",
            });
        }
        Ok(())
    }
}

#[cfg(feature = "offline-migration")]
pub struct MigrationManifestInput {
    pub source_manifest_hash: String,
    pub stream_mappings: Vec<StreamMapping>,
    pub dispositions: MigrationDispositionCounters,
    pub target_revision_count: u64,
    pub target_policy_count: u64,
    pub imported_at: u64,
    pub imported_by_role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSnapshot {
    pub legacy_agent_id: String,
    pub legacy_tier: String,
    pub cid: String,
    pub object_envelope_hash: String,
    pub entry_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleMapping {
    pub legacy_agent_id: String,
    pub target_role_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupMapping {
    pub legacy_group_id: String,
    pub target_role_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamMapping {
    pub legacy_agent_id: String,
    pub memory_id: String,
    pub legacy_revision_ids: Vec<String>,
    pub legacy_created_at_by_revision: Vec<LegacyCreatedAtMapping>,
    pub target_head_revision_id: String,
    pub tombstone_revision_id: Option<String>,
    pub legacy_deleted_at: Option<u64>,
    pub policy_mappings: Vec<PolicyMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyCreatedAtMapping {
    pub revision_id: String,
    pub legacy_created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyMapping {
    pub effective_from_revision_id: String,
    pub source_scope: ManifestSourceScope,
    pub target_mode: ManifestTargetMode,
    pub audience_cutoff_roles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestSourceScope {
    Private,
    Shared,
    Group { legacy_group_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestTargetMode {
    Private,
    ExplicitRoleSet,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationDispositionCounters {
    pub importance_values_discarded: u64,
    pub access_statistic_fields_discarded: u64,
    pub ttl_fields_discarded: u64,
    pub embedded_vectors_discarded: u64,
    pub projection_rebuilds_required: u64,
    pub legacy_created_at_fields_discarded: u64,
    pub deleted_heads_converted_to_tombstones: u64,
}

fn canonical_bytes_and_hash<T: Serialize>(value: &T, domain: &[u8]) -> Result<(Vec<u8>, String), LedgerError> {
    let bytes = serde_json_canonicalizer::to_vec(value).map_err(|_| LedgerError::Invalid {
        category: "jcs_canonicalization_failed",
    })?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(&bytes);
    Ok((bytes, format!("{:x}", hasher.finalize())))
}

fn is_lower_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn strict_roles(values: &[String]) -> bool {
    !values.is_empty()
        && values.iter().all(|value| !value.trim().is_empty())
        && values.windows(2).all(|pair| pair[0] < pair[1])
}

fn unique_nonempty(values: &[String]) -> bool {
    let mut unique = HashSet::new();
    values
        .iter()
        .all(|value| !value.trim().is_empty() && unique.insert(value))
}

#[cfg(all(test, feature = "offline-migration"))]
mod tests {
    use super::*;
    use crate::memory::{CanonicalRevision, MemoryContent, MemoryId, MemoryRevisionId, MemoryTier, MemoryType};

    use super::super::{MigrationPolicyInput, MigrationRevisionInput, PolicyRecord};

    fn valid_target() -> (
        SourceManifest,
        MigrationManifest,
        Vec<CanonicalRevision>,
        Vec<PolicyRecord>,
    ) {
        let revision_id = uuid::Uuid::new_v4().to_string();
        let source = SourceManifest::new(SourceManifestInput {
            source_index_hash: "a".repeat(64),
            credential_role_cutoff_hash: "b".repeat(64),
            lock_created: false,
            legacy_namespace: None,
            source_snapshots: vec![SourceSnapshot {
                legacy_agent_id: "legacy-a".into(),
                legacy_tier: "working".into(),
                cid: "c".repeat(64),
                object_envelope_hash: "d".repeat(64),
                entry_count: 1,
            }],
            source_entry_count: 1,
            source_stream_count: 1,
            source_ttl_field_count: 0,
            source_embedded_vector_count: 0,
            authorized_role_ids_at_cutoff: vec!["personal-owner".into(), "role-a".into()],
            role_mappings: vec![RoleMapping {
                legacy_agent_id: "legacy-a".into(),
                target_role_id: "role-a".into(),
            }],
            group_mappings: vec![],
        })
        .unwrap();
        let memory_id = MemoryId::from(revision_id.clone());
        let revision = CanonicalRevision::migration_import(MigrationRevisionInput {
            sequence: 1,
            memory_id: memory_id.clone(),
            revision_id: MemoryRevisionId::from(revision_id.clone()),
            parent_revision_id: None,
            content: MemoryContent::Text("evidence".into()),
            tags: vec![],
            memory_type: MemoryType::Untyped,
            cognitive_tier: MemoryTier::Working,
            deleted_at: None,
            committed_at: 42,
            actor: crate::PERSONAL_OWNER_ROLE_ID.into(),
        })
        .unwrap();
        let policy = PolicyRecord::migration_private(MigrationPolicyInput {
            sequence: 1,
            policy_id: uuid::Uuid::new_v4().to_string(),
            memory_id,
            effective_from_revision_id: MemoryRevisionId::from(revision_id.clone()),
            source_role: "role-a".into(),
            committed_at: 42,
            actor: crate::PERSONAL_OWNER_ROLE_ID.into(),
        })
        .unwrap();
        let (_, source_hash) = source.canonical_bytes_and_hash().unwrap();
        let manifest = MigrationManifest::new(MigrationManifestInput {
            source_manifest_hash: source_hash,
            stream_mappings: vec![StreamMapping {
                legacy_agent_id: "legacy-a".into(),
                memory_id: revision_id.clone(),
                legacy_revision_ids: vec![revision_id.clone()],
                legacy_created_at_by_revision: vec![LegacyCreatedAtMapping {
                    revision_id: revision_id.clone(),
                    legacy_created_at: 1,
                }],
                target_head_revision_id: revision_id.clone(),
                tombstone_revision_id: None,
                legacy_deleted_at: None,
                policy_mappings: vec![PolicyMapping {
                    effective_from_revision_id: revision_id,
                    source_scope: ManifestSourceScope::Private,
                    target_mode: ManifestTargetMode::Private,
                    audience_cutoff_roles: vec!["personal-owner".into(), "role-a".into()],
                }],
            }],
            dispositions: MigrationDispositionCounters {
                importance_values_discarded: 1,
                access_statistic_fields_discarded: 2,
                projection_rebuilds_required: 1,
                legacy_created_at_fields_discarded: 1,
                ..MigrationDispositionCounters::default()
            },
            target_revision_count: 1,
            target_policy_count: 1,
            imported_at: 42,
            imported_by_role: crate::PERSONAL_OWNER_ROLE_ID.into(),
        })
        .unwrap();
        (source, manifest, vec![revision], vec![policy])
    }

    #[test]
    fn origin_swap_and_scope_mode_tampering_fail_closed() {
        let (source, manifest, revisions, policies) = valid_target();
        manifest
            .validate_target_records(&source, &revisions, &policies, 0)
            .unwrap();

        let mut swapped = policies.clone();
        swapped[0].origin_role_id = "role-b".into();
        assert!(manifest
            .validate_target_records(&source, &revisions, &swapped, 0)
            .is_err());

        let mut wrong_mode = manifest;
        wrong_mode.stream_mappings[0].policy_mappings[0].target_mode = ManifestTargetMode::ExplicitRoleSet;
        assert!(wrong_mode
            .validate_target_records(&source, &revisions, &policies, 0)
            .is_err());
    }
}
