//! Heuristic Intent Router — keyword/pattern matching for common operations.
//!
//! No external dependency — always available. Handles the most common
//! NL patterns in both English and Chinese, delegating temporal phrases
//! to the existing TemporalResolver.

use crate::api::semantic::ApiRequest;
use crate::temporal::resolve_heuristic;
use super::{IntentRouter, ResolvedIntent, IntentError, RoutingAction};

pub struct HeuristicRouter;

impl Default for HeuristicRouter {
    fn default() -> Self {
        Self
    }
}

impl HeuristicRouter {
    pub fn new() -> Self {
        Self
    }
}

struct PatternMatch {
    action_type: ActionType,
    confidence: f32,
    query_text: String,
    tags: Vec<String>,
    temporal_text: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum ActionType {
    Search,
    Create,
    Delete,
    Update,
    Remember,
    Recall,
    ListAgents,
    RegisterAgent,
    ListTools,
    Explore,
}

fn classify(text: &str) -> Option<PatternMatch> {
    let lower = text.to_lowercase();
    let trimmed = text.trim();

    // Register agent patterns
    if matches_any(&lower, &["register agent", "register a new agent", "create agent",
        "注册代理", "注册智能体", "创建代理", "创建智能体"]) {
        let name = extract_after_patterns(trimmed, &["named ", "called ", "名为", "叫做"])
            .unwrap_or_else(|| "unnamed".to_string());
        return Some(PatternMatch {
            action_type: ActionType::RegisterAgent,
            confidence: 0.95,
            query_text: name,
            tags: vec![],
            temporal_text: None,
        });
    }

    // List agents patterns
    if matches_any(&lower, &["list agents", "show agents", "list all agents",
        "列出代理", "显示代理", "列出智能体"]) {
        return Some(PatternMatch {
            action_type: ActionType::ListAgents,
            confidence: 0.95,
            query_text: String::new(),
            tags: vec![],
            temporal_text: None,
        });
    }

    // List tools patterns
    if matches_any(&lower, &["list tools", "show tools", "what tools",
        "列出工具", "显示工具", "有哪些工具"]) {
        return Some(PatternMatch {
            action_type: ActionType::ListTools,
            confidence: 0.95,
            query_text: String::new(),
            tags: vec![],
            temporal_text: None,
        });
    }

    // Remember patterns
    if starts_with_any(&lower, &["remember ", "记住", "记忆"]) {
        let content = strip_prefixes(trimmed, &["remember ", "remember that ",
            "记住", "记忆"]);
        return Some(PatternMatch {
            action_type: ActionType::Remember,
            confidence: 0.9,
            query_text: content,
            tags: vec![],
            temporal_text: None,
        });
    }

    // Recall patterns
    if matches_any(&lower, &["recall", "recall memories", "what do i remember",
        "回忆", "想起", "记忆回顾"]) {
        return Some(PatternMatch {
            action_type: ActionType::Recall,
            confidence: 0.9,
            query_text: String::new(),
            tags: vec![],
            temporal_text: None,
        });
    }

    // Delete patterns (must come before search since "remove" could be ambiguous)
    if starts_with_any(&lower, &["delete ", "remove ", "删除", "移除"]) {
        let target = strip_prefixes(trimmed, &["delete ", "remove ", "删除", "移除"]);
        return Some(PatternMatch {
            action_type: ActionType::Delete,
            confidence: 0.85,
            query_text: target,
            tags: vec![],
            temporal_text: None,
        });
    }

    // Create/store patterns
    if starts_with_any(&lower, &["store ", "save ", "create ", "put ",
        "保存", "存储", "创建", "存入"]) {
        let content = strip_prefixes(trimmed, &["store ", "save ", "create ", "put ",
            "保存", "存储", "创建", "存入"]);
        let tags = extract_tag_hints(&lower);
        return Some(PatternMatch {
            action_type: ActionType::Create,
            confidence: 0.85,
            query_text: content,
            tags,
            temporal_text: None,
        });
    }

    // Update patterns
    if starts_with_any(&lower, &["update ", "modify ", "change ", "更新", "修改"]) {
        let content = strip_prefixes(trimmed, &["update ", "modify ", "change ",
            "更新", "修改"]);
        return Some(PatternMatch {
            action_type: ActionType::Update,
            confidence: 0.8,
            query_text: content,
            tags: vec![],
            temporal_text: None,
        });
    }

    // Explore/graph patterns
    if starts_with_any(&lower, &["explore ", "graph of ", "neighbors of ",
        "探索", "图谱"]) {
        let target = strip_prefixes(trimmed, &["explore ", "graph of ",
            "neighbors of ", "探索", "图谱"]);
        return Some(PatternMatch {
            action_type: ActionType::Explore,
            confidence: 0.85,
            query_text: target,
            tags: vec![],
            temporal_text: None,
        });
    }

    // Search patterns (broadest — match last)
    if starts_with_any(&lower, &["find ", "search ", "look for ", "query ",
        "查找", "搜索", "找", "搜", "查询"]) ||
        contains_any(&lower, &["find ", "search for ", "look for ",
            "查找", "搜索"])
    {
        let query = strip_prefixes(trimmed, &[
            "find ", "search for ", "search ", "look for ", "query ",
            "查找", "搜索", "找", "搜", "查询",
        ]);
        let tags = extract_tag_hints(&lower);
        let temporal = extract_temporal_hint(&lower);
        return Some(PatternMatch {
            action_type: ActionType::Search,
            confidence: 0.85,
            query_text: query,
            tags,
            temporal_text: temporal,
        });
    }

    None
}

fn to_api_request(m: PatternMatch, agent_id: &str) -> ResolvedIntent {
    let routing_action = RoutingAction::SingleAction;
    let (action, explanation) = match m.action_type {
        ActionType::Search => {
            let (since, until) = resolve_temporal_bounds(m.temporal_text.as_deref());
            let action = ApiRequest::Search {
                query: m.query_text.clone(),
                agent_id: agent_id.to_string(),
                limit: Some(10),
                offset: None,
                require_tags: m.tags.clone(),
                exclude_tags: vec![],
                since,
                until,
            };
            let time_note = if since.is_some() || until.is_some() {
                " with time filter"
            } else {
                ""
            };
            (action, format!("Search for '{}'{}", m.query_text, time_note))
        }
        ActionType::Create => {
            let action = ApiRequest::Create {
                content: m.query_text.clone(),
                content_encoding: Default::default(),
                tags: m.tags.clone(),
                agent_id: agent_id.to_string(),
                intent: None,
            };
            (action, format!("Create object with content '{}'", truncate(&m.query_text, 50)))
        }
        ActionType::Delete => {
            (ApiRequest::Delete {
                cid: m.query_text.clone(),
                agent_id: agent_id.to_string(),
            }, format!("Delete object '{}'", m.query_text))
        }
        ActionType::Update => {
            (ApiRequest::Update {
                cid: String::new(),
                content: m.query_text.clone(),
                content_encoding: Default::default(),
                new_tags: None,
                agent_id: agent_id.to_string(),
            }, format!("Update object with '{}'", truncate(&m.query_text, 50)))
        }
        ActionType::Remember => {
            (ApiRequest::Remember {
                agent_id: agent_id.to_string(),
                content: m.query_text.clone(),
            }, format!("Remember '{}'", truncate(&m.query_text, 50)))
        }
        ActionType::Recall => {
            (ApiRequest::Recall {
                agent_id: agent_id.to_string(),
            }, "Recall all memories".to_string())
        }
        ActionType::ListAgents => {
            (ApiRequest::ListAgents, "List all agents".to_string())
        }
        ActionType::RegisterAgent => {
            (ApiRequest::RegisterAgent {
                name: m.query_text.clone(),
            }, format!("Register agent '{}'", m.query_text))
        }
        ActionType::ListTools => {
            (ApiRequest::ToolList {
                agent_id: agent_id.to_string(),
            }, "List available tools".to_string())
        }
        ActionType::Explore => {
            (ApiRequest::Explore {
                cid: m.query_text.clone(),
                edge_type: None,
                depth: Some(2),
                agent_id: agent_id.to_string(),
            }, format!("Explore graph around '{}'", m.query_text))
        }
    };
    ResolvedIntent {
        routing_action,
        confidence: m.confidence,
        action,
        explanation,
    }
}

impl IntentRouter for HeuristicRouter {
    fn resolve(&self, text: &str, agent_id: &str) -> Result<Vec<ResolvedIntent>, IntentError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(IntentError::Unresolvable("empty input".to_string()));
        }

        // Multi-intent detection: split on compound connectors.
        let parts = split_compound(trimmed);
        if parts.len() > 1 {
            // Compound query → MultiAction
            let intents: Vec<ResolvedIntent> = parts
                .iter()
                .filter_map(|part| {
                    let part = part.trim();
                    if part.is_empty() {
                        return None;
                    }
                    match classify(part) {
                        Some(m) => {
                            let mut ri = to_api_request(m, agent_id);
                            ri.routing_action = RoutingAction::SingleAction;
                            Some(ri)
                        }
                        None => {
                            // Fallback search for this part
                            let temporal = extract_temporal_hint(&part.to_lowercase());
                            let (since, until) = resolve_temporal_bounds(temporal.as_deref());
                            let action = ApiRequest::Search {
                                query: part.to_string(),
                                agent_id: agent_id.to_string(),
                                limit: Some(10),
                                offset: None,
                                require_tags: vec![],
                                exclude_tags: vec![],
                                since,
                                until,
                            };
                            Some(ResolvedIntent {
                                routing_action: RoutingAction::LowConfidence,
                                confidence: 0.3,
                                action,
                                explanation: format!("Fallback search for '{}'", truncate(part, 50)),
                            })
                        }
                    }
                })
                .collect();
            if intents.len() == 1 {
                return Ok(intents);
            }
            // Mark all as MultiAction
            let intents: Vec<ResolvedIntent> = intents
                .into_iter()
                .map(|mut ri| {
                    ri.routing_action = RoutingAction::MultiAction;
                    ri
                })
                .collect();
            return Ok(intents);
        }

        // Single intent
        match classify(trimmed) {
            Some(m) => Ok(vec![to_api_request(m, agent_id)]),
            None => {
                // Low-confidence fallback: treat as search query
                let temporal = extract_temporal_hint(&trimmed.to_lowercase());
                let (since, until) = resolve_temporal_bounds(temporal.as_deref());
                let action = ApiRequest::Search {
                    query: trimmed.to_string(),
                    agent_id: agent_id.to_string(),
                    limit: Some(10),
                    offset: None,
                    require_tags: vec![],
                    exclude_tags: vec![],
                    since,
                    until,
                };
                Ok(vec![ResolvedIntent {
                    routing_action: RoutingAction::LowConfidence,
                    confidence: 0.3,
                    action,
                    explanation: format!("Fallback search for '{}'", truncate(trimmed, 50)),
                }])
            }
        }
    }
}

// ─── Helper Functions ───────────────────────────────────────────────

/// Split a compound query into parts.
/// Connectors: " and ", " also ", " then ", " and then "
fn split_compound(text: &str) -> Vec<String> {
    let separators = [" and then ", " then ", " also ", " and "];
    let lower = text.to_lowercase();

    // Find the earliest separator
    let mut earliest: Option<(usize, &str)> = None;
    for sep in &separators {
        if let Some(pos) = lower.find(sep) {
            match earliest {
                None => earliest = Some((pos, sep)),
                Some((epos, _)) if pos < epos => earliest = Some((pos, sep)),
                _ => {}
            }
        }
    }

    match earliest {
        Some((pos, sep)) => {
            let mut parts = Vec::new();
            let before = text[..pos].trim();
            let after = text[pos + sep.len()..].trim();
            if !before.is_empty() {
                parts.push(before.to_string());
            }
            if !after.is_empty() {
                // Recursively split the rest
                parts.extend(split_compound(after));
            }
            if parts.is_empty() {
                parts.push(text.to_string());
            }
            parts
        }
        None => vec![text.to_string()],
    }
}

fn matches_any(text: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| text.contains(p))
}

fn starts_with_any(text: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| text.starts_with(p))
}

fn contains_any(text: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| text.contains(p))
}

fn strip_prefixes(text: &str, prefixes: &[&str]) -> String {
    let lower = text.to_lowercase();
    for prefix in prefixes {
        if lower.starts_with(prefix) {
            return text[prefix.len()..].trim().to_string();
        }
    }
    text.trim().to_string()
}

fn extract_after_patterns(text: &str, patterns: &[&str]) -> Option<String> {
    let lower = text.to_lowercase();
    for pattern in patterns {
        if let Some(pos) = lower.find(pattern) {
            let after = &text[pos + pattern.len()..];
            let name = after.trim().trim_matches('"').trim_matches('\'').to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

fn extract_tag_hints(lower: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let tag_patterns = ["tagged ", "tagged with ", "with tag ", "with tags ",
        "标签", "标记"];
    for pattern in &tag_patterns {
        if let Some(pos) = lower.find(pattern) {
            let after = &lower[pos + pattern.len()..];
            for tag in after.split([',', ' ', '、']) {
                let t = tag.trim().trim_matches('"').trim_matches('\'');
                if !t.is_empty() && t.len() < 50 {
                    tags.push(t.to_string());
                }
            }
            break;
        }
    }
    tags
}

fn extract_temporal_hint(lower: &str) -> Option<String> {
    let temporal_patterns = [
        "from last week", "last week", "from yesterday", "yesterday",
        "from last month", "last month", "this week", "today",
        "from a few days ago", "few days ago", "recently",
        "几天前", "上周", "上个月", "昨天", "今天", "这周", "最近",
        "前几天", "上周末",
    ];
    for pattern in &temporal_patterns {
        if lower.contains(pattern) {
            return Some(pattern.to_string());
        }
    }
    None
}

fn resolve_temporal_bounds(temporal_text: Option<&str>) -> (Option<i64>, Option<i64>) {
    let Some(text) = temporal_text else {
        return (None, None);
    };
    let today = chrono::Local::now().date_naive();
    match resolve_heuristic(text, &today) {
        Some((start_date, end_date, _confidence, _granularity)) => {
            let start_ms = start_date
                .and_hms_opt(0, 0, 0)
                .map(|dt| dt.and_utc().timestamp_millis());
            let end_ms = end_date
                .and_hms_opt(23, 59, 59)
                .map(|dt| dt.and_utc().timestamp_millis());
            (start_ms, end_ms)
        }
        None => (None, None),
    }
}

fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len { s } else { &s[..max_len] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_english() {
        let router = HeuristicRouter::new();
        let results = router.resolve("find documents about scheduling", "agent1").unwrap();
        assert!(!results.is_empty());
        assert!(results[0].confidence >= 0.8);
        assert!(results[0].explanation.contains("Search"));
    }

    #[test]
    fn test_search_chinese() {
        let router = HeuristicRouter::new();
        let results = router.resolve("搜索关于调度的文档", "agent1").unwrap();
        assert!(!results.is_empty());
        assert!(results[0].confidence >= 0.8);
    }

    #[test]
    fn test_create() {
        let router = HeuristicRouter::new();
        let results = router.resolve("store this meeting summary", "agent1").unwrap();
        assert!(!results.is_empty());
        if let ApiRequest::Create { content, .. } = &results[0].action {
            assert!(content.contains("meeting summary"));
        } else {
            panic!("Expected Create");
        }
    }

    #[test]
    fn test_remember() {
        let router = HeuristicRouter::new();
        let results = router.resolve("remember that the API uses JSON format", "agent1").unwrap();
        assert!(!results.is_empty());
        if let ApiRequest::Remember { content, .. } = &results[0].action {
            assert!(content.contains("API"));
        } else {
            panic!("Expected Remember");
        }
    }

    #[test]
    fn test_list_agents() {
        let router = HeuristicRouter::new();
        let results = router.resolve("list agents", "agent1").unwrap();
        assert!(!results.is_empty());
        assert!(matches!(results[0].action, ApiRequest::ListAgents));
    }

    #[test]
    fn test_list_tools() {
        let router = HeuristicRouter::new();
        let results = router.resolve("what tools are available", "agent1").unwrap();
        assert!(!results.is_empty());
        assert!(matches!(results[0].action, ApiRequest::ToolList { .. }));
    }

    #[test]
    fn test_temporal_search() {
        let router = HeuristicRouter::new();
        let results = router.resolve("find reports from last week", "agent1").unwrap();
        assert!(!results.is_empty());
        if let ApiRequest::Search { since, until, .. } = &results[0].action {
            assert!(since.is_some(), "temporal should resolve to since");
            assert!(until.is_some(), "temporal should resolve to until");
        } else {
            panic!("Expected Search");
        }
    }

    #[test]
    fn test_fallback_low_confidence() {
        let router = HeuristicRouter::new();
        let results = router.resolve("something ambiguous here", "agent1").unwrap();
        assert!(!results.is_empty());
        assert!(results[0].confidence < 0.5);
    }

    #[test]
    fn test_register_agent() {
        let router = HeuristicRouter::new();
        let results = router.resolve("register agent named data-processor", "agent1").unwrap();
        assert!(!results.is_empty());
        if let ApiRequest::RegisterAgent { name } = &results[0].action {
            assert_eq!(name, "data-processor");
        } else {
            panic!("Expected RegisterAgent");
        }
    }

    #[test]
    fn test_delete() {
        let router = HeuristicRouter::new();
        let results = router.resolve("delete abc123", "agent1").unwrap();
        assert!(!results.is_empty());
        if let ApiRequest::Delete { cid, .. } = &results[0].action {
            assert_eq!(cid, "abc123");
        } else {
            panic!("Expected Delete");
        }
    }

    #[test]
    fn test_split_compound_simple() {
        let parts = split_compound("search docs and create ticket");
        assert_eq!(parts.len(), 2);
        assert!(parts[0].contains("search docs"));
        assert!(parts[1].contains("create ticket"));
    }

    #[test]
    fn test_split_compound_three_way() {
        let parts = split_compound("search docs and create ticket and send message");
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn test_split_compound_no_split() {
        let parts = split_compound("search docs");
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn test_multi_intent_compound_query() {
        let router = HeuristicRouter::new();
        let results = router.resolve("search docs and create ticket", "agent1").unwrap();
        assert!(results.len() == 2, "compound query should produce 2 intents, got {}", results.len());
        // When multiple intents detected, routing_action = MultiAction
        assert_eq!(results[0].routing_action, RoutingAction::MultiAction);
        assert_eq!(results[1].routing_action, RoutingAction::MultiAction);
    }

    #[test]
    fn test_single_intent_has_single_action_routing() {
        let router = HeuristicRouter::new();
        let results = router.resolve("search docs", "agent1").unwrap();
        assert!(results.len() == 1);
        assert_eq!(results[0].routing_action, RoutingAction::SingleAction);
    }
}
