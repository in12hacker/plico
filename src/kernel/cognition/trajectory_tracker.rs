//! 认知轨迹追踪器 —— 记录和分析 Agent 的认知轨迹
//!
//! 追踪 Agent 的意图、操作、成功/失败，为认知优化提供数据基础。

use std::collections::HashMap;
use tokio::sync::RwLock;

use super::{TrajectoryPoint, now_ms};

/// 失败记录
#[derive(Debug, Clone)]
pub struct FailureRecord {
    pub session_id: String,
    pub intent: String,
    pub operation: String,
    pub timestamp_ms: u64,
    pub context_cids: Vec<String>,
}

/// 认知轨迹追踪器
pub struct TrajectoryTracker {
    /// Agent ID -> 轨迹点列表
    trajectories: RwLock<HashMap<String, Vec<TrajectoryPoint>>>,
    
    /// Agent ID -> 失败记录列表
    failures: RwLock<HashMap<String, Vec<FailureRecord>>>,
    
    /// 最大保留轨迹长度（防止内存无限增长）
    max_trajectory_len: usize,
    
    /// 最大保留失败记录数
    max_failure_records: usize,
}

impl Default for TrajectoryTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl TrajectoryTracker {
    pub fn new() -> Self {
        Self {
            trajectories: RwLock::new(HashMap::new()),
            failures: RwLock::new(HashMap::new()),
            max_trajectory_len: 10000,
            max_failure_records: 1000,
        }
    }

    /// 记录意图声明
    pub async fn record_intent(&self, agent_id: &str, intent: &str) {
        let point = TrajectoryPoint {
            timestamp_ms: now_ms(),
            intent: intent.to_string(),
            operation: "declare_intent".to_string(),
            success: true,
            context_cids: Vec::new(),
        };

        let mut trajectories = self.trajectories.write().await;
        let entry = trajectories.entry(agent_id.to_string()).or_default();
        entry.push(point);

        // 限制长度
        if entry.len() > self.max_trajectory_len {
            let excess = entry.len() - self.max_trajectory_len;
            entry.drain(0..excess);
        }
    }

    /// 记录操作完成
    pub async fn record_operation(&self, agent_id: &str, operation: &str, success: bool) {
        let point = TrajectoryPoint {
            timestamp_ms: now_ms(),
            intent: String::new(), // Will be filled from last intent if needed
            operation: operation.to_string(),
            success,
            context_cids: Vec::new(),
        };

        let mut trajectories = self.trajectories.write().await;
        let entry = trajectories.entry(agent_id.to_string()).or_default();
        entry.push(point);

        if entry.len() > self.max_trajectory_len {
            let excess = entry.len() - self.max_trajectory_len;
            entry.drain(0..excess);
        }
    }

    /// 记录失败
    pub async fn record_failure(&self, agent_id: &str, operation: &str) {
        let record = FailureRecord {
            session_id: "unknown".to_string(), // TODO: track session
            intent: String::new(),
            operation: operation.to_string(),
            timestamp_ms: now_ms(),
            context_cids: Vec::new(),
        };

        let mut failures = self.failures.write().await;
        let entry = failures.entry(agent_id.to_string()).or_default();
        entry.push(record);

        if entry.len() > self.max_failure_records {
            let excess = entry.len() - self.max_failure_records;
            entry.drain(0..excess);
        }
    }

    /// 获取 Agent 的完整轨迹
    pub async fn get_trajectory(&self, agent_id: &str) -> Vec<TrajectoryPoint> {
        let trajectories = self.trajectories.read().await;
        trajectories.get(agent_id).cloned().unwrap_or_default()
    }

    /// 获取最近 N 条轨迹
    pub async fn get_recent_trajectory(&self, agent_id: &str, n: usize) -> Vec<TrajectoryPoint> {
        let trajectories = self.trajectories.read().await;
        if let Some(traj) = trajectories.get(agent_id) {
            let start = traj.len().saturating_sub(n);
            traj[start..].to_vec()
        } else {
            Vec::new()
        }
    }

    /// 获取最近 N 次失败
    pub async fn get_recent_failures(&self, agent_id: &str, n: usize) -> Vec<FailureRecord> {
        let failures = self.failures.read().await;
        if let Some(records) = failures.get(agent_id) {
            let start = records.len().saturating_sub(n);
            records[start..].to_vec()
        } else {
            Vec::new()
        }
    }

    /// 获取失败统计
    pub async fn get_failure_stats(&self, agent_id: &str) -> FailureStats {
        let failures = self.failures.read().await;
        let records = failures.get(agent_id).cloned().unwrap_or_default();

        let total = records.len();
        let by_operation: HashMap<String, usize> = records.iter()
            .fold(HashMap::new(), |mut acc, r| {
                *acc.entry(r.operation.clone()).or_default() += 1;
                acc
            });

        FailureStats {
            total_failures: total,
            by_operation,
            most_recent_ms: records.last().map(|r| r.timestamp_ms),
        }
    }

    /// 清空 Agent 轨迹
    pub async fn clear_trajectory(&self, agent_id: &str) {
        let mut trajectories = self.trajectories.write().await;
        trajectories.remove(agent_id);
        let mut failures = self.failures.write().await;
        failures.remove(agent_id);
    }
}

/// 失败统计
#[derive(Debug, Clone)]
pub struct FailureStats {
    pub total_failures: usize,
    pub by_operation: HashMap<String, usize>,
    pub most_recent_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_creates_empty_tracker() {
        let tracker = TrajectoryTracker::new();
        assert!(tracker.get_trajectory("agent1").await.is_empty());
        assert!(tracker.get_recent_failures("agent1", 10).await.is_empty());
    }

    #[tokio::test]
    async fn record_intent_stores_intent() {
        let tracker = TrajectoryTracker::new();
        tracker.record_intent("agent1", "test intent").await;
        let traj = tracker.get_trajectory("agent1").await;
        assert_eq!(traj.len(), 1);
        assert_eq!(traj[0].intent, "test intent");
        assert_eq!(traj[0].operation, "declare_intent");
        assert!(traj[0].success);
    }

    #[tokio::test]
    async fn record_operation_stores_operation() {
        let tracker = TrajectoryTracker::new();
        tracker.record_operation("agent1", "do_something", true).await;
        let traj = tracker.get_trajectory("agent1").await;
        assert_eq!(traj.len(), 1);
        assert_eq!(traj[0].operation, "do_something");
        assert!(traj[0].success);

        tracker.record_operation("agent1", "do_another", false).await;
        let traj = tracker.get_trajectory("agent1").await;
        assert_eq!(traj.len(), 2);
        assert!(!traj[1].success);
    }

    #[tokio::test]
    async fn record_failure_stores_failure() {
        let tracker = TrajectoryTracker::new();
        tracker.record_failure("agent1", "failed_op").await;
        let failures = tracker.get_recent_failures("agent1", 10).await;
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].operation, "failed_op");
    }

    #[tokio::test]
    async fn get_trajectory_returns_full_history() {
        let tracker = TrajectoryTracker::new();
        tracker.record_intent("agent1", "intent1").await;
        tracker.record_operation("agent1", "op1", true).await;
        tracker.record_operation("agent1", "op2", false).await;

        let traj = tracker.get_trajectory("agent1").await;
        assert_eq!(traj.len(), 3);
    }

    #[tokio::test]
    async fn get_recent_trajectory_returns_last_n() {
        let tracker = TrajectoryTracker::new();
        tracker.record_intent("agent1", "intent1").await;
        tracker.record_operation("agent1", "op1", true).await;
        tracker.record_operation("agent1", "op2", false).await;

        let recent = tracker.get_recent_trajectory("agent1", 2).await;
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].operation, "op1");
        assert_eq!(recent[1].operation, "op2");

        let recent = tracker.get_recent_trajectory("agent1", 10).await;
        assert_eq!(recent.len(), 3);
    }

    #[tokio::test]
    async fn get_recent_failures_returns_last_n() {
        let tracker = TrajectoryTracker::new();
        tracker.record_failure("agent1", "fail1").await;
        tracker.record_failure("agent1", "fail2").await;
        tracker.record_failure("agent1", "fail3").await;

        let recent = tracker.get_recent_failures("agent1", 2).await;
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].operation, "fail2");
        assert_eq!(recent[1].operation, "fail3");

        let all = tracker.get_recent_failures("agent1", 10).await;
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn get_failure_stats_returns_correct_counts() {
        let tracker = TrajectoryTracker::new();
        tracker.record_failure("agent1", "op_a").await;
        tracker.record_failure("agent1", "op_a").await;
        tracker.record_failure("agent1", "op_b").await;

        let stats = tracker.get_failure_stats("agent1").await;
        assert_eq!(stats.total_failures, 3);
        assert_eq!(stats.by_operation.get("op_a"), Some(&2));
        assert_eq!(stats.by_operation.get("op_b"), Some(&1));
        assert!(stats.most_recent_ms.is_some());
    }

    #[tokio::test]
    async fn clear_trajectory_removes_all_data() {
        let tracker = TrajectoryTracker::new();
        tracker.record_intent("agent1", "intent1").await;
        tracker.record_operation("agent1", "op1", true).await;
        tracker.record_failure("agent1", "fail1").await;

        tracker.clear_trajectory("agent1").await;
        assert!(tracker.get_trajectory("agent1").await.is_empty());
        assert!(tracker.get_recent_failures("agent1", 10).await.is_empty());
        let stats = tracker.get_failure_stats("agent1").await;
        assert_eq!(stats.total_failures, 0);
    }
}
