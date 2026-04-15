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
    NaiveDate::from_ymd_opt(reference.year(), q_month as u32, 1).unwrap()
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
                let since = NaiveDate::from_ymd_opt(prev_y, prev_m as u32, 1).unwrap();
                let until = start_of_month(reference) - Duration::days(1);
                Some((since, until))
            }
            "两个月前" => {
                let (y, m) = (reference.year(), reference.month());
                let (prev_y, prev_m) = if m <= 2 { (y - 1, m + 10) } else { (y, m - 2) };
                let since = NaiveDate::from_ymd_opt(prev_y, prev_m as u32, 1).unwrap();
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
    /// Returns `(since_unix_ms, until_unix_ms, confidence)` or `None`
    /// if the expression doesn't match any rule.
    pub fn resolve(&self, expression: &str) -> Option<(i64, i64, f32)> {
        let reference = chrono::Local::now().date_naive();
        let (since, until, confidence, _) = resolve_heuristic(expression, &reference)?;
        Some((
            since.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis(),
            until.and_hms_opt(23, 59, 59)?.and_utc().timestamp_millis(),
            confidence,
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
        let (since, until, confidence) = self.resolve(expression)?;
        Some(TemporalRange {
            since,
            until,
            confidence,
            granularity: Granularity::Fuzzy,
            expression: expression.to_string(),
        })
    }
}

/// Convert Unix milliseconds to (year, month, day) for debugging.
#[allow(dead_code)]
pub fn ms_to_ymd(ms: i64) -> (i32, u32, u32) {
    use chrono::{TimeZone, Utc};
    let dt = Utc.timestamp_millis_opt(ms).single().unwrap_or_default();
    (dt.date_naive().year(), dt.date_naive().month(), dt.date_naive().day())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heuristic_today() {
        let result = RULE_BASED_RESOLVER.resolve("今天");
        assert!(result.is_some());
        let (_, _, conf) = result.unwrap();
        assert!(conf >= 0.9);
    }

    #[test]
    fn test_heuristic_last_week() {
        let result = RULE_BASED_RESOLVER.resolve("上周");
        assert!(result.is_some());
        let (_, _, conf) = result.unwrap();
        assert!(conf >= 0.8);
    }

    #[test]
    fn test_heuristic_last_month() {
        let result = RULE_BASED_RESOLVER.resolve("上个月");
        assert!(result.is_some());
        let (_, _, conf) = result.unwrap();
        assert!(conf >= 0.85);
    }

    #[test]
    fn test_heuristic_fuzzy_days_ago() {
        let result = RULE_BASED_RESOLVER.resolve("几天前");
        assert!(result.is_some());
        let (_, _, conf) = result.unwrap();
        assert!(conf < 0.7);
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
    }

    #[test]
    fn test_this_year() {
        let result = RULE_BASED_RESOLVER.resolve("今年");
        assert!(result.is_some());
    }
}
