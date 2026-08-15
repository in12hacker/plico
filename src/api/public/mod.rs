//! Stable public protocol for a single person's local digital twin.
//!
//! This module owns the external wire schema. Internal kernel commands do not
//! appear here, and public inputs never accept a self-asserted role or storage
//! namespace.

mod input;
mod output;

pub use input::*;
pub use output::*;

pub const PERSONAL_PROTOCOL: &str = "plico.personal.v2";

pub const PUBLIC_OPERATIONS: [&str; 14] = [
    "capabilities.describe",
    "runtime.readiness",
    "object.put",
    "object.get",
    "object.search",
    "memory.create",
    "memory.get",
    "memory.recall",
    "projection.status",
    "projection.rebuild",
    "memory.update",
    "memory.delete",
    "session.start",
    "session.end",
];

pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_OBJECT_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_QUERY_BYTES: usize = 8 * 1024;
pub const MAX_AUTH_BYTES: usize = 4 * 1024;
pub const MAX_TAGS: usize = 32;
pub const MAX_TAG_BYTES: usize = 64;
pub const DEFAULT_LIMIT: usize = 20;
pub const MAX_LIMIT: usize = 100;

fn validate_non_empty_bounded(value: &str, maximum: usize, field: &str) -> Result<(), ValidationError> {
    let size = value.len();
    if size == 0 || size > maximum {
        return Err(ValidationError::new(format!(
            "{field} must contain 1..={maximum} UTF-8 bytes"
        )));
    }
    Ok(())
}

fn validate_tags(tags: &[String]) -> Result<(), ValidationError> {
    if tags.len() > MAX_TAGS {
        return Err(ValidationError::new(format!(
            "tags must contain at most {MAX_TAGS} items"
        )));
    }
    for tag in tags {
        validate_non_empty_bounded(tag, MAX_TAG_BYTES, "tag")?;
    }
    Ok(())
}

fn validate_limit(limit: usize) -> Result<(), ValidationError> {
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(ValidationError::new(format!("limit must be in 1..={MAX_LIMIT}")));
    }
    Ok(())
}

fn validate_uuid(value: uuid::Uuid, field: &str) -> Result<(), ValidationError> {
    if value.is_nil() {
        return Err(ValidationError::new(format!("{field} must not be a nil UUID")));
    }
    Ok(())
}

fn validate_cid(value: &str) -> Result<(), ValidationError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ValidationError::new(
            "cid must be a 64-character lowercase SHA-256 hex value",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
