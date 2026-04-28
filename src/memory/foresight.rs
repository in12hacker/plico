//! Foresight Assembler — predictive context assembly via Markov access chains.
//!
//! Maintains a global memory access log across all agents. From these logs,
//! builds a transition probability matrix:
//!
//!   P(next_memory | current_memory) = count(current → next) / count(current)
//!
//! When an agent declares an intent, the system:
//! 1. Finds the most relevant historical memory for the intent keywords
//! 2. Walks the Markov chain to predict the K most likely next-needed memories
//! 3. Pre-assembles them into context
//!
//! CPU-only: Pure statistical co-occurrence with time-decay weighting.
//! LLM-enhanced: LLM predicts what information the agent will need next.

use std::collections::HashMap;

/// A single access event in the global log.
#[derive(Debug, Clone)]
pub struct AccessEvent {
    pub agent_id: String,
    pub memory_id: String,
    pub timestamp_ms: u64,
}

/// Markov chain over memory access patterns.
#[derive(Debug, Clone)]
pub struct MarkovAccessChain {
    transitions: HashMap<String, HashMap<String, f64>>,
    access_counts: HashMap<String, u64>,
    time_decay_halflife_ms: u64,
}

impl MarkovAccessChain {
    pub fn new(time_decay_halflife_ms: u64) -> Self {
        Self {
            transitions: HashMap::new(),
            access_counts: HashMap::new(),
            time_decay_halflife_ms,
        }
    }

    /// Default chain with 1-hour half-life.
    pub fn default_chain() -> Self {
        Self::new(3_600_000)
    }

    /// Build the chain from a sequence of access events.
    ///
    /// Events should be sorted by timestamp. Consecutive accesses by the same
    /// agent within a session window form transitions.
    pub fn build_from_events(&mut self, events: &[AccessEvent], session_gap_ms: u64) {
        self.transitions.clear();
        self.access_counts.clear();

        if events.len() < 2 {
            return;
        }

        let now = events.last().map(|e| e.timestamp_ms).unwrap_or(0);

        for window in events.windows(2) {
            let prev = &window[0];
            let curr = &window[1];

            if prev.agent_id != curr.agent_id {
                continue;
            }
            if curr.timestamp_ms.saturating_sub(prev.timestamp_ms) > session_gap_ms {
                continue;
            }

            let weight = self.time_decay_weight(curr.timestamp_ms, now);

            *self.access_counts.entry(prev.memory_id.clone()).or_insert(0) += 1;

            let transitions = self
                .transitions
                .entry(prev.memory_id.clone())
                .or_default();
            *transitions.entry(curr.memory_id.clone()).or_insert(0.0) += weight;
        }
    }

    /// Predict the top-K most likely next memories given a starting memory.
    pub fn predict(&self, current_memory_id: &str, top_k: usize) -> Vec<(String, f64)> {
        let transitions = match self.transitions.get(current_memory_id) {
            Some(t) => t,
            None => return vec![],
        };

        let total: f64 = transitions.values().sum();
        if total == 0.0 {
            return vec![];
        }

        let mut ranked: Vec<(String, f64)> = transitions
            .iter()
            .map(|(id, &weight)| (id.clone(), weight / total))
            .collect();

        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(top_k);
        ranked
    }

    /// Multi-hop prediction: walk K steps along the chain, collecting unique
    /// memories with cumulative probability.
    pub fn predict_multihop(
        &self,
        start_memory_id: &str,
        hops: usize,
        top_k_per_hop: usize,
    ) -> Vec<(String, f64)> {
        let mut collected: HashMap<String, f64> = HashMap::new();
        let mut frontier: Vec<(String, f64)> = vec![(start_memory_id.to_string(), 1.0)];

        for _ in 0..hops {
            let mut next_frontier = Vec::new();
            for (mem_id, prob) in &frontier {
                let predictions = self.predict(mem_id, top_k_per_hop);
                for (next_id, transition_prob) in predictions {
                    if next_id == start_memory_id {
                        continue;
                    }
                    let cumulative = prob * transition_prob;
                    *collected.entry(next_id.clone()).or_insert(0.0) += cumulative;
                    next_frontier.push((next_id, cumulative));
                }
            }
            frontier = next_frontier;
        }

        let mut result: Vec<(String, f64)> = collected.into_iter().collect();
        result.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        result
    }

    /// Get the total number of unique memories seen.
    pub fn memory_count(&self) -> usize {
        let mut ids: std::collections::HashSet<&String> = self.transitions.keys().collect();
        for nexts in self.transitions.values() {
            for id in nexts.keys() {
                ids.insert(id);
            }
        }
        ids.len()
    }

    /// Get the total number of transitions recorded.
    pub fn transition_count(&self) -> usize {
        self.transitions.values().map(|t| t.len()).sum()
    }

    fn time_decay_weight(&self, event_time: u64, now: u64) -> f64 {
        if self.time_decay_halflife_ms == 0 {
            return 1.0;
        }
        let age = now.saturating_sub(event_time) as f64;
        let halflife = self.time_decay_halflife_ms as f64;
        (-age * (2.0_f64.ln()) / halflife).exp()
    }
}

/// Build an LLM prompt for predictive assembly.
pub fn foresight_prompt(intent_description: &str, recent_memories: &[String]) -> String {
    let recent = if recent_memories.is_empty() {
        "No recent context available.".to_string()
    } else {
        recent_memories
            .iter()
            .enumerate()
            .map(|(i, m)| format!("  {}. {}", i + 1, m))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "An AI agent has declared the following intent:\n\
         \"{}\"\n\n\
         Recent memory context:\n{}\n\n\
         Predict what information the agent will most likely need next.\n\
         List up to 5 topics/keywords, one per line.",
        intent_description, recent
    )
}

/// Parse LLM response into keywords for memory retrieval.
pub fn parse_foresight_response(response: &str) -> Vec<String> {
    response
        .lines()
        .map(|l| l.trim().trim_start_matches(|c: char| c.is_numeric() || c == '.' || c == '-'))
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && l.len() < 200)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_events(sequence: &[(&str, &str, u64)]) -> Vec<AccessEvent> {
        sequence
            .iter()
            .map(|(agent, mem, ts)| AccessEvent {
                agent_id: agent.to_string(),
                memory_id: mem.to_string(),
                timestamp_ms: *ts,
            })
            .collect()
    }

    #[test]
    fn test_empty_chain() {
        let chain = MarkovAccessChain::default_chain();
        assert_eq!(chain.memory_count(), 0);
        assert_eq!(chain.transition_count(), 0);
        assert!(chain.predict("any", 5).is_empty());
    }

    #[test]
    fn test_build_simple_chain() {
        let mut chain = MarkovAccessChain::new(0);
        let events = make_events(&[
            ("a", "m1", 100),
            ("a", "m2", 200),
            ("a", "m3", 300),
        ]);
        chain.build_from_events(&events, 1000);

        assert_eq!(chain.transition_count(), 2);
        let predictions = chain.predict("m1", 5);
        assert_eq!(predictions.len(), 1);
        assert_eq!(predictions[0].0, "m2");
        assert!((predictions[0].1 - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_branching_transitions() {
        let mut chain = MarkovAccessChain::new(0);
        let events = make_events(&[
            ("a", "m1", 100),
            ("a", "m2", 200),
            ("a", "m1", 300),
            ("a", "m3", 400),
            ("a", "m1", 500),
            ("a", "m2", 600),
        ]);
        chain.build_from_events(&events, 1000);

        let predictions = chain.predict("m1", 5);
        assert_eq!(predictions.len(), 2);
        let m2_prob = predictions.iter().find(|(id, _)| id == "m2").unwrap().1;
        let m3_prob = predictions.iter().find(|(id, _)| id == "m3").unwrap().1;
        assert!(m2_prob > m3_prob, "m2 should be more likely (2 vs 1 transition)");
    }

    #[test]
    fn test_session_gap_breaks_chain() {
        let mut chain = MarkovAccessChain::new(0);
        let events = make_events(&[
            ("a", "m1", 100),
            ("a", "m2", 200),
            ("a", "m3", 10000),
        ]);
        chain.build_from_events(&events, 500);

        let from_m2 = chain.predict("m2", 5);
        assert!(from_m2.is_empty(), "session gap should break the chain");
    }

    #[test]
    fn test_cross_agent_events_ignored() {
        let mut chain = MarkovAccessChain::new(0);
        let events = make_events(&[
            ("a", "m1", 100),
            ("b", "m2", 200),
        ]);
        chain.build_from_events(&events, 1000);

        assert_eq!(chain.transition_count(), 0);
    }

    #[test]
    fn test_multihop_prediction() {
        let mut chain = MarkovAccessChain::new(0);
        let events = make_events(&[
            ("a", "m1", 100),
            ("a", "m2", 200),
            ("a", "m3", 300),
            ("a", "m1", 400),
            ("a", "m2", 500),
            ("a", "m3", 600),
        ]);
        chain.build_from_events(&events, 1000);

        let multihop = chain.predict_multihop("m1", 2, 3);
        assert!(!multihop.is_empty());
        let ids: Vec<&str> = multihop.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"m2"), "should predict m2 from m1");
        assert!(ids.contains(&"m3"), "should predict m3 via m1→m2→m3");
    }

    #[test]
    fn test_time_decay_weighting() {
        let mut chain_nodecay = MarkovAccessChain::new(0);
        let mut chain_decay = MarkovAccessChain::new(1000);

        let events = make_events(&[
            ("a", "m1", 100),
            ("a", "m2", 200),
            ("a", "m1", 9000),
            ("a", "m3", 9100),
        ]);

        chain_nodecay.build_from_events(&events, 10000);
        chain_decay.build_from_events(&events, 10000);

        let pred_nodecay = chain_nodecay.predict("m1", 5);
        let pred_decay = chain_decay.predict("m1", 5);

        assert_eq!(pred_nodecay.len(), 2);
        assert_eq!(pred_decay.len(), 2);

        let m3_decay = pred_decay.iter().find(|(id, _)| id == "m3").unwrap().1;
        let m2_decay = pred_decay.iter().find(|(id, _)| id == "m2").unwrap().1;
        assert!(m3_decay > m2_decay, "recent transition m1→m3 should be weighted higher with decay");
    }

    #[test]
    fn test_foresight_prompt_format() {
        let prompt = foresight_prompt("deploy to production", &["setup CI".into(), "run tests".into()]);
        assert!(prompt.contains("deploy to production"));
        assert!(prompt.contains("setup CI"));
    }

    #[test]
    fn test_parse_foresight_response() {
        let response = "1. deployment configuration\n2. rollback procedure\n3. monitoring setup";
        let keywords = parse_foresight_response(response);
        assert_eq!(keywords.len(), 3);
        assert!(keywords[0].contains("deployment"));
    }

    #[test]
    fn test_cold_start_empty_predictions() {
        let chain = MarkovAccessChain::default_chain();
        let predictions = chain.predict("new-memory", 5);
        assert!(predictions.is_empty(), "cold start should return empty");

        let multihop = chain.predict_multihop("new-memory", 3, 5);
        assert!(multihop.is_empty());
    }
}
