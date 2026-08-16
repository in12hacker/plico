//! Execution Observation Ledger v1 — Phase 1A pure type and validation core
//! (ADR-0007, milestone v53 WP1).
//!
//! Crate-private, in-memory only: typed identifiers, fixed wire schemas,
//! RFC 8785/JCS canonicalization, domain-separated SHA-256, and strict
//! field/boundary/transition validators. No I/O, no CAS namespace, no store,
//! no kernel/scheduler wiring, no public API surface.

#![allow(dead_code)] // WP1 has no production caller by contract; wiring starts at the WP2 store (ADR-0007 §10)

pub(crate) mod canonical;
pub(crate) mod error;
pub(crate) mod hash;
pub(crate) mod ids;
pub(crate) mod model;
pub(crate) mod validation;

#[cfg(test)]
mod tests;
