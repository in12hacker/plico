# Project Instructions — 太初 (Plico)

This file contains foundational mandates for the Plico project. These instructions take absolute precedence over general defaults.

## Workspace Conventions

### 1. Root Directory Hygiene
- **Rule**: Never generate temporary files, logs, or scripts directly in the project root directory.
- **Convention**: All runtime-related transient data, logs, and temporary scripts must be placed in the `.runtime/` directory.
- **Example**: Move `*.log`, `*.sh` (transient), and `*.py` (probes) to `.runtime/`.

### 2. Architecture & Soul Alignment
- Follow **Soul 3.0** principles as defined in `system-v3.md`.
- Adhere to the **9 Architecture Red Lines**.
- Prioritize **polymorphic core verbs** (get, list, store, etc.) for API interactions.

### 3. Engineering Standards
- Use **Surgical Changes**: Touch only what is necessary.
- **Atomic Writes**: Always use `atomic_write_json` or `atomic_write_bytes` from `src/kernel/persistence.rs` for persistence.
- **UTF-8 Safety**: Use `crate::util::safe_truncate` and `crate::util::safe_range` when slicing strings to avoid char boundary panics.
