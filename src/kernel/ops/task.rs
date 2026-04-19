//! TaskStore — Multi-Agent Task Delegation (F-14).
//!
//! Manages task state transitions (Pending → InProgress → Completed/Failed)
//! for single-node Agent间委托. Tasks persist to disk following P-0规范.
//!
//! Design: F-14 in docs/design-node4-collaborative-ecosystem.md

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use crate::api::semantic::TaskStatus;
use crate::kernel::event_bus::{EventBus, KernelEvent};
use crate::kernel::persistence::atomic_write_json;

/// A delegated task with full state tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique task identifier (provided by caller).
    pub task_id: String,
    /// Agent that delegated the task.
    pub from_agent: String,
    /// Agent assigned to execute the task.
    pub to_agent: String,
    /// Natural-language intent description for the task.
    pub intent: String,
    /// Content CIDs providing context for the task.
    pub context_cids: Vec<String>,
    /// Optional deadline in milliseconds (Unix timestamp).
    /// Task auto-transitions to Failed when deadline expires.
    pub deadline_ms: Option<u64>,
    /// Current task status.
    pub status: TaskStatus,
    /// Result CIDs produced by the task (populated on Completed).
    pub result_cids: Vec<String>,
    /// When the task was created.
    pub created_at_ms: u64,
    /// When the task was last updated.
    pub updated_at_ms: u64,
    /// Optional failure reason (populated on Failed).
    pub failure_reason: Option<String>,
}

impl Task {
    /// Create a new task in Pending state.
    pub fn new(
        task_id: String,
        from_agent: String,
        to_agent: String,
        intent: String,
        context_cids: Vec<String>,
        deadline_ms: Option<u64>,
    ) -> Self {
        let now = now_ms();
        Self {
            task_id,
            from_agent,
            to_agent,
            intent,
            context_cids,
            deadline_ms,
            status: TaskStatus::Pending,
            result_cids: Vec::new(),
            created_at_ms: now,
            updated_at_ms: now,
            failure_reason: None,
        }
    }

    /// Transition to InProgress.
    pub fn start(&mut self) {
        self.status = TaskStatus::InProgress;
        self.updated_at_ms = now_ms();
    }

    /// Mark as Completed with result CIDs.
    pub fn complete(&mut self, result_cids: Vec<String>) {
        self.status = TaskStatus::Completed;
        self.result_cids = result_cids;
        self.updated_at_ms = now_ms();
    }

    /// Mark as Failed with reason.
    pub fn fail(&mut self, reason: String) {
        self.status = TaskStatus::Failed;
        self.failure_reason = Some(reason);
        self.updated_at_ms = now_ms();
    }

    /// Check if the task has exceeded its deadline.
    pub fn is_expired(&self) -> bool {
        if let Some(deadline) = self.deadline_ms {
            now_ms() > deadline
        } else {
            false
        }
    }
}

/// TaskStore — manages all delegated tasks for single-node Agent间委托.
///
/// TaskStore follows P-0 persistence规范:
/// - persist() saves all tasks to disk atomically
/// - restore() loads tasks from disk on startup
///
/// Task state transitions:
/// ```text
/// Pending → InProgress → Completed
///                      ↘ Failed (deadline expired or explicit fail)
/// ```
pub struct TaskStore {
    /// In-memory task registry: task_id → Task.
    tasks: RwLock<HashMap<String, Task>>,
    /// Root path for persistence.
    root: PathBuf,
    /// Event bus for emitting TaskDelegated/TaskCompleted events.
    event_bus: Arc<EventBus>,
}

impl TaskStore {
    /// Create a new TaskStore (does not restore).
    pub fn new(root: PathBuf, event_bus: Arc<EventBus>) -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            root,
            event_bus,
        }
    }

    /// Restore TaskStore from disk (P-0 persist restore).
    pub fn restore(root: PathBuf, event_bus: Arc<EventBus>) -> Self {
        let path = root.join("tasks.json");
        let tasks = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(json) => {
                    match serde_json::from_str::<HashMap<String, Task>>(&json) {
                        Ok(t) => {
                            tracing::info!("Restored {} tasks from persistent storage", t.len());
                            t
                        }
                        Err(e) => {
                            tracing::warn!("Failed to parse tasks.json: {e}. Starting fresh.");
                            HashMap::new()
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to read tasks.json: {e}. Starting fresh.");
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        Self {
            tasks: RwLock::new(tasks),
            root,
            event_bus,
        }
    }

    /// Persist all tasks to disk atomically (P-0 persist).
    pub fn persist(&self) {
        let tasks = self.tasks.read().unwrap();
        let path = self.root.join("tasks.json");
        atomic_write_json(&path, &*tasks);
        tracing::debug!("Persisted {} tasks to disk", tasks.len());
    }

    /// Create a new task and emit TaskDelegated event.
    ///
    /// Returns the created task.
    pub fn create_task(
        &self,
        task_id: String,
        from_agent: String,
        to_agent: String,
        intent: String,
        context_cids: Vec<String>,
        deadline_ms: Option<u64>,
    ) -> Task {
        let task = Task::new(task_id, from_agent.clone(), to_agent.clone(), intent, context_cids, deadline_ms);

        // Emit TaskDelegated event for the target agent to subscribe to
        self.event_bus.emit(KernelEvent::TaskDelegated {
            task_id: task.task_id.clone(),
            from_agent,
            to_agent,
        });

        let mut tasks = self.tasks.write().unwrap();
        tasks.insert(task.task_id.clone(), task.clone());
        task
    }

    /// Get a task by ID.
    pub fn get(&self, task_id: &str) -> Option<Task> {
        let tasks = self.tasks.read().unwrap();
        tasks.get(task_id).cloned()
    }

    /// Transition a task to InProgress (target agent picks up the task).
    pub fn start_task(&self, task_id: &str, agent_id: &str) -> Result<Task, String> {
        let mut tasks = self.tasks.write().unwrap();
        let task = tasks.get_mut(task_id)
            .ok_or_else(|| format!("Task not found: {}", task_id))?;

        if task.to_agent != agent_id {
            return Err(format!("Task {} is assigned to {}, not {}", task_id, task.to_agent, agent_id));
        }

        if task.status != TaskStatus::Pending {
            return Err(format!("Task {} is not pending (current status: {:?})", task_id, task.status));
        }

        task.start();
        let updated = task.clone();
        Ok(updated)
    }

    /// Complete a task with result CIDs (target agent reports completion).
    pub fn complete_task(&self, task_id: &str, agent_id: &str, result_cids: Vec<String>) -> Result<Task, String> {
        let mut tasks = self.tasks.write().unwrap();
        let task = tasks.get_mut(task_id)
            .ok_or_else(|| format!("Task not found: {}", task_id))?;

        if task.to_agent != agent_id {
            return Err(format!("Task {} is assigned to {}, not {}", task_id, task.to_agent, agent_id));
        }

        if task.status != TaskStatus::InProgress {
            return Err(format!("Task {} is not in_progress (current status: {:?})", task_id, task.status));
        }

        task.complete(result_cids.clone());

        // Emit TaskCompleted event
        drop(tasks);
        self.event_bus.emit(KernelEvent::TaskCompleted {
            task_id: task_id.to_string(),
            agent_id: agent_id.to_string(),
            result_cids,
        });

        let tasks = self.tasks.read().unwrap();
        Ok(tasks.get(task_id).cloned().unwrap())
    }

    /// Fail a task with reason (target agent reports failure or deadline expired).
    pub fn fail_task(&self, task_id: &str, agent_id: &str, reason: String) -> Result<Task, String> {
        let mut tasks = self.tasks.write().unwrap();
        let task = tasks.get_mut(task_id)
            .ok_or_else(|| format!("Task not found: {}", task_id))?;

        if task.to_agent != agent_id {
            return Err(format!("Task {} is assigned to {}, not {}", task_id, task.to_agent, agent_id));
        }

        if task.status == TaskStatus::Completed || task.status == TaskStatus::Failed {
            return Err(format!("Task {} is already {:?} (cannot fail)", task_id, task.status));
        }

        task.fail(reason);
        let updated = task.clone();
        Ok(updated)
    }

    /// Check for and expire tasks past their deadline.
    /// Returns list of (task_id, to_agent) for expired tasks.
    pub fn check_deadlines(&self) -> Vec<(String, String)> {
        let mut tasks = self.tasks.write().unwrap();
        let mut expired = Vec::new();

        for (id, task) in tasks.iter_mut() {
            if task.is_expired() && task.status == TaskStatus::Pending {
                task.fail("Deadline expired".to_string());
                expired.push((id.clone(), task.to_agent.clone()));
            }
        }

        if !expired.is_empty() {
            tracing::info!("Expired {} tasks past deadline", expired.len());
        }

        expired
    }

    /// List all tasks assigned to a specific agent.
    pub fn tasks_for_agent(&self, agent_id: &str) -> Vec<Task> {
        let tasks = self.tasks.read().unwrap();
        tasks.values()
            .filter(|t| t.to_agent == agent_id)
            .cloned()
            .collect()
    }

    /// List all tasks owned by a specific agent (delegated by).
    pub fn tasks_by_agent(&self, agent_id: &str) -> Vec<Task> {
        let tasks = self.tasks.read().unwrap();
        tasks.values()
            .filter(|t| t.from_agent == agent_id)
            .cloned()
            .collect()
    }

    /// Get total task count.
    pub fn len(&self) -> usize {
        let tasks = self.tasks.read().unwrap();
        tasks.len()
    }

    /// Check if store is empty.
    pub fn is_empty(&self) -> bool {
        let tasks = self.tasks.read().unwrap();
        tasks.is_empty()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (TaskStore, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let store = TaskStore::new(tmp.path().to_path_buf(), Arc::new(EventBus::new()));
        (store, tmp)
    }

    #[test]
    fn test_task_creation() {
        let (store, _tmp) = temp_store();
        let task = store.create_task(
            "t1".to_string(),
            "agent-a".to_string(),
            "agent-b".to_string(),
            "Analyze this document".to_string(),
            vec!["cid1".to_string()],
            None,
        );

        assert_eq!(task.task_id, "t1");
        assert_eq!(task.status, TaskStatus::Pending);
        assert!(task.result_cids.is_empty());
        assert!(task.failure_reason.is_none());
    }

    #[test]
    fn test_task_start() {
        let (store, _tmp) = temp_store();
        store.create_task(
            "t1".to_string(),
            "agent-a".to_string(),
            "agent-b".to_string(),
            "Analyze".to_string(),
            vec![],
            None,
        );

        let updated = store.start_task("t1", "agent-b").unwrap();
        assert_eq!(updated.status, TaskStatus::InProgress);
    }

    #[test]
    fn test_task_complete() {
        let (store, _tmp) = temp_store();
        store.create_task(
            "t1".to_string(),
            "agent-a".to_string(),
            "agent-b".to_string(),
            "Analyze".to_string(),
            vec![],
            None,
        );
        store.start_task("t1", "agent-b").unwrap();

        let completed = store.complete_task("t1", "agent-b", vec!["result-cid".to_string()]).unwrap();
        assert_eq!(completed.status, TaskStatus::Completed);
        assert_eq!(completed.result_cids, vec!["result-cid".to_string()]);
    }

    #[test]
    fn test_task_fail() {
        let (store, _tmp) = temp_store();
        store.create_task(
            "t1".to_string(),
            "agent-a".to_string(),
            "agent-b".to_string(),
            "Analyze".to_string(),
            vec![],
            None,
        );
        store.start_task("t1", "agent-b").unwrap();

        let failed = store.fail_task("t1", "agent-b", "Analysis failed".to_string()).unwrap();
        assert_eq!(failed.status, TaskStatus::Failed);
        assert_eq!(failed.failure_reason, Some("Analysis failed".to_string()));
    }

    #[test]
    fn test_task_deadline() {
        let (store, _tmp) = temp_store();
        let task = store.create_task(
            "t1".to_string(),
            "agent-a".to_string(),
            "agent-b".to_string(),
            "Analyze".to_string(),
            vec![],
            Some(now_ms() - 1), // deadline already passed
        );

        assert!(task.is_expired());
    }

    #[test]
    fn test_task_not_found() {
        let (store, _tmp) = temp_store();
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn test_wrong_agent_start() {
        let (store, _tmp) = temp_store();
        store.create_task(
            "t1".to_string(),
            "agent-a".to_string(),
            "agent-b".to_string(),
            "Analyze".to_string(),
            vec![],
            None,
        );

        let err = store.start_task("t1", "wrong-agent").unwrap_err();
        assert!(err.contains("assigned to agent-b"));
    }

    #[test]
    fn test_already_started() {
        let (store, _tmp) = temp_store();
        store.create_task(
            "t1".to_string(),
            "agent-a".to_string(),
            "agent-b".to_string(),
            "Analyze".to_string(),
            vec![],
            None,
        );
        store.start_task("t1", "agent-b").unwrap();

        let err = store.start_task("t1", "agent-b").unwrap_err();
        assert!(err.contains("not pending"));
    }

    #[test]
    fn test_tasks_for_agent() {
        let (store, _tmp) = temp_store();
        store.create_task("t1".to_string(), "a".to_string(), "b".to_string(), "T1".to_string(), vec![], None);
        store.create_task("t2".to_string(), "a".to_string(), "c".to_string(), "T2".to_string(), vec![], None);
        store.create_task("t3".to_string(), "b".to_string(), "c".to_string(), "T3".to_string(), vec![], None);

        let b_tasks = store.tasks_for_agent("b");
        assert_eq!(b_tasks.len(), 1);
        assert_eq!(b_tasks[0].task_id, "t1");

        let c_tasks = store.tasks_for_agent("c");
        assert_eq!(c_tasks.len(), 2);
    }

    #[test]
    fn test_persist_and_restore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let event_bus = Arc::new(EventBus::new());

        {
            let store = TaskStore::new(root.clone(), event_bus.clone());
            store.create_task(
                "t1".to_string(),
                "agent-a".to_string(),
                "agent-b".to_string(),
                "Analyze".to_string(),
                vec!["cid1".to_string()],
                Some(now_ms() + 10000),
            );
            store.persist();
        }

        // Restore from disk
        let store = TaskStore::restore(root, event_bus);
        let task = store.get("t1").unwrap();
        assert_eq!(task.task_id, "t1");
        assert_eq!(task.from_agent, "agent-a");
        assert_eq!(task.to_agent, "agent-b");
        assert_eq!(task.context_cids, vec!["cid1".to_string()]);
    }
}
