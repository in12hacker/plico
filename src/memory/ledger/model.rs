use serde::{Deserialize, Serialize};

use crate::memory::{CanonicalContentHash, MemoryContent, MemoryId, MemoryRevisionId, MemoryTier, MemoryType};

pub(super) const REVISION_SCHEMA: &str = "plico.memory.revision/v1";
pub(super) const POLICY_SCHEMA: &str = "plico.memory.policy/v1";
pub(super) const RELATION_SCHEMA: &str = "plico.memory.relation/v1";
pub(super) const REVISION_SEGMENT_SCHEMA: &str = "plico.memory.revision-segment/v1";
pub(super) const POLICY_SEGMENT_SCHEMA: &str = "plico.memory.policy-segment/v1";
pub(super) const RELATION_SEGMENT_SCHEMA: &str = "plico.memory.relation-segment/v1";
pub(super) const ROOT_SCHEMA: &str = "plico.memory.root/v1";
pub(super) const CURRENT_VIEW_SCHEMA: &str = "plico.memory.current-view/v1";
pub(super) const ROOT_POINTER_SCHEMA: &str = "plico.memory.root-pointer/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalRevision {
    pub schema: String,
    pub sequence: u64,
    pub memory_id: MemoryId,
    pub revision_id: MemoryRevisionId,
    pub parent_revision_id: Option<MemoryRevisionId>,
    pub content: MemoryContent,
    pub content_hash: CanonicalContentHash,
    pub tags: Vec<String>,
    pub memory_type: MemoryType,
    pub cognitive_tier: MemoryTier,
    pub deleted_at: Option<u64>,
    pub committed_at: u64,
    pub committed_by_role: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    Private,
    ExplicitRoleSet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRecord {
    pub schema: String,
    pub sequence: u64,
    pub policy_id: String,
    pub memory_id: MemoryId,
    pub effective_from_revision_id: MemoryRevisionId,
    pub origin_role_id: String,
    pub mode: PolicyMode,
    pub reader_roles: Vec<String>,
    pub writer_roles: Vec<String>,
    pub committed_at: u64,
    pub committed_by_role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RelationRecord {
    pub schema: String,
    pub sequence: u64,
    pub relation_id: String,
    pub subject_revision_id: MemoryRevisionId,
    pub predicate: String,
    pub object_revision_id: MemoryRevisionId,
    pub epistemic_state: String,
    pub provenance_manifest_id: String,
    pub committed_at: u64,
    pub committed_by_role: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LogKind {
    Revision,
    Policy,
    Relation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Segment<T> {
    pub schema: String,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub previous_segment_hash: Option<String>,
    pub records: Vec<T>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRoot {
    pub schema: String,
    pub generation: u64,
    pub previous_root_hash: Option<String>,
    pub revision_head: Option<String>,
    pub revision_watermark: u64,
    pub policy_head: Option<String>,
    pub policy_watermark: u64,
    pub relation_head: Option<String>,
    pub relation_watermark: u64,
    pub current_view_hash: String,
    pub migration_manifest_hash: Option<String>,
    pub committed_at: u64,
    pub committed_by_role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CurrentStream {
    pub memory_id: MemoryId,
    pub head_revision_id: MemoryRevisionId,
    pub deleted: bool,
    pub policy_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentView {
    pub schema: String,
    pub generation: u64,
    pub revision_watermark: u64,
    pub policy_watermark: u64,
    pub relation_watermark: u64,
    pub(super) streams: Vec<CurrentStream>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedHead {
    Absent,
    Revision(MemoryRevisionId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerCommit {
    pub generation: u64,
    pub revision_sequence: u64,
    pub revision_id: MemoryRevisionId,
    pub committed_at: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("ledger I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ledger serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("ledger CAS error: {0}")]
    Cas(String),
    #[error("invalid canonical ledger: {category}")]
    Invalid { category: &'static str },
    #[error("durable policy is unsupported: {category}")]
    UnsupportedPolicy { category: &'static str },
    #[error("canonical memory head conflict")]
    HeadConflict {
        memory_id: MemoryId,
        expected: ExpectedHead,
        actual: Option<MemoryRevisionId>,
    },
    #[error("canonical ledger publish outcome is indeterminate; restart is required")]
    CommitIndeterminate,
    #[error("canonical ledger writer is poisoned; restart is required")]
    WriterPoisoned,
    #[error("personal vault is already locked by another runtime")]
    VaultLocked,
}

impl LedgerError {
    pub(crate) fn category(&self) -> &'static str {
        match self {
            Self::Io(_) => "ledger_io",
            Self::Serialization(_) => "ledger_serialization",
            Self::Cas(_) => "ledger_cas",
            Self::Invalid { category } | Self::UnsupportedPolicy { category } => category,
            Self::HeadConflict { .. } => "head_conflict",
            Self::CommitIndeterminate => "commit_indeterminate",
            Self::WriterPoisoned => "writer_poisoned",
            Self::VaultLocked => "vault_locked",
        }
    }
}

#[cfg(feature = "offline-migration")]
impl CanonicalRevision {
    pub fn migration_import(input: MigrationRevisionInput) -> Result<Self, LedgerError> {
        let content_hash = input
            .content
            .canonical_content_hash()
            .map_err(|category| LedgerError::Invalid { category })?;
        Ok(Self {
            schema: REVISION_SCHEMA.to_string(),
            sequence: input.sequence,
            memory_id: input.memory_id,
            revision_id: input.revision_id,
            parent_revision_id: input.parent_revision_id,
            content: input.content,
            content_hash,
            tags: input.tags,
            memory_type: input.memory_type,
            cognitive_tier: input.cognitive_tier,
            deleted_at: input.deleted_at,
            committed_at: input.committed_at,
            committed_by_role: input.actor,
        })
    }
}

#[cfg(feature = "offline-migration")]
pub struct MigrationRevisionInput {
    pub sequence: u64,
    pub memory_id: MemoryId,
    pub revision_id: MemoryRevisionId,
    pub parent_revision_id: Option<MemoryRevisionId>,
    pub content: MemoryContent,
    pub tags: Vec<String>,
    pub memory_type: MemoryType,
    pub cognitive_tier: MemoryTier,
    pub deleted_at: Option<u64>,
    pub committed_at: u64,
    pub actor: String,
}

#[cfg(feature = "offline-migration")]
impl PolicyRecord {
    pub fn migration_private(input: MigrationPolicyInput) -> Result<Self, LedgerError> {
        validate_role(&input.source_role)?;
        validate_role(&input.actor)?;
        let roles = sorted_roles([input.source_role.clone(), crate::PERSONAL_OWNER_ROLE_ID.to_string()]);
        Ok(Self {
            schema: POLICY_SCHEMA.to_string(),
            sequence: input.sequence,
            policy_id: input.policy_id,
            memory_id: input.memory_id,
            effective_from_revision_id: input.effective_from_revision_id,
            origin_role_id: input.source_role,
            mode: PolicyMode::Private,
            reader_roles: roles.clone(),
            writer_roles: roles,
            committed_at: input.committed_at,
            committed_by_role: input.actor,
        })
    }

    pub fn migration_explicit_role_set(
        input: MigrationPolicyInput,
        reader_roles: Vec<String>,
    ) -> Result<Self, LedgerError> {
        validate_role(&input.source_role)?;
        validate_role(&input.actor)?;
        if reader_roles.is_empty()
            || !strict_sorted_roles(&reader_roles)
            || !reader_roles.contains(&input.source_role)
            || !reader_roles.iter().any(|role| role == crate::PERSONAL_OWNER_ROLE_ID)
        {
            return Err(LedgerError::Invalid {
                category: "invalid_migration_audience",
            });
        }
        let writers = sorted_roles([input.source_role.clone(), crate::PERSONAL_OWNER_ROLE_ID.to_string()]);
        Ok(Self {
            schema: POLICY_SCHEMA.to_string(),
            sequence: input.sequence,
            policy_id: input.policy_id,
            memory_id: input.memory_id,
            effective_from_revision_id: input.effective_from_revision_id,
            origin_role_id: input.source_role,
            mode: PolicyMode::ExplicitRoleSet,
            reader_roles,
            writer_roles: writers,
            committed_at: input.committed_at,
            committed_by_role: input.actor,
        })
    }
}

#[cfg(feature = "offline-migration")]
pub struct MigrationPolicyInput {
    pub sequence: u64,
    pub policy_id: String,
    pub memory_id: MemoryId,
    pub effective_from_revision_id: MemoryRevisionId,
    pub source_role: String,
    pub committed_at: u64,
    pub actor: String,
}

#[cfg(feature = "offline-migration")]
fn sorted_roles(roles: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut roles: Vec<_> = roles.into_iter().collect();
    roles.sort();
    roles.dedup();
    roles
}

#[cfg(feature = "offline-migration")]
fn validate_role(role: &str) -> Result<(), LedgerError> {
    if role.trim().is_empty() {
        Err(LedgerError::Invalid {
            category: "empty_migration_role",
        })
    } else {
        Ok(())
    }
}

#[cfg(feature = "offline-migration")]
fn strict_sorted_roles(roles: &[String]) -> bool {
    roles.iter().all(|role| !role.trim().is_empty()) && roles.windows(2).all(|pair| pair[0] < pair[1])
}
