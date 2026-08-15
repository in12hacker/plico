//! Durable control plane for rebuildable memory embedding projections.

mod coordinator_core;
mod current_view;
mod hash;
mod model;
mod store;
mod validate;

pub(crate) use coordinator_core::{
    ProjectionCoordinatorCore, ProjectionCoreClaim, ProjectionCoreInspection, ProjectionCoreOpenError,
    ProjectionCoreUnavailable, ProjectionCutoverReceipt, ProjectionDurableReceipt, ProjectionRebuildError,
    ProjectionRebuildSelector, ProjectionRecoveredGenesis, ProjectionStatusObservation, ProjectionStatusState,
};
pub(crate) use hash::builder_spec_bytes_and_hash;
#[cfg(test)]
pub(crate) use model::ProjectionState;
pub(crate) use model::{
    AbsentReason, BuilderSpec, CanonicalSourceIdentity, CanonicalWatermark, EmbeddingInputContract,
    EmbeddingNormalization, EmbeddingOperationContract, FailureCategory, ProjectionError, ProjectionKind, QueueReason,
    StaleReason, BUILDER_SPEC_SCHEMA, EMBEDDING_ARTIFACT_SCHEMA,
};

#[cfg(test)]
mod tests;
