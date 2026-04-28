//! Tier Maintenance — automatic memory promotion/demotion and eviction.
//!
//! Implements the Memory Tier Automation for v12.0:
//! - Automatic promotion based on access thresholds
//! - Automatic eviction of low-importance ephemeral entries
//! - Background tier maintenance processing
//!
//! The eviction logic uses importance < 70 as threshold (from existing test).

use crate::memory::layered::{LayeredMemory, MemoryEntry, MemoryTier};
use crate::memory::relevance::{check_promotion, PromotionThresholds};

/// Importance threshold for ephemeral eviction.
/// Entries with importance < 70 are discarded during eviction.
const EPHEMERAL_EVICTION_IMPORTANCE_THRESHOLD: u8 = 70;

/// Tier maintenance processor — handles automatic promotion, eviction, and tier health.
pub struct TierMaintenance {
    /// Promotion thresholds to use (defaults from relevance module).
    thresholds: PromotionThresholds,
}

impl TierMaintenance {
    /// Create a new TierMaintenance with default thresholds.
    pub fn new() -> Self {
        Self {
            thresholds: PromotionThresholds::default(),
        }
    }

    /// Create a TierMaintenance with custom thresholds.
    pub fn with_thresholds(thresholds: PromotionThresholds) -> Self {
        Self { thresholds }
    }

    /// Process ephemeral eviction for an agent.
    ///
    /// Evicts entries with importance < 70 (discards them).
    /// Entries with importance >= 70 are promoted to Working Memory (L1).
    ///
    /// Returns the list of discarded entries.
    pub fn process_ephemeral_eviction(&self, memory: &LayeredMemory, agent_id: &str) -> Vec<MemoryEntry> {
        memory.evict_ephemeral(agent_id)
    }

    /// Process promotions for all tiers of an agent.
    ///
    /// Checks Ephemeral → Working (access >= 3) and
    /// Working → LongTerm (access >= 10 && importance >= 50).
    pub fn process_promotions(&self, memory: &LayeredMemory, agent_id: &str) {
        memory.promote_check(agent_id);
    }

    /// Run a complete maintenance cycle for an agent:
    /// 1. Process ephemeral eviction
    /// 2. Process promotions across all tiers
    ///
    /// Returns summary statistics.
    pub fn run_maintenance_cycle(&self, memory: &LayeredMemory, agent_id: &str) -> MaintenanceStats {
        let before_ephemeral = memory.get_tier(agent_id, MemoryTier::Ephemeral).len();
        let before_working = memory.get_tier(agent_id, MemoryTier::Working).len();
        let before_longterm = memory.get_tier(agent_id, MemoryTier::LongTerm).len();

        // Step 1: Evict low-importance ephemeral entries
        let _discarded = self.process_ephemeral_eviction(memory, agent_id);

        // Step 2: Process promotions
        self.process_promotions(memory, agent_id);

        let after_ephemeral = memory.get_tier(agent_id, MemoryTier::Ephemeral).len();
        let after_working = memory.get_tier(agent_id, MemoryTier::Working).len();
        let after_longterm = memory.get_tier(agent_id, MemoryTier::LongTerm).len();

        MaintenanceStats {
            ephemeral_before: before_ephemeral,
            ephemeral_after: after_ephemeral,
            working_before: before_working,
            working_after: after_working,
            longterm_before: before_longterm,
            longterm_after: after_longterm,
            promoted_count: (after_working - before_working) + (after_longterm - before_longterm),
            evicted_count: before_ephemeral.saturating_sub(after_ephemeral)
                .saturating_sub(after_working - before_working),
            linked_count: 0, // Set by caller after linking
        }
    }

    /// Check if a specific entry should be promoted.
    ///
    /// Returns Some(new_tier) if promotion should occur, None otherwise.
    pub fn check_entry_promotion(&self, entry: &MemoryEntry) -> Option<MemoryTier> {
        check_promotion(entry, &self.thresholds)
    }

    /// Check if an ephemeral entry should be evicted based on importance.
    ///
    /// Returns true if the entry should be discarded (importance < 70).
    pub fn should_evict_ephemeral(&self, entry: &MemoryEntry) -> bool {
        entry.tier == MemoryTier::Ephemeral && entry.importance < EPHEMERAL_EVICTION_IMPORTANCE_THRESHOLD
    }
}

impl Default for TierMaintenance {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics from a maintenance cycle run.
#[derive(Debug, Clone)]
pub struct MaintenanceStats {
    pub ephemeral_before: usize,
    pub ephemeral_after: usize,
    pub working_before: usize,
    pub working_after: usize,
    pub longterm_before: usize,
    pub longterm_after: usize,
    pub promoted_count: usize,
    pub evicted_count: usize,
    /// Number of memories linked to KG during consolidation (set by caller).
    pub linked_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryContent;

    fn make_entry(agent_id: &str, tier: MemoryTier, importance: u8, access_count: u32) -> MemoryEntry {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            tenant_id: "default".to_string(),
            tier,
            content: MemoryContent::Text("test content".to_string()),
            importance,
            access_count,
            last_accessed: now,
            created_at: now,
            tags: Vec::new(),
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: crate::memory::MemoryScope::Private,
            memory_type: crate::memory::MemoryType::default(),
            causal_parent: None,
            supersedes: None,
        }
    }

    #[test]
    fn test_no_promotion_below_threshold() {
        let mem = LayeredMemory::new();
        let maintenance = TierMaintenance::new();

        // Store ephemeral entry with access_count < 3
        let entry = make_entry("a1", MemoryTier::Ephemeral, 50, 2);
        mem.store(entry);

        // Run promotion check
        maintenance.process_promotions(&mem, "a1");

        // Entry should NOT be promoted
        let working = mem.get_tier("a1", MemoryTier::Working);
        assert_eq!(working.len(), 0);
        let ephemeral = mem.get_tier("a1", MemoryTier::Ephemeral);
        assert_eq!(ephemeral.len(), 1);
    }

    #[test]
    fn test_no_promotion_working_low_importance() {
        let mem = LayeredMemory::new();
        let maintenance = TierMaintenance::new();

        // Store working entry with access_count >= 10 but importance < 50
        let entry = make_entry("a1", MemoryTier::Working, 30, 10);
        mem.store(entry);

        // Run promotion check
        maintenance.process_promotions(&mem, "a1");

        // Entry should NOT be promoted to LongTerm
        let longterm = mem.get_tier("a1", MemoryTier::LongTerm);
        assert_eq!(longterm.len(), 0);
        let working = mem.get_tier("a1", MemoryTier::Working);
        assert_eq!(working.len(), 1);
    }

    #[test]
    fn test_eviction_low_importance() {
        let mem = LayeredMemory::new();
        let maintenance = TierMaintenance::new();

        // Store ephemeral entries with importance < 70
        mem.store(make_entry("a1", MemoryTier::Ephemeral, 69, 0));
        mem.store(make_entry("a1", MemoryTier::Ephemeral, 50, 0));
        mem.store(make_entry("a1", MemoryTier::Ephemeral, 30, 0));

        // Run eviction
        let discarded = maintenance.process_ephemeral_eviction(&mem, "a1");

        // All should be discarded (returned)
        assert_eq!(discarded.len(), 3);

        // Ephemeral should be empty
        let ephemeral = mem.get_tier("a1", MemoryTier::Ephemeral);
        assert_eq!(ephemeral.len(), 0);
    }

    #[test]
    fn test_maintenance_stats() {
        let mem = LayeredMemory::new();
        let maintenance = TierMaintenance::new();

        // Add some entries
        mem.store(make_entry("a1", MemoryTier::Ephemeral, 80, 3)); // will promote
        mem.store(make_entry("a1", MemoryTier::Ephemeral, 69, 0)); // will evict

        let stats = maintenance.run_maintenance_cycle(&mem, "a1");

        // High-importance ephemeral should be promoted, low-importance evicted
        assert!(stats.evicted_count >= 1 || stats.promoted_count >= 1);
    }
}
