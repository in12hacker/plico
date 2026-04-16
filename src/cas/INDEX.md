# Module: cas

Content-Addressed Storage — SHA-256 hash as object identity, automatic deduplication, immutability by default.

Status: stable | Fan-in: 5 | Fan-out: 0

## Dependents (Fan-in: 5)

- `src/fs/semantic_fs.rs` → CASStorage (read/write objects, rebuild index)
- `src/fs/context_loader.rs` → CASStorage (L2 full content reads)
- `src/memory/persist.rs` → CASStorage, AIObject, AIObjectMeta (memory persistence)
- `src/kernel/mod.rs` → CASStorage, AIObject, AIObjectMeta (kernel wiring)
- `src/bin/plicod.rs` [indirect via kernel] → CAS operations through API

## Modification Risk

- Add field to `AIObjectMeta` → compatible if `#[serde(default)]`, update all constructors
- Change `ContentType` variants → BREAKING, update all `match` arms in fs/kernel/api
- Change CID algorithm (SHA-256) → BREAKING, invalidates all stored objects
- Change shard layout → BREAKING, existing objects become unfindable

## Task Routing

- Add new content type → modify `src/cas/object.rs` ContentType enum + Display + from_extension
- Fix CID computation → modify `src/cas/object.rs` AIObject::compute_cid
- Change storage layout → modify `src/cas/storage.rs` shard_dir/object_path
- Add metadata field → modify `src/cas/object.rs` AIObjectMeta + all callers

## Public API

| Export | File | Description |
|--------|------|-------------|
| `AIObject` | `object.rs` | Fundamental data unit — CID + data + metadata |
| `AIObjectMeta` | `object.rs` | Semantic metadata (tags, content_type, created_by) |
| `ContentType` | `object.rs` | Content classification enum (Text/Image/Audio/Video/etc.) |
| `CASStorage` | `storage.rs` | Disk-backed content-addressed store |
| `CASError` | `storage.rs` | Typed errors (NotFound, IntegrityFailed, Io, Serialization) |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `object.rs` | ~213 | AIObject, AIObjectMeta, ContentType definitions |
| `storage.rs` | ~254 | CASStorage engine: put/get/delete/list with sharding |
| `mod.rs` | ~18 | Re-exports |

## Dependencies (Fan-out: 0)

None — CAS is the lowest layer, depends only on std + external crates (sha2, serde, serde_json).

## Interface Contract

- `CASStorage::put()`: idempotent — same content always returns same CID; integrity verified before write
- `CASStorage::get()`: returns `CASError::NotFound` if CID absent; integrity verified on read
- `AIObject::new()`: CID computed automatically from data bytes via SHA-256
- Thread safety: `CASStorage` is safe for concurrent reads; concurrent writes to different CIDs are safe (different shard dirs)
- Side effect: `put()` creates shard directories and writes files atomically (temp file → rename)

## Tests

- Unit: `src/cas/object.rs` mod tests, `src/cas/storage.rs` mod tests
- Integration: `tests/kernel_test.rs` (exercises CAS through kernel)
- Critical: `test_put_and_get`, `test_deduplication`, `test_cid_is_content_hash`
