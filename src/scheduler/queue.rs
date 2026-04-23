//! Priority-Based Intent Queue
//!
//! Binary-heap scheduler queue ordering intents by priority (Critical > High > Medium > Low)
//! with timestamp tiebreaking (older intents dispatched first at equal priority).

use std::collections::BinaryHeap;
use std::cmp::Ordering;

use super::agent::{Intent, IntentId};

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("Queue is empty")]
    Empty,

    #[error("Intent not found: {0}")]
    IntentNotFound(IntentId),
}

/// Scheduler queue — min-heap ordered by (priority DESC, timestamp ASC).
pub struct SchedulerQueue {
    heap: BinaryHeap<IntentEntry>,
}

#[derive(Debug, Clone)]
struct IntentEntry {
    intent: Intent,
    /// Secondary sort key: submission timestamp (older = higher priority)
    timestamp: u64,
}

impl PartialEq for IntentEntry {
    fn eq(&self, other: &Self) -> bool {
        self.intent.id == other.intent.id
    }
}

impl Eq for IntentEntry {}

impl PartialOrd for IntentEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IntentEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority first, then older (lower timestamp) first (FIFO within same priority).
        // BinaryHeap is a max-heap: higher Ordering = popped first.
        // For timestamp: reverse comparison so older (lower) timestamps rank higher.
        self.intent
            .priority
            .cmp(&other.intent.priority)
            .then_with(|| other.timestamp.cmp(&self.timestamp))
    }
}

impl SchedulerQueue {
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
        }
    }

    /// Add an intent to the queue.
    pub fn push(&mut self, intent: Intent) {
        let timestamp = intent.submitted_at;
        self.heap.push(IntentEntry { intent, timestamp });
    }

    /// Pop the highest-priority, oldest intent.
    pub fn pop(&mut self) -> Option<Intent> {
        self.heap.pop().map(|e| e.intent)
    }

    /// Peek at the next intent without removing it.
    pub fn peek(&self) -> Option<&Intent> {
        self.heap.peek().map(|e| &e.intent)
    }

    /// Number of pending intents.
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

impl Default for SchedulerQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::agent::{Intent, IntentId, IntentPriority};

    fn make_intent(priority: IntentPriority, submitted_at: u64) -> Intent {
        Intent {
            id: IntentId(uuid::Uuid::new_v4().to_string()),
            description: "test".into(),
            priority,
            action: None,
            agent_id: None,
            submitted_at,
        }
    }

    #[test]
    fn test_push_pop_fifo_same_priority() {
        let mut q = SchedulerQueue::new();
        let i1 = make_intent(IntentPriority::Medium, 100);
        let i2 = make_intent(IntentPriority::Medium, 200);
        let id1 = i1.id.clone();
        let id2 = i2.id.clone();
        q.push(i1);
        q.push(i2);
        assert_eq!(q.len(), 2);
        assert_eq!(q.pop().unwrap().id, id1);
        assert_eq!(q.pop().unwrap().id, id2);
    }

    #[test]
    fn test_priority_ordering() {
        let mut q = SchedulerQueue::new();
        let low = make_intent(IntentPriority::Low, 100);
        let critical = make_intent(IntentPriority::Critical, 200);
        let high = make_intent(IntentPriority::High, 150);
        let crit_id = critical.id.clone();
        let high_id = high.id.clone();
        let low_id = low.id.clone();
        q.push(low);
        q.push(critical);
        q.push(high);
        assert_eq!(q.pop().unwrap().id, crit_id);
        assert_eq!(q.pop().unwrap().id, high_id);
        assert_eq!(q.pop().unwrap().id, low_id);
    }

    #[test]
    fn test_empty_queue() {
        let mut q = SchedulerQueue::new();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
        assert!(q.pop().is_none());
        assert!(q.peek().is_none());
    }

    #[test]
    fn test_peek_does_not_remove() {
        let mut q = SchedulerQueue::new();
        let i = make_intent(IntentPriority::Medium, 100);
        let id = i.id.clone();
        q.push(i);
        assert_eq!(q.peek().unwrap().id, id);
        assert_eq!(q.len(), 1);
        assert_eq!(q.pop().unwrap().id, id);
        assert!(q.is_empty());
    }
}
