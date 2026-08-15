use uuid::Uuid;

use super::hash::{artifact_bytes_and_hash, builder_spec_bytes_and_hash, projection_id, validate_hash};
use super::model::{
    ArtifactDescriptor, BuilderSpec, CanonicalSourceIdentity, CanonicalWatermark, EmbeddingArtifact,
    EmbeddingInputContract, EmbeddingNormalization, EmbeddingOperationContract, ProjectionError, ProjectionKind,
    ProjectionState, BUILDER_SPEC_SCHEMA, EMBEDDING_ARTIFACT_SCHEMA, MAX_EMBEDDING_ARTIFACT_BYTES,
    MAX_EMBEDDING_DIMENSION,
};
use crate::memory::CanonicalProjectionSnapshot;

const MAX_JCS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub(super) fn validate_watermark(value: &CanonicalWatermark) -> Result<(), ProjectionError> {
    validate_hash(&value.root_hash)?;
    if [
        value.generation,
        value.revision_watermark,
        value.policy_watermark,
        value.relation_watermark,
    ]
    .into_iter()
    .any(|number| number > MAX_JCS_SAFE_INTEGER)
        || value.revision_watermark == 0 && (value.policy_watermark != 0 || value.relation_watermark != 0)
    {
        return Err(invalid("invalid_canonical_watermark"));
    }
    Ok(())
}

pub(super) fn validate_builder_spec(value: &BuilderSpec) -> Result<(), ProjectionError> {
    if value.schema != BUILDER_SPEC_SCHEMA
        || value.projection_kind != ProjectionKind::MemoryEmbedding
        || !safe_identifier(&value.builder_id)
        || !safe_identifier(&value.builder_version)
        || !safe_identifier(&value.provider_family)
        || !safe_identifier(&value.provider_compatibility_id)
        || !safe_identifier(&value.model_id)
        || value.raw_dimension == 0
        || value.raw_dimension > MAX_EMBEDDING_DIMENSION
        || value.dimension == 0
        || value.dimension > MAX_EMBEDDING_DIMENSION
        || value.dimension > value.raw_dimension
        || value.input_contract != EmbeddingInputContract::MemoryTextUtf8V1
        || value.operation_contract != EmbeddingOperationContract::DocumentV1
        || !valid_transform(value)
        || value.artifact_schema != EMBEDDING_ARTIFACT_SCHEMA
    {
        return Err(invalid("invalid_builder_spec"));
    }
    Ok(())
}

fn valid_transform(value: &BuilderSpec) -> bool {
    match value.normalization {
        EmbeddingNormalization::ProviderNative => {
            value.raw_dimension == value.dimension && value.transform_contract_id == "provider-native-document-v1"
        }
        EmbeddingNormalization::L2AfterMatryoshkaTruncationV1 => {
            value.raw_dimension > value.dimension && value.transform_contract_id == "plico-matryoshka-truncate-l2-v1"
        }
    }
}

pub(super) fn validate_builder_hash(value: &BuilderSpec, expected: &str) -> Result<(), ProjectionError> {
    validate_builder_spec(value)?;
    validate_hash(expected)?;
    if builder_spec_bytes_and_hash(value)?.1 != expected {
        return Err(invalid("builder_spec_hash_mismatch"));
    }
    Ok(())
}

pub(super) fn validate_source(value: &CanonicalSourceIdentity) -> Result<(), ProjectionError> {
    if value.canonical_kind != "memory_revision"
        || value.memory_id.is_empty()
        || value.revision_id.is_empty()
        || value.revision_sequence == 0
        || value.revision_sequence > MAX_JCS_SAFE_INTEGER
    {
        return Err(invalid("invalid_projection_source"));
    }
    let memory_id = canonical_uuid(value.memory_id.as_str())?;
    let revision_id = canonical_uuid(value.revision_id.as_str())?;
    if memory_id.is_nil() || revision_id.is_nil() || projection_id(&value.revision_id)?.is_nil() {
        return Err(invalid("invalid_projection_source"));
    }
    validate_hash(value.content_hash.as_str())
}

pub(super) fn validate_artifact(
    artifact: &EmbeddingArtifact,
    descriptor: &ArtifactDescriptor,
    builder: &BuilderSpec,
) -> Result<Vec<u8>, ProjectionError> {
    validate_builder_hash(builder, &artifact.builder_spec_hash)?;
    if artifact.schema != EMBEDDING_ARTIFACT_SCHEMA
        || artifact.encoding != "f32-json/v1"
        || artifact.dimension == 0
        || descriptor.artifact_schema != EMBEDDING_ARTIFACT_SCHEMA
        || descriptor.dimension != artifact.dimension
        || descriptor.source_revision_id != artifact.source_revision_id
        || descriptor.source_content_hash != artifact.source_content_hash
        || descriptor.builder_spec_hash != artifact.builder_spec_hash
        || artifact.builder_spec_hash != builder_spec_bytes_and_hash(builder)?.1
        || descriptor.byte_length == 0
        || artifact.projection_id != projection_id(&artifact.source_revision_id)?
        || artifact.dimension != builder.dimension
    {
        return Err(invalid("invalid_embedding_artifact"));
    }
    validate_embedding_output(builder, &artifact.vector)?;
    validate_hash(&artifact.builder_spec_hash)?;
    let (bytes, hash) = artifact_bytes_and_hash(artifact)?;
    if descriptor.artifact_hash != hash || descriptor.byte_length != bytes.len() as u64 {
        return Err(invalid("artifact_descriptor_mismatch"));
    }
    Ok(bytes)
}

pub(super) fn validate_embedding_output(builder: &BuilderSpec, vector: &[f32]) -> Result<(), ProjectionError> {
    if vector.len() != builder.dimension as usize
        || vector.iter().any(|component| !component.is_finite())
        || vector.iter().all(|component| *component == 0.0)
    {
        return Err(invalid("invalid_embedding_output"));
    }
    if builder.normalization == EmbeddingNormalization::L2AfterMatryoshkaTruncationV1 {
        let norm = vector
            .iter()
            .map(|component| f64::from(*component).powi(2))
            .sum::<f64>()
            .sqrt();
        if !norm.is_finite() || (norm - 1.0).abs() > 1e-4 {
            return Err(invalid("artifact_normalization_mismatch"));
        }
    }
    Ok(())
}

pub(super) fn validate_state_shape(state: &ProjectionState, committed_at: u64) -> Result<(), ProjectionError> {
    match state {
        ProjectionState::Building {
            attempt,
            attempt_id,
            lease_expires_at,
        } => {
            if *attempt == 0
                || attempt_id.is_nil()
                || *lease_expires_at <= committed_at
                || *lease_expires_at > MAX_JCS_SAFE_INTEGER
            {
                return Err(invalid("invalid_building_state"));
            }
        }
        ProjectionState::Ready {
            attempt,
            attempt_id,
            artifact,
        } => {
            if *attempt == 0 || attempt_id.is_nil() {
                return Err(invalid("invalid_ready_state"));
            }
            validate_descriptor(artifact)?;
        }
        ProjectionState::Failed {
            attempt,
            attempt_id,
            retryable,
            retry_not_before,
            ..
        } => {
            if *attempt == 0
                || attempt_id.is_nil()
                || (*retryable && retry_not_before.is_none_or(|retry_at| retry_at <= committed_at))
                || retry_not_before.is_some_and(|retry_at| retry_at > MAX_JCS_SAFE_INTEGER)
                || (!*retryable && retry_not_before.is_some())
            {
                return Err(invalid("invalid_failed_state"));
            }
        }
        ProjectionState::Stale { artifact, .. } => validate_descriptor(artifact)?,
        ProjectionState::AbsentByPolicy { .. } | ProjectionState::Queued { .. } => {}
    }
    Ok(())
}

pub(super) fn validate_source_against_canonical(
    source: &CanonicalSourceIdentity,
    snapshot: &CanonicalProjectionSnapshot,
) -> Result<(), ProjectionError> {
    let Some(revision) = snapshot
        .revisions
        .iter()
        .find(|revision| revision.revision_id == source.revision_id)
    else {
        return Err(invalid("projection_source_not_canonical"));
    };
    if revision.memory_id != source.memory_id
        || revision.sequence != source.revision_sequence
        || revision.content_hash != source.content_hash
    {
        return Err(invalid("projection_source_identity_mismatch"));
    }
    Ok(())
}

pub(super) fn validate_reconciled_coverage(
    watermark: &CanonicalWatermark,
    snapshot: &CanonicalProjectionSnapshot,
) -> Result<(), ProjectionError> {
    validate_watermark(watermark)?;
    if watermark.root_hash != snapshot.root_hash
        || watermark.generation != snapshot.root.generation
        || watermark.revision_watermark != snapshot.root.revision_watermark
        || watermark.policy_watermark != snapshot.root.policy_watermark
        || watermark.relation_watermark != snapshot.root.relation_watermark
    {
        return Err(invalid("canonical_watermark_mismatch"));
    }
    Ok(())
}

fn validate_descriptor(value: &ArtifactDescriptor) -> Result<(), ProjectionError> {
    validate_hash(&value.artifact_hash)?;
    validate_hash(value.source_content_hash.as_str())?;
    validate_hash(&value.builder_spec_hash)?;
    if value.byte_length == 0
        || value.byte_length > MAX_EMBEDDING_ARTIFACT_BYTES
        || value.byte_length > MAX_JCS_SAFE_INTEGER
        || value.artifact_schema != EMBEDDING_ARTIFACT_SCHEMA
        || value.dimension == 0
        || value.dimension > MAX_EMBEDDING_DIMENSION
        || Uuid::parse_str(value.source_revision_id.as_str()).is_err()
    {
        return Err(invalid("invalid_artifact_descriptor"));
    }
    Ok(())
}

fn canonical_uuid(value: &str) -> Result<Uuid, ProjectionError> {
    let parsed = Uuid::parse_str(value).map_err(|_| invalid("invalid_projection_uuid"))?;
    if parsed.hyphenated().to_string() != value {
        return Err(invalid("non_canonical_projection_uuid"));
    }
    Ok(parsed)
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.contains("://")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'"' | b'\'' | b'\\'))
}

pub(super) const fn invalid(category: &'static str) -> ProjectionError {
    ProjectionError::Invalid { category }
}
