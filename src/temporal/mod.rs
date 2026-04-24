//! Temporal Reasoning — Natural Language Time → Time Ranges
//!
//! Resolves vague human time expressions ("几天前", "上周", "last month") into
//! concrete Unix-millisecond ranges, enabling time-bounded search.
//!
//! # Architecture
//!
//! ```text
//! TemporalResolver (trait)
//! └── HeuristicTemporalResolver — fast rule-based fallback
//! ```
//!
//! # Confidence-Driven Search Strategy
//!
//! | Confidence | Behavior |
//! |------------|---------|
//! | ≥ 0.8     | Strict: use resolved range exactly |
//! | 0.5–0.8   | Expanded: extend range ±7 days, rerank by recency |
//! | < 0.5     | Fallback: pure semantic search, ignore time filter |
//!
//! # Key Design Insight
//!
//! Human memory is organized around **events**, not files. When a user says
//! "the Q2 planning meeting from a few days ago", they reference an event
//! container — the meeting bundles: attendees, documents, photos, decisions.
//!
//! TemporalResolver bridges the gap between vague event-anchored references
//! and precise time-bounded CAS queries.

mod resolver;
mod rules;

pub use resolver::{TemporalResolver, TemporalRange, StubTemporalResolver};
pub use rules::{Granularity, TemporalRule, RULE_BASED_RESOLVER, HeuristicTemporalResolver, resolve_heuristic};
