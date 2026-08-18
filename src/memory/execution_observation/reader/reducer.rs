//! The single pure attempt-state reducer moved to `store::reducer` by the
//! WP3B.1-A convergence (ADR-0010 §6: exactly one reducer crate-wide). This
//! module re-exports it so the reader's existing import paths are unchanged.

pub(super) use crate::memory::execution_observation::store::reducer::{
    attempt_ordering, reduce, ReducibleAttemptV1, ReducibleEventV1, ReducibleKindV1, ReducibleReceiptV1,
};
