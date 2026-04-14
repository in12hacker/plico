//! Priority-based intent queue for the scheduler.

use std::collections::BinaryHeap;
use std::cmp::Ordering;

use super::agent::{Intent, IntentId, IntentPriority};

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
        // Higher priority first, then older (lower timestamp) first
        self.intent
            .priority
            .cmp(&other.intent.priority)
            .then_with(|| self.timestamp.cmp(&other.timestamp))
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
