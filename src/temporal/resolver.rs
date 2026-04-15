//! TemporalResolver — trait + implementations
//!
//! The trait is intentionally small so it can be implemented for testing
//! (stub), for fast heuristic-only resolution, or for LLM-powered resolution.

use crate::temporal::rules::Granularity;
use chrono::TimeZone;

/// A resolved time range with confidence metadata.
#[derive(Debug, Clone)]
pub struct TemporalRange {
    /// Lower bound (inclusive), Unix milliseconds.
    pub since: i64,
    /// Upper bound (inclusive), Unix milliseconds.
    pub until: i64,
    /// Confidence in the resolution [0, 1].
    pub confidence: f32,
    /// Resolved granularity.
    pub granularity: Granularity,
    /// Original expression (echoed back).
    pub expression: String,
}

impl TemporalRange {
    /// Expand the range symmetrically by `days` in both directions.
    /// Used for medium-confidence resolutions.
    #[allow(dead_code)]
    pub fn expanded(&self, days: i64) -> Self {
        let day_ms = days * 86_400_000;
        Self {
            since: self.since.saturating_sub(day_ms),
            until: self.until.saturating_add(day_ms),
            confidence: self.confidence,
            granularity: Granularity::Fuzzy,
            expression: self.expression.clone(),
        }
    }
}

/// Resolves natural-language time expressions to concrete time ranges.
///
/// Implementations range from fast rule-based (no LLM) to full LLM-powered
/// (handles novel expressions the rules don't cover).
pub trait TemporalResolver: Send + Sync {
    /// Resolve a time expression.
    ///
    /// `reference_ms` — Unix milliseconds of the reference date (default: now).
    /// Returns `None` if resolution fails (expression not understood).
    fn resolve(&self, expression: &str, reference_ms: Option<i64>) -> Option<TemporalRange>;
}

// ─── Ollama-powered resolver ─────────────────────────────────────────────────

/// LLM-powered resolver using a local Ollama daemon.
///
/// Falls back to the heuristic rules for common expressions (faster, no LLM call).
/// Only uses the LLM for novel expressions the rules don't cover.
pub struct OllamaTemporalResolver {
    url: String,
    model: String,
    /// Cache to avoid repeated LLM calls for the same expression.
    cache: std::sync::RwLock<lru::LruCache<String, TemporalRange>>,
}

impl OllamaTemporalResolver {
    pub fn new(url: &str, model: &str) -> std::io::Result<Self> {
        Ok(Self {
            url: url.to_string(),
            model: model.to_string(),
            cache: std::sync::RwLock::new(lru::LruCache::new(256)),
        })
    }

    fn resolve_with_llm(&self, expression: &str, reference_ms: i64) -> Option<TemporalRange> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .ok()?;

        let prompt = format!(
            r#"You are a time expression parser. Convert natural-language time expressions into a JSON object.
Input: "{expression}"
Reference date (Unix ms): {reference_ms}
Output ONLY valid JSON with no explanation:
{{"since_unix_ms": <number>, "until_unix_ms": <number>, "confidence": <0.0-1.0>, "granularity": "exactday|week|month|quarter|year|fuzzy", "reasoning": "<brief explanation in Chinese or English>"}}

Rules:
- "几天前"/"前几天" = approximately last 7 days, confidence ~0.6
- "上周" = previous calendar week (Monday to Sunday), confidence ~0.85
- "上个月" = previous calendar month, confidence ~0.9
- "最近" = last 30 days, confidence ~0.5
- If the expression is ambiguous or in the future, set confidence < 0.5
"#
        );

        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false,
            "options": {
                "temperature": 0.1,
                "num_predict": 128
            }
        });

        let resp = client
            .post(format!("{}/api/generate", self.url))
            .json(&body)
            .send()
            .ok()?;

        let raw: serde_json::Value = resp.json().ok()?;

        let text = raw.get("response")?.as_str()?;
        // Extract JSON object from the response (model might prepend text)
        let json_start = text.find('{')?;
        let json_end = text.rfind('}').map(|i| i + 1).unwrap_or(text.len());
        let json_str = &text[json_start..json_end];

        let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;

        let since = parsed.get("since_unix_ms")?.as_i64()?;
        let until = parsed.get("until_unix_ms")?.as_i64()?;
        let confidence = parsed.get("confidence")?.as_f64()? as f32;
        let gran_str = parsed.get("granularity")?.as_str()?;
        let granularity = match gran_str {
            "exactday" => Granularity::ExactDay,
            "week" => Granularity::Week,
            "month" => Granularity::Month,
            "quarter" => Granularity::Quarter,
            "year" => Granularity::Year,
            _ => Granularity::Fuzzy,
        };

        Some(TemporalRange {
            since,
            until,
            confidence,
            granularity,
            expression: expression.to_string(),
        })
    }
}

impl TemporalResolver for OllamaTemporalResolver {
    fn resolve(&self, expression: &str, reference_ms: Option<i64>) -> Option<TemporalRange> {
        let ref_ms = reference_ms.unwrap_or_else(|| {
            chrono::Local::now().timestamp_millis()
        });

        // Check cache first
        let expr_owned = expression.to_string();
        {
            let mut cache = self.cache.write().unwrap();
            if let Some(cached) = cache.get_mut(&expr_owned) {
                return Some(cached.clone());
            }
        }

        // Try heuristic first (fast path, no LLM)
        let reference = chrono::Utc.timestamp_millis_opt(ref_ms)
            .single()?
            .date_naive();

        if let Some((since, until, confidence, granularity)) =
            crate::temporal::rules::resolve_heuristic(expression, &reference)
        {
            let range = TemporalRange {
                since: since.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis(),
                until: until.and_hms_opt(23, 59, 59)?.and_utc().timestamp_millis(),
                confidence,
                granularity,
                expression: expression.to_string(),
            };
            // Cache even heuristic results
            let mut cache = self.cache.write().unwrap();
            cache.put(expr_owned.clone(), range.clone());
            return Some(range);
        }

        // Fallback: LLM
        let range = self.resolve_with_llm(expression, ref_ms)?;

        // Cache
        let mut cache = self.cache.write().unwrap();
        cache.put(expr_owned.clone(), range.clone());
        Some(range)
    }
}

// ─── Stub resolver (for testing / stub embedding) ───────────────────────────

/// Stub resolver that always returns `None` — forces pure semantic search.
pub struct StubTemporalResolver;

impl TemporalResolver for StubTemporalResolver {
    fn resolve(&self, _: &str, _: Option<i64>) -> Option<TemporalRange> {
        None
    }
}
