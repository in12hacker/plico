//! Adaptive Retrieval Router — intent-aware query routing.
//!
//! Classifies query intent and routes to the optimal retrieval strategy.
//! LLM-first when available, rule-based fallback otherwise.
//!
//! Strategies:
//! - FACTUAL: dense HNSW+BM25 (fastest)
//! - TEMPORAL: temporal KG path + time-decay boost
//! - MULTI_HOP: KG PPR + dense hybrid
//! - PREFERENCE: Semantic-type top-k
//! - AGGREGATION: broad recall + dedup merge

use serde::{Deserialize, Serialize};

/// Query intent classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueryIntent {
    Factual,
    Temporal,
    MultiHop,
    Preference,
    Aggregation,
}

impl QueryIntent {
    pub fn name(&self) -> &'static str {
        match self {
            QueryIntent::Factual => "factual",
            QueryIntent::Temporal => "temporal",
            QueryIntent::MultiHop => "multi_hop",
            QueryIntent::Preference => "preference",
            QueryIntent::Aggregation => "aggregation",
        }
    }
}

impl std::fmt::Display for QueryIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Result of intent classification with confidence.
#[derive(Debug, Clone)]
pub struct ClassifiedIntent {
    pub intent: QueryIntent,
    pub confidence: f32,
    pub method: ClassificationMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassificationMethod {
    Llm,
    RuleBased,
}

/// Classify query intent using keyword rules (fallback strategy).
pub fn classify_by_rules(query: &str) -> ClassifiedIntent {
    let q = query.to_lowercase();

    if is_temporal_query_rule(&q) {
        return ClassifiedIntent {
            intent: QueryIntent::Temporal,
            confidence: 0.7,
            method: ClassificationMethod::RuleBased,
        };
    }

    if is_multi_hop_query_rule(&q) {
        return ClassifiedIntent {
            intent: QueryIntent::MultiHop,
            confidence: 0.6,
            method: ClassificationMethod::RuleBased,
        };
    }

    if is_preference_query_rule(&q) {
        return ClassifiedIntent {
            intent: QueryIntent::Preference,
            confidence: 0.7,
            method: ClassificationMethod::RuleBased,
        };
    }

    if is_aggregation_query_rule(&q) {
        return ClassifiedIntent {
            intent: QueryIntent::Aggregation,
            confidence: 0.6,
            method: ClassificationMethod::RuleBased,
        };
    }

    ClassifiedIntent {
        intent: QueryIntent::Factual,
        confidence: 0.5,
        method: ClassificationMethod::RuleBased,
    }
}

/// Classify query intent using an LLM (preferred strategy).
///
/// Returns None if the LLM response cannot be parsed, allowing
/// the caller to fall back to rule-based classification.
pub fn classify_by_llm_response(llm_response: &str) -> Option<ClassifiedIntent> {
    let resp = llm_response.trim().to_lowercase();

    let intent = if resp.contains("temporal") {
        QueryIntent::Temporal
    } else if resp.contains("multi_hop") || resp.contains("multi-hop") || resp.contains("multihop") {
        QueryIntent::MultiHop
    } else if resp.contains("preference") {
        QueryIntent::Preference
    } else if resp.contains("aggregation") {
        QueryIntent::Aggregation
    } else if resp.contains("factual") {
        QueryIntent::Factual
    } else {
        return None;
    };

    Some(ClassifiedIntent {
        intent,
        confidence: 0.85,
        method: ClassificationMethod::Llm,
    })
}

/// Build the LLM prompt for intent classification.
pub fn intent_classification_prompt(query: &str) -> String {
    format!(
        "Classify the following query into exactly ONE category. \
         Output ONLY the category name, nothing else.\n\n\
         Categories:\n\
         - factual: looking up a single known fact or number (\"What is X?\", \"How many Y per day?\", \"Who did Z?\")\n\
         - temporal: time-related queries (\"When did\", \"before\", \"after\", \"last week\")\n\
         - multi_hop: requires connecting multiple pieces of information (\"Why did X cause Y?\")\n\
         - preference: about preferences/opinions (\"What does user prefer?\", \"favorite\")\n\
         - aggregation: requires listing or summarizing MULTIPLE distinct items (\"List all X\", \"Summarize all Y\")\n\n\
         Query: {query}\n\n\
         Category:"
    )
}

/// Per-intent retrieval configuration.
///
/// `top_k` values calibrated against MemMachine ablation study (2026):
/// k=30 is the single most impactful parameter (+4.2%), k=50 degrades.
/// `use_reranker` is intent-routed: disabled for multi-session/temporal
/// queries where cross-encoder reduces diversity (wakamex/longmem finding).
#[derive(Debug, Clone)]
pub struct RetrievalConfig {
    pub top_k: usize,
    pub use_kg: bool,
    pub use_ppr: bool,
    pub use_bm25: bool,
    pub use_vector: bool,
    pub time_decay_boost: bool,
    pub typed_retrieval: Option<crate::memory::MemoryType>,
    pub bm25_weight: f32,
    pub vector_weight: f32,
    pub use_reranker: bool,
}

impl RetrievalConfig {
    pub fn for_intent(intent: QueryIntent) -> Self {
        match intent {
            QueryIntent::Factual => Self {
                top_k: 20,
                use_kg: false,
                use_ppr: false,
                use_bm25: true,
                use_vector: true,
                time_decay_boost: false,
                typed_retrieval: None,
                bm25_weight: 1.0,
                vector_weight: 1.0,
                use_reranker: true,
            },
            QueryIntent::Temporal => Self {
                top_k: 25,
                use_kg: true,
                use_ppr: false,
                use_bm25: true,
                use_vector: true,
                time_decay_boost: true,
                typed_retrieval: Some(crate::memory::MemoryType::Episodic),
                bm25_weight: 0.8,
                vector_weight: 1.2,
                use_reranker: false,
            },
            QueryIntent::MultiHop => Self {
                top_k: 30,
                use_kg: true,
                use_ppr: true,
                use_bm25: true,
                use_vector: true,
                time_decay_boost: false,
                typed_retrieval: None,
                bm25_weight: 0.7,
                vector_weight: 1.3,
                use_reranker: false,
            },
            QueryIntent::Preference => Self {
                top_k: 20,
                use_kg: false,
                use_ppr: false,
                use_bm25: true,
                use_vector: true,
                time_decay_boost: false,
                typed_retrieval: Some(crate::memory::MemoryType::Semantic),
                bm25_weight: 1.0,
                vector_weight: 1.0,
                use_reranker: true,
            },
            QueryIntent::Aggregation => Self {
                top_k: 30,
                use_kg: true,
                use_ppr: false,
                use_bm25: true,
                use_vector: true,
                time_decay_boost: false,
                typed_retrieval: None,
                bm25_weight: 1.2,
                vector_weight: 0.8,
                use_reranker: false,
            },
        }
    }
}

fn is_temporal_query_rule(q: &str) -> bool {
    let temporal_keywords = [
        "when", "what time", "before", "after", "last week", "yesterday",
        "last month", "last year", "ago", "since", "until", "during",
        "recently", "earlier", "later", "previous", "next",
        "之前", "之后", "上周", "昨天", "上个月", "去年", "最近",
        "以前", "以后", "期间", "何时",
    ];
    temporal_keywords.iter().any(|kw| q.contains(kw))
}

fn is_multi_hop_query_rule(q: &str) -> bool {
    let multi_hop_keywords = [
        "why", "because", "caused", "led to", "result of", "consequence",
        "how did", "what happened after", "relationship between",
        "connected to", "related to",
        "为什么", "因为", "导致", "关系", "原因",
    ];
    multi_hop_keywords.iter().any(|kw| q.contains(kw))
}

fn is_preference_query_rule(q: &str) -> bool {
    let pref_keywords = [
        "prefer", "like", "favorite", "always", "usually", "tend to",
        "habit", "opinion", "taste",
        "喜欢", "偏好", "习惯", "总是", "通常",
    ];
    pref_keywords.iter().any(|kw| q.contains(kw))
}

fn is_aggregation_query_rule(q: &str) -> bool {
    let agg_keywords = [
        "list all", "how many", "summarize", "total", "count",
        "overview", "all the", "everything",
        "列出", "多少", "总结", "所有", "汇总",
    ];
    agg_keywords.iter().any(|kw| q.contains(kw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_temporal_queries() {
        let cases = [
            "When did the meeting happen?",
            "What happened before the deployment?",
            "Show me logs from last week",
            "Events since yesterday",
        ];
        for q in cases {
            let result = classify_by_rules(q);
            assert_eq!(result.intent, QueryIntent::Temporal, "failed for: {q}");
        }
    }

    #[test]
    fn test_classify_multi_hop_queries() {
        let cases = [
            "Why did the server crash?",
            "What caused the regression?",
            "How did the deployment lead to the outage?",
        ];
        for q in cases {
            let result = classify_by_rules(q);
            assert_eq!(result.intent, QueryIntent::MultiHop, "failed for: {q}");
        }
    }

    #[test]
    fn test_classify_preference_queries() {
        let cases = [
            "What does the user prefer for formatting?",
            "What's my favorite programming language?",
            "Do I usually use tabs or spaces?",
        ];
        for q in cases {
            let result = classify_by_rules(q);
            assert_eq!(result.intent, QueryIntent::Preference, "failed for: {q}");
        }
    }

    #[test]
    fn test_classify_aggregation_queries() {
        let cases = [
            "List all open bugs",
            "How many sessions have we had?",
            "Summarize the project status",
        ];
        for q in cases {
            let result = classify_by_rules(q);
            assert_eq!(result.intent, QueryIntent::Aggregation, "failed for: {q}");
        }
    }

    #[test]
    fn test_classify_factual_default() {
        let cases = [
            "What is the capital of France?",
            "Who is the CEO?",
            "What degree did I graduate with?",
        ];
        for q in cases {
            let result = classify_by_rules(q);
            assert_eq!(result.intent, QueryIntent::Factual, "failed for: {q}");
        }
    }

    #[test]
    fn test_classify_chinese_temporal() {
        let result = classify_by_rules("昨天的会议讨论了什么？");
        assert_eq!(result.intent, QueryIntent::Temporal);
    }

    #[test]
    fn test_classify_chinese_preference() {
        let result = classify_by_rules("用户喜欢什么编程语言？");
        assert_eq!(result.intent, QueryIntent::Preference);
    }

    #[test]
    fn test_llm_response_parsing() {
        assert_eq!(classify_by_llm_response("factual").unwrap().intent, QueryIntent::Factual);
        assert_eq!(classify_by_llm_response("TEMPORAL").unwrap().intent, QueryIntent::Temporal);
        assert_eq!(classify_by_llm_response("multi_hop").unwrap().intent, QueryIntent::MultiHop);
        assert_eq!(classify_by_llm_response("preference").unwrap().intent, QueryIntent::Preference);
        assert_eq!(classify_by_llm_response("aggregation").unwrap().intent, QueryIntent::Aggregation);
        assert!(classify_by_llm_response("unknown_category").is_none());
    }

    #[test]
    fn test_retrieval_config_factual() {
        let config = RetrievalConfig::for_intent(QueryIntent::Factual);
        assert!(!config.use_kg);
        assert!(!config.use_ppr);
        assert!(config.use_bm25);
        assert!(config.use_vector);
        assert_eq!(config.top_k, 20);
        assert!(config.use_reranker);
    }

    #[test]
    fn test_retrieval_config_multi_hop_uses_ppr() {
        let config = RetrievalConfig::for_intent(QueryIntent::MultiHop);
        assert!(config.use_kg);
        assert!(config.use_ppr);
        assert_eq!(config.top_k, 30);
        assert!(!config.use_reranker);
    }

    #[test]
    fn test_retrieval_config_preference_uses_semantic_type() {
        let config = RetrievalConfig::for_intent(QueryIntent::Preference);
        assert_eq!(config.typed_retrieval, Some(crate::memory::MemoryType::Semantic));
    }

    #[test]
    fn test_retrieval_config_temporal_uses_episodic_type() {
        let config = RetrievalConfig::for_intent(QueryIntent::Temporal);
        assert!(config.time_decay_boost);
        assert_eq!(config.typed_retrieval, Some(crate::memory::MemoryType::Episodic));
    }

    #[test]
    fn test_intent_classification_prompt_contains_query() {
        let prompt = intent_classification_prompt("When did X happen?");
        assert!(prompt.contains("When did X happen?"));
        assert!(prompt.contains("factual"));
        assert!(prompt.contains("temporal"));
    }
}
