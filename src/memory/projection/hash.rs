use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::model::ProjectionError;
use crate::memory::MemoryRevisionId;

const SEGMENT_DOMAIN: &[u8] = b"plico.projection.manifest-segment.v1\0";
const ROOT_DOMAIN: &[u8] = b"plico.projection.manifest-root.v1\0";
const VIEW_DOMAIN: &[u8] = b"plico.projection.current-view.v1\0";
const BUILDER_SPEC_DOMAIN: &[u8] = b"plico.projection.builder-spec.v1\0";
const ARTIFACT_DOMAIN: &[u8] = b"plico.projection.embedding-artifact.v1\0";

pub fn projection_id(revision_id: &MemoryRevisionId) -> Result<Uuid, ProjectionError> {
    let revision = Uuid::parse_str(revision_id.as_str()).map_err(|_| ProjectionError::Invalid {
        category: "invalid_projection_revision_id",
    })?;
    if revision.is_nil() || revision.hyphenated().to_string() != revision_id.as_str() {
        return Err(ProjectionError::Invalid {
            category: "non_canonical_projection_revision_id",
        });
    }
    let namespace = Uuid::new_v5(&Uuid::NAMESPACE_URL, b"https://plico.ai/schema/projection/v1");
    let mut name = b"memory_embedding\0".to_vec();
    name.extend_from_slice(revision.hyphenated().to_string().as_bytes());
    Ok(Uuid::new_v5(&namespace, &name))
}

pub fn builder_spec_bytes_and_hash<T: Serialize>(value: &T) -> Result<(Vec<u8>, String), ProjectionError> {
    canonical_bytes_and_hash(BUILDER_SPEC_DOMAIN, value)
}

pub fn segment_bytes_and_hash<T: Serialize>(value: &T) -> Result<(Vec<u8>, String), ProjectionError> {
    canonical_bytes_and_hash(SEGMENT_DOMAIN, value)
}

pub fn root_bytes_and_hash<T: Serialize>(value: &T) -> Result<(Vec<u8>, String), ProjectionError> {
    canonical_bytes_and_hash(ROOT_DOMAIN, value)
}

pub fn view_bytes_and_hash<T: Serialize>(value: &T) -> Result<(Vec<u8>, String), ProjectionError> {
    canonical_bytes_and_hash(VIEW_DOMAIN, value)
}

pub fn artifact_bytes_and_hash<T: Serialize>(value: &T) -> Result<(Vec<u8>, String), ProjectionError> {
    canonical_bytes_and_hash(ARTIFACT_DOMAIN, value)
}

pub fn pointer_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ProjectionError> {
    canonical_bytes(value)
}

pub fn parse_canonical_with_schema<T: DeserializeOwned + Serialize>(
    bytes: &[u8],
    expected_schema: &str,
    component: &'static str,
    validate_nested_schemas: impl FnOnce(&serde_json::Value) -> Result<(), ProjectionError>,
) -> Result<T, ProjectionError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    require_schema(&value, expected_schema, component)?;
    if canonical_bytes(&value)? != bytes {
        return Err(ProjectionError::Invalid {
            category: "non_canonical_projection_json",
        });
    }
    validate_nested_schemas(&value)?;
    serde_json::from_value(value).map_err(ProjectionError::Serialization)
}

pub fn require_schema(
    value: &serde_json::Value,
    expected_schema: &str,
    component: &'static str,
) -> Result<(), ProjectionError> {
    let Some(schema) = value.get("schema").and_then(serde_json::Value::as_str) else {
        return Err(ProjectionError::Invalid {
            category: "missing_projection_schema",
        });
    };
    if schema == expected_schema {
        Ok(())
    } else {
        Err(ProjectionError::UnsupportedFormat { component })
    }
}

pub fn verify_hash<T: Serialize>(value: &T, expected: &str, kind: HashKind) -> Result<(), ProjectionError> {
    validate_hash(expected)?;
    let actual = match kind {
        HashKind::Segment => segment_bytes_and_hash(value)?.1,
        HashKind::Root => root_bytes_and_hash(value)?.1,
        HashKind::View => view_bytes_and_hash(value)?.1,
    };
    if actual == expected {
        Ok(())
    } else {
        Err(ProjectionError::Invalid {
            category: "projection_hash_mismatch",
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum HashKind {
    Segment,
    Root,
    View,
}

pub fn validate_hash(value: &str) -> Result<(), ProjectionError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(ProjectionError::Invalid {
            category: "invalid_projection_hash",
        })
    }
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ProjectionError> {
    serde_json_canonicalizer::to_vec(value).map_err(|_| ProjectionError::Invalid {
        category: "projection_jcs_canonicalization_failed",
    })
}

fn canonical_bytes_and_hash<T: Serialize>(domain: &[u8], value: &T) -> Result<(Vec<u8>, String), ProjectionError> {
    let bytes = canonical_bytes(value)?;
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(&bytes);
    Ok((bytes, format!("{:x}", digest.finalize())))
}
