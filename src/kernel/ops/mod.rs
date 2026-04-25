//! Kernel operation modules — split from mod.rs for Ariadne compliance (<700 lines).
//!
//! Each submodule contains one logical group of AIKernel methods.
//! All impl blocks extend AIKernel — Rust allows multiple impl blocks per type.

pub mod fs;
pub mod agent;
pub mod memory;
pub mod events;
pub mod graph;
pub mod dispatch;
pub mod messaging;
pub mod dashboard;
pub mod tools_external;
pub mod permission;
pub mod prefetch_cache;
pub mod prefetch_profile;
pub mod prefetch;
pub mod tenant;
pub mod tier_maintenance;
pub mod observability;
pub mod cost_ledger;
pub mod batch;
pub mod cache;
pub mod causal_hook;
pub mod verification;
pub mod checkpoint;
pub mod distributed;
pub mod model;
pub mod delta;
pub mod session;
pub mod hybrid;
pub mod skill_discovery;
pub mod task;
pub mod intent;
pub mod intent_executor;
pub mod intent_decomposer;
pub mod self_healing;
pub mod cross_domain_skill;
pub mod goal_generator;
pub mod temporal_projection;
