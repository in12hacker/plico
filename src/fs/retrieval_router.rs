//! Query intent classification and the retrieval options currently consumed by
//! memory recall. LLM classification is used when available, with rule-based
//! classification as the local fallback.

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
         - temporal: queries ABOUT TIME or DATES (\"When did X happen?\", \"How many days between X and Y?\", \"Which event happened first?\")\n\
         - multi_hop: requires connecting multiple pieces of information from different sources (\"Why did X cause Y?\", \"What is the relationship between X and Y?\")\n\
         - preference: asking for recommendations, suggestions, opinions, or personal preferences (\"Can you recommend X?\", \"Suggest some Y\", \"What does user prefer?\", \"favorite\", \"What should I Z?\")\n\
         - aggregation: requires counting or listing MULTIPLE distinct items across many entries (\"List all X\", \"How many total Y?\", \"Give me an overview of all Z\")\n\n\
         IMPORTANT: \"Can you recommend/suggest X?\" is PREFERENCE (asking for personalized advice), \
         NOT aggregation (counting items). \"How many X in total?\" is AGGREGATION.\n\n\
         Query: {query}\n\n\
         Category:"
    )
}

/// Per-intent options that are applied by the current memory recall path.
#[derive(Debug, Clone)]
pub struct RetrievalConfig {
    pub top_k: usize,
    pub typed_retrieval: Option<crate::memory::MemoryType>,
    pub use_reranker: bool,
}

impl RetrievalConfig {
    pub fn for_intent(intent: QueryIntent) -> Self {
        // Allow top_k override via env var for ablation experiments
        let top_k_override: Option<usize> = std::env::var("PLICO_TOP_K").ok().and_then(|v| v.parse().ok());
        let mut config = match intent {
            QueryIntent::Factual => Self {
                top_k: 15,
                typed_retrieval: None,
                use_reranker: true,
            },
            QueryIntent::Temporal => Self {
                top_k: 15,
                typed_retrieval: Some(crate::memory::MemoryType::Episodic),
                use_reranker: true,
            },
            QueryIntent::MultiHop => Self {
                top_k: 30,
                typed_retrieval: None,
                use_reranker: true,
            },
            QueryIntent::Preference => Self {
                top_k: 15,
                typed_retrieval: Some(crate::memory::MemoryType::Semantic),
                use_reranker: true,
            },
            QueryIntent::Aggregation => Self {
                top_k: 15,
                typed_retrieval: None,
                use_reranker: true,
            },
        };
        // Apply top_k override if set
        if let Some(k) = top_k_override {
            config.top_k = k;
        }
        config
    }
}

fn is_temporal_query_rule(q: &str) -> bool {
    // Strong temporal signals — "before"/"after" removed (too ambiguous,
    // e.g. "Where did X move to after Y?" is factual, not temporal)
    let temporal_keywords = [
        "when",
        "what time",
        "last week",
        "yesterday",
        "last month",
        "last year",
        "ago",
        "since",
        "until",
        "during",
        "recently",
        "earlier",
        "later",
        "previous",
        "next",
        "how many days",
        "how many weeks",
        "how many months",
        "which happened first",
        "in order",
        "之前",
        "之后",
        "上周",
        "昨天",
        "上个月",
        "去年",
        "最近",
        "以前",
        "以后",
        "期间",
        "何时",
    ];
    temporal_keywords.iter().any(|kw| q.contains(kw))
}

fn is_multi_hop_query_rule(q: &str) -> bool {
    let multi_hop_keywords = [
        "why",
        "because",
        "caused",
        "led to",
        "result of",
        "consequence",
        "how did",
        "what happened after",
        "relationship between",
        "connected to",
        "related to",
        "为什么",
        "因为",
        "导致",
        "关系",
        "原因",
    ];
    multi_hop_keywords.iter().any(|kw| q.contains(kw))
}

fn is_preference_query_rule(q: &str) -> bool {
    let pref_keywords = [
        "prefer",
        "like",
        "favorite",
        "always",
        "usually",
        "tend to",
        "habit",
        "opinion",
        "taste",
        "recommend",
        "suggest",
        "should i",
        "what would you",
        "best way to",
        "any tips",
        "any ideas",
        "喜欢",
        "偏好",
        "习惯",
        "总是",
        "通常",
        "推荐",
        "建议",
    ];
    pref_keywords.iter().any(|kw| q.contains(kw))
}

fn is_aggregation_query_rule(q: &str) -> bool {
    let agg_keywords = [
        "list all",
        "how many",
        "summarize",
        "total",
        "count",
        "overview",
        "all the",
        "everything",
        "列出",
        "多少",
        "总结",
        "所有",
        "汇总",
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
            "How many days passed between event A and event B?",
            "Show me logs from last week",
            "Events since yesterday",
            "Which happened first, the wedding or the trip?",
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
        assert_eq!(
            classify_by_llm_response("factual").unwrap().intent,
            QueryIntent::Factual
        );
        assert_eq!(
            classify_by_llm_response("TEMPORAL").unwrap().intent,
            QueryIntent::Temporal
        );
        assert_eq!(
            classify_by_llm_response("multi_hop").unwrap().intent,
            QueryIntent::MultiHop
        );
        assert_eq!(
            classify_by_llm_response("preference").unwrap().intent,
            QueryIntent::Preference
        );
        assert_eq!(
            classify_by_llm_response("aggregation").unwrap().intent,
            QueryIntent::Aggregation
        );
        assert!(classify_by_llm_response("unknown_category").is_none());
    }

    #[test]
    fn test_retrieval_config_factual() {
        let config = RetrievalConfig::for_intent(QueryIntent::Factual);
        assert_eq!(config.top_k, 15);
        assert!(config.use_reranker);
    }

    #[test]
    fn test_retrieval_config_multi_hop_uses_broader_top_k() {
        let config = RetrievalConfig::for_intent(QueryIntent::MultiHop);
        assert_eq!(config.top_k, 30);
        assert!(config.use_reranker);
    }

    #[test]
    fn test_retrieval_config_preference_uses_semantic_type() {
        let config = RetrievalConfig::for_intent(QueryIntent::Preference);
        assert_eq!(config.typed_retrieval, Some(crate::memory::MemoryType::Semantic));
    }

    #[test]
    fn test_retrieval_config_temporal_uses_episodic_type() {
        let config = RetrievalConfig::for_intent(QueryIntent::Temporal);
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
