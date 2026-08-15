//! Kernel operation modules — split from mod.rs for Ariadne compliance (<700 lines).
//!
//! Each submodule contains one logical group of AIKernel methods.
//! All impl blocks extend AIKernel — Rust allows multiple impl blocks per type.

pub mod agent;
pub mod agent_profile;
pub mod batch;
pub mod cache;
pub mod causal_hook;
pub mod checkpoint;
pub mod cognitive_pipeline;
pub mod conflict_detector;
pub mod cost_ledger;
pub mod dashboard;
pub mod delta;
pub mod diagnostic;
pub mod dispatch;
pub mod entity_resolver;
pub mod events;
pub mod fs;
pub mod graph;
pub mod ingest;
pub mod intent;
pub mod intent_executor;
pub mod kg_builder;
pub mod memory;
pub mod messaging;
pub mod model;
pub mod observability;
pub mod permission;
pub mod prefetch;
pub mod prefetch_cache;
pub mod prefetch_profile;
pub(crate) mod projection_controller;
pub(crate) mod projection_runtime;
pub mod readiness;
pub mod security;
pub mod session;
pub mod skill_forge;
pub mod task;
pub mod tools_external;
pub mod verification;
