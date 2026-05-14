//! Query Decomposition — breaks multi-hop queries into single-hop sub-queries.
//!
//! For multi-hop questions like "Why did X cause Y?", decomposition produces:
//! - Sub-query 1: "What happened to X?" (find X's context)
//! - Sub-query 2: "What connects X to Y?" (find the causal link)
//!
//! Strategy: rule-based pattern matching (no LLM dependency).
//! LLM decomposition can be layered on top via `decompose_with_llm()`.

/// A decomposed multi-hop query.
#[derive(Debug, Clone)]
pub struct DecomposedQuery {
    /// The original query.
    pub original: String,
    /// Sub-queries to execute in order.
    pub sub_queries: Vec<SubQuery>,
    /// Intermediate entities extracted from the query.
    pub entities: Vec<String>,
}

/// A single sub-query within a decomposition.
#[derive(Debug, Clone)]
pub struct SubQuery {
    /// The sub-query text.
    pub query: String,
    /// Which hop this is (0-indexed).
    pub hop: usize,
    /// Role of this sub-query in the reasoning chain.
    pub role: SubQueryRole,
}

/// Role of a sub-query in multi-hop reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubQueryRole {
    /// Find information about an entity.
    EntityLookup,
    /// Find a connection/relationship between entities.
    RelationFind,
    /// Find causal explanation.
    CausalExplain,
    /// Find temporal sequence.
    TemporalSequence,
}

/// Decompose a query into sub-queries using rule-based patterns.
///
/// Returns None if the query is not multi-hop (single-hop factual queries
/// should go through normal retrieval).
pub fn decompose(query: &str) -> Option<DecomposedQuery> {
    let q = query.trim();
    let q_lower = q.to_lowercase();

    // Extract entities (capitalized words / quoted phrases / proper nouns)
    let entities = extract_entities(q);
    if entities.len() < 2 {
        // Single entity or no entities — not multi-hop
        return None;
    }

    // Detect decomposition pattern
    if let Some(sub_queries) = decompose_causal(&q_lower, &entities) {
        return Some(DecomposedQuery {
            original: q.to_string(),
            sub_queries,
            entities,
        });
    }

    if let Some(sub_queries) = decompose_relationship(&q_lower, &entities) {
        return Some(DecomposedQuery {
            original: q.to_string(),
            sub_queries,
            entities,
        });
    }

    if let Some(sub_queries) = decompose_chain(&q_lower, &entities) {
        return Some(DecomposedQuery {
            original: q.to_string(),
            sub_queries,
            entities,
        });
    }

    // Generic multi-hop: look up each entity, then find connections
    let mut sub_queries = Vec::new();
    for (i, entity) in entities.iter().enumerate() {
        sub_queries.push(SubQuery {
            query: format!("What is {entity}?"),
            hop: i,
            role: SubQueryRole::EntityLookup,
        });
    }
    sub_queries.push(SubQuery {
        query: format!("How are {} connected?", entities.join(" and ")),
        hop: entities.len(),
        role: SubQueryRole::RelationFind,
    });

    Some(DecomposedQuery {
        original: q.to_string(),
        sub_queries,
        entities,
    })
}

/// Parse an LLM decomposition response into sub-queries.
///
/// Expected format: one sub-query per line, optionally prefixed with "1. ", "2. ", etc.
pub fn parse_llm_decomposition(response: &str, original: &str) -> Option<DecomposedQuery> {
    let lines: Vec<&str> = response
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    if lines.len() < 2 {
        return None; // Need at least 2 sub-queries for decomposition
    }

    let entities = extract_entities(original);
    let sub_queries: Vec<SubQuery> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            // Strip numbering prefix: "1. ", "2. ", etc.
            let query = if let Some(stripped) = line.strip_prefix(char::is_numeric) {
                stripped.trim_start_matches(['.', ')', ' '])
                    .trim()
                    .to_string()
            } else {
                line.to_string()
            };
            SubQuery {
                query,
                hop: i,
                role: SubQueryRole::EntityLookup,
            }
        })
        .collect();

    Some(DecomposedQuery {
        original: original.to_string(),
        sub_queries,
        entities,
    })
}

/// Build the LLM prompt for query decomposition.
pub fn decomposition_prompt(query: &str) -> String {
    format!(
        "Decompose the following multi-hop question into 2-4 simpler sub-questions. \
         Each sub-question should be answerable independently from a single document. \
         Output one sub-question per line, numbered.\n\n\
         Example:\n\
         Question: Why did the deployment cause the outage?\n\
         1. What happened during the deployment?\n\
         2. What was the state of the system before the outage?\n\
         3. What connects the deployment to the outage?\n\n\
         Question: {query}\n"
    )
}

// ── Internal: Entity Extraction ─────────────────────────────────────────────

/// Extract entities from a query.
///
/// Heuristics:
/// 1. Quoted phrases: "Project Alpha" → "Project Alpha"
/// 2. Capitalized words (not sentence-initial): "Alice" in "Why did Alice leave?"
/// 3. "the X" patterns: "the deployment" → "deployment"
/// 4. Chinese entities: consecutive non-punctuation characters after markers
fn extract_entities(query: &str) -> Vec<String> {
    let mut entities = Vec::new();

    // 1. Quoted phrases
    let mut in_quote = false;
    let mut quote_start = 0;
    for (i, c) in query.char_indices() {
        if c == '"' || c == '\u{201c}' || c == '\u{201d}' {
            if in_quote {
                let phrase = &query[quote_start..i];
                if !phrase.is_empty() && !entities.contains(&phrase.to_string()) {
                    entities.push(phrase.to_string());
                }
                in_quote = false;
            } else {
                in_quote = true;
                quote_start = i + c.len_utf8();
            }
        }
    }

    // 2. Capitalized words and "the X" patterns
    let skip_words: &[&str] = &[
        "what", "why", "how", "when", "where", "which", "who",
        "did", "does", "do", "is", "are", "was", "were", "be", "been",
        "the", "a", "an", "and", "or", "but", "in", "on", "at",
        "to", "for", "of", "with", "from", "by", "between",
        "this", "that", "these", "those", "it", "its", "they",
        "cause", "caused", "causing", "lead", "led", "result", "resulted",
        "happen", "happened", "related", "connected", "relationship",
        "about", "after", "before", "during", "then", "now",
    ];

    let words: Vec<&str> = query.split_whitespace().collect();
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric());
        if clean.is_empty() {
            i += 1;
            continue;
        }

        let clean_lower = clean.to_lowercase();
        let is_skip = skip_words.contains(&clean_lower.as_str());

        // Capitalized entity (skip sentence-initial)
        if !is_skip && i > 0 && clean.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
            && !entities.contains(&clean.to_string())
        {
            entities.push(clean.to_string());
        }

        // "the X" or "the X Y" pattern — extract noun phrase after "the"
        if clean_lower == "the" && i + 1 < words.len() {
            let next = words[i + 1].trim_matches(|c: char| !c.is_alphanumeric());
            let next_lower = next.to_lowercase();
            if !skip_words.contains(&next_lower.as_str()) && next.len() >= 3
                && !entities.contains(&next.to_string())
            {
                entities.push(next.to_string());
            }
        }

        i += 1;
    }

    // 3. Chinese entities: extract by scanning CJK characters.
    //    CJK text has no spaces — mark skip-words first, then extract remaining 2-char sequences.
    let skip_cjk: &[&str] = &[
        "为什么", "什么", "怎么", "如何", "哪个", "哪些", "多少",
        "导致", "因为", "所以", "然后", "接着", "首先", "最后",
        "关系", "联系", "之间", "可以", "可能", "已经", "正在",
    ];
    // Also skip single-char particles
    let skip_cjk_single: &[char] = &['的', '了', '是', '在', '有', '和', '与', '或', '把', '被', '从', '到', '着', '过', '地', '得', '也', '就', '都', '又', '再', '才', '却', '而', '但'];
    let cjk_chars: Vec<char> = query.chars().filter(|c| *c >= '\u{4e00}' && *c <= '\u{9fff}').collect();
    let mut used = vec![false; cjk_chars.len()];

    // Pass 1a: mark single-char particles
    for (i, c) in cjk_chars.iter().enumerate() {
        if skip_cjk_single.contains(c) {
            used[i] = true;
        }
    }
    // Pass 1b: mark multi-char skip-words at their positions
    for skip in skip_cjk {
        let skip_chars: Vec<char> = skip.chars().collect();
        let skip_len = skip_chars.len();
        for i in 0..cjk_chars.len().saturating_sub(skip_len - 1) {
            if used[i..i + skip_len].iter().any(|u| *u) { continue; }
            let window: String = cjk_chars[i..i + skip_len].iter().collect();
            if window == *skip {
                used[i..i + skip_len].fill(true);
            }
        }
    }

    // Pass 2: extract remaining 2-char sequences as entities
    let mut i = 0;
    while i < cjk_chars.len() {
        if used[i] {
            i += 1;
            continue;
        }
        // Find the end of this unused run
        let start = i;
        while i < cjk_chars.len() && !used[i] {
            i += 1;
        }
        let run_len = i - start;
        // Extract as 2-char words (most common Chinese word length)
        let mut j = start;
        while j + 2 <= i {
            let word: String = cjk_chars[j..j + 2].iter().collect();
            if !entities.contains(&word) {
                entities.push(word);
            }
            j += 2;
        }
        // If odd number of chars, include the last one with the previous or standalone
        if run_len % 2 == 1 && run_len >= 3 {
            // Already covered by the 2-char windows above
        } else if run_len == 1 {
            // Single char — skip (too short to be meaningful entity)
        }
    }

    entities
}

// ── Internal: Decomposition Patterns ────────────────────────────────────────

/// Causal pattern: "Why did X cause Y?", "What led from X to Y?"
fn decompose_causal(q: &str, entities: &[String]) -> Option<Vec<SubQuery>> {
    let causal_markers = [
        "why did", "what caused", "what led", "what resulted",
        "how did", "what was the reason", "because of",
        "为什么", "什么原因", "导致",
    ];

    if !causal_markers.iter().any(|m| q.contains(m)) {
        return None;
    }

    let mut subs = Vec::new();
    // Sub 1: What happened to the cause entity?
    if let Some(first) = entities.first() {
        subs.push(SubQuery {
            query: format!("What happened involving {first}?"),
            hop: 0,
            role: SubQueryRole::EntityLookup,
        });
    }
    // Sub 2: What is the effect entity?
    if entities.len() >= 2 {
        subs.push(SubQuery {
            query: format!("What happened involving {}?", entities[1]),
            hop: 1,
            role: SubQueryRole::EntityLookup,
        });
    }
    // Sub 3: What connects them causally?
    subs.push(SubQuery {
        query: format!("How did {} cause or lead to {}?",
            entities.first().map(|s| s.as_str()).unwrap_or("this"),
            entities.get(1).map(|s| s.as_str()).unwrap_or("that")),
        hop: subs.len(),
        role: SubQueryRole::CausalExplain,
    });

    Some(subs)
}

/// Relationship pattern: "What is the relationship between X and Y?"
fn decompose_relationship(q: &str, entities: &[String]) -> Option<Vec<SubQuery>> {
    let relation_markers = [
        "relationship between", "connection between", "link between",
        "difference between", "similarity between", "related to",
        "connected to", "associated with",
        "关系", "联系", "区别", "相似",
    ];

    if !relation_markers.iter().any(|m| q.contains(m)) {
        return None;
    }

    let mut subs = Vec::new();
    for (i, entity) in entities.iter().enumerate() {
        subs.push(SubQuery {
            query: format!("What is {entity}?"),
            hop: i,
            role: SubQueryRole::EntityLookup,
        });
    }
    subs.push(SubQuery {
        query: format!("What is the relationship between {}?", entities.join(" and ")),
        hop: entities.len(),
        role: SubQueryRole::RelationFind,
    });

    Some(subs)
}

/// Chain pattern: "What happened after X before Y?" / temporal sequence
fn decompose_chain(q: &str, entities: &[String]) -> Option<Vec<SubQuery>> {
    let chain_markers = [
        "and then", "after that", "before that", "first", "then",
        "next", "finally", "in order", "sequence",
        "然后", "接着", "首先", "最后",
    ];

    if !chain_markers.iter().any(|m| q.contains(m)) {
        return None;
    }

    let mut subs = Vec::new();
    for (i, entity) in entities.iter().enumerate() {
        subs.push(SubQuery {
            query: format!("What happened with {entity}?"),
            hop: i,
            role: SubQueryRole::TemporalSequence,
        });
    }
    subs.push(SubQuery {
        query: format!("In what order did events involving {} occur?", entities.join(", ")),
        hop: entities.len(),
        role: SubQueryRole::TemporalSequence,
    });

    Some(subs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decompose_causal_two_entities() {
        let result = decompose("Why did the deployment cause the outage?");
        assert!(result.is_some());
        let dq = result.unwrap();
        assert!(dq.sub_queries.len() >= 2);
        assert!(dq.entities.len() >= 2);
        assert_eq!(dq.sub_queries.last().unwrap().role, SubQueryRole::CausalExplain);
    }

    #[test]
    fn test_decompose_relationship() {
        let result = decompose("What is the relationship between Alice and Bob?");
        assert!(result.is_some());
        let dq = result.unwrap();
        assert!(dq.sub_queries.len() >= 3); // lookup each + find relationship
        assert_eq!(dq.sub_queries.last().unwrap().role, SubQueryRole::RelationFind);
    }

    #[test]
    fn test_decompose_chain() {
        let result = decompose("What happened with Project Alpha and then Project Beta?");
        assert!(result.is_some());
        let dq = result.unwrap();
        assert!(dq.sub_queries.len() >= 2);
    }

    #[test]
    fn test_decompose_generic_multi_entity() {
        let result = decompose("How are the Kernel and the Filesystem related in Plico?");
        assert!(result.is_some());
        let dq = result.unwrap();
        assert!(dq.entities.len() >= 2);
        assert!(dq.sub_queries.len() >= 2);
    }

    #[test]
    fn test_decompose_single_entity_returns_none() {
        // Only one entity — not multi-hop
        let result = decompose("What is Plico?");
        assert!(result.is_none());
    }

    #[test]
    fn test_decompose_no_entities_returns_none() {
        let result = decompose("what is the meaning of life?");
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_entities_quoted() {
        let entities = extract_entities("What happened to \"Project Alpha\"?");
        assert!(entities.contains(&"Project Alpha".to_string()));
    }

    #[test]
    fn test_extract_entities_capitalized() {
        let entities = extract_entities("Why did Alice talk to Bob?");
        assert!(entities.contains(&"Alice".to_string()));
        assert!(entities.contains(&"Bob".to_string()));
    }

    #[test]
    fn test_extract_entities_cjk() {
        let entities = extract_entities("为什么部署导致了故障？");
        assert!(entities.iter().any(|e| e == "部署"), "should find 部署, got {:?}", entities);
        assert!(entities.iter().any(|e| e == "故障"), "should find 故障, got {:?}", entities);
    }

    #[test]
    fn test_parse_llm_decomposition() {
        let response = "1. What happened during the deployment?\n2. What was the system state?\n3. What connects them?";
        let result = parse_llm_decomposition(response, "Why did the deployment cause the outage?");
        assert!(result.is_some());
        let dq = result.unwrap();
        assert_eq!(dq.sub_queries.len(), 3);
        assert_eq!(dq.sub_queries[0].query, "What happened during the deployment?");
    }

    #[test]
    fn test_parse_llm_decomposition_too_few() {
        let response = "What happened?";
        let result = parse_llm_decomposition(response, "Why X?");
        assert!(result.is_none());
    }

    #[test]
    fn test_decomposition_prompt() {
        let prompt = decomposition_prompt("Why did X cause Y?");
        assert!(prompt.contains("Why did X cause Y?"));
        assert!(prompt.contains("sub-question"));
    }

    #[test]
    fn test_causal_chinese() {
        let result = decompose("为什么部署导致了故障？");
        assert!(result.is_some());
        let dq = result.unwrap();
        assert!(dq.entities.len() >= 2);
        assert!(dq.sub_queries.iter().any(|sq| sq.role == SubQueryRole::CausalExplain));
    }

    #[test]
    fn test_sub_query_hops_are_sequential() {
        let result = decompose("Why did Alice and Bob disagree?");
        let dq = result.unwrap();
        for (i, sq) in dq.sub_queries.iter().enumerate() {
            assert_eq!(sq.hop, i, "hop should be sequential");
        }
    }
}
