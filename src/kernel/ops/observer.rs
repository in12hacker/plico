//! Observer+Reflector — async memory pattern detection and consolidation.
//!
//! The Observer runs lightweight heuristics on memory writes to detect:
//! - **Duplicates**: new content is very similar to an existing entry
//! - **Contradictions**: new content contradicts an existing entry
//!
//! Detected patterns are sent to a bounded channel for the Reflector to process.
//! The Reflector periodically drains the queue and executes consolidation actions
//! (merge duplicates, supersede contradicted entries).

use std::sync::{Arc, OnceLock, Mutex};
use tokio::sync::mpsc;

use crate::memory::layered::{LayeredMemory, MemoryEntry, MemoryContent};

/// Global observer channel sender. Set once via `init_observer`.
static OBSERVER_TX: OnceLock<mpsc::Sender<Observation>> = OnceLock::new();

/// Global reflector. Set once via `init_observer`.
static REFLECTOR: OnceLock<Mutex<Reflector>> = OnceLock::new();

/// Maximum number of observations queued before oldest are dropped.
const OBSERVATION_QUEUE_SIZE: usize = 256;

/// An observation about a memory write pattern.
#[derive(Debug, Clone)]
pub enum Observation {
    /// New entry is very similar to an existing one (potential duplicate).
    Duplicate {
        agent_id: String,
        new_entry_id: String,
        existing_entry_id: String,
        similarity: f32,
    },
    /// New entry contradicts an existing one.
    Contradiction {
        agent_id: String,
        new_entry_id: String,
        contradicted_entry_id: String,
    },
}

/// Observer — lightweight pattern detector for memory writes.
///
/// Runs inline during `remember` / `remember_working` (non-blocking heuristic).
/// Sends observations to the Reflector via a bounded channel.
pub struct Observer {
    tx: mpsc::Sender<Observation>,
}

impl Observer {
    /// Create a new Observer with the given channel sender.
    pub fn new(tx: mpsc::Sender<Observation>) -> Self {
        Self { tx }
    }

    /// Check a newly stored entry against existing memories.
    ///
    /// Runs fast heuristics (substring + keyword overlap). Does NOT block on
    /// embedding or LLM calls. Sends observations to the Reflector channel.
    pub fn check(&self, agent_id: &str, new_entry: &MemoryEntry, memory: &LayeredMemory) {
        let existing = memory.get_active(agent_id);
        let new_text = match &new_entry.content {
            MemoryContent::Text(s) => s.as_str(),
            _ => return,
        };

        for entry in &existing {
            if entry.id == new_entry.id {
                continue;
            }
            let existing_text = match &entry.content {
                MemoryContent::Text(s) => s.as_str(),
                _ => continue,
            };

            // Duplicate detection: high text overlap
            let sim = text_similarity(new_text, existing_text);
            if sim > 0.85 {
                let _ = self.tx.try_send(Observation::Duplicate {
                    agent_id: agent_id.to_string(),
                    new_entry_id: new_entry.id.clone(),
                    existing_entry_id: entry.id.clone(),
                    similarity: sim,
                });
                continue;
            }

            // Contradiction detection: keyword "not", "no longer", "incorrect"
            if is_contradictory(new_text, existing_text) {
                let _ = self.tx.try_send(Observation::Contradiction {
                    agent_id: agent_id.to_string(),
                    new_entry_id: new_entry.id.clone(),
                    contradicted_entry_id: entry.id.clone(),
                });
            }
        }
    }
}

/// Reflector — periodic consolidation of observed patterns.
///
/// Drains the observation queue and executes:
/// - Duplicate merge: supersede the older entry (keep newer)
/// - Contradiction resolution: supersede the contradicted entry
pub struct Reflector {
    rx: mpsc::Receiver<Observation>,
    memory: Arc<LayeredMemory>,
}

impl Reflector {
    /// Create a new Reflector.
    pub fn new(rx: mpsc::Receiver<Observation>, memory: Arc<LayeredMemory>) -> Self {
        Self { rx, memory }
    }

    /// Drain all pending observations and execute consolidation actions.
    ///
    /// Returns the number of actions taken.
    pub fn reflect(&mut self) -> usize {
        let mut actions = 0;
        while let Ok(obs) = self.rx.try_recv() {
            match obs {
                Observation::Duplicate {
                    agent_id,
                    new_entry_id,
                    existing_entry_id,
                    similarity: _,
                } => {
                    // Keep newer entry, supersede older one.
                    // Since new_entry was just written, supersede the existing one.
                    if self.memory.mark_superseded(&agent_id, &existing_entry_id, &new_entry_id) {
                        tracing::debug!(
                            agent = %agent_id,
                            kept = %new_entry_id,
                            superseded = %existing_entry_id,
                            "Reflector: merged duplicate"
                        );
                        actions += 1;
                    }
                }
                Observation::Contradiction {
                    agent_id,
                    new_entry_id,
                    contradicted_entry_id,
                } => {
                    // New entry contradicts old — supersede the old one.
                    if self.memory.mark_superseded(&agent_id, &contradicted_entry_id, &new_entry_id) {
                        tracing::debug!(
                            agent = %agent_id,
                            kept = %new_entry_id,
                            superseded = %contradicted_entry_id,
                            "Reflector: resolved contradiction"
                        );
                        actions += 1;
                    }
                }
            }
        }
        actions
    }
}

/// Create the Observer+Reflector channel pair.
pub fn create_observer_pair(
    memory: Arc<LayeredMemory>,
) -> (Observer, Reflector) {
    let (tx, rx) = mpsc::channel(OBSERVATION_QUEUE_SIZE);
    (Observer::new(tx), Reflector::new(rx, memory))
}

/// Initialize the global Observer+Reflector.
///
/// Must be called once during kernel startup. Subsequent calls are no-ops.
pub fn init_observer(memory: Arc<LayeredMemory>) {
    let (tx, rx) = mpsc::channel(OBSERVATION_QUEUE_SIZE);
    let _ = OBSERVER_TX.set(tx);
    let _ = REFLECTOR.set(Mutex::new(Reflector::new(rx, memory)));
}

/// Check a newly stored entry against existing memories using the global Observer.
///
/// No-op if observer is not initialized. Non-blocking (uses try_send).
pub fn check_memory_write(agent_id: &str, entry: &MemoryEntry, memory: &LayeredMemory) {
    if let Some(tx) = OBSERVER_TX.get() {
        let observer = Observer::new(tx.clone());
        observer.check(agent_id, entry, memory);
    }
}

/// Run the global Reflector to process pending observations.
///
/// Returns the number of consolidation actions taken. No-op if not initialized.
pub fn run_reflector() -> usize {
    if let Some(reflector) = REFLECTOR.get() {
        if let Ok(mut r) = reflector.lock() {
            return r.reflect();
        }
    }
    0
}

/// Fast text similarity using word-level Jaccard index.
fn text_similarity(a: &str, b: &str) -> f32 {
    let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();
    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }
    let intersection = words_a.intersection(&words_b).count() as f32;
    let union = words_a.union(&words_b).count() as f32;
    if union < 1e-10 { 0.0 } else { intersection / union }
}

/// Check if two texts are likely contradictory using keyword heuristics.
fn is_contradictory(new: &str, existing: &str) -> bool {
    let negation_markers = ["not ", "no longer", "incorrect", "wrong", "false", "reversed", "changed to"];
    let new_lower = new.to_lowercase();
    let existing_lower = existing.to_lowercase();

    // Check if new text contains negation markers that reference existing content
    for marker in &negation_markers {
        if new_lower.contains(marker) {
            // Check if there's significant word overlap (same topic)
            let sim = text_similarity(&new_lower, &existing_lower);
            if sim > 0.3 {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_similarity_identical() {
        assert!((text_similarity("hello world", "hello world") - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_text_similarity_no_overlap() {
        assert!(text_similarity("hello", "world") < 0.1);
    }

    #[test]
    fn test_text_similarity_partial() {
        let sim = text_similarity("the cat sat on the mat", "the cat sat on a mat");
        assert!(sim > 0.6);
    }

    #[test]
    fn test_is_contradictory() {
        assert!(is_contradictory(
            "The server is no longer using port 8080",
            "The server uses port 8080 for API requests"
        ));
    }

    #[test]
    fn test_is_not_contradictory() {
        assert!(!is_contradictory(
            "The server uses port 8080",
            "The database runs on port 5432"
        ));
    }

    #[tokio::test]
    async fn test_observer_detects_duplicate() {
        use std::sync::Arc;
        let memory = Arc::new(LayeredMemory::new());
        let (observer, mut reflector) = create_observer_pair(memory.clone());

        // Store an existing entry
        let existing = MemoryEntry::ephemeral("agent-1", "The quick brown fox jumps over the lazy dog");
        memory.store(existing.clone());

        // Check a near-duplicate
        let new_entry = MemoryEntry::ephemeral("agent-1", "The quick brown fox jumps over the lazy dog");
        observer.check("agent-1", &new_entry, &memory);

        // Reflector should process the duplicate
        let actions = reflector.reflect();
        assert!(actions >= 1, "Should detect at least 1 duplicate");
    }

    #[tokio::test]
    async fn test_observer_detects_contradiction() {
        use std::sync::Arc;
        let memory = Arc::new(LayeredMemory::new());
        let (observer, mut reflector) = create_observer_pair(memory.clone());

        // Store an existing entry
        let existing = MemoryEntry::ephemeral("agent-1", "The server uses port 8080 for API");
        memory.store(existing.clone());

        // Check a contradictory entry
        let new_entry = MemoryEntry::ephemeral("agent-1", "The server is not using port 8080 anymore");
        observer.check("agent-1", &new_entry, &memory);

        let actions = reflector.reflect();
        assert!(actions >= 1, "Should detect at least 1 contradiction");
    }

    #[tokio::test]
    async fn test_reflector_supersedes_old_entry() {
        use std::sync::Arc;
        let memory = Arc::new(LayeredMemory::new());
        let (observer, mut reflector) = create_observer_pair(memory.clone());

        let old = MemoryEntry::ephemeral("agent-1", "Important fact: X is true");
        memory.store(old.clone());

        let new = MemoryEntry::ephemeral("agent-1", "Important fact: X is true");
        observer.check("agent-1", &new, &memory);
        reflector.reflect();

        // Old entry should be superseded
        let all = memory.get_all("agent-1");
        let old_entry = all.iter().find(|e| e.id == old.id).unwrap();
        assert!(old_entry.superseded_by.is_some(), "Old entry should be marked as superseded");
    }
}
