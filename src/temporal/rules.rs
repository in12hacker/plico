//! Pre-defined heuristic temporal rules (fast fallback / LLM prompt enrichment)
//!
//! These rules complement the LLM-based resolver. They are evaluated first;
//! only unmatched expressions are sent to the LLM.
//!
//! All times are expressed as offsets from a reference date (default: now).
//! Rules are matched case-insensitively.

use chrono::{Datelike, Duration, NaiveDate};
use std::collections::HashMap;

/// Time granularity of a resolved expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    /// Exact calendar day.
    ExactDay,
    /// Full calendar week (Mon–Sun or Sun–Sat, culturally dependent).
    Week,
    /// Full calendar month.
    Month,
    /// Calendar quarter (Q1=Jan-Mar, …, Q4=Oct-Dec).
    Quarter,
    /// Half-year.
    HalfYear,
    /// Full calendar year.
    Year,
    /// Vague / approximate — use expanded search window.
    Fuzzy,
}

// ─── Helper date functions ────────────────────────────────────────────────────

fn start_of_week(reference: &NaiveDate) -> NaiveDate {
    let days_since_monday = reference.weekday().num_days_from_monday();
    *reference - Duration::days(days_since_monday as i64)
}

fn start_of_month(reference: &NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(reference.year(), reference.month(), 1).unwrap()
}

fn start_of_quarter(reference: &NaiveDate) -> NaiveDate {
    let q_month = ((reference.month() - 1) / 3) * 3 + 1;
    NaiveDate::from_ymd_opt(reference.year(), q_month, 1).unwrap()
}

// ─── Rule matching ────────────────────────────────────────────────────────────

/// A pre-defined heuristic rule for a specific time expression.
#[derive(Debug, Clone)]
pub struct TemporalRule {
    /// Case-insensitive patterns to match.
    pub patterns: &'static [&'static str],
    /// Confidence in this rule's accuracy.
    pub confidence: f32,
    /// Resulting granularity.
    pub granularity: Granularity,
}

impl TemporalRule {
    /// Evaluate this rule against a reference date.
    /// Returns `(since, until)` or `None` if the rule doesn't match.
    fn evaluate(&self, reference: &NaiveDate) -> Option<(NaiveDate, NaiveDate)> {
        match self.patterns[0] {
            // ── Exact days ──────────────────────────────────────────────
            "今天" | "today" | "今日" | "本日" => Some((*reference, *reference)),
            "昨天" | "yesterday" | "昨日" => {
                Some((*reference - Duration::days(1), *reference - Duration::days(1)))
            }
            "前天" => Some((*reference - Duration::days(2), *reference - Duration::days(2))),
            "明天" | "tomorrow" | "明日" => {
                Some((*reference + Duration::days(1), *reference + Duration::days(1)))
            }
            "后天" => Some((*reference + Duration::days(2), *reference + Duration::days(2))),
            // ── Relative past ─────────────────────────────────────────
            "几天前" | "前几天" => {
                let since = *reference - Duration::days(7);
                Some((since, *reference))
            }
            "最近" | "recently" => {
                let since = *reference - Duration::days(30);
                Some((since, *reference))
            }
            "今早" | "今天早上" | "this morning" => Some((*reference, *reference)),
            "今晚" | "this evening" | "tonight" => Some((*reference, *reference)),
            // ── Weeks ─────────────────────────────────────────────────
            "上周" | "last week" => {
                let this_monday = start_of_week(reference);
                let last_monday = this_monday - Duration::days(7);
                let last_sunday = this_monday - Duration::days(1);
                Some((last_monday, last_sunday))
            }
            "上上周" => {
                let this_monday = start_of_week(reference);
                let two_weeks_ago_monday = this_monday - Duration::days(14);
                let two_weeks_ago_sunday = this_monday - Duration::days(8);
                Some((two_weeks_ago_monday, two_weeks_ago_sunday))
            }
            // ── Months ─────────────────────────────────────────────────
            "本月" | "this month" => {
                let since = start_of_month(reference);
                Some((since, *reference))
            }
            "上个月" | "last month" | "上月" => {
                let (y, m) = (reference.year(), reference.month());
                let (prev_y, prev_m) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
                let since = NaiveDate::from_ymd_opt(prev_y, prev_m, 1).unwrap();
                let until = start_of_month(reference) - Duration::days(1);
                Some((since, until))
            }
            "两个月前" => {
                let (y, m) = (reference.year(), reference.month());
                let (prev_y, prev_m) = if m <= 2 { (y - 1, m + 10) } else { (y, m - 2) };
                let since = NaiveDate::from_ymd_opt(prev_y, prev_m, 1).unwrap();
                let until = start_of_month(reference) - Duration::days(1);
                Some((since, until))
            }
            // ── Quarters ───────────────────────────────────────────────
            "本季度" | "this quarter" => {
                let since = start_of_quarter(reference);
                Some((since, *reference))
            }
            "上季度" | "last quarter" => {
                let this_q = start_of_quarter(reference);
                let prev_q = if this_q.month() <= 3 {
                    NaiveDate::from_ymd_opt(this_q.year() - 1, this_q.month() + 9, 1).unwrap()
                } else {
                    NaiveDate::from_ymd_opt(this_q.year(), this_q.month() - 3, 1).unwrap()
                };
                let until = this_q - Duration::days(1);
                Some((prev_q, until))
            }
            // ── Years ──────────────────────────────────────────────────
            "去年" | "last year" => {
                let since = NaiveDate::from_ymd_opt(reference.year() - 1, 1, 1).unwrap();
                let until = NaiveDate::from_ymd_opt(reference.year() - 1, 12, 31).unwrap();
                Some((since, until))
            }
            "今年" | "this year" => {
                let since = NaiveDate::from_ymd_opt(reference.year(), 1, 1).unwrap();
                Some((since, *reference))
            }
            // ── Eras ──────────────────────────────────────────────────
            "很久以前" | "long ago" => {
                let since = *reference - Duration::days(365);
                Some((since, *reference))
            }
            _ => None,
        }
    }
}

/// All pre-defined rules.
static RULES: &[TemporalRule] = &[
    // Exact days
    TemporalRule { patterns: &["今天", "today", "今日", "本日"], confidence: 0.95, granularity: Granularity::ExactDay },
    TemporalRule { patterns: &["昨天", "yesterday", "昨日"], confidence: 0.95, granularity: Granularity::ExactDay },
    TemporalRule { patterns: &["前天"], confidence: 0.95, granularity: Granularity::ExactDay },
    TemporalRule { patterns: &["明天", "tomorrow", "明日"], confidence: 0.95, granularity: Granularity::ExactDay },
    TemporalRule { patterns: &["后天"], confidence: 0.90, granularity: Granularity::ExactDay },
    // Relative past
    TemporalRule { patterns: &["几天前", "前几天"], confidence: 0.60, granularity: Granularity::Fuzzy },
    TemporalRule { patterns: &["最近", "recently"], confidence: 0.50, granularity: Granularity::Fuzzy },
    TemporalRule { patterns: &["今早", "今天早上", "this morning"], confidence: 0.85, granularity: Granularity::ExactDay },
    TemporalRule { patterns: &["今晚", "this evening", "tonight"], confidence: 0.85, granularity: Granularity::ExactDay },
    // Weeks
    TemporalRule { patterns: &["上周", "last week"], confidence: 0.85, granularity: Granularity::Week },
    TemporalRule { patterns: &["上上周"], confidence: 0.80, granularity: Granularity::Week },
    // Months
    TemporalRule { patterns: &["本月", "this month"], confidence: 0.90, granularity: Granularity::Month },
    TemporalRule { patterns: &["上个月", "last month", "上月"], confidence: 0.90, granularity: Granularity::Month },
    TemporalRule { patterns: &["两个月前"], confidence: 0.80, granularity: Granularity::Month },
    // Quarters
    TemporalRule { patterns: &["本季度", "this quarter"], confidence: 0.90, granularity: Granularity::Quarter },
    TemporalRule { patterns: &["上季度", "last quarter"], confidence: 0.85, granularity: Granularity::Quarter },
    // Years
    TemporalRule { patterns: &["去年", "last year"], confidence: 0.95, granularity: Granularity::Year },
    TemporalRule { patterns: &["今年", "this year"], confidence: 0.95, granularity: Granularity::Year },
    // Eras
    TemporalRule { patterns: &["很久以前", "long ago"], confidence: 0.30, granularity: Granularity::Fuzzy },
];

/// Build a fast lookup map: lowercase pattern → rule index.
fn build_rule_map() -> HashMap<String, usize> {
    let mut map = HashMap::new();
    for (idx, rule) in RULES.iter().enumerate() {
        for &pattern in rule.patterns {
            map.entry(pattern.to_lowercase()).or_insert(idx);
        }
    }
    map
}

static RULE_MAP: std::sync::LazyLock<HashMap<String, usize>> =
    std::sync::LazyLock::new(build_rule_map);

/// Heuristic temporal resolver — rule-based, synchronous, no LLM needed.
/// Returns `None` if the expression doesn't match any rule.
pub fn resolve_heuristic(expression: &str, reference: &NaiveDate) -> Option<(NaiveDate, NaiveDate, f32, Granularity)> {
    let lower = expression.to_lowercase();
    let trimmed = lower.trim();

    // Direct map lookup
    if let Some(&idx) = RULE_MAP.get(trimmed) {
        let rule = &RULES[idx];
        if let Some((since, until)) = rule.evaluate(reference) {
            return Some((since, until, rule.confidence, rule.granularity));
        }
    }

    // Substring scan (less precise — lower confidence)
    for (idx, rule) in RULES.iter().enumerate() {
        if RULE_MAP.get(trimmed) == Some(&idx) {
            continue; // already checked above
        }
        for &pattern in rule.patterns {
            if trimmed.contains(&pattern.to_lowercase()) {
                if let Some((since, until)) = rule.evaluate(reference) {
                    return Some((since, until, rule.confidence * 0.9, Granularity::Fuzzy));
                }
            }
        }
    }

    None
}

/// Default rule-based resolver for when no LLM is available.
pub static RULE_BASED_RESOLVER: HeuristicTemporalResolver = HeuristicTemporalResolver;

/// Minimal synchronous resolver that only uses the pre-defined rules.
pub struct HeuristicTemporalResolver;

// Allow rules module to reference sibling module types (resolver declared first in mod.rs)
use super::resolver::TemporalRange;

// Granularity is defined in this module (rules), so no import needed here

impl HeuristicTemporalResolver {
    pub fn new() -> Self {
        HeuristicTemporalResolver
    }

    /// Resolve a natural-language time expression.
    ///
    /// Returns `(since_unix_ms, until_unix_ms, confidence, granularity)` or `None`
    /// if the expression doesn't match any rule.
    pub fn resolve(&self, expression: &str) -> Option<(i64, i64, f32, Granularity)> {
        let reference = chrono::Local::now().date_naive();
        let (since, until, confidence, granularity) = resolve_heuristic(expression, &reference)?;
        Some((
            since.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis(),
            until.and_hms_opt(23, 59, 59)?.and_utc().timestamp_millis(),
            confidence,
            granularity,
        ))
    }
}

impl Default for HeuristicTemporalResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl super::resolver::TemporalResolver for HeuristicTemporalResolver {
    fn resolve(&self, expression: &str, _reference_ms: Option<i64>) -> Option<TemporalRange> {
        let (since, until, confidence, granularity) = self.resolve(expression)?;
        Some(TemporalRange {
            since,
            until,
            confidence,
            granularity,
            expression: expression.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temporal::TemporalResolver;

    #[test]
    fn test_heuristic_today() {
        let result = RULE_BASED_RESOLVER.resolve("今天");
        assert!(result.is_some());
        let (_, _, conf, gran) = result.unwrap();
        assert!(conf >= 0.9);
        assert_eq!(gran, Granularity::ExactDay);
    }

    #[test]
    fn test_heuristic_last_week() {
        let result = RULE_BASED_RESOLVER.resolve("上周");
        assert!(result.is_some());
        let (_, _, conf, gran) = result.unwrap();
        assert!(conf >= 0.8);
        assert_eq!(gran, Granularity::Week);
    }

    #[test]
    fn test_heuristic_last_month() {
        let result = RULE_BASED_RESOLVER.resolve("上个月");
        assert!(result.is_some());
        let (_, _, conf, gran) = result.unwrap();
        assert!(conf >= 0.85);
        assert_eq!(gran, Granularity::Month);
    }

    #[test]
    fn test_heuristic_fuzzy_days_ago() {
        let result = RULE_BASED_RESOLVER.resolve("几天前");
        assert!(result.is_some());
        let (_, _, conf, gran) = result.unwrap();
        assert!(conf < 0.7);
        assert_eq!(gran, Granularity::Fuzzy);
    }

    #[test]
    fn test_unknown_expression() {
        let result = RULE_BASED_RESOLVER.resolve("当我还是个孩子的时候");
        assert!(result.is_none());
    }

    #[test]
    fn test_yesterday() {
        let result = RULE_BASED_RESOLVER.resolve("昨天");
        assert!(result.is_some());
        let (_, _, _, gran) = result.unwrap();
        assert_eq!(gran, Granularity::ExactDay);
    }

    #[test]
    fn test_this_year() {
        let result = RULE_BASED_RESOLVER.resolve("今年");
        assert!(result.is_some());
        let (_, _, _, gran) = result.unwrap();
        assert_eq!(gran, Granularity::Year);
    }

    #[test]
    fn test_granularity_propagated_through_trait() {
        let resolver: &dyn TemporalResolver = &RULE_BASED_RESOLVER;
        let range = resolver.resolve("上周", None).expect("should resolve");
        assert_eq!(range.granularity, Granularity::Week);

        let range = resolver.resolve("昨天", None).expect("should resolve");
        assert_eq!(range.granularity, Granularity::ExactDay);

        let range = resolver.resolve("最近", None).expect("should resolve");
        assert_eq!(range.granularity, Granularity::Fuzzy);
    }

    // ─── Helper function tests ──────────────────────────────────────────────

    #[test]
    fn test_start_of_week_monday() {
        // 2026-05-11 is a Monday
        let monday = NaiveDate::from_ymd_opt(2026, 5, 11).unwrap();
        assert_eq!(start_of_week(&monday), monday);
    }

    #[test]
    fn test_start_of_week_sunday() {
        // 2026-05-10 is a Sunday — start_of_week should be Mon 2026-05-04
        let sunday = NaiveDate::from_ymd_opt(2026, 5, 10).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 5, 4).unwrap();
        assert_eq!(start_of_week(&sunday), expected);
    }

    #[test]
    fn test_start_of_month_mid_month() {
        let date = NaiveDate::from_ymd_opt(2026, 5, 15).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
        assert_eq!(start_of_month(&date), expected);
    }

    #[test]
    fn test_start_of_quarter_q1() {
        // February is in Q1
        let date = NaiveDate::from_ymd_opt(2026, 2, 10).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(start_of_quarter(&date), expected);
    }

    #[test]
    fn test_start_of_quarter_q2() {
        // May is in Q2
        let date = NaiveDate::from_ymd_opt(2026, 5, 10).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        assert_eq!(start_of_quarter(&date), expected);
    }

    #[test]
    fn test_start_of_quarter_q3() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
        assert_eq!(start_of_quarter(&date), expected);
    }

    #[test]
    fn test_start_of_quarter_q4() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 30).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        assert_eq!(start_of_quarter(&date), expected);
    }

    // ─── Exact date resolution via resolve_heuristic ────────────────────────

    #[test]
    fn test_resolve_today_exact_dates() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, conf, gran) = resolve_heuristic("今天", &reference).unwrap();
        assert_eq!(since, reference);
        assert_eq!(until, reference);
        assert!((conf - 0.95).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::ExactDay);
    }

    #[test]
    fn test_resolve_yesterday_exact_dates() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, _) = resolve_heuristic("昨天", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 5, 11).unwrap();
        assert_eq!(since, expected);
        assert_eq!(until, expected);
    }

    #[test]
    fn test_resolve_day_before_yesterday() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, gran) = resolve_heuristic("前天", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 5, 10).unwrap();
        assert_eq!(since, expected);
        assert_eq!(until, expected);
        assert_eq!(gran, Granularity::ExactDay);
    }

    #[test]
    fn test_resolve_tomorrow() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, gran) = resolve_heuristic("明天", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        assert_eq!(since, expected);
        assert_eq!(until, expected);
        assert_eq!(gran, Granularity::ExactDay);
    }

    #[test]
    fn test_resolve_day_after_tomorrow() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, conf, gran) = resolve_heuristic("后天", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 5, 14).unwrap();
        assert_eq!(since, expected);
        assert_eq!(until, expected);
        assert!((conf - 0.90).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::ExactDay);
    }

    #[test]
    fn test_resolve_few_days_ago_range() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, _) = resolve_heuristic("几天前", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2026, 5, 5).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, reference);
    }

    #[test]
    fn test_resolve_recently_range() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, gran) = resolve_heuristic("最近", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2026, 4, 12).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, reference);
        assert_eq!(gran, Granularity::Fuzzy);
    }

    #[test]
    fn test_resolve_this_morning() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, conf, gran) = resolve_heuristic("今早", &reference).unwrap();
        assert_eq!(since, reference);
        assert_eq!(until, reference);
        assert!((conf - 0.85).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::ExactDay);
    }

    #[test]
    fn test_resolve_this_evening() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, gran) = resolve_heuristic("今晚", &reference).unwrap();
        assert_eq!(since, reference);
        assert_eq!(until, reference);
        assert_eq!(gran, Granularity::ExactDay);
    }

    // ─── Week patterns ──────────────────────────────────────────────────────

    #[test]
    fn test_resolve_last_week_dates() {
        // 2026-05-12 is a Tuesday; Monday of this week is 2026-05-11
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, gran) = resolve_heuristic("上周", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2026, 5, 4).unwrap(); // Mon
        let expected_until = NaiveDate::from_ymd_opt(2026, 5, 10).unwrap(); // Sun
        assert_eq!(since, expected_since);
        assert_eq!(until, expected_until);
        assert_eq!(gran, Granularity::Week);
    }

    #[test]
    fn test_resolve_week_before_last() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, conf, gran) = resolve_heuristic("上上周", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();
        let expected_until = NaiveDate::from_ymd_opt(2026, 5, 3).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, expected_until);
        assert!((conf - 0.80).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::Week);
    }

    // ─── Month patterns ─────────────────────────────────────────────────────

    #[test]
    fn test_resolve_this_month() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, conf, gran) = resolve_heuristic("本月", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, reference);
        assert!((conf - 0.90).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::Month);
    }

    #[test]
    fn test_resolve_last_month_dates() {
        // May reference → last month is April
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, _) = resolve_heuristic("上个月", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        let expected_until = NaiveDate::from_ymd_opt(2026, 4, 30).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, expected_until);
    }

    #[test]
    fn test_resolve_last_month_january_wraps_year() {
        // January reference → last month is December of previous year
        let reference = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let (since, until, _, _) = resolve_heuristic("上个月", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2025, 12, 1).unwrap();
        let expected_until = NaiveDate::from_ymd_opt(2025, 12, 31).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, expected_until);
    }

    #[test]
    fn test_resolve_two_months_ago() {
        // May reference → two months ago is March
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, conf, gran) = resolve_heuristic("两个月前", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let expected_until = NaiveDate::from_ymd_opt(2026, 4, 30).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, expected_until);
        assert!((conf - 0.80).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::Month);
    }

    #[test]
    fn test_resolve_two_months_ago_january_wraps_year() {
        // January reference → two months ago is November of previous year
        let reference = NaiveDate::from_ymd_opt(2026, 1, 10).unwrap();
        let (since, until, _, _) = resolve_heuristic("两个月前", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2025, 11, 1).unwrap();
        let expected_until = NaiveDate::from_ymd_opt(2025, 12, 31).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, expected_until);
    }

    #[test]
    fn test_resolve_two_months_ago_february_wraps_year() {
        // February reference → two months ago is December of previous year
        let reference = NaiveDate::from_ymd_opt(2026, 2, 15).unwrap();
        let (since, until, _, _) = resolve_heuristic("两个月前", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2025, 12, 1).unwrap();
        let expected_until = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, expected_until);
    }

    // ─── Quarter patterns ───────────────────────────────────────────────────

    #[test]
    fn test_resolve_this_quarter() {
        // May is in Q2 (Apr–Jun)
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, conf, gran) = resolve_heuristic("本季度", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, reference);
        assert!((conf - 0.90).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::Quarter);
    }

    #[test]
    fn test_resolve_last_quarter_from_q2() {
        // May (Q2) → last quarter is Q1 (Jan–Mar)
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, conf, gran) = resolve_heuristic("上季度", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let expected_until = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, expected_until);
        assert!((conf - 0.85).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::Quarter);
    }

    #[test]
    fn test_resolve_last_quarter_from_q1_wraps_year() {
        // February (Q1) → last quarter is Q4 of previous year (Oct–Dec)
        let reference = NaiveDate::from_ymd_opt(2026, 2, 10).unwrap();
        let (since, until, _, _) = resolve_heuristic("上季度", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2025, 10, 1).unwrap();
        let expected_until = NaiveDate::from_ymd_opt(2025, 12, 31).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, expected_until);
    }

    // ─── Year patterns ──────────────────────────────────────────────────────

    #[test]
    fn test_resolve_last_year() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, conf, gran) = resolve_heuristic("去年", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let expected_until = NaiveDate::from_ymd_opt(2025, 12, 31).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, expected_until);
        assert!((conf - 0.95).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::Year);
    }

    #[test]
    fn test_resolve_this_year_dates() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, _) = resolve_heuristic("今年", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, reference);
    }

    // ─── Era patterns ───────────────────────────────────────────────────────

    #[test]
    fn test_resolve_long_ago() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, conf, gran) = resolve_heuristic("很久以前", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2025, 5, 12).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, reference);
        assert!((conf - 0.30).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::Fuzzy);
    }

    // ─── English alternatives ───────────────────────────────────────────────

    #[test]
    fn test_resolve_english_today() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, gran) = resolve_heuristic("today", &reference).unwrap();
        assert_eq!(since, reference);
        assert_eq!(until, reference);
        assert_eq!(gran, Granularity::ExactDay);
    }

    #[test]
    fn test_resolve_english_yesterday() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, _) = resolve_heuristic("yesterday", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 5, 11).unwrap();
        assert_eq!(since, expected);
        assert_eq!(until, expected);
    }

    #[test]
    fn test_resolve_english_tomorrow() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, _) = resolve_heuristic("tomorrow", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        assert_eq!(since, expected);
        assert_eq!(until, expected);
    }

    #[test]
    fn test_resolve_english_recently() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (_, _, conf, gran) = resolve_heuristic("recently", &reference).unwrap();
        assert!((conf - 0.50).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::Fuzzy);
    }

    #[test]
    fn test_resolve_english_last_week() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (_, _, _, gran) = resolve_heuristic("last week", &reference).unwrap();
        assert_eq!(gran, Granularity::Week);
    }

    #[test]
    fn test_resolve_english_this_month() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, _, _, gran) = resolve_heuristic("this month", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
        assert_eq!(since, expected);
        assert_eq!(gran, Granularity::Month);
    }

    #[test]
    fn test_resolve_english_last_month() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, _, _, _) = resolve_heuristic("last month", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        assert_eq!(since, expected);
    }

    #[test]
    fn test_resolve_english_this_quarter() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, _, _, gran) = resolve_heuristic("this quarter", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        assert_eq!(since, expected);
        assert_eq!(gran, Granularity::Quarter);
    }

    #[test]
    fn test_resolve_english_last_quarter() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (_, _, _, gran) = resolve_heuristic("last quarter", &reference).unwrap();
        assert_eq!(gran, Granularity::Quarter);
    }

    #[test]
    fn test_resolve_english_last_year() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, gran) = resolve_heuristic("last year", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let expected_until = NaiveDate::from_ymd_opt(2025, 12, 31).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, expected_until);
        assert_eq!(gran, Granularity::Year);
    }

    #[test]
    fn test_resolve_english_this_year() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, _, _, _) = resolve_heuristic("this year", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(since, expected);
    }

    #[test]
    fn test_resolve_english_long_ago() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (_, _, conf, gran) = resolve_heuristic("long ago", &reference).unwrap();
        assert!((conf - 0.30).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::Fuzzy);
    }

    #[test]
    fn test_resolve_english_this_morning() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, gran) = resolve_heuristic("this morning", &reference).unwrap();
        assert_eq!(since, reference);
        assert_eq!(until, reference);
        assert_eq!(gran, Granularity::ExactDay);
    }

    #[test]
    fn test_resolve_english_tonight() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, gran) = resolve_heuristic("tonight", &reference).unwrap();
        assert_eq!(since, reference);
        assert_eq!(until, reference);
        assert_eq!(gran, Granularity::ExactDay);
    }

    // ─── Chinese alternatives (今日/本日/昨日/明日/上月) ─────────────────────

    #[test]
    fn test_resolve_jinri() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, _) = resolve_heuristic("今日", &reference).unwrap();
        assert_eq!(since, reference);
        assert_eq!(until, reference);
    }

    #[test]
    fn test_resolve_benri() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, _) = resolve_heuristic("本日", &reference).unwrap();
        assert_eq!(since, reference);
        assert_eq!(until, reference);
    }

    #[test]
    fn test_resolve_zuori() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, _) = resolve_heuristic("昨日", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 5, 11).unwrap();
        assert_eq!(since, expected);
        assert_eq!(until, expected);
    }

    #[test]
    fn test_resolve_mingri() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, _) = resolve_heuristic("明日", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        assert_eq!(since, expected);
        assert_eq!(until, expected);
    }

    #[test]
    fn test_resolve_shang_yue() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, _, _, gran) = resolve_heuristic("上月", &reference).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        assert_eq!(since, expected);
        assert_eq!(gran, Granularity::Month);
    }

    #[test]
    fn test_resolve_qiantian_alternative() {
        // "前几天" alternative pattern
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (since, until, _, gran) = resolve_heuristic("前几天", &reference).unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2026, 5, 5).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, reference);
        assert_eq!(gran, Granularity::Fuzzy);
    }

    // ─── Substring scan path ────────────────────────────────────────────────

    #[test]
    fn test_substring_scan_matches() {
        // "看看前几天的情况" contains "前几天" as substring
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let result = resolve_heuristic("看看前几天的情况", &reference);
        assert!(result.is_some());
        let (since, until, conf, gran) = result.unwrap();
        let expected_since = NaiveDate::from_ymd_opt(2026, 5, 5).unwrap();
        assert_eq!(since, expected_since);
        assert_eq!(until, reference);
        // Substring match uses reduced confidence (0.9x) and Fuzzy granularity
        assert!((conf - 0.60 * 0.9).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::Fuzzy);
    }

    #[test]
    fn test_substring_scan_longer_expression() {
        // "查询最近的记录" contains "最近" as substring
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let result = resolve_heuristic("查询最近的记录", &reference);
        assert!(result.is_some());
        let (_, _, conf, gran) = result.unwrap();
        // Substring match: confidence * 0.9, granularity forced to Fuzzy
        assert!((conf - 0.50 * 0.9).abs() < f32::EPSILON);
        assert_eq!(gran, Granularity::Fuzzy);
    }

    // ─── Case insensitivity and trimming ────────────────────────────────────

    #[test]
    fn test_case_insensitive_matching() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let (_, _, _, gran) = resolve_heuristic("TODAY", &reference).unwrap();
        assert_eq!(gran, Granularity::ExactDay);
    }

    #[test]
    fn test_whitespace_trimming() {
        let reference = NaiveDate::from_ymd_opt(2026, 5, 12).unwrap();
        let result = resolve_heuristic("  today  ", &reference);
        assert!(result.is_some());
    }

    // ─── HeuristicTemporalResolver construction ─────────────────────────────

    #[test]
    fn test_heuristic_resolver_new() {
        let resolver = HeuristicTemporalResolver::new();
        let result = resolver.resolve("今天");
        assert!(result.is_some());
    }

    #[test]
    fn test_heuristic_resolver_default() {
        let resolver = HeuristicTemporalResolver::default();
        let result = resolver.resolve("今天");
        assert!(result.is_some());
    }

    #[test]
    fn test_resolver_returns_timestamps() {
        let resolver = HeuristicTemporalResolver::new();
        let (since, until, conf, gran) = resolver.resolve("昨天").unwrap();
        // since should be start of day, until should be end of day
        assert!(since < until);
        assert!(conf > 0.0);
        assert_eq!(gran, Granularity::ExactDay);
        // Verify since < until (since=00:00:00, until=23:59:59)
        assert!(until - since > 0);
    }

    // ─── Granularity Debug/Clone/PartialEq ──────────────────────────────────

    #[test]
    fn test_granularity_debug_format() {
        assert_eq!(format!("{:?}", Granularity::ExactDay), "ExactDay");
        assert_eq!(format!("{:?}", Granularity::Week), "Week");
        assert_eq!(format!("{:?}", Granularity::Month), "Month");
        assert_eq!(format!("{:?}", Granularity::Quarter), "Quarter");
        assert_eq!(format!("{:?}", Granularity::HalfYear), "HalfYear");
        assert_eq!(format!("{:?}", Granularity::Year), "Year");
        assert_eq!(format!("{:?}", Granularity::Fuzzy), "Fuzzy");
    }

    #[test]
    fn test_granularity_clone() {
        let g = Granularity::Week;
        let g2 = g;
        assert_eq!(g, g2);
    }

    // ─── TemporalRule struct ────────────────────────────────────────────────

    #[test]
    fn test_temporal_rule_debug() {
        let rule = TemporalRule {
            patterns: &["test"],
            confidence: 0.5,
            granularity: Granularity::Fuzzy,
        };
        let debug = format!("{:?}", rule);
        assert!(debug.contains("TemporalRule"));
        assert!(debug.contains("test"));
    }

    #[test]
    fn test_temporal_rule_clone() {
        let rule = TemporalRule {
            patterns: &["test"],
            confidence: 0.5,
            granularity: Granularity::Fuzzy,
        };
        let cloned = rule.clone();
        assert_eq!(cloned.confidence, 0.5);
        assert_eq!(cloned.granularity, Granularity::Fuzzy);
    }

    // ─── build_rule_map ─────────────────────────────────────────────────────

    #[test]
    fn test_build_rule_map_covers_all_patterns() {
        let map = build_rule_map();
        // Spot-check key patterns exist
        assert!(map.contains_key("今天"));
        assert!(map.contains_key("today"));
        assert!(map.contains_key("yesterday"));
        assert!(map.contains_key("tomorrow"));
        assert!(map.contains_key("最近"));
        assert!(map.contains_key("recently"));
        assert!(map.contains_key("this morning"));
        assert!(map.contains_key("last week"));
        assert!(map.contains_key("this month"));
        assert!(map.contains_key("this quarter"));
        assert!(map.contains_key("this year"));
    }

    #[test]
    fn test_rule_map_synonyms_point_to_same_rule() {
        let map = build_rule_map();
        // "今天", "today", "今日", "本日" should all map to the same rule
        let idx1 = map.get("今天").unwrap();
        let idx2 = map.get("today").unwrap();
        let idx3 = map.get("今日").unwrap();
        let idx4 = map.get("本日").unwrap();
        assert_eq!(idx1, idx2);
        assert_eq!(idx1, idx3);
        assert_eq!(idx1, idx4);
    }
}
