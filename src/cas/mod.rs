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

pub mod object;
pub mod storage;

pub use object::{AIObject, AIObjectMeta, ContentType};
pub use storage::{CASStorage, CASError};
