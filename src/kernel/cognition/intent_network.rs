//! 意图语义网络 —— 维护意图之间的语义关系
//!
//! 支持：因果关系、时序关系、层次关系、关联关系
//! 基于 embedding 相似度 + 操作历史学习

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::fs::embedding::EmbeddingProvider;

use super::{CognitiveResult, TrajectoryPoint};

/// 语义关系类型
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SemanticRelation {
    Causes,
    Precedes,
    PartOf,
    CoOccurs,
    Alternative,
}

/// 语义节点
#[derive(Debug, Clone)]
pub struct SemanticNode {
    pub id: String,
    pub embedding: Vec<f32>,
    pub node_type: NodeType,
    pub metadata: serde_json::Value,
}

/// 节点类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeType {
    Intent,
    Task,
    Skill,
    Knowledge,
    Experience,
}

/// 语义边
#[derive(Debug, Clone)]
pub struct SemanticEdge {
    pub from: String,
    pub to: String,
    pub relation: SemanticRelation,
    pub strength: f32,
    pub evidence_count: usize,
}

/// 相关上下文推荐
#[derive(Debug, Clone)]
pub struct RelatedContext {
    pub cid: String,
    pub score: f32,
    pub relation_path: Vec<SemanticRelation>,
    pub reason: String,
}

/// 经验关联
#[derive(Debug, Clone)]
pub struct ExperienceAssociation {
    pub experience_intent: String,
    pub similarity_score: f32,
    pub reusable_knowledge: Vec<String>,
    pub failure_lessons: Vec<String>,
    pub suggested_skills: Vec<String>,
}

/// 学习报告
#[derive(Debug, Clone, Default)]
pub struct LearningReport {
    pub new_nodes: usize,
    pub new_edges: usize,
    pub strengthened_edges: usize,
    pub discovered_patterns: Vec<String>,
}

/// 意图语义网络
pub struct IntentSemanticNetwork {
    nodes: RwLock<HashMap<String, SemanticNode>>,
    edges: RwLock<HashMap<SemanticRelation, Vec<SemanticEdge>>>,
    embedding: Arc<dyn EmbeddingProvider>,
    trajectories: RwLock<HashMap<String, Vec<TrajectoryPoint>>>,
}

impl IntentSemanticNetwork {
    pub fn new(embedding: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            nodes: RwLock::new(HashMap::new()),
            edges: RwLock::new(HashMap::new()),
            embedding,
            trajectories: RwLock::new(HashMap::new()),
        }
    }

    /// 从 Agent 的操作历史中学习语义关系
    pub async fn learn_from_history(
        &self,
        agent_id: &str,
        trajectory: &[TrajectoryPoint],
    ) -> CognitiveResult<LearningReport> {
        let mut report = LearningReport {
            new_nodes: 0,
            new_edges: 0,
            strengthened_edges: 0,
            discovered_patterns: Vec::new(),
        };

        if trajectory.len() < 2 {
            return Ok(report);
        }

        // 1. 提取意图序列并建立节点
        for point in trajectory {
            let intent = &point.intent;
            let mut nodes = self.nodes.write().await;
            if !nodes.contains_key(intent) {
                let embedding = self.embedding.embed(intent)
                    .map(|r| r.embedding)
                    .unwrap_or_default();
                nodes.insert(intent.clone(), SemanticNode {
                    id: intent.clone(),
                    embedding,
                    node_type: NodeType::Intent,
                    metadata: serde_json::json!({"created_at": point.timestamp_ms}),
                });
                report.new_nodes += 1;
            }
        }

        // 2. 基于时序建立 Precedes 关系
        for window in trajectory.windows(2) {
            let from = &window[0].intent;
            let to = &window[1].intent;
            if from != to {
                self.add_or_strengthen_edge(from, to, SemanticRelation::Precedes).await?;
                report.new_edges += 1;
            }
        }

        // 3. 基于成功-成功链建立 Causes 关系
        for window in trajectory.windows(2) {
            if window[0].success && window[1].success {
                let from = &window[0].intent;
                let to = &window[1].intent;
                if from != to {
                    self.add_or_strengthen_edge(from, to, SemanticRelation::Causes).await?;
                    report.new_edges += 1;
                }
            }
        }

        // 4. 存储轨迹
        {
            let mut trajectories = self.trajectories.write().await;
            let entry = trajectories.entry(agent_id.to_string()).or_default();
            entry.extend(trajectory.iter().cloned());
        }

        // 5. 发现重复模式
        let patterns = self.discover_patterns(agent_id).await?;
        report.discovered_patterns = patterns;

        Ok(report)
    }

    /// 为给定意图查找相关上下文
    pub async fn find_related(
        &self,
        agent_id: &str,
        intent: &str,
    ) -> CognitiveResult<Vec<RelatedContext>> {
        let mut results = Vec::new();

        // 1. 语义相似匹配
        let intent_embedding = self.embedding.embed(intent)
            .map(|r| r.embedding)
            .unwrap_or_default();
        let similar = self.find_semantically_similar(&intent_embedding, 0.75).await?;

        for (node_id, score) in similar {
            if node_id != intent {
                results.push(RelatedContext {
                    cid: node_id.clone(),
                    score,
                    relation_path: vec![SemanticRelation::CoOccurs],
                    reason: format!("Semantically similar to current intent (score: {:.2})", score),
                });
            }
        }

        // 2. 基于关系网络查找
        let related_by_relation = self.expand_by_relation(intent).await?;
        for (node_id, relation, strength) in related_by_relation {
            if node_id != intent && !results.iter().any(|r| r.cid == node_id) {
                results.push(RelatedContext {
                    cid: node_id,
                    score: strength,
                    relation_path: vec![relation.clone()],
                    reason: format!("Linked by {:?} relation", relation),
                });
            }
        }

        // 3. 基于个人历史轨迹
        let from_history = self.find_from_trajectory(agent_id, intent).await?;
        for (node_id, score) in from_history {
            if node_id != intent && !results.iter().any(|r| r.cid == node_id) {
                results.push(RelatedContext {
                    cid: node_id,
                    score,
                    relation_path: vec![SemanticRelation::Precedes],
                    reason: "Frequently followed this intent in your history".to_string(),
                });
            }
        }

        // 按相关性排序
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        results.truncate(10);

        Ok(results)
    }

    /// 预测 Agent 接下来可能需要的上下文
    pub async fn predict_next_context(
        &self,
        agent_id: &str,
        current_intent: &str,
    ) -> CognitiveResult<Vec<RelatedContext>> {
        let mut predictions = Vec::new();

        // 查找 current_intent 的 Precedes 关系
        let edges = self.edges.read().await;
        if let Some(precedes_edges) = edges.get(&SemanticRelation::Precedes) {
            for edge in precedes_edges {
                if edge.from == current_intent {
                    predictions.push(RelatedContext {
                        cid: edge.to.clone(),
                        score: edge.strength,
                        relation_path: vec![SemanticRelation::Precedes],
                        reason: format!(
                            "Historically followed by '{}' ({} occurrences)",
                            edge.to, edge.evidence_count
                        ),
                    });
                }
            }
        }

        // 基于个人历史统计转移概率
        let trajectories = self.trajectories.read().await;
        if let Some(agent_traj) = trajectories.get(agent_id) {
            let transitions = self.compute_transition_probs(agent_traj, current_intent);
            for (next_intent, prob) in transitions {
                if !predictions.iter().any(|p| p.cid == next_intent) {
                    predictions.push(RelatedContext {
                        cid: next_intent,
                        score: prob,
                        relation_path: vec![SemanticRelation::Precedes],
                        reason: format!("Transition probability: {:.1}%", prob * 100.0),
                    });
                }
            }
        }

        predictions.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        predictions.truncate(5);

        Ok(predictions)
    }

    /// 关联历史经验与当前任务
    pub async fn associate_experience(
        &self,
        agent_id: &str,
        current_intent: &str,
    ) -> CognitiveResult<Vec<ExperienceAssociation>> {
        let mut associations = Vec::new();

        let intent_embedding = self.embedding.embed(current_intent)
            .map(|r| r.embedding)
            .unwrap_or_default();

        let trajectories = self.trajectories.read().await;
        if let Some(agent_traj) = trajectories.get(agent_id) {
            // 按意图分组轨迹点
            let mut intent_groups: HashMap<String, Vec<&TrajectoryPoint>> = HashMap::new();
            for point in agent_traj {
                intent_groups.entry(point.intent.clone()).or_default().push(point);
            }

            // 查找语义相似的过往意图
            for (past_intent, points) in intent_groups {
                if past_intent == current_intent {
                    continue;
                }

                let past_embedding = self.embedding.embed(&past_intent)
                    .map(|r| r.embedding)
                    .unwrap_or_default();

                let similarity = cosine_similarity(&intent_embedding, &past_embedding);
                if similarity > 0.7 {
                    let successes: Vec<_> = points.iter().filter(|p| p.success).collect();
                    let failures: Vec<_> = points.iter().filter(|p| !p.success).collect();

                    associations.push(ExperienceAssociation {
                        experience_intent: past_intent.clone(),
                        similarity_score: similarity,
                        reusable_knowledge: successes.iter().map(|p| p.operation.clone()).collect(),
                        failure_lessons: failures.iter().map(|p| p.operation.clone()).collect(),
                        suggested_skills: Vec::new(), // TODO
                    });
                }
            }
        }

        associations.sort_by(|a, b| b.similarity_score.partial_cmp(&a.similarity_score).unwrap());
        associations.truncate(5);

        Ok(associations)
    }

    /// 判断两个意图是否在语义上相关
    pub async fn is_semantically_related(
        &self,
        intent_a: &str,
        intent_b: &str,
    ) -> CognitiveResult<bool> {
        let emb_a = self.embedding.embed(intent_a)
            .map(|r| r.embedding)
            .unwrap_or_default();
        let emb_b = self.embedding.embed(intent_b)
            .map(|r| r.embedding)
            .unwrap_or_default();

        let similarity = cosine_similarity(&emb_a, &emb_b);
        Ok(similarity > 0.6)
    }

    // --- Private helpers ---

    async fn add_or_strengthen_edge(
        &self,
        from: &str,
        to: &str,
        relation: SemanticRelation,
    ) -> CognitiveResult<()> {
        let mut edges = self.edges.write().await;
        let edge_list = edges.entry(relation.clone()).or_default();

        if let Some(existing) = edge_list.iter_mut().find(|e| e.from == from && e.to == to) {
            existing.evidence_count += 1;
            existing.strength = (existing.strength * (existing.evidence_count - 1) as f32 + 1.0)
                / existing.evidence_count as f32;
        } else {
            edge_list.push(SemanticEdge {
                from: from.to_string(),
                to: to.to_string(),
                relation,
                strength: 1.0,
                evidence_count: 1,
            });
        }

        Ok(())
    }

    async fn find_semantically_similar(
        &self,
        embedding: &[f32],
        threshold: f32,
    ) -> CognitiveResult<Vec<(String, f32)>> {
        let nodes = self.nodes.read().await;
        let mut results = Vec::new();

        for (id, node) in nodes.iter() {
            let sim = cosine_similarity(embedding, &node.embedding);
            if sim >= threshold {
                results.push((id.clone(), sim));
            }
        }

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        Ok(results)
    }

    async fn expand_by_relation(
        &self,
        intent: &str,
    ) -> CognitiveResult<Vec<(String, SemanticRelation, f32)>> {
        let mut results = Vec::new();
        let edges = self.edges.read().await;

        for (relation, edge_list) in edges.iter() {
            for edge in edge_list {
                if edge.from == intent {
                    results.push((edge.to.clone(), relation.clone(), edge.strength));
                }
            }
        }

        Ok(results)
    }

    async fn find_from_trajectory(
        &self,
        agent_id: &str,
        current_intent: &str,
    ) -> CognitiveResult<Vec<(String, f32)>> {
        let mut results = Vec::new();
        let trajectories = self.trajectories.read().await;

        if let Some(traj) = trajectories.get(agent_id) {
            let mut transition_counts: HashMap<String, usize> = HashMap::new();
            let mut total = 0;

            for window in traj.windows(2) {
                if window[0].intent == current_intent {
                    *transition_counts.entry(window[1].intent.clone()).or_default() += 1;
                    total += 1;
                }
            }

            if total > 0 {
                for (intent, count) in transition_counts {
                    results.push((intent, count as f32 / total as f32));
                }
            }
        }

        Ok(results)
    }

    fn compute_transition_probs(
        &self,
        trajectory: &[TrajectoryPoint],
        current_intent: &str,
    ) -> Vec<(String, f32)> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut total = 0;

        for window in trajectory.windows(2) {
            if window[0].intent == current_intent {
                *counts.entry(window[1].intent.clone()).or_default() += 1;
                total += 1;
            }
        }

        if total == 0 {
            return Vec::new();
        }

        let mut results: Vec<_> = counts.into_iter()
            .map(|(k, v)| (k, v as f32 / total as f32))
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        results
    }

    async fn discover_patterns(&self, agent_id: &str) -> CognitiveResult<Vec<String>> {
        let mut patterns = Vec::new();
        let trajectories = self.trajectories.read().await;

        if let Some(traj) = trajectories.get(agent_id) {
            // 发现重复序列模式
            let mut seq_counts: HashMap<Vec<String>, usize> = HashMap::new();
            for window in traj.windows(3) {
                let seq: Vec<String> = window.iter().map(|p| p.intent.clone()).collect();
                *seq_counts.entry(seq).or_default() += 1;
            }

            for (seq, count) in seq_counts {
                if count >= 3 {
                    patterns.push(format!("Repeated sequence: {:?} ({}x)", seq, count));
                }
            }
        }

        Ok(patterns)
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use super::*;
    use crate::fs::embedding::{EmbedResult, EmbedError, EmbeddingProvider};

    struct MockEmbeddingProvider;

    impl EmbeddingProvider for MockEmbeddingProvider {
        fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
            let dim = 8;
            let mut vec = vec![0.0f32; dim];
            for (i, byte) in text.bytes().enumerate() {
                vec[i % dim] += byte as f32;
            }
            let norm = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for v in &mut vec {
                    *v /= norm;
                }
            }
            Ok(EmbedResult::new(vec, text.len() as u32 / 4))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            texts.iter().map(|t| self.embed(t)).collect()
        }

        fn dimension(&self) -> usize {
            8
        }

        fn model_name(&self) -> &str {
            "mock"
        }
    }

    fn mock_provider() -> Arc<dyn EmbeddingProvider> {
        Arc::new(MockEmbeddingProvider)
    }

    fn traj_point(intent: &str, op: &str, success: bool) -> TrajectoryPoint {
        TrajectoryPoint {
            timestamp_ms: 0,
            intent: intent.to_string(),
            operation: op.to_string(),
            success,
            context_cids: vec![],
        }
    }

    #[tokio::test]
    async fn new_creates_empty_network() {
        let network = IntentSemanticNetwork::new(mock_provider());
        let _ = network;
    }

    #[tokio::test]
    async fn learn_from_history_empty_returns_empty_report() {
        let network = IntentSemanticNetwork::new(mock_provider());
        let report = network.learn_from_history("agent1", &[]).await.unwrap();
        assert_eq!(report.new_nodes, 0);
        assert_eq!(report.new_edges, 0);
        assert_eq!(report.strengthened_edges, 0);
        assert!(report.discovered_patterns.is_empty());
    }

    #[tokio::test]
    async fn learn_from_history_creates_nodes() {
        let network = IntentSemanticNetwork::new(mock_provider());
        let traj = vec![
            traj_point("search", "op1", true),
            traj_point("store", "op2", true),
        ];
        let report = network.learn_from_history("agent1", &traj).await.unwrap();
        assert_eq!(report.new_nodes, 2);
    }

    #[tokio::test]
    async fn learn_from_history_creates_precedes_edges() {
        let network = IntentSemanticNetwork::new(mock_provider());
        let traj = vec![
            traj_point("A", "op1", true),
            traj_point("B", "op2", false),
        ];
        let report = network.learn_from_history("agent1", &traj).await.unwrap();
        assert_eq!(report.new_edges, 1); // Only Precedes
    }

    #[tokio::test]
    async fn learn_from_history_creates_causes_edges() {
        let network = IntentSemanticNetwork::new(mock_provider());
        let traj = vec![
            traj_point("A", "op1", true),
            traj_point("B", "op2", true),
        ];
        let report = network.learn_from_history("agent1", &traj).await.unwrap();
        // 1 Precedes + 1 Causes = 2 new edges
        assert_eq!(report.new_edges, 2);
    }

    #[tokio::test]
    async fn find_related_empty_returns_empty() {
        let network = IntentSemanticNetwork::new(mock_provider());
        let related = network.find_related("agent1", "search").await.unwrap();
        assert!(related.is_empty());
    }

    #[tokio::test]
    async fn find_related_finds_precedes() {
        let network = IntentSemanticNetwork::new(mock_provider());
        let traj = vec![
            traj_point("A", "op1", true),
            traj_point("B", "op2", true),
        ];
        network.learn_from_history("agent1", &traj).await.unwrap();
        let related = network.find_related("agent1", "A").await.unwrap();
        let has_b = related.iter().any(|r| r.cid == "B");
        assert!(has_b, "Expected to find B as related to A via Precedes edge");
    }

    #[tokio::test]
    async fn predict_next_context_returns_followers() {
        let network = IntentSemanticNetwork::new(mock_provider());
        let traj = vec![
            traj_point("A", "op1", true),
            traj_point("B", "op2", true),
        ];
        network.learn_from_history("agent1", &traj).await.unwrap();
        let predictions = network.predict_next_context("agent1", "A").await.unwrap();
        assert_eq!(predictions.len(), 1);
        assert_eq!(predictions[0].cid, "B");
    }

    #[tokio::test]
    async fn associate_experience_empty_returns_empty() {
        let network = IntentSemanticNetwork::new(mock_provider());
        let assoc = network.associate_experience("agent1", "search").await.unwrap();
        assert!(assoc.is_empty());
    }

    #[tokio::test]
    async fn is_semantically_related_same_intent() {
        let network = IntentSemanticNetwork::new(mock_provider());
        let related = network.is_semantically_related("search", "search").await.unwrap();
        assert!(related);
    }

    #[tokio::test]
    async fn discover_patterns_finds_repeated_sequence() {
        let network = IntentSemanticNetwork::new(mock_provider());
        let traj = vec![
            traj_point("A", "op", true),
            traj_point("B", "op", true),
            traj_point("C", "op", true),
            traj_point("A", "op", true),
            traj_point("B", "op", true),
            traj_point("C", "op", true),
            traj_point("A", "op", true),
            traj_point("B", "op", true),
            traj_point("C", "op", true),
        ];
        let report = network.learn_from_history("agent1", &traj).await.unwrap();
        assert_eq!(report.discovered_patterns.len(), 1);
        assert!(report.discovered_patterns[0].contains("Repeated sequence"));
        assert!(report.discovered_patterns[0].contains("A"));
        assert!(report.discovered_patterns[0].contains("B"));
        assert!(report.discovered_patterns[0].contains("C"));
        assert!(report.discovered_patterns[0].contains("3x"));
    }
}
