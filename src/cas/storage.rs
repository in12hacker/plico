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

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use crate::cas::object::AIObject;

#[cfg(test)]
use crate::cas::AIObjectMeta;

// ── F-22: Access tracking ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccessEntry {
    pub first_accessed_at: u64,
    pub last_accessed_at: u64,
    pub access_count: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug)]
pub struct CASStorage {
    root: PathBuf,
    access_log: RwLock<HashMap<String, AccessEntry>>,
    // F-42: Lazy persistence state
    access_count_total: AtomicU64,
    last_persist_ms: AtomicU64,
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

    // F-1: Invalid CID format (too short or non-hex)
    #[error("Invalid CID format: '{cid}' — must be at least 2 hex characters")]
    InvalidCid { cid: String },
}

impl From<CASError> for io::Error {
    fn from(e: CASError) -> io::Error {
        match e {
            CASError::Io(e) => e,
            CASError::NotFound { cid: _ } => io::Error::new(io::ErrorKind::NotFound, e.to_string()),
            CASError::InvalidCid { cid: _ } => io::Error::new(io::ErrorKind::InvalidInput, e.to_string()),
            CASError::IntegrityFailed { cid: _ } => io::Error::new(io::ErrorKind::InvalidData, e.to_string()),
            CASError::Serialization(e) => io::Error::new(io::ErrorKind::InvalidData, e.to_string()),
        }
    }
}

fn validate_cid(cid: &str) -> Result<(), CASError> {
    if cid.len() < 2 {
        return Err(CASError::InvalidCid { cid: cid.to_string() });
    }
    if !cid.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CASError::InvalidCid { cid: cid.to_string() });
    }
    Ok(())
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
        let access_log = Self::load_access_log(&root_path).unwrap_or_default();
        let initial_count = access_log.values().map(|e| e.access_count).sum();
        Ok(CASStorage {
            root: root_path,
            access_log: RwLock::new(access_log),
            access_count_total: AtomicU64::new(initial_count),
            last_persist_ms: AtomicU64::new(now_ms()),
        })
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

        let shard_dir = self.shard_dir(&obj.cid)?;
        let obj_path = self.object_path(&obj.cid)?;

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
        let obj = self.get_raw(cid)?;
        self.record_access(cid);
        Ok(obj)
    }

    /// Read without recording access (used by metadata queries).
    pub fn get_raw(&self, cid: &str) -> Result<AIObject, CASError> {
        let obj_path = self.object_path(cid)?;

        if !obj_path.exists() {
            return Err(CASError::NotFound { cid: cid.to_string() });
        }

        let json = fs::read(&obj_path)?;
        let obj: AIObject = serde_json::from_slice(&json)?;

        if obj.cid != cid || !obj.verify_integrity() {
            return Err(CASError::IntegrityFailed {
                cid: cid.to_string(),
            });
        }

        Ok(obj)
    }

    /// Check if an object exists.
    pub fn exists(&self, cid: &str) -> bool {
        self.object_path(cid).map(|p| p.exists()).unwrap_or(false)
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
        let obj_path = match self.object_path(cid) {
            Ok(p) => p,
            // Invalid CID or not found → no-op (graceful deletion)
            Err(CASError::NotFound { .. }) | Err(CASError::InvalidCid { .. }) => return Ok(()),
            Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidInput, e.to_string())),
        };
        if obj_path.exists() {
            fs::remove_file(obj_path)?;
        }
        Ok(())
    }

    /// Storage root path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Number of objects stored.
    /// Counts top-level shard directories as a quick estimate; accurate if no partial shards.
    pub fn len(&self) -> usize {
        fs::read_dir(&self.root)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()))
                    .count()
            })
            .unwrap_or(0)
    }

    /// Returns true if no objects are stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // ── F-22: Access tracking methods ──
    // ── F-42: Lazy persist implementation ──

    fn record_access(&self, cid: &str) {
        let now = now_ms();
        let should_persist;

        {
            let mut log = self.access_log.write().unwrap();
            log.entry(cid.to_string())
                .and_modify(|e| { e.last_accessed_at = now; e.access_count += 1; })
                .or_insert(AccessEntry { first_accessed_at: now, last_accessed_at: now, access_count: 1 });

            // F-42: Update counters for lazy persist trigger
            let new_total = self.access_count_total.fetch_add(1, Ordering::Relaxed) + 1;
            should_persist = new_total.is_multiple_of(100);
        }

        // F-42: Trigger lazy persist if threshold reached
        if should_persist {
            let _ = self.persist_access_log();
        }
    }

    /// F-42: Check if periodic persist is needed and execute if so.
    /// Called on each public API entry point for low-overhead check.
    pub fn maybe_persist_access_log(&self) {
        let elapsed = now_ms().saturating_sub(self.last_persist_ms.load(Ordering::Relaxed));
        if elapsed >= 60_000 && self.persist_access_log().is_ok() {
            self.last_persist_ms.store(now_ms(), Ordering::Relaxed);
        }
    }

    /// Get access statistics for a CID. Returns None if never accessed.
    pub fn object_usage(&self, cid: &str) -> Option<AccessEntry> {
        self.access_log.read().unwrap().get(cid).cloned()
    }

    /// List CIDs not accessed within `threshold_ms` milliseconds.
    pub fn cold_objects(&self, threshold_ms: u64) -> Vec<String> {
        let now = now_ms();
        let log = self.access_log.read().unwrap();

        let tracked: HashMap<&String, &AccessEntry> = log.iter().collect();
        let mut cold = Vec::new();

        if let Ok(cids) = self.list_cids() {
            for cid in cids {
                let is_cold = match tracked.get(&cid) {
                    Some(entry) => now.saturating_sub(entry.last_accessed_at) > threshold_ms,
                    None => true, // never accessed = cold
                };
                if is_cold { cold.push(cid); }
            }
        }
        cold
    }

    /// Total disk bytes occupied by CAS objects.
    pub fn total_bytes(&self) -> u64 {
        self.list_cids().unwrap_or_default().iter()
            .filter_map(|cid| {
                let p = self.object_path(cid).ok()?;
                let m = fs::metadata(&p).ok()?;
                Some(m.len())
            })
            .sum()
    }

    /// Persist access log to disk.
    pub fn persist_access_log(&self) -> io::Result<()> {
        let log = self.access_log.read().unwrap();
        let json = serde_json::to_vec(&*log)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let path = self.root.join("_access_log.json");
        let tmp = self.root.join("_access_log.json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn load_access_log(root: &Path) -> io::Result<HashMap<String, AccessEntry>> {
        let path = root.join("_access_log.json");
        if !path.exists() { return Ok(HashMap::new()); }
        let json = fs::read(&path)?;
        serde_json::from_slice(&json)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Compute shard directory for a CID.
    fn shard_dir(&self, cid: &str) -> Result<PathBuf, CASError> {
        validate_cid(cid)?;
        let (prefix, _) = cid.split_at(2);
        Ok(self.root.join(prefix))
    }

    /// Compute object file path for a CID.
    fn object_path(&self, cid: &str) -> Result<PathBuf, CASError> {
        validate_cid(cid)?;
        let (prefix, rest) = cid.split_at(2);
        Ok(self.root.join(prefix).join(rest))
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

    // F-1: CID validation tests
    #[test]
    fn test_get_empty_cid_returns_error() {
        let dir = tempdir().unwrap();
        let storage = CASStorage::new(dir.path().to_path_buf()).unwrap();

        let result = storage.get("");
        assert!(matches!(result, Err(CASError::InvalidCid { cid }) if cid.is_empty()));
    }

    #[test]
    fn test_get_single_char_cid_returns_error() {
        let dir = tempdir().unwrap();
        let storage = CASStorage::new(dir.path().to_path_buf()).unwrap();

        let result = storage.get("a");
        assert!(matches!(result, Err(CASError::InvalidCid { cid }) if cid == "a"));
    }

    #[test]
    fn test_exists_empty_cid_returns_false() {
        let dir = tempdir().unwrap();
        let storage = CASStorage::new(dir.path().to_path_buf()).unwrap();

        assert!(!storage.exists(""));
        assert!(!storage.exists("x"));
    }

    #[test]
    fn test_delete_empty_cid_is_ok() {
        let dir = tempdir().unwrap();
        let storage = CASStorage::new(dir.path().to_path_buf()).unwrap();

        // delete with invalid CID is a no-op (graceful handling)
        assert!(storage.delete("").is_ok());
        assert!(storage.delete("x").is_ok());
    }
}
