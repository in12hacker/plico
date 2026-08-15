use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use uuid::Uuid;

use super::current_view::{head_for, rebuild_current_view};
use super::hash::{
    root_bytes_and_hash, segment_bytes_and_hash, verify_root_hash, verify_segment_hash, verify_view_hash,
    view_bytes_and_hash,
};
use super::migration_manifest::{MigrationManifest, SourceManifest};
use super::model::{
    CanonicalRevision, CurrentView, ExpectedHead, LedgerCommit, LedgerError, LedgerRoot, LogKind, PolicyMode,
    PolicyRecord, RelationRecord, Segment, CURRENT_VIEW_SCHEMA, POLICY_SCHEMA, POLICY_SEGMENT_SCHEMA, RELATION_SCHEMA,
    RELATION_SEGMENT_SCHEMA, REVISION_SCHEMA, REVISION_SEGMENT_SCHEMA, ROOT_POINTER_SCHEMA, ROOT_SCHEMA,
};
use super::validate::{validate_policies, validate_revisions};
use crate::cas::{
    ImmutableLedgerNamespace, ImmutableLedgerStorage, LedgerStorageError, LedgerStorageOpenError, PersonalVaultStorage,
};
use crate::memory::{MemoryContent, MemoryEntry, MemoryRevisionId, MemoryScope, MemoryTier};

const LOCAL_OWNER_ROLE: &str = "personal-owner";
#[cfg(test)]
const LEDGER_DIRECTORY: &str = "memory-ledger";
#[cfg(test)]
const LEGACY_INDEX: &str = "memory_index.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivePointer {
    schema: String,
    root_hash: String,
}

#[derive(Default)]
struct LedgerState {
    poisoned: bool,
    root_hash: Option<String>,
    root: Option<LedgerRoot>,
    view: Option<CurrentView>,
    revisions: Vec<CanonicalRevision>,
    policies: Vec<PolicyRecord>,
    relations: Vec<RelationRecord>,
}

/// The sole runtime writer/reader for canonical memory durability.
pub(crate) trait CanonicalLedger: Send + Sync {
    fn commit_expected(
        &self,
        role_id: &str,
        tier: MemoryTier,
        expected_head: ExpectedHead,
        revision: CanonicalRevision,
    ) -> Result<LedgerCommit, LedgerError>;

    fn commit_roots(
        &self,
        role_id: &str,
        tier: MemoryTier,
        revisions: Vec<CanonicalRevision>,
    ) -> Result<Vec<LedgerCommit>, LedgerError>;

    fn rebuild_origin_role(&self, role_id: &str) -> Result<Vec<(MemoryTier, Vec<CanonicalRevision>)>, LedgerError>;

    fn list_origin_roles(&self) -> Result<Vec<String>, LedgerError>;

    fn readable_active_revision_ids(&self, role_id: &str) -> Result<Vec<MemoryRevisionId>, LedgerError>;

    fn origin_for_revision(&self, role_id: &str, revision_id: &str, write: bool)
        -> Result<Option<String>, LedgerError>;

    fn flush(&self) -> Result<(), LedgerError>;
}

/// Read-only canonical boundary used by derived projection controllers.
pub(crate) trait CanonicalProjectionSource: Send + Sync {
    /// Return text only while the exact revision remains the current eligible
    /// head. The text is never stored in a projection job or manifest event.
    fn guarded_projection_document(
        &self,
        expected: &crate::memory::projection::CanonicalSourceIdentity,
    ) -> Result<Option<String>, LedgerError>;
}

/// Filesystem/CAS-root-scoped implementation of the immutable ledger.
///
/// The `CASStorage` argument deliberately supplies only the personal vault
/// root boundary. Ledger objects live in `memory-ledger/objects` because their
/// domain-separated hashes are not ordinary CAS data CIDs.
pub(crate) struct CASCanonicalLedger {
    _vault: Arc<PersonalVaultStorage>,
    storage: ImmutableLedgerStorage,
    state: Mutex<LedgerState>,
    #[cfg(test)]
    projection_guard_hook: Mutex<Option<(std::sync::mpsc::SyncSender<()>, std::sync::mpsc::Receiver<()>)>>,
}

#[derive(Clone)]
pub(crate) struct CanonicalProjectionSnapshot {
    pub genesis_root_hash: String,
    pub genesis_root: LedgerRoot,
    pub root_hash: String,
    pub root: LedgerRoot,
    pub root_chain: Vec<(String, LedgerRoot)>,
    pub revisions: Vec<CanonicalRevision>,
}

/// Borrowed proof that a canonical projection source is still current. The
/// guard deliberately cannot be cloned, formatted, or serialized and keeps
/// the canonical state lock held for the synchronous projection commit.
pub(crate) struct CanonicalProjectionGuard<'a> {
    source: crate::memory::projection::CanonicalSourceIdentity,
    snapshot: CanonicalProjectionSnapshot,
    _state: MutexGuard<'a, LedgerState>,
}

/// Scoped proof that a trusted local role may observe the exact current,
/// non-deleted canonical revision. It is intentionally neither constructible,
/// cloneable, formattable nor serializable outside this module.
pub(crate) struct AuthorizedCurrentRevisionProof<'a> {
    source: crate::memory::projection::CanonicalSourceIdentity,
    canonical_ancestry: Vec<crate::memory::projection::CanonicalWatermark>,
    _state: MutexGuard<'a, LedgerState>,
}

impl AuthorizedCurrentRevisionProof<'_> {
    pub(crate) fn source(&self) -> &crate::memory::projection::CanonicalSourceIdentity {
        &self.source
    }

    pub(crate) fn reconciled_source_is_ancestor(
        &self,
        candidate: &crate::memory::projection::CanonicalWatermark,
    ) -> bool {
        self.canonical_ancestry.iter().any(|watermark| watermark == candidate)
    }
}

/// Scoped proof for online personal-owner projection maintenance. The fresh
/// canonical snapshot and its state lock live for the whole projection
/// transaction, preventing update/delete races.
pub(crate) struct AuthorizedOwnerProjectionProof<'a> {
    snapshot: CanonicalProjectionSnapshot,
    _state: MutexGuard<'a, LedgerState>,
}

impl AuthorizedOwnerProjectionProof<'_> {
    pub(crate) fn snapshot(&self) -> &CanonicalProjectionSnapshot {
        &self.snapshot
    }
}

impl CanonicalProjectionGuard<'_> {
    pub(crate) fn snapshot(&self) -> &CanonicalProjectionSnapshot {
        &self.snapshot
    }

    pub(crate) fn authorizes(&self, source: &crate::memory::projection::CanonicalSourceIdentity) -> bool {
        self.source == *source
    }
}

#[cfg(feature = "offline-migration")]
pub struct OfflineMigrationTargetInput {
    pub source_manifest: SourceManifest,
    pub migration_manifest: MigrationManifest,
    pub revisions: Vec<CanonicalRevision>,
    pub policies: Vec<PolicyRecord>,
    pub committed_at: u64,
    pub committed_by_role: String,
}

/// Build and publish generation 1 of an offline migration staging vault.
///
/// This entry point is unavailable to runtime builds. It only accepts a fresh
/// storage initialized at the exact empty genesis and runs the same validators
/// and replay loader used at runtime before acknowledging publication.
#[cfg(feature = "offline-migration")]
pub fn build_offline_migration_target(
    target: &mut crate::cas::OfflineMigrationTarget,
    input: OfflineMigrationTargetInput,
) -> Result<String, LedgerError> {
    let source_manifest_hash_for_seal = input.migration_manifest.source_manifest_hash.clone();
    let credential_hash_for_seal = input.source_manifest.credential_role_cutoff_hash.clone();
    let revision_count = input.revisions.len() as u64;
    let policy_count = input.policies.len() as u64;
    let storage = target.ledger_storage();
    CASCanonicalLedger::ensure_genesis(storage)?;
    let genesis = CASCanonicalLedger::load_state(storage)?;
    let genesis_root = genesis.root.as_ref().ok_or(LedgerError::Invalid {
        category: "missing_genesis_root",
    })?;
    if genesis_root.generation != 0
        || !genesis.revisions.is_empty()
        || !genesis.policies.is_empty()
        || !genesis.relations.is_empty()
    {
        return Err(LedgerError::Invalid {
            category: "offline_target_not_empty",
        });
    }
    if input.committed_at == 0
        || input.committed_by_role.trim().is_empty()
        || input.migration_manifest.imported_at != input.committed_at
        || input.migration_manifest.imported_by_role != input.committed_by_role
    {
        return Err(LedgerError::Invalid {
            category: "migration_manifest_commit_boundary_mismatch",
        });
    }
    validate_revisions(&input.revisions)?;
    validate_policies(&input.policies, &input.revisions)?;
    input
        .migration_manifest
        .validate_target_records(&input.source_manifest, &input.revisions, &input.policies, 0)?;
    let (source_bytes, source_hash) = input.source_manifest.canonical_bytes_and_hash()?;
    if input.migration_manifest.source_manifest_hash != source_hash {
        return Err(LedgerError::Invalid {
            category: "migration_source_manifest_mismatch",
        });
    }
    let (manifest_bytes, manifest_hash) = input.migration_manifest.canonical_bytes_and_hash()?;

    let revision_object = if input.revisions.is_empty() {
        None
    } else {
        let segment = Segment {
            schema: REVISION_SEGMENT_SCHEMA.to_string(),
            first_sequence: 1,
            last_sequence: input.revisions.len() as u64,
            previous_segment_hash: None,
            records: input.revisions.clone(),
        };
        let (bytes, hash) = segment_bytes_and_hash(&segment)?;
        Some((bytes, hash))
    };
    let policy_object = if input.policies.is_empty() {
        None
    } else {
        let segment = Segment {
            schema: POLICY_SEGMENT_SCHEMA.to_string(),
            first_sequence: 1,
            last_sequence: input.policies.len() as u64,
            previous_segment_hash: None,
            records: input.policies.clone(),
        };
        let (bytes, hash) = segment_bytes_and_hash(&segment)?;
        Some((bytes, hash))
    };
    let view = rebuild_current_view(1, &input.revisions, &input.policies, 0)?;
    let (view_bytes, view_hash) = view_bytes_and_hash(&view)?;
    let root = LedgerRoot {
        schema: ROOT_SCHEMA.to_string(),
        generation: 1,
        previous_root_hash: genesis.root_hash,
        revision_head: revision_object.as_ref().map(|(_, hash)| hash.clone()),
        revision_watermark: input.revisions.len() as u64,
        policy_head: policy_object.as_ref().map(|(_, hash)| hash.clone()),
        policy_watermark: input.policies.len() as u64,
        relation_head: None,
        relation_watermark: 0,
        current_view_hash: view_hash.clone(),
        migration_manifest_hash: Some(manifest_hash.clone()),
        committed_at: input.committed_at,
        committed_by_role: input.committed_by_role,
    };
    let (root_bytes, root_hash) = root_bytes_and_hash(&root)?;

    storage.put_immutable(&source_hash, &source_bytes)?;
    storage.put_immutable(&manifest_hash, &manifest_bytes)?;
    if let Some((bytes, hash)) = &revision_object {
        storage.put_immutable(hash, bytes)?;
    }
    if let Some((bytes, hash)) = &policy_object {
        storage.put_immutable(hash, bytes)?;
    }
    storage.put_immutable(&view_hash, &view_bytes)?;
    storage.put_immutable(&root_hash, &root_bytes)?;
    let pointer = ActivePointer {
        schema: ROOT_POINTER_SCHEMA.to_string(),
        root_hash: root_hash.clone(),
    };
    storage
        .publish_active(
            &serde_json_canonicalizer::to_vec(&pointer).map_err(|_| LedgerError::Invalid {
                category: "jcs_canonicalization_failed",
            })?,
        )
        .map_err(|error| match error {
            LedgerStorageError::Io(error) => LedgerError::Io(error),
            LedgerStorageError::PublishedButUnsynced(_) => LedgerError::CommitIndeterminate,
        })?;
    let verified = CASCanonicalLedger::load_state(storage)?;
    if verified.root_hash.as_deref() != Some(&root_hash) {
        return Err(LedgerError::Invalid {
            category: "offline_target_replay_mismatch",
        });
    }
    target
        .seal(
            root_hash.clone(),
            source_manifest_hash_for_seal,
            manifest_hash,
            credential_hash_for_seal,
            revision_count,
            policy_count,
        )
        .map_err(|_| LedgerError::Invalid {
            category: "offline_target_seal_failed",
        })?;
    Ok(root_hash)
}

impl CASCanonicalLedger {
    pub(crate) fn new(vault: Arc<PersonalVaultStorage>) -> Result<Self, LedgerError> {
        let storage = vault
            .immutable_ledger(ImmutableLedgerNamespace::Memory)
            .map_err(map_namespace_open_error)?;
        Self::ensure_genesis(&storage)?;
        let state = Self::load_state(&storage)?;
        Ok(Self {
            _vault: vault,
            storage,
            state: Mutex::new(state),
            #[cfg(test)]
            projection_guard_hook: Mutex::new(None),
        })
    }

    pub(crate) fn projection_snapshot(&self) -> Result<CanonicalProjectionSnapshot, LedgerError> {
        let state = self.state.lock().map_err(|_| LedgerError::Invalid {
            category: "canonical_ledger_state_poisoned",
        })?;
        if state.poisoned {
            return Err(LedgerError::WriterPoisoned);
        }
        self.projection_snapshot_from_state(&state)
    }

    fn projection_snapshot_from_state(&self, state: &LedgerState) -> Result<CanonicalProjectionSnapshot, LedgerError> {
        let root_hash = state.root_hash.clone().ok_or(LedgerError::Invalid {
            category: "missing_active_root",
        })?;
        let root = state.root.clone().ok_or(LedgerError::Invalid {
            category: "missing_active_root",
        })?;
        let mut root_chain = vec![(root_hash.clone(), root.clone())];
        let mut cursor = root.clone();
        while let Some(previous_hash) = cursor.previous_root_hash.clone() {
            let previous: LedgerRoot = read_hashed_object(&self.storage, &previous_hash)?;
            verify_root_hash(&previous, &previous_hash)?;
            root_chain.push((previous_hash, previous.clone()));
            cursor = previous;
        }
        root_chain.reverse();
        let (genesis_root_hash, genesis_root) = root_chain.first().cloned().ok_or(LedgerError::Invalid {
            category: "missing_genesis_root",
        })?;
        Ok(CanonicalProjectionSnapshot {
            genesis_root_hash,
            genesis_root,
            root_hash,
            root,
            root_chain,
            revisions: state.revisions.clone(),
        })
    }

    /// Run one projection completion while the canonical source remains the
    /// exact current, policy-backed eligible head. The closure executes under
    /// the canonical state lock, establishing the sole lock order of
    /// canonical state before projection state.
    pub(crate) fn with_current_projection_source<R>(
        &self,
        expected: &crate::memory::projection::CanonicalSourceIdentity,
        complete: impl for<'a> FnOnce(CanonicalProjectionGuard<'a>) -> R,
    ) -> Result<Option<R>, LedgerError> {
        let state = self.state.lock().map_err(|_| LedgerError::Invalid {
            category: "canonical_ledger_state_poisoned",
        })?;
        if state.poisoned {
            return Err(LedgerError::WriterPoisoned);
        }
        if current_projection_revision(&state, expected).is_none() {
            return Ok(None);
        }
        let snapshot = self.projection_snapshot_from_state(&state)?;
        #[cfg(test)]
        if let Some((entered, release)) = self
            .projection_guard_hook
            .lock()
            .map_err(|_| LedgerError::Invalid {
                category: "projection_guard_barrier_poisoned",
            })?
            .take()
        {
            entered.send(()).map_err(|_| LedgerError::Invalid {
                category: "projection_guard_test_hook_disconnected",
            })?;
            release
                .recv_timeout(std::time::Duration::from_secs(2))
                .map_err(|_| LedgerError::Invalid {
                    category: "projection_guard_test_hook_timeout",
                })?;
        }
        Ok(Some(complete(CanonicalProjectionGuard {
            source: expected.clone(),
            snapshot,
            _state: state,
        })))
    }

    pub(crate) fn with_authorized_current_revision<R>(
        &self,
        trusted_role: &str,
        revision_id: &crate::memory::MemoryRevisionId,
        observe: impl for<'a> FnOnce(AuthorizedCurrentRevisionProof<'a>) -> R,
    ) -> Result<Option<R>, LedgerError> {
        let state = self.state.lock().map_err(|_| LedgerError::Invalid {
            category: "canonical_ledger_state_poisoned",
        })?;
        if state.poisoned {
            return Err(LedgerError::WriterPoisoned);
        }
        let Some(actual) = state
            .revisions
            .iter()
            .find(|revision| &revision.revision_id == revision_id)
        else {
            return Ok(None);
        };
        let Some(stream) = state
            .view
            .as_ref()
            .and_then(|view| view.streams.iter().find(|stream| stream.memory_id == actual.memory_id))
        else {
            return Ok(None);
        };
        if stream.deleted || stream.head_revision_id != actual.revision_id || actual.deleted_at.is_some() {
            return Ok(None);
        }
        let authorized = state.policies.iter().any(|policy| {
            policy.policy_id == stream.policy_id
                && policy.memory_id == actual.memory_id
                && policy.reader_roles.iter().any(|role| role == trusted_role)
        });
        if !authorized {
            return Ok(None);
        }
        let source = crate::memory::projection::CanonicalSourceIdentity {
            canonical_kind: "memory_revision".to_string(),
            memory_id: actual.memory_id.clone(),
            revision_id: actual.revision_id.clone(),
            revision_sequence: actual.sequence,
            content_hash: actual.content_hash.clone(),
        };
        let canonical_ancestry = self
            .projection_snapshot_from_state(&state)?
            .root_chain
            .into_iter()
            .map(|(root_hash, root)| crate::memory::projection::CanonicalWatermark {
                root_hash,
                generation: root.generation,
                revision_watermark: root.revision_watermark,
                policy_watermark: root.policy_watermark,
                relation_watermark: root.relation_watermark,
            })
            .collect();
        Ok(Some(observe(AuthorizedCurrentRevisionProof {
            source,
            canonical_ancestry,
            _state: state,
        })))
    }

    pub(crate) fn with_owner_projection_maintenance<R>(
        &self,
        trusted_role: &str,
        maintain: impl for<'a> FnOnce(AuthorizedOwnerProjectionProof<'a>) -> R,
    ) -> Result<Option<R>, LedgerError> {
        if trusted_role != crate::PERSONAL_OWNER_ROLE_ID {
            return Ok(None);
        }
        let state = self.state.lock().map_err(|_| LedgerError::Invalid {
            category: "canonical_ledger_state_poisoned",
        })?;
        if state.poisoned {
            return Err(LedgerError::WriterPoisoned);
        }
        #[cfg(test)]
        if let Some((entered, release)) = self
            .projection_guard_hook
            .lock()
            .map_err(|_| LedgerError::Invalid {
                category: "projection_guard_barrier_poisoned",
            })?
            .take()
        {
            entered.send(()).map_err(|_| LedgerError::Invalid {
                category: "projection_guard_test_hook_disconnected",
            })?;
            release
                .recv_timeout(std::time::Duration::from_secs(2))
                .map_err(|_| LedgerError::Invalid {
                    category: "projection_guard_test_hook_timeout",
                })?;
        }
        let snapshot = self.projection_snapshot_from_state(&state)?;
        Ok(Some(maintain(AuthorizedOwnerProjectionProof {
            snapshot,
            _state: state,
        })))
    }

    #[cfg(test)]
    pub(crate) fn inject_post_exchange_sync_failure_once(&self) {
        self.storage.inject_post_exchange_sync_failure_once();
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_guard_hook(
        &self,
        entered: std::sync::mpsc::SyncSender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        *self.projection_guard_hook.lock().unwrap() = Some((entered, release));
    }

    fn load_state(storage: &ImmutableLedgerStorage) -> Result<LedgerState, LedgerError> {
        let Some(pointer_bytes) = storage.read_active()? else {
            return Err(LedgerError::Invalid {
                category: "missing_active_pointer",
            });
        };
        let pointer: ActivePointer = serde_json::from_slice(&pointer_bytes)?;
        if serde_json_canonicalizer::to_vec(&pointer).map_err(|_| LedgerError::Invalid {
            category: "jcs_canonicalization_failed",
        })? != pointer_bytes
        {
            return Err(LedgerError::Invalid {
                category: "non_canonical_active_pointer",
            });
        }
        if pointer.schema != ROOT_POINTER_SCHEMA {
            return Err(LedgerError::Invalid {
                category: "unsupported_active_pointer_schema",
            });
        }
        let root: LedgerRoot = read_hashed_object(storage, &pointer.root_hash)?;
        verify_root_hash(&root, &pointer.root_hash)?;
        if root.schema != ROOT_SCHEMA {
            return Err(LedgerError::Invalid {
                category: "invalid_ledger_root",
            });
        }
        validate_root_chain(storage, &pointer.root_hash, &root)?;
        let view: CurrentView = read_hashed_object(storage, &root.current_view_hash)?;
        verify_view_hash(&view, &root.current_view_hash)?;
        let revisions = load_segment_chain(
            storage,
            LogKind::Revision,
            root.revision_head.as_deref(),
            root.revision_watermark,
        )?;
        let policies = load_segment_chain(
            storage,
            LogKind::Policy,
            root.policy_head.as_deref(),
            root.policy_watermark,
        )?;
        let relations = load_segment_chain(
            storage,
            LogKind::Relation,
            root.relation_head.as_deref(),
            root.relation_watermark,
        )?;
        validate_revisions(&revisions)?;
        validate_policies(&policies, &revisions)?;
        validate_relations(&relations, &revisions)?;
        let rebuilt = rebuild_current_view(root.generation, &revisions, &policies, root.relation_watermark)?;
        if view.schema != CURRENT_VIEW_SCHEMA
            || view.generation != root.generation
            || view.revision_watermark != root.revision_watermark
            || view.policy_watermark != root.policy_watermark
            || view.relation_watermark != root.relation_watermark
            || serde_json_canonicalizer::to_vec(&view).map_err(|_| LedgerError::Invalid {
                category: "jcs_canonicalization_failed",
            })? != serde_json_canonicalizer::to_vec(&rebuilt).map_err(|_| LedgerError::Invalid {
                category: "jcs_canonicalization_failed",
            })?
        {
            return Err(LedgerError::Invalid {
                category: "current_view_rebuild_mismatch",
            });
        }
        Ok(LedgerState {
            poisoned: false,
            root_hash: Some(pointer.root_hash),
            root: Some(root),
            view: Some(view),
            revisions,
            policies,
            relations,
        })
    }

    fn ensure_genesis(storage: &ImmutableLedgerStorage) -> Result<(), LedgerError> {
        if storage.read_active()?.is_some() {
            return Ok(());
        }
        let view = CurrentView {
            schema: CURRENT_VIEW_SCHEMA.to_string(),
            generation: 0,
            revision_watermark: 0,
            policy_watermark: 0,
            relation_watermark: 0,
            streams: Vec::new(),
        };
        let (view_bytes, view_hash) = view_bytes_and_hash(&view)?;
        let root = LedgerRoot {
            schema: ROOT_SCHEMA.to_string(),
            generation: 0,
            previous_root_hash: None,
            revision_head: None,
            revision_watermark: 0,
            policy_head: None,
            policy_watermark: 0,
            relation_head: None,
            relation_watermark: 0,
            current_view_hash: view_hash.clone(),
            migration_manifest_hash: None,
            committed_at: 0,
            committed_by_role: crate::PERSONAL_OWNER_ROLE_ID.to_string(),
        };
        let (root_bytes, root_hash) = root_bytes_and_hash(&root)?;
        let allowed = std::collections::HashSet::from([view_hash.clone(), root_hash.clone()]);
        if storage
            .list_immutable_hashes()?
            .into_iter()
            .any(|hash| !allowed.contains(&hash))
        {
            return Err(LedgerError::Invalid {
                category: "missing_active_pointer",
            });
        }
        storage.put_immutable(&view_hash, &view_bytes)?;
        storage.put_immutable(&root_hash, &root_bytes)?;
        let pointer = ActivePointer {
            schema: ROOT_POINTER_SCHEMA.to_string(),
            root_hash,
        };
        storage
            .publish_active(
                &serde_json_canonicalizer::to_vec(&pointer).map_err(|_| LedgerError::Invalid {
                    category: "jcs_canonicalization_failed",
                })?,
            )
            .map_err(|error| match error {
                LedgerStorageError::Io(error) | LedgerStorageError::PublishedButUnsynced(error) => {
                    LedgerError::Io(error)
                }
            })
    }

    fn commit_records(
        &self,
        state: &mut LedgerState,
        role_id: &str,
        tier: MemoryTier,
        mut revisions: Vec<CanonicalRevision>,
        expected_heads: &[ExpectedHead],
    ) -> Result<Vec<LedgerCommit>, LedgerError> {
        if revisions.len() != expected_heads.len() || revisions.is_empty() {
            return Err(LedgerError::Invalid {
                category: "invalid_commit_batch",
            });
        }
        if tier == MemoryTier::Ephemeral {
            return Err(LedgerError::Invalid {
                category: "ephemeral_revision_not_durable",
            });
        }
        if state.poisoned {
            return Err(LedgerError::WriterPoisoned);
        }
        let current_view = state.view.clone().unwrap_or(CurrentView {
            schema: CURRENT_VIEW_SCHEMA.to_string(),
            generation: 0,
            revision_watermark: 0,
            policy_watermark: 0,
            relation_watermark: 0,
            streams: Vec::new(),
        });
        let mut batch_memory_ids = std::collections::HashSet::new();
        for (revision, expected) in revisions.iter().zip(expected_heads) {
            if revision.cognitive_tier != tier
                || (matches!(expected, ExpectedHead::Absent) && revision.committed_by_role != role_id)
            {
                return Err(LedgerError::Invalid {
                    category: "commit_boundary_mismatch",
                });
            }
            if !batch_memory_ids.insert(revision.memory_id.clone()) {
                return Err(LedgerError::Invalid {
                    category: "duplicate_memory_in_batch",
                });
            }
            let actual = head_for(&current_view, &revision.memory_id);
            let matches = match expected {
                ExpectedHead::Absent => actual.is_none() && revision.parent_revision_id.is_none(),
                ExpectedHead::Revision(expected_id) => {
                    actual.as_ref() == Some(expected_id) && revision.parent_revision_id.as_ref() == Some(expected_id)
                }
            };
            if !matches {
                return Err(LedgerError::HeadConflict {
                    memory_id: revision.memory_id.clone(),
                    expected: expected.clone(),
                    actual,
                });
            }
        }

        let first_revision_sequence = state.revisions.len() as u64 + 1;
        let first_policy_sequence = state.policies.len() as u64 + 1;
        let committed_at = crate::util::now_ms();
        let mut policies = Vec::with_capacity(revisions.len());
        for revision in &revisions {
            let actual = head_for(&current_view, &revision.memory_id);
            if actual.is_some() {
                let policy_id = current_view
                    .streams
                    .iter()
                    .find(|stream| stream.memory_id == revision.memory_id)
                    .map(|stream| stream.policy_id.as_str())
                    .ok_or(LedgerError::Invalid {
                        category: "missing_current_policy",
                    })?;
                let policy = state
                    .policies
                    .iter()
                    .find(|policy| policy.policy_id == policy_id)
                    .ok_or(LedgerError::Invalid {
                        category: "missing_current_policy",
                    })?;
                if !policy.writer_roles.iter().any(|writer| writer == role_id) {
                    return Err(LedgerError::UnsupportedPolicy {
                        category: "role_not_policy_writer",
                    });
                }
            }
        }
        for (offset, revision) in revisions.iter_mut().enumerate() {
            revision.schema = REVISION_SCHEMA.to_string();
            revision.sequence = first_revision_sequence + offset as u64;
            revision.committed_at = committed_at;
            revision.committed_by_role = role_id.to_string();
            if revision.deleted_at.is_some() {
                revision.deleted_at = Some(committed_at);
            }
        }
        for (revision, expected) in revisions.iter().zip(expected_heads) {
            if matches!(expected, ExpectedHead::Absent) {
                policies.push(PolicyRecord {
                    schema: POLICY_SCHEMA.to_string(),
                    sequence: first_policy_sequence + policies.len() as u64,
                    policy_id: Uuid::new_v4().to_string(),
                    memory_id: revision.memory_id.clone(),
                    effective_from_revision_id: revision.revision_id.clone(),
                    origin_role_id: role_id.to_string(),
                    mode: PolicyMode::Private,
                    reader_roles: private_roles(role_id),
                    writer_roles: private_roles(role_id),
                    committed_at,
                    committed_by_role: role_id.to_string(),
                });
            }
        }
        let mut candidate_revisions = state.revisions.clone();
        candidate_revisions.extend(revisions.iter().cloned());
        let mut candidate_policies = state.policies.clone();
        candidate_policies.extend(policies.iter().cloned());
        validate_revisions(&candidate_revisions)?;
        validate_policies(&candidate_policies, &candidate_revisions)?;

        let generation = state.root.as_ref().map_or(1, |root| root.generation + 1);
        let revision_segment = Segment {
            schema: REVISION_SEGMENT_SCHEMA.to_string(),
            first_sequence: first_revision_sequence,
            last_sequence: first_revision_sequence + revisions.len() as u64 - 1,
            previous_segment_hash: state.root.as_ref().and_then(|root| root.revision_head.clone()),
            records: revisions.clone(),
        };
        let (revision_bytes, revision_hash) = segment_bytes_and_hash(&revision_segment)?;
        let policy_object = if policies.is_empty() {
            None
        } else {
            let segment = Segment {
                schema: POLICY_SEGMENT_SCHEMA.to_string(),
                first_sequence: first_policy_sequence,
                last_sequence: first_policy_sequence + policies.len() as u64 - 1,
                previous_segment_hash: state.root.as_ref().and_then(|root| root.policy_head.clone()),
                records: policies.clone(),
            };
            let (bytes, hash) = segment_bytes_and_hash(&segment)?;
            Some((segment.last_sequence, bytes, hash))
        };
        let candidate_view = rebuild_current_view(
            generation,
            &candidate_revisions,
            &candidate_policies,
            state.relations.len() as u64,
        )?;
        let (view_bytes, view_hash) = view_bytes_and_hash(&candidate_view)?;
        let candidate_root = LedgerRoot {
            schema: ROOT_SCHEMA.to_string(),
            generation,
            previous_root_hash: state.root_hash.clone(),
            revision_head: Some(revision_hash.clone()),
            revision_watermark: revision_segment.last_sequence,
            policy_head: policy_object
                .as_ref()
                .map(|(_, _, hash)| hash.clone())
                .or_else(|| state.root.as_ref().and_then(|root| root.policy_head.clone())),
            policy_watermark: policy_object
                .as_ref()
                .map_or(state.policies.len() as u64, |(sequence, _, _)| *sequence),
            relation_head: state.root.as_ref().and_then(|root| root.relation_head.clone()),
            relation_watermark: state.root.as_ref().map_or(0, |root| root.relation_watermark),
            current_view_hash: view_hash.clone(),
            migration_manifest_hash: state
                .root
                .as_ref()
                .and_then(|root| root.migration_manifest_hash.clone()),
            committed_at,
            committed_by_role: role_id.to_string(),
        };
        let (root_bytes, root_hash) = root_bytes_and_hash(&candidate_root)?;

        tracing::debug!(
            operation = "memory.ledger_commit",
            role_kind = role_kind(role_id),
            tier = %tier,
            phase = "persist_ledger",
            generation,
            revision_count = revisions.len(),
            canonical_bytes = revisions.iter().map(|revision| serde_json::to_vec(revision).map_or(0, |bytes| bytes.len())).sum::<usize>(),
            hash_verified = true,
            "canonical ledger candidate validated"
        );
        self.storage.put_immutable(&revision_hash, &revision_bytes)?;
        if let Some((_, bytes, hash)) = &policy_object {
            self.storage.put_immutable(hash, bytes)?;
        }
        self.storage.put_immutable(&view_hash, &view_bytes)?;
        self.storage.put_immutable(&root_hash, &root_bytes)?;
        let pointer = ActivePointer {
            schema: ROOT_POINTER_SCHEMA.to_string(),
            root_hash: root_hash.clone(),
        };
        match self
            .storage
            .publish_active(
                &serde_json_canonicalizer::to_vec(&pointer).map_err(|_| LedgerError::Invalid {
                    category: "jcs_canonicalization_failed",
                })?,
            ) {
            Ok(()) => {}
            Err(LedgerStorageError::Io(error)) => return Err(LedgerError::Io(error)),
            Err(LedgerStorageError::PublishedButUnsynced(_)) => {
                state.poisoned = true;
                return Err(LedgerError::CommitIndeterminate);
            }
        }

        let commits = revisions
            .iter()
            .map(|revision| LedgerCommit {
                generation,
                revision_sequence: revision.sequence,
                revision_id: revision.revision_id.clone(),
                committed_at,
            })
            .collect();
        state.root_hash = Some(root_hash);
        state.root = Some(candidate_root);
        state.view = Some(candidate_view);
        state.revisions = candidate_revisions;
        state.policies = candidate_policies;
        tracing::info!(
            operation = "memory.ledger_commit",
            role_kind = role_kind(role_id),
            tier = %tier,
            phase = "publish_current_view",
            generation,
            result_category = "committed",
            "canonical ledger generation published"
        );
        Ok(commits)
    }

    fn entry_to_revision(entry: &MemoryEntry) -> Result<CanonicalRevision, LedgerError> {
        if entry.tenant_id != crate::DEFAULT_TENANT {
            return Err(LedgerError::UnsupportedPolicy {
                category: "non_default_namespace",
            });
        }
        if entry.scope != MemoryScope::Private {
            return Err(LedgerError::UnsupportedPolicy {
                category: "non_private_scope",
            });
        }
        if entry.ttl_ms.is_some() || entry.original_ttl_ms.is_some() {
            return Err(LedgerError::UnsupportedPolicy {
                category: "ttl_policy_not_implemented",
            });
        }
        if entry.causal_parent.is_some() {
            return Err(LedgerError::UnsupportedPolicy {
                category: "relation_log_write_not_implemented",
            });
        }
        if entry.supersedes.is_some() || entry.superseded_by.is_some() {
            return Err(LedgerError::Invalid {
                category: "legacy_supersession_state",
            });
        }
        Ok(CanonicalRevision {
            schema: REVISION_SCHEMA.to_string(),
            sequence: 0,
            memory_id: entry.memory_id.clone(),
            revision_id: entry.id.as_str().into(),
            parent_revision_id: entry.parent_revision_id.clone(),
            content: entry.content.clone(),
            content_hash: entry.canonical_content_hash.clone(),
            tags: entry.tags.clone(),
            memory_type: entry.memory_type,
            cognitive_tier: entry.tier,
            deleted_at: entry.deleted_at,
            committed_at: entry.created_at,
            committed_by_role: entry.agent_id.clone(),
        })
    }
}

impl CanonicalProjectionSource for CASCanonicalLedger {
    fn guarded_projection_document(
        &self,
        expected: &crate::memory::projection::CanonicalSourceIdentity,
    ) -> Result<Option<String>, LedgerError> {
        let state = self.state.lock().map_err(|_| LedgerError::Invalid {
            category: "canonical_ledger_state_poisoned",
        })?;
        if state.poisoned {
            return Err(LedgerError::WriterPoisoned);
        }
        let Some(actual) = current_projection_revision(&state, expected) else {
            return Ok(None);
        };
        match &actual.content {
            MemoryContent::Text(text) if !text.trim().is_empty() => Ok(Some(text.clone())),
            _ => Ok(None),
        }
    }
}

fn current_projection_revision<'a>(
    state: &'a LedgerState,
    expected: &crate::memory::projection::CanonicalSourceIdentity,
) -> Option<&'a CanonicalRevision> {
    let actual = state
        .revisions
        .iter()
        .find(|revision| revision.revision_id == expected.revision_id)?;
    if actual.memory_id != expected.memory_id
        || actual.sequence != expected.revision_sequence
        || actual.content_hash != expected.content_hash
    {
        return None;
    }
    let stream = state
        .view
        .as_ref()?
        .streams
        .iter()
        .find(|stream| stream.memory_id == actual.memory_id)?;
    if stream.deleted
        || stream.head_revision_id != actual.revision_id
        || actual.deleted_at.is_some()
        || !matches!(actual.cognitive_tier, MemoryTier::Working | MemoryTier::LongTerm)
    {
        return None;
    }
    if !state.policies.iter().any(|policy| {
        policy.policy_id == stream.policy_id
            && policy.memory_id == actual.memory_id
            && policy
                .reader_roles
                .iter()
                .any(|role| role == crate::PERSONAL_OWNER_ROLE_ID)
    }) {
        return None;
    }
    match &actual.content {
        MemoryContent::Text(text) if !text.trim().is_empty() => Some(actual),
        _ => None,
    }
}

fn map_namespace_open_error(error: LedgerStorageOpenError) -> LedgerError {
    match error {
        LedgerStorageOpenError::RejectedMarker => LedgerError::Invalid {
            category: "unexpected_namespace_marker_check",
        },
        LedgerStorageOpenError::NamespaceAlreadyClaimed => LedgerError::Invalid {
            category: "memory_ledger_namespace_already_claimed",
        },
        LedgerStorageOpenError::ProjectionResetPending => LedgerError::Invalid {
            category: "projection_reset_pending",
        },
        LedgerStorageOpenError::ProjectionResetMaintenanceRequired => LedgerError::Invalid {
            category: "projection_reset_maintenance_required",
        },
        LedgerStorageOpenError::ProjectionResetIndeterminate => LedgerError::Invalid {
            category: "projection_reset_indeterminate",
        },
        LedgerStorageOpenError::ProjectionResetManualIntervention => LedgerError::Invalid {
            category: "projection_reset_manual_intervention",
        },
        LedgerStorageOpenError::UnsupportedProjectionFormat => LedgerError::Invalid {
            category: "unsupported_projection_format",
        },
        LedgerStorageOpenError::Io(error) => LedgerError::Io(error),
    }
}

impl CanonicalLedger for CASCanonicalLedger {
    fn commit_expected(
        &self,
        role_id: &str,
        tier: MemoryTier,
        expected_head: ExpectedHead,
        revision: CanonicalRevision,
    ) -> Result<LedgerCommit, LedgerError> {
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.commit_records(&mut state, role_id, tier, vec![revision], &[expected_head])?
            .pop()
            .ok_or(LedgerError::Invalid {
                category: "missing_commit_receipt",
            })
    }

    fn commit_roots(
        &self,
        role_id: &str,
        tier: MemoryTier,
        revisions: Vec<CanonicalRevision>,
    ) -> Result<Vec<LedgerCommit>, LedgerError> {
        let expected = vec![ExpectedHead::Absent; revisions.len()];
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.commit_records(&mut state, role_id, tier, revisions, &expected)
    }

    fn rebuild_origin_role(&self, role_id: &str) -> Result<Vec<(MemoryTier, Vec<CanonicalRevision>)>, LedgerError> {
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let origin_memories: std::collections::HashSet<_> = state
            .policies
            .iter()
            .filter(|policy| policy.origin_role_id == role_id)
            .map(|policy| policy.memory_id.clone())
            .collect();
        let mut by_tier: HashMap<MemoryTier, Vec<CanonicalRevision>> = HashMap::new();
        for revision in state
            .revisions
            .iter()
            .filter(|revision| origin_memories.contains(&revision.memory_id))
        {
            by_tier
                .entry(revision.cognitive_tier)
                .or_default()
                .push(revision.clone());
        }
        Ok([MemoryTier::Working, MemoryTier::LongTerm, MemoryTier::Procedural]
            .into_iter()
            .filter_map(|tier| by_tier.remove(&tier).map(|records| (tier, records)))
            .collect())
    }

    fn list_origin_roles(&self) -> Result<Vec<String>, LedgerError> {
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut roles: Vec<_> = state
            .policies
            .iter()
            .map(|policy| policy.origin_role_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        roles.sort();
        Ok(roles)
    }

    fn readable_active_revision_ids(&self, role_id: &str) -> Result<Vec<MemoryRevisionId>, LedgerError> {
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let view = state.view.as_ref().ok_or(LedgerError::Invalid {
            category: "missing_current_view",
        })?;
        let mut revision_ids = view
            .streams
            .iter()
            .filter(|stream| !stream.deleted)
            .filter_map(|stream| {
                state
                    .policies
                    .iter()
                    .find(|policy| policy.policy_id == stream.policy_id)
                    .filter(|policy| policy.reader_roles.iter().any(|reader| reader == role_id))
                    .map(|_| stream.head_revision_id.clone())
            })
            .collect::<Vec<_>>();
        revision_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        Ok(revision_ids)
    }

    fn origin_for_revision(
        &self,
        role_id: &str,
        revision_id: &str,
        write: bool,
    ) -> Result<Option<String>, LedgerError> {
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(revision) = state
            .revisions
            .iter()
            .find(|revision| revision.revision_id.as_str() == revision_id)
        else {
            return Ok(None);
        };
        let Some(stream) = state.view.as_ref().and_then(|view| {
            view.streams
                .iter()
                .find(|stream| stream.memory_id == revision.memory_id)
        }) else {
            return Ok(None);
        };
        let policy = state
            .policies
            .iter()
            .find(|policy| policy.policy_id == stream.policy_id)
            .ok_or(LedgerError::Invalid {
                category: "missing_current_policy",
            })?;
        let roles = if write {
            &policy.writer_roles
        } else {
            &policy.reader_roles
        };
        Ok(roles
            .iter()
            .any(|authorized| authorized == role_id)
            .then(|| policy.origin_role_id.clone()))
    }

    fn flush(&self) -> Result<(), LedgerError> {
        self.storage.flush()?;
        Ok(())
    }
}

impl CanonicalRevision {
    pub(crate) fn from_entry(entry: &MemoryEntry) -> Result<Self, LedgerError> {
        CASCanonicalLedger::entry_to_revision(entry)
    }

    pub(crate) fn into_runtime_entry(self, origin_role: &str) -> MemoryEntry {
        MemoryEntry {
            id: self.revision_id.to_string(),
            memory_id: self.memory_id,
            parent_revision_id: self.parent_revision_id,
            canonical_content_hash: self.content_hash,
            agent_id: origin_role.to_string(),
            tenant_id: crate::DEFAULT_TENANT.to_string(),
            tier: self.cognitive_tier,
            content: self.content,
            importance: 50,
            access_count: 0,
            last_accessed: self.committed_at,
            created_at: self.committed_at,
            tags: self.tags,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: self.memory_type,
            causal_parent: None,
            supersedes: None,
            superseded_by: None,
            deleted_at: self.deleted_at,
        }
    }
}

fn load_segment_chain<T: DeserializeOwned + Serialize + Clone>(
    storage: &ImmutableLedgerStorage,
    expected_kind: LogKind,
    head: Option<&str>,
    watermark: u64,
) -> Result<Vec<T>, LedgerError> {
    if watermark == 0 {
        if head.is_some() {
            return Err(LedgerError::Invalid {
                category: "empty_log_has_segment",
            });
        }
        return Ok(Vec::new());
    }
    let mut segments = Vec::new();
    let mut next = head.map(str::to_string);
    let mut seen = std::collections::HashSet::new();
    while let Some(hash) = next {
        if !seen.insert(hash.clone()) {
            return Err(LedgerError::Invalid {
                category: "segment_cycle",
            });
        }
        let segment: Segment<T> = read_hashed_object(storage, &hash)?;
        verify_segment_hash(&segment, &hash)?;
        if segment.schema != segment_schema(expected_kind) || segment.records.is_empty() {
            return Err(LedgerError::Invalid {
                category: "invalid_segment",
            });
        }
        next = segment.previous_segment_hash.clone();
        segments.push(segment);
    }
    segments.reverse();
    let mut records = Vec::new();
    let mut expected_sequence = 1;
    for segment in segments {
        if segment.first_sequence != expected_sequence
            || segment.last_sequence != segment.first_sequence + segment.records.len() as u64 - 1
        {
            return Err(LedgerError::Invalid {
                category: "non_contiguous_segment_sequence",
            });
        }
        expected_sequence = segment.last_sequence + 1;
        records.extend(segment.records);
    }
    if records.len() as u64 != watermark {
        return Err(LedgerError::Invalid {
            category: "log_head_sequence_mismatch",
        });
    }
    Ok(records)
}

fn validate_relations(records: &[RelationRecord], revisions: &[CanonicalRevision]) -> Result<(), LedgerError> {
    let revision_ids: std::collections::HashSet<_> = revisions.iter().map(|record| &record.revision_id).collect();
    for (offset, record) in records.iter().enumerate() {
        if record.schema != RELATION_SCHEMA
            || record.sequence != offset as u64 + 1
            || uuid::Uuid::parse_str(&record.relation_id).is_err()
            || !revision_ids.contains(&record.subject_revision_id)
            || !revision_ids.contains(&record.object_revision_id)
            || record.subject_revision_id == record.object_revision_id
            || record.predicate != "caused_by"
            || record.epistemic_state != "imported_unverified"
            || record.provenance_manifest_id.trim().is_empty()
        {
            return Err(LedgerError::Invalid {
                category: "invalid_relation_record",
            });
        }
    }
    Ok(())
}

fn validate_root_chain(
    storage: &ImmutableLedgerStorage,
    active_hash: &str,
    active_root: &LedgerRoot,
) -> Result<(), LedgerError> {
    let mut expected_generation = active_root.generation;
    let mut hash = active_hash.to_string();
    let mut root = active_root.clone();
    let mut seen = std::collections::HashSet::new();
    loop {
        if !seen.insert(hash.clone()) || root.schema != ROOT_SCHEMA || root.generation != expected_generation {
            return Err(LedgerError::Invalid {
                category: "invalid_root_chain",
            });
        }
        validate_root_snapshot(storage, &root)?;
        match &root.previous_root_hash {
            Some(previous_hash) => {
                expected_generation = expected_generation.checked_sub(1).ok_or(LedgerError::Invalid {
                    category: "invalid_root_generation",
                })?;
                let previous: LedgerRoot = read_hashed_object(storage, previous_hash)?;
                verify_root_hash(&previous, previous_hash)?;
                if root.migration_manifest_hash.is_some() && previous.migration_manifest_hash.is_none() {
                    validate_migration_boundary(storage, &root)?;
                }
                if root.revision_watermark < previous.revision_watermark
                    || root.policy_watermark < previous.policy_watermark
                    || root.relation_watermark < previous.relation_watermark
                    || !head_extends(
                        storage,
                        LogKind::Revision,
                        root.revision_head.as_deref(),
                        previous.revision_head.as_deref(),
                    )?
                    || !head_extends(
                        storage,
                        LogKind::Policy,
                        root.policy_head.as_deref(),
                        previous.policy_head.as_deref(),
                    )?
                    || !head_extends(
                        storage,
                        LogKind::Relation,
                        root.relation_head.as_deref(),
                        previous.relation_head.as_deref(),
                    )?
                    || previous.migration_manifest_hash.is_some()
                        && root.migration_manifest_hash != previous.migration_manifest_hash
                    || previous.migration_manifest_hash.is_none()
                        && root.migration_manifest_hash.is_some()
                        && previous.generation != 0
                {
                    return Err(LedgerError::Invalid {
                        category: "historical_root_not_prefix",
                    });
                }
                hash = previous_hash.clone();
                root = previous;
            }
            None if expected_generation == 0
                && root.revision_head.is_none()
                && root.revision_watermark == 0
                && root.policy_head.is_none()
                && root.policy_watermark == 0
                && root.relation_head.is_none()
                && root.relation_watermark == 0
                && root.migration_manifest_hash.is_none()
                && root.committed_at == 0
                && root.committed_by_role == crate::PERSONAL_OWNER_ROLE_ID =>
            {
                return Ok(())
            }
            None => {
                return Err(LedgerError::Invalid {
                    category: "truncated_root_chain",
                })
            }
        }
    }
}

fn validate_root_snapshot(storage: &ImmutableLedgerStorage, root: &LedgerRoot) -> Result<(), LedgerError> {
    let view: CurrentView = read_hashed_object(storage, &root.current_view_hash)?;
    verify_view_hash(&view, &root.current_view_hash)?;
    let revisions = load_segment_chain(
        storage,
        LogKind::Revision,
        root.revision_head.as_deref(),
        root.revision_watermark,
    )?;
    let policies = load_segment_chain(
        storage,
        LogKind::Policy,
        root.policy_head.as_deref(),
        root.policy_watermark,
    )?;
    let relations: Vec<RelationRecord> = load_segment_chain(
        storage,
        LogKind::Relation,
        root.relation_head.as_deref(),
        root.relation_watermark,
    )?;
    validate_revisions(&revisions)?;
    validate_policies(&policies, &revisions)?;
    validate_relations(&relations, &revisions)?;
    verify_migration_manifest_objects(storage, root.migration_manifest_hash.as_deref())?;
    let rebuilt = rebuild_current_view(root.generation, &revisions, &policies, root.relation_watermark)?;
    let view_bytes = serde_json_canonicalizer::to_vec(&view).map_err(|_| LedgerError::Invalid {
        category: "jcs_canonicalization_failed",
    })?;
    let rebuilt_bytes = serde_json_canonicalizer::to_vec(&rebuilt).map_err(|_| LedgerError::Invalid {
        category: "jcs_canonicalization_failed",
    })?;
    if view.schema != CURRENT_VIEW_SCHEMA
        || view.generation != root.generation
        || view.revision_watermark != root.revision_watermark
        || view.policy_watermark != root.policy_watermark
        || view.relation_watermark != root.relation_watermark
        || view_bytes != rebuilt_bytes
    {
        return Err(LedgerError::Invalid {
            category: "historical_root_view_mismatch",
        });
    }
    Ok(())
}

fn verify_migration_manifest_objects(
    storage: &ImmutableLedgerStorage,
    manifest_hash: Option<&str>,
) -> Result<(), LedgerError> {
    let Some(manifest_hash) = manifest_hash else {
        return Ok(());
    };
    let manifest: MigrationManifest = read_hashed_object(storage, manifest_hash)?;
    manifest.verify_hash(manifest_hash)?;
    let source: SourceManifest = read_hashed_object(storage, &manifest.source_manifest_hash)?;
    source.verify_hash(&manifest.source_manifest_hash)
}

fn validate_migration_boundary(storage: &ImmutableLedgerStorage, root: &LedgerRoot) -> Result<(), LedgerError> {
    let manifest_hash = root.migration_manifest_hash.as_deref().ok_or(LedgerError::Invalid {
        category: "missing_migration_manifest",
    })?;
    let manifest: MigrationManifest = read_hashed_object(storage, manifest_hash)?;
    manifest.verify_hash(manifest_hash)?;
    let source: SourceManifest = read_hashed_object(storage, &manifest.source_manifest_hash)?;
    source.verify_hash(&manifest.source_manifest_hash)?;
    if manifest.imported_at != root.committed_at || manifest.imported_by_role != root.committed_by_role {
        return Err(LedgerError::Invalid {
            category: "migration_manifest_commit_boundary_mismatch",
        });
    }
    let revisions = load_segment_chain(
        storage,
        LogKind::Revision,
        root.revision_head.as_deref(),
        root.revision_watermark,
    )?;
    let policies = load_segment_chain(
        storage,
        LogKind::Policy,
        root.policy_head.as_deref(),
        root.policy_watermark,
    )?;
    let relations: Vec<RelationRecord> = load_segment_chain(
        storage,
        LogKind::Relation,
        root.relation_head.as_deref(),
        root.relation_watermark,
    )?;
    manifest.validate_target_records(&source, &revisions, &policies, relations.len())
}

fn head_extends(
    storage: &ImmutableLedgerStorage,
    kind: LogKind,
    current: Option<&str>,
    previous: Option<&str>,
) -> Result<bool, LedgerError> {
    let Some(previous) = previous else {
        return Ok(true);
    };
    let mut next = current.map(str::to_string);
    let mut seen = std::collections::HashSet::new();
    while let Some(hash) = next {
        if hash == previous {
            return Ok(true);
        }
        if !seen.insert(hash.clone()) {
            return Ok(false);
        }
        next = verified_previous_segment(storage, kind, &hash)?;
    }
    Ok(false)
}

fn verified_previous_segment(
    storage: &ImmutableLedgerStorage,
    kind: LogKind,
    hash: &str,
) -> Result<Option<String>, LedgerError> {
    macro_rules! read_segment {
        ($record:ty) => {{
            let segment: Segment<$record> = read_hashed_object(storage, hash)?;
            verify_segment_hash(&segment, hash)?;
            if segment.schema != segment_schema(kind) || segment.records.is_empty() {
                return Err(LedgerError::Invalid {
                    category: "invalid_segment",
                });
            }
            segment.previous_segment_hash
        }};
    }
    Ok(match kind {
        LogKind::Revision => read_segment!(CanonicalRevision),
        LogKind::Policy => read_segment!(PolicyRecord),
        LogKind::Relation => read_segment!(RelationRecord),
    })
}

fn read_hashed_object<T: DeserializeOwned + Serialize>(
    storage: &ImmutableLedgerStorage,
    hash: &str,
) -> Result<T, LedgerError> {
    let bytes = storage.get_immutable(hash)?;
    let value: T = serde_json::from_slice(&bytes)?;
    let canonical = serde_json_canonicalizer::to_vec(&value).map_err(|_| LedgerError::Invalid {
        category: "jcs_canonicalization_failed",
    })?;
    if bytes != canonical {
        return Err(LedgerError::Invalid {
            category: "non_canonical_object_bytes",
        });
    }
    Ok(value)
}

fn segment_schema(kind: LogKind) -> &'static str {
    match kind {
        LogKind::Revision => REVISION_SEGMENT_SCHEMA,
        LogKind::Policy => POLICY_SEGMENT_SCHEMA,
        LogKind::Relation => RELATION_SEGMENT_SCHEMA,
    }
}

fn private_roles(role_id: &str) -> Vec<String> {
    let mut roles = vec![role_id.to_string()];
    if role_id != LOCAL_OWNER_ROLE {
        roles.push(LOCAL_OWNER_ROLE.to_string());
        roles.sort();
    }
    roles
}

fn role_kind(role_id: &str) -> &'static str {
    if role_id == crate::PERSONAL_OWNER_ROLE_ID {
        "personal_owner"
    } else {
        "authenticated_role"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryContent, MemoryEntry};

    fn open(root: &std::path::Path) -> CASCanonicalLedger {
        let vault = Arc::new(PersonalVaultStorage::open(root, Some(LEGACY_INDEX)).unwrap());
        CASCanonicalLedger::new(vault).unwrap()
    }

    fn root_entry(role: &str, text: &str) -> MemoryEntry {
        let mut entry = MemoryEntry::ephemeral(role, text);
        entry.tier = MemoryTier::Working;
        entry
    }

    fn child_entry(parent: &MemoryEntry, role: &str, text: &str, deleted: bool) -> MemoryEntry {
        let content = MemoryContent::Text(text.to_string());
        let now = crate::util::now_ms();
        MemoryEntry {
            id: Uuid::new_v4().to_string(),
            memory_id: parent.memory_id.clone(),
            parent_revision_id: Some(parent.id.as_str().into()),
            canonical_content_hash: content.canonical_content_hash().unwrap(),
            agent_id: role.to_string(),
            tenant_id: crate::DEFAULT_TENANT.to_string(),
            tier: MemoryTier::Working,
            content,
            importance: 50,
            access_count: 0,
            last_accessed: now,
            created_at: now,
            tags: parent.tags.clone(),
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: parent.memory_type,
            causal_parent: None,
            supersedes: None,
            superseded_by: None,
            deleted_at: deleted.then_some(now),
        }
    }

    #[test]
    fn create_update_delete_restart_preserves_immutable_records() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = open(directory.path());
        let root = root_entry("role-a", "one");
        let root_bytes = serde_json_canonicalizer::to_vec(&CanonicalRevision::from_entry(&root).unwrap()).unwrap();
        ledger
            .commit_expected(
                "role-a",
                MemoryTier::Working,
                ExpectedHead::Absent,
                CanonicalRevision::from_entry(&root).unwrap(),
            )
            .unwrap();
        let update = child_entry(&root, "role-a", "two", false);
        ledger
            .commit_expected(
                "role-a",
                MemoryTier::Working,
                ExpectedHead::Revision(root.id.as_str().into()),
                CanonicalRevision::from_entry(&update).unwrap(),
            )
            .unwrap();
        let tombstone = child_entry(&update, "role-a", "two", true);
        let tombstone_commit = ledger
            .commit_expected(
                "role-a",
                MemoryTier::Working,
                ExpectedHead::Revision(update.id.as_str().into()),
                CanonicalRevision::from_entry(&tombstone).unwrap(),
            )
            .unwrap();
        let rebuilt = ledger.rebuild_origin_role("role-a").unwrap();
        assert_eq!(rebuilt[0].1.len(), 3);
        assert_eq!(
            rebuilt[0].1.last().unwrap().deleted_at,
            Some(tombstone_commit.committed_at)
        );
        assert_eq!(serde_json_canonicalizer::to_vec(&rebuilt[0].1[0]).unwrap(), {
            let mut parsed: CanonicalRevision = serde_json::from_slice(&root_bytes).unwrap();
            parsed.schema = REVISION_SCHEMA.to_string();
            parsed.sequence = 1;
            parsed.committed_at = rebuilt[0].1[0].committed_at;
            serde_json_canonicalizer::to_vec(&parsed).unwrap()
        });
        drop(ledger);
        let restarted = open(directory.path());
        assert_eq!(restarted.rebuild_origin_role("role-a").unwrap()[0].1.len(), 3);
    }

    #[test]
    fn stale_head_is_a_typed_conflict() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = open(directory.path());
        let root = root_entry("role-a", "one");
        ledger
            .commit_expected(
                "role-a",
                MemoryTier::Working,
                ExpectedHead::Absent,
                CanonicalRevision::from_entry(&root).unwrap(),
            )
            .unwrap();
        let first = child_entry(&root, "role-a", "two", false);
        ledger
            .commit_expected(
                "role-a",
                MemoryTier::Working,
                ExpectedHead::Revision(root.id.as_str().into()),
                CanonicalRevision::from_entry(&first).unwrap(),
            )
            .unwrap();
        let stale = child_entry(&root, "role-a", "stale", false);
        assert!(matches!(
            ledger.commit_expected(
                "role-a",
                MemoryTier::Working,
                ExpectedHead::Revision(root.id.as_str().into()),
                CanonicalRevision::from_entry(&stale).unwrap(),
            ),
            Err(LedgerError::HeadConflict { .. })
        ));
    }

    #[test]
    fn second_runtime_cannot_open_the_same_vault() {
        let directory = tempfile::tempdir().unwrap();
        let first = open(directory.path());
        assert!(matches!(
            PersonalVaultStorage::open(directory.path(), Some(LEGACY_INDEX)),
            Err(LedgerStorageOpenError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
        drop(first);
        open(directory.path());
    }

    #[test]
    fn legacy_snapshot_is_always_rejected() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(LEGACY_INDEX), b"{}").unwrap();
        assert!(matches!(
            PersonalVaultStorage::open(directory.path(), Some(LEGACY_INDEX)),
            Err(LedgerStorageOpenError::RejectedMarker)
        ));
    }

    #[test]
    fn empty_genesis_restart_and_exact_orphan_recovery_are_safe() {
        let directory = tempfile::tempdir().unwrap();
        drop(open(directory.path()));
        drop(open(directory.path()));
        std::fs::write(directory.path().join(LEDGER_DIRECTORY).join("roots").join("active"), []).unwrap();
        let recovered = open(directory.path());
        assert!(recovered.list_origin_roles().unwrap().is_empty());
    }

    #[test]
    fn noncanonical_active_pointer_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        drop(open(directory.path()));
        let active = directory.path().join(LEDGER_DIRECTORY).join("roots").join("active");
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&active).unwrap()).unwrap();
        std::fs::write(&active, serde_json::to_string_pretty(&value).unwrap()).unwrap();
        let vault = Arc::new(PersonalVaultStorage::open(directory.path(), Some(LEGACY_INDEX)).unwrap());
        assert!(matches!(
            CASCanonicalLedger::new(vault),
            Err(LedgerError::Invalid {
                category: "non_canonical_active_pointer"
            })
        ));
    }

    #[test]
    fn ephemeral_commit_is_rejected_and_commit_time_is_writer_stamped() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = open(directory.path());
        let mut entry = root_entry("role-a", "one");
        entry.created_at = 1;
        entry.last_accessed = 1;
        let mut ephemeral = CanonicalRevision::from_entry(&entry).unwrap();
        ephemeral.cognitive_tier = MemoryTier::Ephemeral;
        assert!(matches!(
            ledger.commit_expected("role-a", MemoryTier::Ephemeral, ExpectedHead::Absent, ephemeral),
            Err(LedgerError::Invalid {
                category: "ephemeral_revision_not_durable"
            })
        ));
        let receipt = ledger
            .commit_expected(
                "role-a",
                MemoryTier::Working,
                ExpectedHead::Absent,
                CanonicalRevision::from_entry(&entry).unwrap(),
            )
            .unwrap();
        assert_ne!(receipt.committed_at, 1);
        assert_eq!(
            ledger.rebuild_origin_role("role-a").unwrap()[0].1[0].committed_at,
            receipt.committed_at
        );
    }
}
