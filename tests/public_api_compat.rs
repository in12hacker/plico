//! W0.1 public-API compatibility canary.
//!
//! References the three crate-public symbols that W-0's B-10 sweep deleted
//! by in-repo-reference count alone. They are reinstated as deprecated
//! compatibility surface, and this external-view integration test keeps
//! real `plico::...` paths exercising them so a future "zero in-repo
//! references" grep can never again mistake them for dead code. The
//! file-level allow is deliberate: the deprecation itself is expected —
//! only its warning is silenced for the `-D warnings` gates; a raw compile
//! without the allow shows exactly the three deprecation notes.

#![allow(deprecated)]

use plico::temporal::{Granularity, TemporalRange};
use plico::util::safe_range;

#[test]
fn public_compat_canary_reaches_reinstated_symbols() {
    // plico::util::safe_range — pre-W-0 slicing behavior.
    assert_eq!(safe_range("hello world", 2, 7), "llo w");
    assert_eq!(safe_range("你好世界", 0, 3), "你");

    // plico::temporal::TemporalRange::expanded — pre-W-0 expansion behavior.
    let range = TemporalRange {
        since: 86_400_000,
        until: 172_800_000,
        confidence: 0.5,
        granularity: Granularity::Week,
        expression: "test".to_string(),
    };
    let expanded = range.expanded(1);
    assert_eq!(expanded.since, 0);
    assert_eq!(expanded.until, 259_200_000);
    assert_eq!(expanded.granularity, Granularity::Fuzzy);

    // plico::temporal::Granularity::HalfYear — pre-W-0 variant encoding.
    assert_eq!(format!("{:?}", Granularity::HalfYear), "HalfYear");
}
