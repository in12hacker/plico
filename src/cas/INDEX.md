# Module: cas — Content-Addressed Storage

The foundation of Plico's AI-native filesystem. Provides immutable, content-addressed object storage.

Status: stable | Fan-in: 3 (kernel, fs, scheduler) | Fan-out: 0

## Public API

| Export | File | Description |
|--------|------|-------------|
| `AIObject` | `object.rs` | Fundamental data unit: CID + data + AIObjectMeta |
| `AIObjectMeta` | `object.rs` | Semantic metadata: ContentType + tags + intent |
| `ContentType` | `object.rs` | Enum: Text/Image/Audio/Video/Structured/Binary/Unknown |
| `CASStorage` | `storage.rs` | Storage engine: put/get/exists/list_cids/delete |
| `CASError` | `storage.rs` | Error: NotFound, IntegrityFailed, Serialization, Io |

## Dependencies (Fan-out: 0)

None — this is a leaf module.

## Dependents (Fan-in: 3)

- `src/kernel/mod.rs` → `CASStorage::put`, `CASStorage::get`
- `src/fs/semantic_fs.rs` → `CASStorage`
- `src/bin/aicli.rs` → via kernel

## Interface Contract

- `AIObject::new(data, meta)`: CID computed automatically as SHA-256. **Panics**: never.
- `CASStorage::put(obj)`: Returns CID. Idempotent (dedup). **Errors**: `CASError::IntegrityFailed` if content doesn't match CID. **Side effect**: atomic file write.
- `CASStorage::get(cid)`: **Errors**: `CASError::NotFound`. **Side effect**: integrity check on every read.
- CID prefix sharding: `root/AB/CDEF...` — prevents >10k files per directory.

## Modification Risk

- Add field to `AIObject` → compatible, no consumer changes
- Change CID algorithm (SHA-256 → BLAKE3) → **BREAKING**, update all CID storage and references
- Change shard strategy → **BREAKING**, all stored objects unreadable

## Task Routing

- Change CID hash algorithm → modify `object.rs` `AIObject::compute_cid()`
- Change serialization format → modify `storage.rs` `put()`/`get()`, all stored objects affected
- Add new ContentType variant → update `ContentType::from_extension()` and `ContentType::is_*` methods
- Change shard depth → modify `storage.rs` shard_dir/object_path methods

## Tests

- `cargo test --lib -- cas::object` — CID computation, integrity, ContentType
- `cargo test --lib -- cas::storage` — put/get, deduplication, not-found
