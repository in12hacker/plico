//! Immutable canonical revision ledger.

mod current_view;
mod hash;
mod migration_manifest;
mod model;
mod store;
mod validate;

pub use model::{
    CanonicalRevision, CurrentView, ExpectedHead, LedgerCommit, LedgerError, LedgerRoot, PolicyMode, PolicyRecord,
};
pub(crate) use store::{
    AuthorizedCurrentRevisionProof, AuthorizedOwnerProjectionProof, CanonicalProjectionGuard,
    CanonicalProjectionSnapshot,
};
pub(crate) use store::{CASCanonicalLedger, CanonicalLedger, CanonicalProjectionSource};

pub use migration_manifest::{
    GroupMapping, LegacyCreatedAtMapping, ManifestSourceScope, ManifestTargetMode, MigrationDispositionCounters,
    MigrationManifest, PolicyMapping, RoleMapping, SourceManifest, SourceSnapshot, StreamMapping,
};
#[cfg(feature = "offline-migration")]
pub use store::{build_offline_migration_target, OfflineMigrationTargetInput};

#[cfg(feature = "offline-migration")]
pub use validate::validate_migration_record_sets;

#[cfg(feature = "offline-migration")]
pub use model::{MigrationPolicyInput, MigrationRevisionInput};

#[cfg(feature = "offline-migration")]
pub use migration_manifest::{MigrationManifestInput, SourceManifestInput};

#[cfg(feature = "offline-migration")]
pub fn deterministic_migration_revision_id(source_manifest_hash: &str, legacy_id: &str, kind: &str) -> String {
    let namespace = uuid::Uuid::NAMESPACE_OID;
    uuid::Uuid::new_v5(
        &namespace,
        format!("plico-memory-migration:{source_manifest_hash}:{legacy_id}:{kind}").as_bytes(),
    )
    .to_string()
}

#[cfg(feature = "offline-migration")]
pub fn deterministic_migration_policy_id(source_manifest_hash: &str, memory_id: &str, sequence: u64) -> String {
    let namespace = uuid::Uuid::NAMESPACE_OID;
    uuid::Uuid::new_v5(
        &namespace,
        format!("plico-memory-policy:{source_manifest_hash}:{memory_id}:{sequence}").as_bytes(),
    )
    .to_string()
}
