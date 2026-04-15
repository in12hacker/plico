//! CAS Storage Engine
//!
//! Stores [`AIObject`] on disk using a content-addressed layout.
//!
//! # Layout Strategy
//!
//! Objects are sharded by CID prefix to avoid filesystem directory limits:
//! ```text
//! root/
//! ├── 00/  01/  02/  ... ff/   (256 top-level shard directories)
//! │   └── <remaining CID>.json  (object serialized as JSON)
//! ```
//!
//! # Design Decisions
//!
//! - JSON serialization: human-readable for debugging; machine-parseable.
//!   A future version may switch to binary CBOR for performance.
//! - CID prefix sharding: prevents >10k files per directory (filesystem limit).
//! - Atomic writes: write to temp file, then rename to prevent partial writes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cas::object::AIObject;

#[cfg(test)]
use crate::cas::AIObjectMeta;

#[derive(Debug)]
pub struct CASStorage {
    root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum CASError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Object not found: CID={cid}")]
    NotFound { cid: String },

    #[error("Integrity check failed for CID={cid}")]
    IntegrityFailed { cid: String },
}

impl CASStorage {
    /// Open or create a CAS storage at `root_path`.
    ///
    /// # Example
    ///
    /// ```
    /// use plico::{CASStorage, AIObject, AIObjectMeta};
    /// let dir = std::env::temp_dir().join("plico_doctest");
    /// let storage = CASStorage::new(dir.clone()).unwrap();
    /// let obj = AIObject::new(b"data".to_vec(), AIObjectMeta::text(["tag"]));
    /// let cid = storage.put(&obj).unwrap();
    /// let retrieved = storage.get(&cid).unwrap();
    /// std::fs::remove_dir_all(dir).ok();
    /// ```
    pub fn new(root_path: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&root_path)?;
        Ok(CASStorage { root: root_path })
    }

    /// Store an object. Returns the CID.
    ///
    /// If an object with the same CID already exists, this is a no-op (idempotent).
    pub fn put(&self, obj: &AIObject) -> io::Result<String> {
        // Verify integrity before storing
        if !obj.verify_integrity() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Object integrity check failed for CID={}", obj.cid),
            ));
        }

        let shard_dir = self.shard_dir(&obj.cid);
        let obj_path = self.object_path(&obj.cid);

        // If already exists (deduplication), return early
        if obj_path.exists() {
            return Ok(obj.cid.clone());
        }

        // Atomic write: temp file → rename
        fs::create_dir_all(&shard_dir)?;
        let temp_path = shard_dir.join(format!(".tmp_{}", obj.cid));
        let json = serde_json::to_vec(obj)?;
        fs::write(&temp_path, json)?;
        fs::rename(&temp_path, &obj_path)?;

        Ok(obj.cid.clone())
    }

    /// Retrieve an object by CID.
    ///
    /// # Errors
    ///
    /// - `CASError::NotFound` if CID does not exist
    /// - `CASError::IntegrityFailed` if stored content doesn't match CID
    pub fn get(&self, cid: &str) -> Result<AIObject, CASError> {
        let obj_path = self.object_path(cid);

        if !obj_path.exists() {
            return Err(CASError::NotFound { cid: cid.to_string() });
        }

        let json = fs::read(&obj_path)?;
        let obj: AIObject = serde_json::from_slice(&json)?;

        // Verify integrity on read
        if obj.cid != cid || !obj.verify_integrity() {
            return Err(CASError::IntegrityFailed {
                cid: cid.to_string(),
            });
        }

        Ok(obj)
    }

    /// Check if an object exists.
    pub fn exists(&self, cid: &str) -> bool {
        self.object_path(cid).exists()
    }

    /// List all CIDs stored in this CAS.
    /// Note: expensive for large stores; use with caution.
    pub fn list_cids(&self) -> io::Result<Vec<String>> {
        let mut cids = Vec::new();

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let dir_name = entry.file_name();
            let dir_name_str = dir_name.to_string_lossy();

            // Each shard dir is a 2-char hex prefix
            if dir_name_str.len() != 2 {
                continue;
            }

            for sub_entry in fs::read_dir(entry.path())? {
                let sub_entry = sub_entry?;
                let name = sub_entry.file_name();
                let name_str = name.to_string_lossy();

                // Skip temp files
                if name_str.starts_with(".tmp_") {
                    continue;
                }

                // Rebuild CID: dir_name + filename (strip .json if present)
                let rest = name_str.strip_prefix(".json").unwrap_or(&name_str);
                let cid = format!("{}{}", dir_name_str, rest);
                cids.push(cid);
            }
        }

        Ok(cids)
    }

    /// Delete an object by CID (physical delete).
    /// WARNING: this is irreversible. Consider logical delete via the semantic FS layer.
    pub fn delete(&self, cid: &str) -> io::Result<()> {
        let obj_path = self.object_path(cid);
        if obj_path.exists() {
            fs::remove_file(obj_path)?;
        }
        Ok(())
    }

    /// Storage root path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Compute shard directory for a CID.
    fn shard_dir(&self, cid: &str) -> PathBuf {
        let (prefix, _) = cid.split_at(2);
        self.root.join(prefix)
    }

    /// Compute object file path for a CID.
    fn object_path(&self, cid: &str) -> PathBuf {
        let (prefix, rest) = cid.split_at(2);
        self.root.join(prefix).join(rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_put_and_get() {
        let dir = tempdir().unwrap();
        let storage = CASStorage::new(dir.path().to_path_buf()).unwrap();

        let obj = AIObject::new(
            b"hello, plico!".to_vec(),
            AIObjectMeta::text(["greeting", "test"]),
        );
        let cid = obj.cid.clone();

        storage.put(&obj).unwrap();
        let retrieved = storage.get(&cid).unwrap();

        assert_eq!(retrieved.data, b"hello, plico!");
        assert!(retrieved.meta.tags.contains(&"greeting".to_string()));
    }

    #[test]
    fn test_deduplication() {
        let dir = tempdir().unwrap();
        let storage = CASStorage::new(dir.path().to_path_buf()).unwrap();

        let obj1 = AIObject::new(b"same content".to_vec(), AIObjectMeta::text(["tag1"]));
        let obj2 = AIObject::new(b"same content".to_vec(), AIObjectMeta::text(["tag2"]));

        let cid1 = storage.put(&obj1).unwrap();
        let cid2 = storage.put(&obj2).unwrap();

        // Same content → same CID
        assert_eq!(cid1, cid2);
        // Only one physical file
        assert_eq!(storage.list_cids().unwrap().len(), 1);
    }

    #[test]
    fn test_not_found() {
        let dir = tempdir().unwrap();
        let storage = CASStorage::new(dir.path().to_path_buf()).unwrap();

        let result = storage.get("0000000000000000000000000000000000000000000000000000000000000000");
        assert!(matches!(result, Err(CASError::NotFound { .. })));
    }
}
