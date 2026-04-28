//! Memory Pressure Scheduler — OS-level eviction when multi-agent memory exceeds budget.
//!
//! Like Linux's OOM Killer but for AI memory. When total memory across all agents
//! exceeds the system budget, this module decides what to evict.
//!
//! Eviction priority (low = evicted first):
//! 1. Untyped + Ephemeral
//! 2. Episodic + Ephemeral
//! 3. Untyped + Working
//! 4. Episodic + Working (expired TTL)
//! 5. Semantic + Working
//! 6. LongTerm + low access_count
//! 7. Procedural (last resort)
//!
//! Fair scheduling: agents exceeding their quota are penalized first.

use crate::memory::layered::{MemoryEntry, MemoryTier, MemoryType};
use std::collections::HashMap;

/// Priority score for eviction — lower score = evicted first.
pub fn eviction_priority(entry: &MemoryEntry, now_ms: u64) -> u32 {
    let tier_score = match entry.tier {
        MemoryTier::Ephemeral => 0,
        MemoryTier::Working => 100,
        MemoryTier::LongTerm => 200,
        MemoryTier::Procedural => 300,
    };

    let type_score = match entry.memory_type {
        MemoryType::Untyped => 0,
        MemoryType::Episodic => 10,
        MemoryType::Semantic => 20,
        MemoryType::Procedural => 30,
    };

    let ttl_penalty = if let Some(ttl) = entry.ttl_ms {
        if entry.created_at + ttl < now_ms {
            0
        } else {
            5
        }
    } else {
        5
    };

    let access_bonus = entry.access_count.min(50) * 2;

    let superseded_penalty = if entry.supersedes.is_some() || is_likely_superseded(entry) {
        0
    } else {
        10
    };

    tier_score + type_score + ttl_penalty + access_bonus + superseded_penalty
}

fn is_likely_superseded(entry: &MemoryEntry) -> bool {
    entry.importance < 25
}

/// Per-agent memory usage summary.
#[derive(Debug, Clone)]
pub struct AgentUsage {
    pub agent_id: String,
    pub entry_count: usize,
    pub quota: usize,
}

impl AgentUsage {
    pub fn is_over_quota(&self) -> bool {
        self.quota > 0 && self.entry_count > self.quota
    }

    pub fn excess(&self) -> usize {
        if self.quota == 0 {
            return 0;
        }
        self.entry_count.saturating_sub(self.quota)
    }
}

/// Compute per-agent usage from entries.
pub fn compute_agent_usage(
    entries: &[MemoryEntry],
    quotas: &HashMap<String, usize>,
) -> Vec<AgentUsage> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for entry in entries {
        *counts.entry(entry.agent_id.clone()).or_insert(0) += 1;
    }

    counts
        .into_iter()
        .map(|(agent_id, entry_count)| {
            let quota = quotas.get(&agent_id).copied().unwrap_or(0);
            AgentUsage {
                agent_id,
                entry_count,
                quota,
            }
        })
        .collect()
}

/// Select entries to evict to bring total memory under the global budget.
///
/// Returns the IDs of entries to evict, ordered by eviction priority (lowest first).
/// Fair scheduling: over-quota agents' entries are penalized.
pub fn select_evictions(
    entries: &[MemoryEntry],
    global_budget: usize,
    agent_quotas: &HashMap<String, usize>,
    now_ms: u64,
) -> Vec<String> {
    if entries.len() <= global_budget {
        return vec![];
    }

    let to_evict = entries.len() - global_budget;

    let usage = compute_agent_usage(entries, agent_quotas);
    let over_quota_agents: HashMap<String, usize> = usage
        .iter()
        .filter(|u| u.is_over_quota())
        .map(|u| (u.agent_id.clone(), u.excess()))
        .collect();

    let mut scored: Vec<(usize, u32)> = entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let base = eviction_priority(entry, now_ms);
            let fairness_penalty = if over_quota_agents.contains_key(&entry.agent_id) {
                0
            } else {
                50
            };
            (idx, base + fairness_penalty)
        })
        .collect();

    scored.sort_by_key(|&(_, score)| score);

    scored
        .into_iter()
        .take(to_evict)
        .map(|(idx, _)| entries[idx].id.clone())
        .collect()
}

/// Check if the system is under memory pressure.
pub fn is_under_pressure(entry_count: usize, global_budget: usize) -> bool {
    global_budget > 0 && entry_count > global_budget
}

/// Compute the pressure ratio (0.0 = no pressure, >1.0 = over budget).
pub fn pressure_ratio(entry_count: usize, global_budget: usize) -> f64 {
    if global_budget == 0 {
        return 0.0;
    }
    entry_count as f64 / global_budget as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make_entry(
        id: &str, agent: &str, tier: MemoryTier,
        mem_type: MemoryType, access_count: u32, importance: u8,
    ) -> MemoryEntry {
        let mut e = MemoryEntry::ephemeral(agent, format!("content-{}", id));
        e.id = id.to_string();
        e.tier = tier;
        e.memory_type = mem_type;
        e.access_count = access_count;
        e.importance = importance;
        e
    }

    #[test]
    fn test_eviction_priority_ordering() {
        let now = 1000000;
        let ephemeral_untyped = make_entry("e1", "a", MemoryTier::Ephemeral, MemoryType::Untyped, 0, 50);
        let ephemeral_episodic = make_entry("e2", "a", MemoryTier::Ephemeral, MemoryType::Episodic, 0, 50);
        let working_semantic = make_entry("e3", "a", MemoryTier::Working, MemoryType::Semantic, 0, 50);
        let longterm_low = make_entry("e4", "a", MemoryTier::LongTerm, MemoryType::Semantic, 1, 50);
        let procedural = make_entry("e5", "a", MemoryTier::Procedural, MemoryType::Procedural, 5, 100);

        let p1 = eviction_priority(&ephemeral_untyped, now);
        let p2 = eviction_priority(&ephemeral_episodic, now);
        let p3 = eviction_priority(&working_semantic, now);
        let p4 = eviction_priority(&longterm_low, now);
        let p5 = eviction_priority(&procedural, now);

        assert!(p1 < p2, "Untyped+Ephemeral < Episodic+Ephemeral");
        assert!(p2 < p3, "Episodic+Ephemeral < Semantic+Working");
        assert!(p3 < p4, "Semantic+Working < LongTerm");
        assert!(p4 < p5, "LongTerm < Procedural");
    }

    #[test]
    fn test_access_count_increases_priority() {
        let now = 1000000;
        let low_access = make_entry("e1", "a", MemoryTier::LongTerm, MemoryType::Semantic, 0, 50);
        let high_access = make_entry("e2", "a", MemoryTier::LongTerm, MemoryType::Semantic, 20, 50);

        assert!(
            eviction_priority(&low_access, now) < eviction_priority(&high_access, now),
            "higher access should have higher priority (harder to evict)"
        );
    }

    #[test]
    fn test_no_eviction_under_budget() {
        let entries = vec![
            make_entry("e1", "a", MemoryTier::Ephemeral, MemoryType::Untyped, 0, 50),
            make_entry("e2", "a", MemoryTier::Working, MemoryType::Semantic, 0, 50),
        ];
        let evictions = select_evictions(&entries, 10, &HashMap::new(), 1000000);
        assert!(evictions.is_empty());
    }

    #[test]
    fn test_eviction_selects_lowest_priority() {
        let entries = vec![
            make_entry("ephemeral", "a", MemoryTier::Ephemeral, MemoryType::Untyped, 0, 50),
            make_entry("procedural", "a", MemoryTier::Procedural, MemoryType::Procedural, 10, 100),
            make_entry("working", "a", MemoryTier::Working, MemoryType::Semantic, 0, 50),
        ];
        let evictions = select_evictions(&entries, 2, &HashMap::new(), 1000000);
        assert_eq!(evictions.len(), 1);
        assert_eq!(evictions[0], "ephemeral");
    }

    #[test]
    fn test_fair_scheduling_over_quota_agents_penalized() {
        let mut quotas = HashMap::new();
        quotas.insert("agent-a".to_string(), 1);
        quotas.insert("agent-b".to_string(), 5);

        let entries = vec![
            make_entry("a1", "agent-a", MemoryTier::Working, MemoryType::Semantic, 0, 50),
            make_entry("a2", "agent-a", MemoryTier::Working, MemoryType::Semantic, 0, 50),
            make_entry("b1", "agent-b", MemoryTier::Working, MemoryType::Semantic, 0, 50),
        ];

        let evictions = select_evictions(&entries, 2, &quotas, 1000000);
        assert_eq!(evictions.len(), 1);
        assert!(
            evictions[0].starts_with("a"),
            "over-quota agent-a's entry should be evicted first, got: {}",
            evictions[0]
        );
    }

    #[test]
    fn test_pressure_ratio() {
        assert!((pressure_ratio(50, 100) - 0.5).abs() < 0.01);
        assert!((pressure_ratio(200, 100) - 2.0).abs() < 0.01);
        assert!((pressure_ratio(0, 0) - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_is_under_pressure() {
        assert!(!is_under_pressure(50, 100));
        assert!(is_under_pressure(150, 100));
        assert!(!is_under_pressure(100, 0));
    }
}
