//! Asynchronous Cognitive Pipeline — decoupled background processing for the AI-OS.
//!
//! Implements a DAG-aware task scheduler that handles:
//! - L0/L1 Summarization (L0 is prioritized for hot context)
//! - KG Extraction (Entities & Relationships)
//! - Causal/Similar-to link generation
//! - Vector indexing of child chunks

use crate::fs::summarizer::SummaryLayer;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::mpsc;
use tokio::task::{JoinError, JoinSet};

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
    KgExtract { cid: String, agent_id: String },
    /// Generate similarity links to existing objects.
    LinkSimilarity { cid: String, agent_id: String },
    /// Full document processing lifecycle.
    ProcessDocument {
        cid: String,
        agent_id: String,
        force_chunking: bool,
    },
}

/// Stable queue failures for callers that must choose a typed fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CognitivePipelineError {
    #[error("cognitive pipeline queue is full")]
    QueueFull,
    #[error("cognitive pipeline queue is closed")]
    QueueClosed,
    #[error("cognitive pipeline progress counter is exhausted")]
    CounterExhausted,
}

/// Coherent, side-effect-free progress for accepted cognitive work.
///
/// `accepted` is the latest assigned watermark. `completed` is the latest
/// contiguous completed watermark, so callers may safely wait for
/// `completed >= accepted_at_ingest_end` even though tasks execute concurrently.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CognitivePipelineSnapshot {
    pub accepted: u64,
    pub completed: u64,
    pub in_flight: u64,
    /// Document attempts with a real root-object vector.
    pub document_vector_succeeded_attempts: u64,
    /// Document attempts that retained lexical indexing only.
    pub document_lexical_degraded_attempts: u64,
    /// Task attempts that returned an error or unwound.
    pub task_failed_attempts: u64,
    /// Successful non-document cognitive tasks.
    pub other_succeeded_attempts: u64,
    /// Document attempts processed inline because the queue was unavailable.
    pub inline_document_attempts: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CognitiveTaskOutcome {
    VectorSucceeded,
    LexicalDegraded,
    OtherSucceeded,
    TaskFailed,
}

#[derive(Default)]
struct CognitivePipelineProgress {
    state: Mutex<CognitivePipelineProgressState>,
}

#[derive(Default)]
struct CognitivePipelineProgressState {
    accepted: u64,
    completed: u64,
    in_flight: u64,
    document_vector_succeeded_attempts: u64,
    document_lexical_degraded_attempts: u64,
    task_failed_attempts: u64,
    other_succeeded_attempts: u64,
    inline_document_attempts: u64,
    completed_out_of_order: BTreeSet<u64>,
}

struct CognitiveTaskCompletion {
    progress: Arc<CognitivePipelineProgress>,
    watermark: u64,
    outcome: CognitiveTaskOutcome,
}

impl CognitiveTaskCompletion {
    fn record(&mut self, outcome: CognitiveTaskOutcome) {
        self.outcome = outcome;
    }
}

impl Drop for CognitiveTaskCompletion {
    fn drop(&mut self) {
        self.progress.complete(self.watermark, self.outcome);
    }
}

impl CognitivePipelineProgress {
    fn state(&self) -> MutexGuard<'_, CognitivePipelineProgressState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn accept(&self) -> Result<u64, CognitivePipelineError> {
        let mut state = self.state();
        let watermark = state
            .accepted
            .checked_add(1)
            .ok_or(CognitivePipelineError::CounterExhausted)?;
        let in_flight = state
            .in_flight
            .checked_add(1)
            .ok_or(CognitivePipelineError::CounterExhausted)?;
        state.accepted = watermark;
        state.in_flight = in_flight;
        Ok(watermark)
    }

    fn complete(&self, watermark: u64, outcome: CognitiveTaskOutcome) {
        let mut state = self.state();
        if watermark <= state.completed || state.completed_out_of_order.contains(&watermark) {
            return;
        }
        state.in_flight = state.in_flight.saturating_sub(1);
        Self::record_outcome(&mut state, outcome);
        if watermark != state.completed.saturating_add(1) {
            state.completed_out_of_order.insert(watermark);
            return;
        }
        state.completed = watermark;
        loop {
            let next = state.completed.saturating_add(1);
            if !state.completed_out_of_order.remove(&next) {
                break;
            }
            state.completed = next;
        }
    }

    fn complete_inline_document(&self, outcome: CognitiveTaskOutcome) {
        let mut state = self.state();
        state.inline_document_attempts = state.inline_document_attempts.saturating_add(1);
        Self::record_outcome(&mut state, outcome);
    }

    fn record_outcome(state: &mut CognitivePipelineProgressState, outcome: CognitiveTaskOutcome) {
        match outcome {
            CognitiveTaskOutcome::VectorSucceeded => {
                state.document_vector_succeeded_attempts = state.document_vector_succeeded_attempts.saturating_add(1);
            }
            CognitiveTaskOutcome::LexicalDegraded => {
                state.document_lexical_degraded_attempts = state.document_lexical_degraded_attempts.saturating_add(1);
            }
            CognitiveTaskOutcome::TaskFailed => {
                state.task_failed_attempts = state.task_failed_attempts.saturating_add(1);
            }
            CognitiveTaskOutcome::OtherSucceeded => {
                state.other_succeeded_attempts = state.other_succeeded_attempts.saturating_add(1);
            }
        }
    }

    fn snapshot(&self) -> CognitivePipelineSnapshot {
        let state = self.state();
        CognitivePipelineSnapshot {
            accepted: state.accepted,
            completed: state.completed,
            in_flight: state.in_flight,
            document_vector_succeeded_attempts: state.document_vector_succeeded_attempts,
            document_lexical_degraded_attempts: state.document_lexical_degraded_attempts,
            task_failed_attempts: state.task_failed_attempts,
            other_succeeded_attempts: state.other_succeeded_attempts,
            inline_document_attempts: state.inline_document_attempts,
        }
    }
}

pub(crate) struct QueuedCognitiveTask {
    task: CognitiveTask,
    completion: CognitiveTaskCompletion,
}

/// Handle to the asynchronous cognitive pipeline.
#[derive(Clone)]
pub struct CognitivePipelineHandle {
    sender: mpsc::Sender<QueuedCognitiveTask>,
    progress: Arc<CognitivePipelineProgress>,
}

impl CognitivePipelineHandle {
    /// Enqueue a task into the pipeline.
    pub async fn enqueue(&self, task: CognitiveTask) -> Result<u64, CognitivePipelineError> {
        let permit = self
            .sender
            .reserve()
            .await
            .map_err(|_| CognitivePipelineError::QueueClosed)?;
        let watermark = self.progress.accept()?;
        permit.send(QueuedCognitiveTask {
            task,
            completion: CognitiveTaskCompletion {
                progress: Arc::clone(&self.progress),
                watermark,
                outcome: CognitiveTaskOutcome::TaskFailed,
            },
        });
        Ok(watermark)
    }

    /// Synchronous version for use in non-async contexts.
    pub fn enqueue_sync(&self, task: CognitiveTask) -> Result<u64, CognitivePipelineError> {
        let permit = self.sender.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => CognitivePipelineError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => CognitivePipelineError::QueueClosed,
        })?;
        let watermark = self.progress.accept()?;
        permit.send(QueuedCognitiveTask {
            task,
            completion: CognitiveTaskCompletion {
                progress: Arc::clone(&self.progress),
                watermark,
                outcome: CognitiveTaskOutcome::TaskFailed,
            },
        });
        Ok(watermark)
    }

    /// Return one coherent progress snapshot without probing providers.
    pub fn snapshot(&self) -> CognitivePipelineSnapshot {
        self.progress.snapshot()
    }

    pub(crate) fn record_inline_document_outcome(&self, outcome: crate::fs::semantic_fs::DocumentProcessingOutcome) {
        let outcome = match outcome {
            crate::fs::semantic_fs::DocumentProcessingOutcome::VectorSucceeded => CognitiveTaskOutcome::VectorSucceeded,
            crate::fs::semantic_fs::DocumentProcessingOutcome::LexicalDegraded => CognitiveTaskOutcome::LexicalDegraded,
        };
        self.progress.complete_inline_document(outcome);
    }

    pub(crate) fn record_inline_document_failure(&self) {
        self.progress.complete_inline_document(CognitiveTaskOutcome::TaskFailed);
    }

    #[cfg(test)]
    pub(crate) fn channel_for_test(buffer_size: usize) -> (Self, mpsc::Receiver<QueuedCognitiveTask>) {
        let (sender, receiver) = mpsc::channel(buffer_size);
        (
            Self {
                sender,
                progress: Arc::new(CognitivePipelineProgress::default()),
            },
            receiver,
        )
    }
}

/// Start the cognitive pipeline worker loop.
pub fn start_cognitive_pipeline(
    kernel: Arc<crate::kernel::AIKernel>,
    buffer_size: usize,
    max_in_flight: usize,
) -> CognitivePipelineHandle {
    let max_in_flight = max_in_flight.clamp(1, 64);
    let buffer_size = buffer_size.clamp(max_in_flight, 65_536);
    let (tx, rx) = mpsc::channel(buffer_size);
    let progress = Arc::new(CognitivePipelineProgress::default());

    let kernel_ref = Arc::downgrade(&kernel);
    tokio::spawn(async move {
        tracing::info!(buffer_size, max_in_flight, "Async Cognitive Pipeline started");

        run_bounded_receiver(rx, max_in_flight, move |queued| {
            let kernel_ref = kernel_ref.clone();
            async move {
                let Some(kernel) = kernel_ref.upgrade() else {
                    return;
                };
                let QueuedCognitiveTask { task, mut completion } = queued;
                let (agent_id, cid) = match &task {
                    CognitiveTask::Summarize { agent_id, cid, .. } => (agent_id.clone(), Some(cid.clone())),
                    CognitiveTask::KgExtract { agent_id, cid } => (agent_id.clone(), Some(cid.clone())),
                    CognitiveTask::LinkSimilarity { agent_id, cid } => (agent_id.clone(), Some(cid.clone())),
                    CognitiveTask::ProcessDocument { agent_id, cid, .. } => (agent_id.clone(), Some(cid.clone())),
                };

                match process_task(kernel.clone(), task).await {
                    Ok(outcome) => completion.record(outcome),
                    Err(e) => {
                        tracing::error!("Cognitive task failed: {}", e);
                        kernel.diagnostic_store.record_failure(&agent_id, cid, &e);
                    }
                }
            }
        })
        .await;
    });

    CognitivePipelineHandle { sender: tx, progress }
}

/// Drive a bounded set of tasks without receiving beyond the execution limit.
///
/// Keeping the receive operation behind the limit is intentional: pending work
/// remains in the bounded channel where callers can observe backpressure.
async fn run_bounded_receiver<T, F, Fut>(mut receiver: mpsc::Receiver<T>, max_in_flight: usize, mut task: F)
where
    T: Send + 'static,
    F: FnMut(T) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut running = JoinSet::new();
    loop {
        if running.len() >= max_in_flight {
            report_worker_join(running.join_next().await);
            continue;
        }

        tokio::select! {
            queued = receiver.recv() => match queued {
                Some(queued) => {
                    running.spawn(task(queued));
                }
                None => break,
            },
            joined = running.join_next(), if !running.is_empty() => {
                report_worker_join(joined);
            }
        }
    }

    while !running.is_empty() {
        report_worker_join(running.join_next().await);
    }
}

fn report_worker_join(joined: Option<Result<(), JoinError>>) {
    if let Some(Err(error)) = joined {
        tracing::error!(%error, "Cognitive task worker failed to join");
    }
}

async fn process_task(
    kernel: Arc<crate::kernel::AIKernel>,
    task: CognitiveTask,
) -> Result<CognitiveTaskOutcome, String> {
    let outcome = match task {
        CognitiveTask::Summarize {
            cid,
            layer,
            agent_id: _,
        } => {
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
            if text.is_empty() {
                return Ok(CognitiveTaskOutcome::OtherSucceeded);
            }

            if let Some(ref summarizer) = kernel.fs.summarizer() {
                let summary = summarizer.summarize(&text, layer).map_err(|e| e.to_string())?;
                kernel
                    .fs
                    .ctx_loader_arc()
                    .store_l0(&cid, summary)
                    .map_err(|e: std::io::Error| e.to_string())?;
                tracing::debug!(cid = %crate::util::safe_truncate(&cid, 8), "Async summary generated");
            }
            CognitiveTaskOutcome::OtherSucceeded
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
            CognitiveTaskOutcome::OtherSucceeded
        }
        CognitiveTask::LinkSimilarity { cid: _, agent_id: _ } => {
            // Implementation of similarity linking
            CognitiveTaskOutcome::OtherSucceeded
        }
        CognitiveTask::ProcessDocument {
            cid,
            agent_id,
            force_chunking,
        } => {
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
            match kernel
                .fs
                .process_document_background(&cid, &obj, &agent_id, force_chunking)
                .await
                .map_err(|e| e.to_string())?
            {
                crate::fs::semantic_fs::DocumentProcessingOutcome::VectorSucceeded => {
                    CognitiveTaskOutcome::VectorSucceeded
                }
                crate::fs::semantic_fs::DocumentProcessingOutcome::LexicalDegraded => {
                    CognitiveTaskOutcome::LexicalDegraded
                }
            }
        }
    };
    Ok(outcome)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Semaphore;

    #[tokio::test]
    async fn test_start_cognitive_pipeline() {
        let (kernel, _dir) = make_kernel();
        let handle = start_cognitive_pipeline(kernel, 64, 4);
        assert_eq!(handle.snapshot(), CognitivePipelineSnapshot::default());
    }

    #[tokio::test]
    async fn start_cognitive_pipeline_normalizes_zero_capacity() {
        let (kernel, _dir) = make_kernel();
        let handle = start_cognitive_pipeline(kernel, 0, 0);
        assert_eq!(handle.snapshot(), CognitivePipelineSnapshot::default());
    }

    #[tokio::test]
    async fn test_enqueue_sync() {
        let (kernel, _dir) = make_kernel();
        let handle = start_cognitive_pipeline(kernel.clone(), 256, 4);

        let cid = kernel
            .semantic_create(
                b"test content for pipeline".to_vec(),
                vec!["test".to_string()],
                "kernel",
                None,
                crate::cas::ObjectScope::default(),
            )
            .unwrap();

        let task = CognitiveTask::Summarize {
            cid,
            layer: SummaryLayer::L0,
            agent_id: "kernel".to_string(),
        };
        let watermark = handle.enqueue_sync(task).unwrap();
        assert_eq!(watermark, 1);
        assert_eq!(handle.snapshot().accepted, 1);
    }

    #[tokio::test]
    async fn test_enqueue_async() {
        let (kernel, _dir) = make_kernel();
        let handle = start_cognitive_pipeline(kernel.clone(), 256, 4);

        let cid = kernel
            .semantic_create(
                b"async test content".to_vec(),
                vec!["test".to_string()],
                "kernel",
                None,
                crate::cas::ObjectScope::default(),
            )
            .unwrap();

        let task = CognitiveTask::Summarize {
            cid,
            layer: SummaryLayer::L0,
            agent_id: "kernel".to_string(),
        };
        let watermark = handle.enqueue(task).await.unwrap();
        assert_eq!(watermark, 1);
    }

    #[tokio::test]
    async fn test_process_task_summarize_missing_cid() {
        let (kernel, _dir) = make_kernel();
        let task = CognitiveTask::Summarize {
            cid: "nonexistent_cid_12345".to_string(),
            layer: SummaryLayer::L0,
            agent_id: "kernel".to_string(),
        };
        let result = process_task(kernel, task).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_process_task_kg_extract() {
        let (kernel, _dir) = make_kernel();
        let cid = kernel
            .semantic_create(
                b"knowledge content to extract".to_vec(),
                vec!["knowledge".to_string()],
                "kernel",
                None,
                crate::cas::ObjectScope::default(),
            )
            .unwrap();
        let task = CognitiveTask::KgExtract {
            cid,
            agent_id: "kernel".to_string(),
        };
        let result = process_task(kernel, task).await;
        assert_eq!(result, Ok(CognitiveTaskOutcome::OtherSucceeded));
    }

    #[tokio::test]
    async fn test_process_task_link_similarity() {
        let (kernel, _dir) = make_kernel();
        let task = CognitiveTask::LinkSimilarity {
            cid: "any_cid".to_string(),
            agent_id: "kernel".to_string(),
        };
        let result = process_task(kernel, task).await;
        assert_eq!(result, Ok(CognitiveTaskOutcome::OtherSucceeded));
    }

    #[tokio::test]
    async fn test_process_task_process_document_missing() {
        let (kernel, _dir) = make_kernel();
        let task = CognitiveTask::ProcessDocument {
            cid: "nonexistent_doc".to_string(),
            agent_id: "kernel".to_string(),
            force_chunking: false,
        };
        let result = process_task(kernel, task).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_process_task_process_document() {
        let (kernel, _dir) = make_kernel();
        let cid = kernel
            .semantic_create(
                b"document content for full processing pipeline test".to_vec(),
                vec!["document".to_string()],
                "kernel",
                None,
                crate::cas::ObjectScope::default(),
            )
            .unwrap();
        let task = CognitiveTask::ProcessDocument {
            cid,
            agent_id: "kernel".to_string(),
            force_chunking: false,
        };
        let result = process_task(kernel, task).await;
        assert_eq!(result, Ok(CognitiveTaskOutcome::LexicalDegraded));
    }

    #[tokio::test]
    async fn test_cognitive_task_serialization() {
        let task = CognitiveTask::Summarize {
            cid: "test_cid".to_string(),
            layer: SummaryLayer::L0,
            agent_id: "agent1".to_string(),
        };
        let json = serde_json::to_string(&task).unwrap();
        let deserialized: CognitiveTask = serde_json::from_str(&json).unwrap();
        match deserialized {
            CognitiveTask::Summarize { cid, agent_id, .. } => {
                assert_eq!(cid, "test_cid");
                assert_eq!(agent_id, "agent1");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[tokio::test]
    async fn test_enqueue_sync_channel_full() {
        let (handle, _receiver) = CognitivePipelineHandle::channel_for_test(1);
        handle
            .enqueue_sync(CognitiveTask::LinkSimilarity {
                cid: "first".to_string(),
                agent_id: "kernel".to_string(),
            })
            .unwrap();
        let error = handle
            .enqueue_sync(CognitiveTask::LinkSimilarity {
                cid: "second".to_string(),
                agent_id: "kernel".to_string(),
            })
            .unwrap_err();

        assert_eq!(error, CognitivePipelineError::QueueFull);
        assert_eq!(
            handle.snapshot(),
            CognitivePipelineSnapshot {
                accepted: 1,
                completed: 0,
                in_flight: 1,
                ..CognitivePipelineSnapshot::default()
            }
        );
    }

    #[test]
    fn closed_queue_does_not_advance_the_accepted_watermark() {
        let (handle, receiver) = CognitivePipelineHandle::channel_for_test(1);
        drop(receiver);

        let error = handle
            .enqueue_sync(CognitiveTask::LinkSimilarity {
                cid: "closed".to_string(),
                agent_id: "kernel".to_string(),
            })
            .unwrap_err();

        assert_eq!(error, CognitivePipelineError::QueueClosed);
        assert_eq!(handle.snapshot(), CognitivePipelineSnapshot::default());
    }

    #[test]
    fn dropping_an_accepted_queued_task_records_a_failed_attempt() {
        let (handle, receiver) = CognitivePipelineHandle::channel_for_test(1);
        handle
            .enqueue_sync(CognitiveTask::LinkSimilarity {
                cid: "cancelled".to_string(),
                agent_id: "kernel".to_string(),
            })
            .unwrap();

        drop(receiver);

        assert_eq!(
            handle.snapshot(),
            CognitivePipelineSnapshot {
                accepted: 1,
                completed: 1,
                in_flight: 0,
                task_failed_attempts: 1,
                ..CognitivePipelineSnapshot::default()
            }
        );
    }

    #[test]
    fn completed_watermark_advances_only_after_contiguous_concurrent_work() {
        let progress = CognitivePipelineProgress::default();
        let first = progress.accept().unwrap();
        let second = progress.accept().unwrap();
        let third = progress.accept().unwrap();

        progress.complete(second, CognitiveTaskOutcome::VectorSucceeded);
        progress.complete(third, CognitiveTaskOutcome::LexicalDegraded);
        assert_eq!(
            progress.snapshot(),
            CognitivePipelineSnapshot {
                accepted: 3,
                completed: 0,
                in_flight: 1,
                document_vector_succeeded_attempts: 1,
                document_lexical_degraded_attempts: 1,
                task_failed_attempts: 0,
                other_succeeded_attempts: 0,
                inline_document_attempts: 0,
            }
        );

        progress.complete(first, CognitiveTaskOutcome::TaskFailed);
        assert_eq!(
            progress.snapshot(),
            CognitivePipelineSnapshot {
                accepted: 3,
                completed: 3,
                in_flight: 0,
                document_vector_succeeded_attempts: 1,
                document_lexical_degraded_attempts: 1,
                task_failed_attempts: 1,
                other_succeeded_attempts: 0,
                inline_document_attempts: 0,
            }
        );
    }

    #[test]
    fn dropping_task_completion_guard_advances_the_watermark() {
        let progress = Arc::new(CognitivePipelineProgress::default());
        let watermark = progress.accept().unwrap();
        {
            let _completion = CognitiveTaskCompletion {
                progress: Arc::clone(&progress),
                watermark,
                outcome: CognitiveTaskOutcome::TaskFailed,
            };
        }

        assert_eq!(
            progress.snapshot(),
            CognitivePipelineSnapshot {
                accepted: 1,
                completed: 1,
                in_flight: 0,
                document_vector_succeeded_attempts: 0,
                document_lexical_degraded_attempts: 0,
                task_failed_attempts: 1,
                other_succeeded_attempts: 0,
                inline_document_attempts: 0,
            }
        );
    }

    #[tokio::test]
    async fn worker_completion_reaches_the_accepted_watermark_after_processing() {
        let (kernel, _dir) = make_kernel();
        let handle = start_cognitive_pipeline(Arc::clone(&kernel), 4, 4);
        let watermark = handle
            .enqueue(CognitiveTask::LinkSimilarity {
                cid: "any-cid".to_string(),
                agent_id: "kernel".to_string(),
            })
            .await
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while handle.snapshot().completed < watermark {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cognitive task completion watermark must advance");
        assert_eq!(handle.snapshot().in_flight, 0);
    }

    #[tokio::test]
    async fn bounded_receiver_never_runs_more_than_four_tasks() {
        let (sender, receiver) = mpsc::channel(16);
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Semaphore::new(0));
        let worker = {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            let release = Arc::clone(&release);
            tokio::spawn(run_bounded_receiver(receiver, 4, move |_: usize| {
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                let release = Arc::clone(&release);
                async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    let permit = release.acquire().await.expect("test gate stays open");
                    permit.forget();
                    active.fetch_sub(1, Ordering::SeqCst);
                }
            }))
        };
        for task in 0..8 {
            sender.send(task).await.unwrap();
        }

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while active.load(Ordering::SeqCst) < 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("four tasks should enter the bounded worker");
        assert_eq!(peak.load(Ordering::SeqCst), 4);

        release.add_permits(8);
        drop(sender);
        worker.await.unwrap();
        assert_eq!(peak.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn receiver_leaves_backlog_in_queue_and_queue_full_does_not_accept() {
        let (handle, receiver) = CognitivePipelineHandle::channel_for_test(1);
        let active = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Semaphore::new(0));
        let worker = {
            let active = Arc::clone(&active);
            let release = Arc::clone(&release);
            tokio::spawn(run_bounded_receiver(receiver, 1, move |queued| {
                let active = Arc::clone(&active);
                let release = Arc::clone(&release);
                async move {
                    active.fetch_add(1, Ordering::SeqCst);
                    let permit = release.acquire().await.expect("test gate stays open");
                    permit.forget();
                    let QueuedCognitiveTask { mut completion, .. } = queued;
                    completion.record(CognitiveTaskOutcome::OtherSucceeded);
                    active.fetch_sub(1, Ordering::SeqCst);
                }
            }))
        };

        let task = |cid: &str| CognitiveTask::LinkSimilarity {
            cid: cid.to_string(),
            agent_id: "kernel".to_string(),
        };
        handle.enqueue_sync(task("running")).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while active.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first task should start");
        handle.enqueue_sync(task("queued")).unwrap();
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            handle.enqueue_sync(task("rejected")),
            Err(CognitivePipelineError::QueueFull)
        );
        assert_eq!(handle.snapshot().accepted, 2);

        release.add_permits(2);
        drop(handle);
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn panic_releases_slot_and_completion_guard_records_failure() {
        let (handle, receiver) = CognitivePipelineHandle::channel_for_test(2);
        let worker = tokio::spawn(run_bounded_receiver(receiver, 1, |queued| async move {
            let QueuedCognitiveTask { task, mut completion } = queued;
            if matches!(task, CognitiveTask::LinkSimilarity { ref cid, .. } if cid == "panic") {
                panic!("injected cognitive task panic");
            }
            completion.record(CognitiveTaskOutcome::OtherSucceeded);
        }));

        for cid in ["panic", "after-panic"] {
            handle
                .enqueue(CognitiveTask::LinkSimilarity {
                    cid: cid.to_string(),
                    agent_id: "kernel".to_string(),
                })
                .await
                .unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while handle.snapshot().completed != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("panic must release the only execution slot");
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.task_failed_attempts, 1);
        assert_eq!(snapshot.other_succeeded_attempts, 1);
        assert_eq!(snapshot.in_flight, 0);
        drop(handle);
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn task_error_releases_slot_for_following_work() {
        let (kernel, _dir) = make_kernel();
        let handle = start_cognitive_pipeline(Arc::clone(&kernel), 2, 1);
        handle
            .enqueue(CognitiveTask::Summarize {
                cid: "missing".to_string(),
                layer: SummaryLayer::L0,
                agent_id: "kernel".to_string(),
            })
            .await
            .unwrap();
        handle
            .enqueue(CognitiveTask::LinkSimilarity {
                cid: "after-error".to_string(),
                agent_id: "kernel".to_string(),
            })
            .await
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while handle.snapshot().completed != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("task error must release the only execution slot");
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.task_failed_attempts, 1);
        assert_eq!(snapshot.other_succeeded_attempts, 1);
        assert_eq!(snapshot.in_flight, 0);
    }

    #[tokio::test]
    async fn start_workers_twice_preserves_pipeline_and_progress() {
        let (kernel, _dir) = make_kernel();
        kernel.start_workers();
        let first = kernel.cognitive_pipeline.read().unwrap().clone().unwrap();
        let watermark = first
            .enqueue(CognitiveTask::LinkSimilarity {
                cid: "idempotent".to_string(),
                agent_id: "kernel".to_string(),
            })
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while first.snapshot().completed < watermark {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("initial pipeline task should complete");
        let before = first.snapshot();

        kernel.start_workers();
        let second = kernel.cognitive_pipeline.read().unwrap().clone().unwrap();
        assert!(Arc::ptr_eq(&first.progress, &second.progress));
        assert_eq!(second.snapshot(), before);
    }
}
