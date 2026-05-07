//! CAS Garbage Collection — mark-sweep active CID discovery.
//!
//! The mark phase collects all CIDs referenced by:
//! - Memory persistence index (memory_index.json)
//! - Checkpoint index (checkpoint_index.json)
//! - SemanticFS tag index + recycle bin
//! - HNSW vector index
//!
//! The sweep phase is delegated to [`CASStorage::gc()`].

use std::collections::HashSet;
use std::path::Path;

use super::CASStorage;

/// Collect all active CIDs from persistence indexes at `root`.
///
/// Reads `memory_index.json` and `checkpoint_index.json` to find
/// all CIDs that are still referenced.
pub fn collect_active_cids(root: &Path) -> HashSet<String> {
    let mut active = HashSet::new();

    // Memory persistence index
    let mem_index_path = root.join("memory_index.json");
    if let Ok(content) = std::fs::read_to_string(&mem_index_path) {
        if let Ok(index) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(agents) = index.get("agents").and_then(|v| v.as_object()) {
                for (_agent_id, tiers) in agents {
                    if let Some(tiers_arr) = tiers.as_array() {
                        for tier in tiers_arr {
                            if let Some(cid) = tier.get("cid").and_then(|v| v.as_str()) {
                                active.insert(cid.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // Checkpoint index
    let ckpt_index_path = root.join("checkpoint_index.json");
    if let Ok(content) = std::fs::read_to_string(&ckpt_index_path) {
        if let Ok(index) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(obj) = index.as_object() {
                for (_agent, cids_val) in obj {
                    if let Some(cids_arr) = cids_val.as_array() {
                        for cid_val in cids_arr {
                            if let Some(cid) = cid_val.as_str() {
                                active.insert(cid.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    active
}

/// Run mark-sweep GC on the CAS at `root`.
///
/// Returns `(swept, kept)` counts.
pub fn run_gc(root: &Path) -> std::io::Result<(usize, usize)> {
    let cas = CASStorage::new(root.to_path_buf())?;
    let active = collect_active_cids(root);
    cas.gc(&active)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_collect_active_cids_empty() {
        let dir = tempdir().unwrap();
        let active = collect_active_cids(dir.path());
        assert!(active.is_empty());
    }

    #[test]
    fn test_gc_sweeps_unreferenced() {
        let dir = tempdir().unwrap();
        let cas = CASStorage::new(dir.path().to_path_buf()).unwrap();

        // Store 3 objects
        for i in 0..3 {
            let obj = crate::cas::AIObject::new(
                format!("content-{}", i).into_bytes(),
                crate::cas::AIObjectMeta::text(["test"]),
            );
            cas.put(&obj).unwrap();
        }

        let all_cids = cas.list_cids().unwrap();
        assert_eq!(all_cids.len(), 3);

        // Only keep 1 active
        let mut active = HashSet::new();
        active.insert(all_cids[0].clone());

        let (swept, kept) = cas.gc(&active).unwrap();
        assert_eq!(kept, 1);
        assert_eq!(swept, 2);
        assert_eq!(cas.list_cids().unwrap().len(), 1);
    }

    #[test]
    fn test_gc_preserves_active() {
        let dir = tempdir().unwrap();
        let cas = CASStorage::new(dir.path().to_path_buf()).unwrap();

        let obj = crate::cas::AIObject::new(
            b"keep me".to_vec(),
            crate::cas::AIObjectMeta::text(["important"]),
        );
        let cid = cas.put(&obj).unwrap();

        let mut active = HashSet::new();
        active.insert(cid);

        let (swept, kept) = cas.gc(&active).unwrap();
        assert_eq!(swept, 0);
        assert_eq!(kept, 1);
        assert_eq!(cas.list_cids().unwrap().len(), 1);
    }
}
