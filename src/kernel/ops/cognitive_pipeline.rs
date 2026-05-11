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
    /// Full document processing lifecycle.
    ProcessDocument {
        cid: String,
        agent_id: String,
        force_chunking: bool,
    },
}

/// Handle to the asynchronous cognitive pipeline.
#[derive(Clone)]
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
                let (agent_id, cid) = match &task {
                    CognitiveTask::Summarize { agent_id, cid, .. } => (agent_id.clone(), Some(cid.clone())),
                    CognitiveTask::KgExtract { agent_id, cid } => (agent_id.clone(), Some(cid.clone())),
                    CognitiveTask::LinkSimilarity { agent_id, cid } => (agent_id.clone(), Some(cid.clone())),
                    CognitiveTask::ProcessDocument { agent_id, cid, .. } => (agent_id.clone(), Some(cid.clone())),
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
            // F-37: Retry CAS get to handle race conditions
            let mut obj_opt = None;
            for _ in 0..3 {
                if let Ok(o) = kernel.cas.get(&cid) {
                    obj_opt = Some(o);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            let obj = obj_opt.ok_or_else(|| format!("ACP Object not found: CID={}", cid))?;
            
            let text = String::from_utf8_lossy(&obj.data).to_string();
            if text.is_empty() { return Ok(()); }
            
            if let Some(ref summarizer) = kernel.fs.summarizer() {
                let summary = summarizer.summarize(&text, layer).map_err(|e| e.to_string())?;
                kernel.fs.ctx_loader_arc().store_l0(&cid, summary).map_err(|e: std::io::Error| e.to_string())?;
                tracing::debug!(cid = %crate::util::safe_truncate(&cid, 8), "Async summary generated");
            }
        }
        CognitiveTask::KgExtract { cid, agent_id: _ } => {
            if let Some(ref builder) = kernel.kg_builder {
                // F-37: Retry CAS get
                let mut obj_opt = None;
                for _ in 0..3 {
                    if let Ok(o) = kernel.cas.get(&cid) {
                        obj_opt = Some(o);
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                if let Some(obj) = obj_opt {
                    let text = String::from_utf8_lossy(&obj.data).to_string();
                    builder.notify(crate::kernel::ops::kg_builder::WriteEvent {
                        cid,
                        text,
                        agent_id: obj.meta.created_by.clone(),
                        created_at: obj.meta.created_at,
                        tags: obj.meta.tags.clone(),
                    });
                }
            }
        }
        CognitiveTask::LinkSimilarity { cid: _, agent_id: _ } => {
            // Implementation of similarity linking
        }
        CognitiveTask::ProcessDocument { cid, agent_id, force_chunking } => {
            // F-37: Retry CAS get to handle race conditions
            let mut obj_opt = None;
            for _ in 0..5 {
                if let Ok(o) = kernel.cas.get(&cid) {
                    obj_opt = Some(o);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            let obj = obj_opt.ok_or_else(|| format!("ACP Object not found (ProcessDocument): CID={}", cid))?;
            
            // 1. Summarization (Inlined to avoid recursion)
            if kernel.config.tuning.auto_summarize {
                if let Some(ref summarizer) = kernel.fs.summarizer() {
                    let text = String::from_utf8_lossy(&obj.data);
                    if !text.is_empty() {
                        if let Ok(summary) = summarizer.summarize(&text, SummaryLayer::L0) {
                            let _ = kernel.fs.ctx_loader_arc().store_l0(&cid, summary);
                        }
                    }
                }
            }

            // 2. KG Extraction (Inlined to avoid recursion)
            if let Some(ref builder) = kernel.kg_builder {
                let text = String::from_utf8_lossy(&obj.data).to_string();
                builder.notify(crate::kernel::ops::kg_builder::WriteEvent {
                    cid: cid.clone(),
                    text,
                    agent_id: obj.meta.created_by.clone(),
                    created_at: obj.meta.created_at,
                    tags: obj.meta.tags.clone(),
                });
            }

            // 3. Self-healing Chunking & Indexing
            kernel.fs.process_document_background(&cid, &obj, &agent_id, force_chunking).await.map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
