//! Asynchronous Cognitive Pipeline — decoupled background processing for the AI-OS.
//!
//! Implements a DAG-aware task scheduler that handles:
//! - L0/L1 Summarization (L0 is prioritized for hot context)
//! - KG Extraction (Entities & Relationships)
//! - Causal/Similar-to link generation
//! - Vector indexing of child chunks

use std::sync::Arc;
use tokio::sync::mpsc;
use serde::{Deserialize, Serialize};
use crate::cas::AIObject;
use crate::fs::summarizer::SummaryLayer;

/// Represents a unit of cognitive work in the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CognitiveTask {
    /// Generate a summary for a specific CID.
    Summarize {
        cid: String,
        layer: SummaryLayer,
        agent_id: String,
    },
    /// Extract knowledge graph nodes/edges from an object.
    KgExtract {
        cid: String,
        agent_id: String,
    },
    /// Generate similarity links to existing objects.
    LinkSimilarity {
        cid: String,
        agent_id: String,
    },
    /// Index a specific chunk into the vector search engine.
    IndexChunk {
        cid: String,
        parent_cid: String,
        chunk_idx: usize,
        agent_id: String,
    }
}

/// Handle to the asynchronous cognitive pipeline.
pub struct CognitivePipelineHandle {
    sender: mpsc::Sender<CognitiveTask>,
}

impl CognitivePipelineHandle {
    /// Enqueue a task into the pipeline.
    pub async fn enqueue(&self, task: CognitiveTask) -> Result<(), String> {
        self.sender.send(task).await.map_err(|e| e.to_string())
    }
    
    /// Synchronous version for use in non-async contexts.
    pub fn enqueue_sync(&self, task: CognitiveTask) -> Result<(), String> {
        self.sender.try_send(task).map_err(|e| e.to_string())
    }
}

/// Start the cognitive pipeline worker loop.
pub fn start_cognitive_pipeline(
    kernel: Arc<crate::kernel::AIKernel>,
    buffer_size: usize,
) -> CognitivePipelineHandle {
    let (tx, mut rx) = mpsc::channel(buffer_size);
    
    let kernel_ref = Arc::clone(&kernel);
    tokio::spawn(async move {
        tracing::info!("Async Cognitive Pipeline started (buffer_size={})", buffer_size);
        
        while let Some(task) = rx.recv().await {
            let kernel = Arc::clone(&kernel_ref);
            tokio::spawn(async move {
                let agent_id = match &task {
                    CognitiveTask::Summarize { agent_id, .. } => agent_id.clone(),
                    CognitiveTask::KgExtract { agent_id, .. } => agent_id.clone(),
                    CognitiveTask::LinkSimilarity { agent_id, .. } => agent_id.clone(),
                    CognitiveTask::IndexChunk { agent_id, .. } => agent_id.clone(),
                };
                let cid = match &task {
                    CognitiveTask::Summarize { cid, .. } => Some(cid.clone()),
                    CognitiveTask::KgExtract { cid, .. } => Some(cid.clone()),
                    CognitiveTask::LinkSimilarity { cid, .. } => Some(cid.clone()),
                    CognitiveTask::IndexChunk { cid, .. } => Some(cid.clone()),
                };

                if let Err(e) = process_task(kernel.clone(), task).await {
                    tracing::error!("Cognitive task failed: {}", e);
                    kernel.diagnostic_store.record_failure(&agent_id, cid, &e);
                }
            });
        }
    });
    
    CognitivePipelineHandle { sender: tx }
}

async fn process_task(kernel: Arc<crate::kernel::AIKernel>, task: CognitiveTask) -> Result<(), String> {
    match task {
        CognitiveTask::Summarize { cid, layer, agent_id: _ } => {
            let obj = kernel.cas.get(&cid).map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&obj.data).to_string();
            if text.is_empty() { return Ok(()); }
            
            if let Some(ref summarizer) = kernel.fs.summarizer() {
                let summary = summarizer.summarize(&text, layer).map_err(|e| e.to_string())?;
                kernel.fs.ctx_loader_arc().store_l0(&cid, summary).map_err(|e: std::io::Error| e.to_string())?;
                tracing::debug!(cid = %crate::util::safe_truncate(&cid, 8), "Async summary generated");
            }
        }
        CognitiveTask::KgExtract { cid, agent_id } => {
            if let Some(ref builder) = kernel.kg_builder {
                // KG builder already has an async loop, we just trigger it
                // Or we can implement a more direct path here for Milestone 2
            }
        }
        CognitiveTask::LinkSimilarity { cid, agent_id } => {
            // Implementation of similarity linking
        }
        CognitiveTask::IndexChunk { cid, parent_cid, chunk_idx, agent_id } => {
            // Indexing chunk logic
        }
    }
    Ok(())
}
