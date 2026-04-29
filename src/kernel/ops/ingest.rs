//! Ingest Pipeline — LLM-assisted fact extraction at memory storage time.
//!
//! When raw conversational text is stored, the pipeline:
//! 1. Extracts atomic facts (who, what, when)
//! 2. Detects preferences (implicit and explicit)
//! 3. Parses temporal references into structured dates
//! 4. Extracts named entities for KG linking
//!
//! Each extracted fact becomes a separate memory entry with typed tags,
//! enabling precise retrieval that raw passage-level storage cannot match.

use crate::llm::{ChatMessage, ChatOptions, LlmProvider};
use crate::memory::layered::MemoryType;

/// A structured fact extracted from raw text by the ingest pipeline.
#[derive(Debug, Clone)]
pub struct ExtractedFact {
    pub text: String,
    pub fact_type: FactType,
    pub entities: Vec<String>,
    pub tags: Vec<String>,
    pub temporal_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FactType {
    Fact,
    Preference,
    Event,
    Procedure,
}

impl FactType {
    pub fn to_memory_type(&self) -> MemoryType {
        match self {
            FactType::Fact => MemoryType::Semantic,
            FactType::Preference => MemoryType::Semantic,
            FactType::Event => MemoryType::Episodic,
            FactType::Procedure => MemoryType::Procedural,
        }
    }

    pub fn tag(&self) -> &'static str {
        match self {
            FactType::Fact => "fact",
            FactType::Preference => "preference",
            FactType::Event => "event",
            FactType::Procedure => "procedure",
        }
    }
}

const FACT_EXTRACTION_PROMPT: &str = r#"Extract atomic facts from the text below. For each fact, output one line in this exact format:
TYPE|ENTITIES|FACT_TEXT

TYPE must be one of: FACT, PREFERENCE, EVENT, PROCEDURE
ENTITIES is a comma-separated list of key entities mentioned (people, tools, projects, etc.)
FACT_TEXT is a concise, self-contained statement of the fact.

Rules:
- Each line = one atomic fact. Do not combine multiple facts.
- PREFERENCE: any stated or implied preference, opinion, or choice (e.g. "prefers X", "likes Y", "finds Z better")
- EVENT: anything with a time reference (explicit or implied)
- PROCEDURE: how-to knowledge, steps, workflows
- FACT: everything else (definitions, states, relationships)
- Keep FACT_TEXT short (one sentence). Preserve the original meaning.
- Extract ALL facts, even implicit ones.
- If the text mentions a preference indirectly (e.g. "I find X more reliable"), classify as PREFERENCE.

Text:
{text}

Output (one fact per line, no other text):"#;

/// Run LLM-based fact extraction on raw text.
/// Returns a list of structured facts, or falls back to a single passthrough fact on error.
pub fn extract_facts(llm: &dyn LlmProvider, text: &str) -> Vec<ExtractedFact> {
    if text.trim().len() < 10 {
        return vec![passthrough_fact(text)];
    }

    let prompt = FACT_EXTRACTION_PROMPT.replace("{text}", text);
    let opts = ChatOptions { temperature: 0.0, max_tokens: Some(500) };
    let msgs = [
        ChatMessage::system("You are a fact extraction engine. Output only the requested format."),
        ChatMessage::user(prompt),
    ];

    match llm.chat(&msgs, &opts) {
        Ok((response, _in_tok, _out_tok)) => {
            let facts = parse_extraction_response(&response, text);
            if facts.is_empty() {
                vec![passthrough_fact(text)]
            } else {
                facts
            }
        }
        Err(e) => {
            tracing::warn!("fact extraction LLM call failed: {e}, using passthrough");
            vec![passthrough_fact(text)]
        }
    }
}

fn passthrough_fact(text: &str) -> ExtractedFact {
    let entities = extract_entities_regex(text);
    let temporal = extract_temporal_hint(text);
    ExtractedFact {
        text: text.to_string(),
        fact_type: FactType::Fact,
        entities,
        tags: vec!["raw".to_string()],
        temporal_hint: temporal,
    }
}

fn parse_extraction_response(response: &str, _original: &str) -> Vec<ExtractedFact> {
    let mut facts = Vec::new();

    for line in response.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("Output") {
            continue;
        }

        let parts: Vec<&str> = line.splitn(3, '|').collect();
        if parts.len() < 3 {
            continue;
        }

        let fact_type = match parts[0].trim().to_uppercase().as_str() {
            "FACT" => FactType::Fact,
            "PREFERENCE" => FactType::Preference,
            "EVENT" => FactType::Event,
            "PROCEDURE" => FactType::Procedure,
            _ => FactType::Fact,
        };

        let entities: Vec<String> = parts[1]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let fact_text = parts[2].trim().to_string();
        if fact_text.is_empty() {
            continue;
        }

        let temporal_hint = extract_temporal_hint(&fact_text);
        let mut tags = vec![fact_type.tag().to_string()];
        for entity in &entities {
            tags.push(format!("entity:{}", entity.to_lowercase()));
        }
        if temporal_hint.is_some() {
            tags.push("has_temporal".to_string());
        }

        facts.push(ExtractedFact {
            text: fact_text,
            fact_type,
            entities,
            tags,
            temporal_hint,
        });
    }

    facts
}

/// Regex-based preference extraction patterns (MemPalace-inspired fallback).
/// Generates synthetic preference documents from implicit preference expressions.
pub fn extract_preference_signals(text: &str) -> Vec<ExtractedFact> {
    let patterns: &[(&str, &str)] = &[
        ("I prefer ", "User prefers: "),
        ("I usually prefer ", "User usually prefers: "),
        ("I always use ", "User always uses: "),
        ("I like ", "User likes: "),
        ("I love ", "User loves: "),
        ("I enjoy ", "User enjoys: "),
        ("I find ", "User finds: "),
        ("I don't like ", "User dislikes: "),
        ("I hate ", "User dislikes: "),
        ("I never use ", "User avoids: "),
        ("I tend to ", "User tends to: "),
        ("my favorite ", "User's favorite: "),
        ("my preferred ", "User's preferred: "),
        ("I'm a fan of ", "User is a fan of: "),
        ("I switched to ", "User switched to: "),
        ("I recommend ", "User recommends: "),
    ];

    let lower = text.to_lowercase();
    let mut results = Vec::new();

    for (pattern, prefix) in patterns {
        if let Some(pos) = lower.find(&pattern.to_lowercase()) {
            let start = pos + pattern.len();
            let rest = &text[start..];
            let end = rest.find(['.', '!', '?', ',', '\n']).unwrap_or(rest.len());
            let value = rest[..end].trim();
            if !value.is_empty() {
                let synthetic = format!("{prefix}{value}");
                let entities = extract_entities_regex(value);
                let mut tags = vec![
                    "preference".to_string(),
                    "synthetic".to_string(),
                ];
                for e in &entities {
                    tags.push(format!("entity:{}", e.to_lowercase()));
                }
                results.push(ExtractedFact {
                    text: synthetic,
                    fact_type: FactType::Preference,
                    entities,
                    tags,
                    temporal_hint: None,
                });
            }
        }
    }

    results
}

/// Simple entity extraction using capitalization and common patterns.
pub fn extract_entities_regex(text: &str) -> Vec<String> {
    let mut entities = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for word in text.split_whitespace() {
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric());
        if clean.len() < 2 { continue; }

        let first_char = clean.chars().next().unwrap();
        if first_char.is_uppercase() && clean.len() > 1 {
            let skip_words = ["The", "This", "That", "What", "When", "Where",
                "How", "Why", "Who", "Which", "And", "But", "For", "Not",
                "Are", "Was", "Were", "Has", "Have", "Had", "Can", "Could",
                "Will", "Would", "Should", "May", "Must", "Its", "Our"];
            if !skip_words.contains(&clean) {
                let lower = clean.to_lowercase();
                if seen.insert(lower) {
                    entities.push(clean.to_string());
                }
            }
        }
    }

    entities
}

/// Extract temporal hints from text using simple pattern matching.
fn extract_temporal_hint(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let temporal_patterns = [
        "yesterday", "today", "tomorrow", "last week", "this week", "next week",
        "last month", "this month", "next month", "last year", "ago",
        "recently", "just now", "earlier", "later", "morning", "evening",
        "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday",
        "january", "february", "march", "april", "may", "june",
        "july", "august", "september", "october", "november", "december",
        "昨天", "今天", "明天", "上周", "本周", "下周", "上个月", "本月", "下个月",
        "去年", "今年", "明年", "前天", "最近", "刚才",
    ];

    for pattern in &temporal_patterns {
        if lower.contains(pattern) {
            return Some(pattern.to_string());
        }
    }

    // Check for date-like patterns (YYYY-MM-DD, MM/DD, etc.)
    let has_date = text.chars().any(|c| c.is_ascii_digit())
        && (text.contains('/') || text.contains('-'))
        && text.len() > 5;
    if has_date {
        return Some("date_reference".to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_extraction_response() {
        let response = "FACT|PostgreSQL,Production|PostgreSQL is the primary database for the project\n\
                        PREFERENCE|Postgres|User finds Postgres more reliable\n\
                        EVENT|Alice,Login|Alice fixed the login bug yesterday";
        let facts = parse_extraction_response(response, "original");
        assert_eq!(facts.len(), 3);
        assert_eq!(facts[0].fact_type, FactType::Fact);
        assert_eq!(facts[1].fact_type, FactType::Preference);
        assert_eq!(facts[2].fact_type, FactType::Event);
        assert!(facts[2].temporal_hint.is_some());
    }

    #[test]
    fn test_preference_extraction_regex() {
        let text = "I usually prefer PostgreSQL for production databases because I find it more reliable.";
        let prefs = extract_preference_signals(text);
        assert!(!prefs.is_empty());
        assert!(prefs[0].text.contains("prefer"));
    }

    #[test]
    fn test_entity_extraction() {
        let text = "Alice and Bob discussed the PostgreSQL migration in the React frontend.";
        let entities = extract_entities_regex(text);
        assert!(entities.contains(&"Alice".to_string()));
        assert!(entities.contains(&"Bob".to_string()));
        assert!(entities.contains(&"PostgreSQL".to_string()));
        assert!(entities.contains(&"React".to_string()));
    }

    #[test]
    fn test_temporal_hint_detection() {
        assert!(extract_temporal_hint("We discussed this yesterday").is_some());
        assert!(extract_temporal_hint("The meeting is next week").is_some());
        assert!(extract_temporal_hint("Updated on 2026-04-15").is_some());
        assert!(extract_temporal_hint("Just a random statement").is_none());
    }

    #[test]
    fn test_passthrough_on_short_text() {
        struct DummyLlm;
        impl LlmProvider for DummyLlm {
            fn chat(&self, _: &[ChatMessage], _: &ChatOptions) -> Result<(String, u32, u32), crate::llm::LlmError> {
                Ok(("".into(), 0, 0))
            }
            fn model_name(&self) -> &str { "dummy" }
        }
        let facts = extract_facts(&DummyLlm, "hi");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].text, "hi");
    }

    #[test]
    fn test_preference_regex_implicit() {
        let text = "I find dark mode easier on the eyes for late-night coding";
        let prefs = extract_preference_signals(text);
        assert!(!prefs.is_empty());
        assert!(prefs[0].text.to_lowercase().contains("dark mode"));
    }

    #[test]
    fn test_preference_regex_negative() {
        let text = "I don't like using Windows for development";
        let prefs = extract_preference_signals(text);
        assert!(!prefs.is_empty());
        assert!(prefs[0].fact_type == FactType::Preference);
        assert!(prefs[0].text.contains("dislikes"));
    }
}
