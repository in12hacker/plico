//! Content-Addressed Storage (CAS)
//!
//! Core principle: **content address = SHA-256 hash**. The file's address IS its content fingerprint.
//! This guarantees:
//! - Automatic deduplication (same content = same address)
//! - Immutability by default (content cannot be silently modified)
//! - Content integrity verification on every read
//!
//! # Module Structure
//!
//! - [`object`] — AIObject and AIObjectMeta definitions
//! - [`storage`] — CAS storage engine

pub(crate) mod execution_observation_store;
pub(crate) mod ledger_store;
pub mod object;
#[cfg(feature = "offline-migration")]
pub mod offline_migration;
pub(crate) mod projection_store;
pub mod storage;

pub(crate) use ledger_store::{
    ExistingProjectionReadOnly, ImmutableLedgerNamespace, ImmutableLedgerStorage, LedgerStorageError,
    LedgerStorageOpenError, PersonalVaultStorage,
};
pub use object::{AIObject, AIObjectMeta, ContentType, ObjectScope};
#[cfg(feature = "offline-migration")]
pub use offline_migration::{
    OfflineMigrationError, OfflineMigrationPublication, OfflineMigrationTarget, OfflineMigrationVault,
    OfflineReferencedObjectFingerprint, OfflineSnapshotFingerprint, OfflineSourceFingerprint,
};
pub(crate) use projection_store::{
    ProjectionClaimedLiveInspection, ProjectionPairGenesisEvidence, ProjectionPairPublishMode,
    ProjectionPairResetReason, ProjectionStorageBundle,
};
pub use storage::{CASError, CASStorage};
