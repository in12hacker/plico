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
pub mod prefetch;
pub mod tenant;
pub mod tier_maintenance;
pub mod observability;
pub mod batch;
pub mod model;
