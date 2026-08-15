use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use crate::fs::embedding::types::EmbeddingTransformContract;
use crate::fs::{
    EmbedError, EmbedResult, EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingProvider, StubEmbeddingProvider,
};
use crate::memory::projection::{AbsentReason, ProjectionCoreInspection, ProjectionState, QueueReason};
use crate::memory::{CanonicalLedger, CanonicalRevision, ExpectedHead, MemoryContent, MemoryEntry, MemoryTier};

use super::*;

type CapturedFields = Vec<(String, String)>;
type CapturedSpans = Vec<(u64, CapturedFields)>;

#[derive(Clone, Default)]
struct CapturedProjectionTrace {
    events: Arc<Mutex<Vec<CapturedProjectionEvent>>>,
    spans: Arc<Mutex<CapturedSpans>>,
    active_spans: Arc<Mutex<Vec<u64>>>,
    next_span: Arc<AtomicU64>,
}

#[derive(Clone, Debug)]
struct CapturedProjectionEvent {
    parent_span: Option<u64>,
    fields: CapturedFields,
}

struct ProjectionTraceVisitor<'a>(&'a mut CapturedFields);

impl tracing::field::Visit for ProjectionTraceVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push((field.name().to_string(), format!("{value:?}")));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push((field.name().to_string(), value.to_string()));
    }
}

impl tracing::Subscriber for CapturedProjectionTrace {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn register_callsite(&self, _metadata: &'static tracing::Metadata<'static>) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }

    fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
        Some(tracing::metadata::LevelFilter::TRACE)
    }

    fn new_span(&self, attributes: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let mut fields = vec![("span_name".to_string(), attributes.metadata().name().to_string())];
        attributes.record(&mut ProjectionTraceVisitor(&mut fields));
        let id = self.next_span.fetch_add(1, Ordering::Relaxed) + 1;
        self.spans.lock().unwrap().push((id, fields));
        tracing::span::Id::from_u64(id)
    }

    fn record(&self, span: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let mut fields = Vec::new();
        values.record(&mut ProjectionTraceVisitor(&mut fields));
        self.spans.lock().unwrap().push((span.into_u64(), fields));
    }

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = Vec::new();
        event.record(&mut ProjectionTraceVisitor(&mut fields));
        self.events.lock().unwrap().push(CapturedProjectionEvent {
            parent_span: self.active_spans.lock().unwrap().last().copied(),
            fields,
        });
    }

    fn enter(&self, span: &tracing::span::Id) {
        self.active_spans.lock().unwrap().push(span.into_u64());
    }

    fn exit(&self, span: &tracing::span::Id) {
        assert_eq!(self.active_spans.lock().unwrap().pop(), Some(span.into_u64()));
    }
}

impl CapturedProjectionTrace {
    fn has_event(&self, phase: &str, result: &str) -> bool {
        self.events
            .lock()
            .unwrap()
            .iter()
            .any(|event| field_is(&event.fields, "phase", phase) && field_is(&event.fields, "result_category", result))
    }

    fn has_field(&self, name: &str) -> bool {
        let events = self.events.lock().unwrap().clone();
        let spans = self.spans.lock().unwrap().clone();
        events
            .iter()
            .map(|event| &event.fields)
            .chain(spans.iter().map(|(_, fields)| fields))
            .any(|record| record.iter().any(|(field, _)| field == name))
    }

    fn has_span_fields(&self, required: &[&str]) -> bool {
        self.spans.lock().unwrap().iter().any(|(_, span)| {
            required
                .iter()
                .all(|required| span.iter().any(|(field, _)| field == required))
        })
    }

    fn field_names(&self) -> Vec<String> {
        let events = self.events.lock().unwrap().clone();
        let spans = self.spans.lock().unwrap().clone();
        events
            .iter()
            .map(|event| &event.fields)
            .chain(spans.iter().map(|(_, fields)| fields))
            .flat_map(|record| record.iter().map(|(field, _)| field.clone()))
            .collect()
    }

    fn assert_subsequence(&self, expected: &[(&str, &str)]) {
        let events = self.events.lock().unwrap();
        let mut cursor = 0;
        for event in events.iter() {
            if cursor < expected.len()
                && field_is(&event.fields, "phase", expected[cursor].0)
                && field_is(&event.fields, "result_category", expected[cursor].1)
            {
                cursor += 1;
            }
        }
        assert_eq!(
            cursor,
            expected.len(),
            "missing projection trace phase subsequence: {events:?}"
        );
    }

    fn events_are_in_correlated_projection_span(&self, phases: &[&str]) -> bool {
        let spans = self.spans.lock().unwrap();
        let correlated = spans.iter().find_map(|(id, fields)| {
            [
                "run_id",
                "request_id",
                "revision_id",
                "projection_id",
                "manifest_watermark",
            ]
            .iter()
            .all(|required| fields.iter().any(|(field, _)| field == required))
            .then_some(*id)
        });
        let Some(correlated) = correlated else {
            return false;
        };
        self.events.lock().unwrap().iter().all(|event| {
            let phase_is_required = phases.iter().any(|phase| field_is(&event.fields, "phase", phase))
                && (!matches!(
                    event
                        .fields
                        .iter()
                        .find(|(field, _)| field == "phase")
                        .map(|(_, value)| value.as_str()),
                    Some("manifest_transition" | "root_publish")
                ) || field_is(&event.fields, "commit_kind", "ready"));
            !phase_is_required || event.parent_span == Some(correlated)
        })
    }

    fn flattened_values(&self) -> String {
        let events = self.events.lock().unwrap().clone();
        let spans = self.spans.lock().unwrap().clone();
        events
            .iter()
            .map(|event| &event.fields)
            .chain(spans.iter().map(|(_, fields)| fields))
            .flat_map(|record| record.iter().map(|(_, value)| value.as_str()))
            .collect()
    }

    fn contains_lower_hex_value(&self, length: usize) -> bool {
        let events = self.events.lock().unwrap().clone();
        let spans = self.spans.lock().unwrap().clone();
        events
            .iter()
            .map(|event| &event.fields)
            .chain(spans.iter().map(|(_, fields)| fields))
            .flat_map(|record| record.iter().map(|(_, value)| value.as_str()))
            .any(|value| contains_lower_hex_run(value, length))
    }
}

fn field_is(fields: &[(String, String)], name: &str, value: &str) -> bool {
    fields
        .iter()
        .any(|(field, observed)| field == name && observed.trim_matches('"') == value)
}

fn contains_lower_hex_run(value: &str, length: usize) -> bool {
    value.as_bytes().windows(length).any(|window| {
        window
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    })
}

fn capture_projection_trace<R>(run: impl FnOnce() -> R) -> (R, CapturedProjectionTrace) {
    let _guard = crate::TRACE_CAPTURE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let captured = CapturedProjectionTrace::default();
    let result = tracing::subscriber::with_default(captured.clone(), || {
        tracing::callsite::rebuild_interest_cache();
        run()
    });
    (result, captured)
}

fn run_projection_trace_child(test_name: &str, environment_flag: &str) {
    let executable = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(executable)
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env(environment_flag, "1")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "projection trace child failed");
            return;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("projection trace child exceeded deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct DeterministicProvider {
    identity: EmbeddingBuilderIdentity,
    vector: Vec<f32>,
    raw_dimension: usize,
    fail: bool,
}

struct BlockingProvider {
    identity: EmbeddingBuilderIdentity,
    entered: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl EmbeddingProvider for BlockingProvider {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed_document(text)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|text| self.embed_document(text)).collect()
    }

    fn embed_document(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
        self.entered
            .send(())
            .map_err(|_| EmbedError::ServerUnavailable("provider test hook disconnected".into()))?;
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| EmbedError::ServerUnavailable("provider test hook timeout".into()))?;
        Ok(EmbedResult::new(vec![0.6, 0.8], 1))
    }

    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
        Ok(self.identity.clone())
    }

    fn dimension(&self) -> usize {
        2
    }

    fn raw_dimension(&self) -> usize {
        2
    }

    fn model_name(&self) -> String {
        "projection-blocking-test".to_string()
    }
}

struct DriftingProvider {
    first: EmbeddingBuilderIdentity,
    second: EmbeddingBuilderIdentity,
    drifted: AtomicBool,
    calls: AtomicUsize,
}

struct CountingProvider {
    identity: EmbeddingBuilderIdentity,
    calls: Arc<AtomicUsize>,
}

impl EmbeddingProvider for CountingProvider {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed_document(text)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|text| self.embed_document(text)).collect()
    }

    fn embed_document(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(EmbedResult::new(vec![0.6, 0.8], 1))
    }

    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
        Ok(self.identity.clone())
    }

    fn dimension(&self) -> usize {
        2
    }

    fn raw_dimension(&self) -> usize {
        2
    }

    fn model_name(&self) -> String {
        "projection-counting-test".to_string()
    }
}

impl EmbeddingProvider for DriftingProvider {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed_document(text)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|text| self.embed_document(text)).collect()
    }

    fn embed_document(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.drifted.store(true, Ordering::SeqCst);
        Ok(EmbedResult::new(vec![0.6, 0.8], 1))
    }

    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
        if self.drifted.load(Ordering::SeqCst) {
            Ok(self.second.clone())
        } else {
            Ok(self.first.clone())
        }
    }

    fn dimension(&self) -> usize {
        2
    }

    fn raw_dimension(&self) -> usize {
        2
    }

    fn model_name(&self) -> String {
        "projection-drifting-test".to_string()
    }
}

impl DeterministicProvider {
    fn new(contract: &str, vector: Vec<f32>) -> Self {
        let raw_dimension = vector.len();
        Self {
            identity: EmbeddingBuilderIdentity::test_deterministic("projection-test", vector.len() as u32, contract),
            vector,
            raw_dimension,
            fail: false,
        }
    }

    fn failing(contract: &str, dimension: usize) -> Self {
        Self {
            identity: EmbeddingBuilderIdentity::test_deterministic("projection-test", dimension as u32, contract),
            vector: vec![1.0; dimension],
            raw_dimension: dimension,
            fail: true,
        }
    }
}

impl EmbeddingProvider for DeterministicProvider {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed_document(text)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|text| self.embed_document(text)).collect()
    }

    fn embed_document(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
        if self.fail {
            Err(EmbedError::ServerUnavailable(
                "PRIVATE_PROVIDER_URL_CANARY PRIVATE_PROVIDER_BODY_CANARY PRIVATE_PROVIDER_PREFIX_CANARY".to_string(),
            ))
        } else {
            Ok(EmbedResult::new(self.vector.clone(), 1))
        }
    }

    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
        Ok(self.identity.clone())
    }

    fn dimension(&self) -> usize {
        self.vector.len()
    }

    fn raw_dimension(&self) -> usize {
        self.raw_dimension
    }

    fn model_name(&self) -> String {
        "projection-test".to_string()
    }
}

struct CanonicalFixture {
    directory: tempfile::TempDir,
    vault: Arc<PersonalVaultStorage>,
    canonical: Arc<CASCanonicalLedger>,
    head: MemoryEntry,
}

fn canonical_fixture(text: &str) -> CanonicalFixture {
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(PersonalVaultStorage::open(&directory.path().join("vault"), None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let mut head = MemoryEntry::ephemeral(crate::PERSONAL_OWNER_ROLE_ID, text);
    head.tier = MemoryTier::Working;
    canonical
        .commit_expected(
            crate::PERSONAL_OWNER_ROLE_ID,
            MemoryTier::Working,
            ExpectedHead::Absent,
            CanonicalRevision::from_entry(&head).unwrap(),
        )
        .unwrap();
    CanonicalFixture {
        directory,
        vault,
        canonical,
        head,
    }
}

fn provider(contract: &str) -> VerifiedDocumentProviderSnapshot {
    VerifiedDocumentProviderSnapshot::verify(Arc::new(DeterministicProvider::new(contract, vec![0.6, 0.8]))).unwrap()
}

fn bootstrap(fixture: &CanonicalFixture, contract: &str) -> ProjectionCoordinator {
    ProjectionCoordinator::bootstrap_for_owner(
        Arc::clone(&fixture.vault),
        Arc::clone(&fixture.canonical),
        provider(contract),
    )
    .map(|(controller, _)| controller)
    .unwrap()
}

fn commit_child(fixture: &mut CanonicalFixture, text: &str, deleted: bool) {
    let mut child = fixture.head.clone();
    child.id = uuid::Uuid::new_v4().to_string();
    child.parent_revision_id = Some(fixture.head.id.as_str().into());
    child.content = MemoryContent::Text(text.to_string());
    child.canonical_content_hash = child.content.canonical_content_hash().unwrap();
    child.deleted_at = deleted.then_some(1);
    fixture
        .canonical
        .commit_expected(
            crate::PERSONAL_OWNER_ROLE_ID,
            child.tier,
            ExpectedHead::Revision(fixture.head.id.as_str().into()),
            CanonicalRevision::from_entry(&child).unwrap(),
        )
        .unwrap();
    fixture.head = child;
}

fn child_revision(parent: &MemoryEntry, text: &str, deleted: bool) -> CanonicalRevision {
    let mut child = parent.clone();
    child.id = uuid::Uuid::new_v4().to_string();
    child.parent_revision_id = Some(parent.id.as_str().into());
    if !deleted {
        child.content = MemoryContent::Text(text.to_string());
        child.canonical_content_hash = child.content.canonical_content_hash().unwrap();
    }
    child.deleted_at = deleted.then_some(1);
    CanonicalRevision::from_entry(&child).unwrap()
}

fn assert_provider_inflight_change_discards(deleted: bool) {
    let fixture = canonical_fixture("provider inflight source");
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let snapshot = VerifiedDocumentProviderSnapshot::verify(Arc::new(BlockingProvider {
        identity: EmbeddingBuilderIdentity::test_deterministic("projection-blocking-test", 2, "blocking-v1"),
        entered: entered_tx,
        release: Mutex::new(release_rx),
    }))
    .unwrap();
    let controller = Arc::new(
        ProjectionCoordinator::bootstrap_for_owner(
            Arc::clone(&fixture.vault),
            Arc::clone(&fixture.canonical),
            snapshot,
        )
        .map(|(controller, _)| controller)
        .unwrap(),
    );
    let job = controller.claim_one().unwrap().unwrap();
    let completion_controller = Arc::clone(&controller);
    let (result_tx, result_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        result_tx.send(completion_controller.complete_one(job)).unwrap();
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    fixture
        .canonical
        .commit_expected(
            crate::PERSONAL_OWNER_ROLE_ID,
            MemoryTier::Working,
            ExpectedHead::Revision(fixture.head.id.as_str().into()),
            child_revision(&fixture.head, "updated while provider runs", deleted),
        )
        .unwrap();
    release_tx.send(()).unwrap();
    assert!(matches!(
        result_rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap(),
        ProjectionBuildOutcome::Discarded
    ));
    worker.join().unwrap();
    let view = controller.core.current_view_for_test().unwrap();
    assert!(view
        .entries
        .iter()
        .all(|entry| !matches!(entry.state, ProjectionState::Ready { .. })));
}

#[test]
fn identity_unavailable_inspection_creates_no_projection_tree() {
    let fixture = canonical_fixture("unavailable source");
    let vault_path = fixture.directory.path().join("vault");
    assert!(!vault_path.join("projection-manifest").exists());
    assert!(!vault_path.join("projection-artifacts").exists());

    let identity_error = StubEmbeddingProvider::new().builder_identity().unwrap_err();
    let inspected =
        ProjectionCoordinator::inspect_identity_unavailable(&fixture.vault, &fixture.canonical, identity_error)
            .unwrap();
    assert!(matches!(inspected.inspection, ProjectionCoreInspection::Absent));
    assert_eq!(inspected.identity_category, "stub_provider");
    assert!(!vault_path.join("projection-manifest").exists());
    assert!(!vault_path.join("projection-artifacts").exists());
}

#[cfg(unix)]
#[test]
fn identity_unavailable_existing_inspection_is_zero_write_and_does_not_claim() {
    let fixture = canonical_fixture("unavailable existing source");
    let controller = bootstrap(&fixture, "contract-a");
    let vault_path = fixture.directory.path().join("vault");
    drop(controller);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let tree_before = vault.projection_tree_fingerprint_for_test().unwrap();
    let atime_before = vault.projection_tree_atimes_for_test(&tree_before).unwrap();
    let identity_error = StubEmbeddingProvider::new().builder_identity().unwrap_err();
    let inspected = ProjectionCoordinator::inspect_identity_unavailable(&vault, &canonical, identity_error).unwrap();
    assert!(matches!(
        inspected.inspection,
        ProjectionCoreInspection::Existing { .. }
    ));
    assert_eq!(
        vault.projection_tree_atimes_for_test(&tree_before).unwrap(),
        atime_before
    );
    assert_eq!(vault.projection_tree_fingerprint_for_test().unwrap(), tree_before);

    let opened = ProjectionCoordinator::open_existing(vault, canonical, provider("contract-a")).unwrap();
    assert!(!opened
        .core
        .current_view_for_test()
        .unwrap()
        .active_builder_specs
        .is_empty());
}

#[test]
fn fresh_bootstrap_ready_and_restart_replay() {
    let fixture = canonical_fixture("fresh projection source");
    let controller = bootstrap(&fixture, "contract-a");
    let job = controller.claim_one().unwrap().unwrap();
    assert!(matches!(
        controller.complete_one(job).unwrap(),
        ProjectionBuildOutcome::Ready
    ));
    let view = controller.core.current_view_for_test().unwrap();
    assert!(matches!(view.entries[0].state, ProjectionState::Ready { .. }));

    let vault_path = fixture.directory.path().join("vault");
    drop(controller);
    drop(fixture.canonical);
    drop(fixture.vault);
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let restarted = ProjectionCoordinator::open_existing(vault, canonical, provider("contract-a")).unwrap();
    assert!(matches!(
        restarted.core.current_view_for_test().unwrap().entries[0].state,
        ProjectionState::Ready { .. }
    ));
}

#[test]
fn lost_wake_is_recovered_by_restart_reconciliation() {
    let fixture = canonical_fixture("lost queue wake");
    let controller = bootstrap(&fixture, "contract-a");
    let request_id = uuid::Uuid::new_v4();
    let wake = controller
        .wake_for_current(
            fixture.head.memory_id.clone(),
            fixture.head.id.as_str().into(),
            fixture.head.canonical_content_hash.clone(),
            Some(request_id),
        )
        .unwrap();
    assert!(matches!(controller.notify(wake), WakeDisposition::Queued));
    let wake = controller
        .wake_for_current(
            fixture.head.memory_id.clone(),
            fixture.head.id.as_str().into(),
            fixture.head.canonical_content_hash.clone(),
            None,
        )
        .unwrap();
    assert!(matches!(controller.notify(wake), WakeDisposition::Full));
    let vault_path = fixture.directory.path().join("vault");
    drop(controller);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let restarted = ProjectionCoordinator::open_existing(vault, canonical, provider("contract-a")).unwrap();
    let job = restarted.claim_one().unwrap().unwrap();
    assert!(job.run_id != uuid::Uuid::nil());
    assert!(job.request_id.is_none());
    assert!(job.claim_manifest_watermark > 0);
    assert!(job.claim_canonical_watermark.revision_watermark > 0);
    assert_eq!(job.claim.attempt(), 1);
    assert!(matches!(
        restarted.complete_one(job).unwrap(),
        ProjectionBuildOutcome::Ready
    ));
}

#[cfg(unix)]
#[test]
fn builder_mismatch_is_zero_write_and_owner_change_can_reuse_same_vault() {
    let fixture = canonical_fixture("builder mismatch source");
    let controller = bootstrap(&fixture, "contract-a");
    let vault_path = fixture.directory.path().join("vault");
    drop(controller);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let before = vault.projection_tree_fingerprint_for_test().unwrap();
    assert!(matches!(
        ProjectionCoordinator::open_existing(Arc::clone(&vault), Arc::clone(&canonical), provider("contract-b")),
        Err(ProjectionControllerError::BuilderChangeRequiresOwner)
    ));
    assert_eq!(vault.projection_tree_fingerprint_for_test().unwrap(), before);

    let changed = ProjectionCoordinator::change_builder_for_owner(vault, canonical, provider("contract-b"))
        .map(|(controller, _)| controller)
        .unwrap();
    assert_eq!(
        changed.core.current_view_for_test().unwrap().active_builder_specs[0]
            .builder_spec
            .provider_compatibility_id,
        provider("contract-b").identity().provider_compatibility_id()
    );
}

#[test]
fn genesis_only_cutpoint_requires_owner_resume_and_normal_open_does_not_claim() {
    let fixture = canonical_fixture("genesis cutpoint source");
    fixture
        .canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::bootstrap_genesis_only_for_test(Arc::clone(&fixture.vault), &proof)
        })
        .unwrap()
        .unwrap()
        .unwrap();
    let vault_path = fixture.directory.path().join("vault");
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    assert!(matches!(
        ProjectionCoordinatorCore::inspect_existing(&vault, &canonical.projection_snapshot().unwrap(), None).unwrap(),
        ProjectionCoreInspection::GenesisOnly
    ));
    assert!(matches!(
        ProjectionCoordinator::open_existing(Arc::clone(&vault), Arc::clone(&canonical), provider("contract-a")),
        Err(ProjectionControllerError::NotInitialized)
    ));
    assert!(matches!(
        ProjectionCoordinator::change_builder_for_owner(
            Arc::clone(&vault),
            Arc::clone(&canonical),
            provider("contract-b"),
        ),
        Err(ProjectionControllerError::Projection(ProjectionError::Invalid {
            category: "projection_builder_change_requires_initialized"
        }))
    ));
    let resumed = ProjectionCoordinator::resume_genesis_for_owner(vault, canonical, provider("contract-a"))
        .map(|(controller, _)| controller)
        .unwrap();
    assert!(!resumed
        .core
        .current_view_for_test()
        .unwrap()
        .active_builder_specs
        .is_empty());
}

#[test]
fn artifact_store_cutpoint_records_artifact_store_failure_not_provider_failure() {
    let fixture = canonical_fixture("artifact cutpoint source");
    let controller = bootstrap(&fixture, "contract-a");
    let job = controller.claim_one().unwrap().unwrap();
    controller.core.inject_post_artifact_durability_failure_once();
    assert!(matches!(
        controller.complete_one(job).unwrap(),
        ProjectionBuildOutcome::Failed(FailureCategory::ArtifactStoreUnavailable)
    ));
    assert!(matches!(
        controller.core.current_view_for_test().unwrap().entries[0].state,
        ProjectionState::Failed {
            failure_category: FailureCategory::ArtifactStoreUnavailable,
            retryable: true,
            ..
        }
    ));
}

#[test]
fn canonical_update_before_final_guard_discards_late_result() {
    let mut fixture = canonical_fixture("slow source");
    let controller = bootstrap(&fixture, "contract-a");
    let job = controller.claim_one().unwrap().unwrap();
    commit_child(&mut fixture, "new current source", false);
    assert!(matches!(
        controller.complete_one(job).unwrap(),
        ProjectionBuildOutcome::Discarded
    ));
    let view = controller.core.current_view_for_test().unwrap();
    assert!(view
        .entries
        .iter()
        .all(|entry| !matches!(entry.state, ProjectionState::Ready { .. })));
}

#[test]
fn canonical_update_during_provider_call_discards_late_result() {
    assert_provider_inflight_change_discards(false);
}

#[test]
fn canonical_delete_during_provider_call_discards_late_result() {
    assert_provider_inflight_change_discards(true);
}

#[test]
fn provider_identity_drift_after_embedding_is_durable_failure_after_restart() {
    let fixture = canonical_fixture("provider drift source");
    let expected_identity = EmbeddingBuilderIdentity::test_deterministic("projection-drifting-test", 2, "drift-a");
    let provider = Arc::new(DriftingProvider {
        first: expected_identity.clone(),
        second: EmbeddingBuilderIdentity::test_deterministic("projection-drifting-test", 2, "drift-b"),
        drifted: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    });
    let snapshot =
        VerifiedDocumentProviderSnapshot::verify(Arc::clone(&provider) as Arc<dyn EmbeddingProvider>).unwrap();
    let controller = ProjectionCoordinator::bootstrap_for_owner(
        Arc::clone(&fixture.vault),
        Arc::clone(&fixture.canonical),
        snapshot,
    )
    .map(|(controller, _)| controller)
    .unwrap();
    let job = controller.claim_one().unwrap().unwrap();
    assert!(matches!(
        controller.complete_one(job).unwrap(),
        ProjectionBuildOutcome::Failed(FailureCategory::ProviderIdentityChanged)
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        controller.core.current_view_for_test().unwrap().entries[0].state,
        ProjectionState::Failed {
            failure_category: FailureCategory::ProviderIdentityChanged,
            retryable: false,
            retry_not_before: None,
            ..
        }
    ));

    let vault_path = fixture.directory.path().join("vault");
    drop(controller);
    drop(fixture.canonical);
    drop(fixture.vault);
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let stable = VerifiedDocumentProviderSnapshot::verify(Arc::new(DeterministicProvider {
        identity: expected_identity,
        vector: vec![0.6, 0.8],
        raw_dimension: 2,
        fail: false,
    }))
    .unwrap();
    let restarted = ProjectionCoordinator::open_existing(vault, canonical, stable).unwrap();
    assert!(matches!(
        restarted.core.current_view_for_test().unwrap().entries[0].state,
        ProjectionState::Failed {
            failure_category: FailureCategory::ProviderIdentityChanged,
            retryable: false,
            retry_not_before: None,
            ..
        }
    ));
}

#[test]
fn old_sealed_attempt_is_discarded_before_provider_call_after_owner_builder_change() {
    let fixture = canonical_fixture("old attempt source");
    let calls = Arc::new(AtomicUsize::new(0));
    let old_provider = VerifiedDocumentProviderSnapshot::verify(Arc::new(CountingProvider {
        identity: EmbeddingBuilderIdentity::test_deterministic("projection-counting-test", 2, "old-builder"),
        calls: Arc::clone(&calls),
    }))
    .unwrap();
    let old_controller = ProjectionCoordinator::bootstrap_for_owner(
        Arc::clone(&fixture.vault),
        Arc::clone(&fixture.canonical),
        old_provider,
    )
    .map(|(controller, _)| controller)
    .unwrap();
    let old_job = old_controller.claim_one().unwrap().unwrap();
    let vault_path = fixture.directory.path().join("vault");
    drop(old_controller);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let changed = ProjectionCoordinator::change_builder_for_owner(vault, canonical, provider("new-builder"))
        .map(|(controller, _)| controller)
        .unwrap();
    assert!(matches!(
        changed.complete_one(old_job).unwrap(),
        ProjectionBuildOutcome::Discarded
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(changed
        .core
        .current_view_for_test()
        .unwrap()
        .entries
        .iter()
        .all(|entry| !matches!(entry.state, ProjectionState::Ready { .. })));
}

#[test]
fn matryoshka_non_unit_output_is_durable_nonretryable_failure_after_restart() {
    let fixture = canonical_fixture("matryoshka invalid source");
    let identity = EmbeddingBuilderIdentity::test_deterministic("projection-matryoshka-test", 4, "matryoshka-v1")
        .with_adaptive_contract(EmbeddingTransformContract::ProviderNativeInputV1, Some(2))
        .unwrap();
    let snapshot = VerifiedDocumentProviderSnapshot::verify(Arc::new(DeterministicProvider {
        identity: identity.clone(),
        vector: vec![0.5, 0.5],
        raw_dimension: 4,
        fail: false,
    }))
    .unwrap();
    let controller = ProjectionCoordinator::bootstrap_for_owner(
        Arc::clone(&fixture.vault),
        Arc::clone(&fixture.canonical),
        snapshot,
    )
    .map(|(controller, _)| controller)
    .unwrap();
    let job = controller.claim_one().unwrap().unwrap();
    assert!(matches!(
        controller.complete_one(job).unwrap(),
        ProjectionBuildOutcome::Failed(FailureCategory::InvalidProjection)
    ));
    assert!(matches!(
        controller.core.current_view_for_test().unwrap().entries[0].state,
        ProjectionState::Failed {
            failure_category: FailureCategory::InvalidProjection,
            retryable: false,
            retry_not_before: None,
            ..
        }
    ));

    let vault_path = fixture.directory.path().join("vault");
    drop(controller);
    drop(fixture.canonical);
    drop(fixture.vault);
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let stable = VerifiedDocumentProviderSnapshot::verify(Arc::new(DeterministicProvider {
        identity,
        vector: vec![0.6, 0.8],
        raw_dimension: 4,
        fail: false,
    }))
    .unwrap();
    let restarted = ProjectionCoordinator::open_existing(vault, canonical, stable).unwrap();
    assert!(matches!(
        restarted.core.current_view_for_test().unwrap().entries[0].state,
        ProjectionState::Failed {
            failure_category: FailureCategory::InvalidProjection,
            retryable: false,
            retry_not_before: None,
            ..
        }
    ));
}

#[test]
fn final_guard_serializes_ready_before_canonical_update_without_deadlock() {
    let fixture = canonical_fixture("final guard source");
    let controller = Arc::new(bootstrap(&fixture, "contract-a"));
    let job = controller.claim_one().unwrap().unwrap();
    let (guard_entered_tx, guard_entered_rx) = mpsc::sync_channel(1);
    let (guard_release_tx, guard_release_rx) = mpsc::sync_channel(1);
    fixture
        .canonical
        .inject_projection_guard_hook(guard_entered_tx, guard_release_rx);

    let completion_controller = Arc::clone(&controller);
    let (completion_tx, completion_rx) = mpsc::channel();
    let completion = std::thread::spawn(move || {
        completion_tx.send(completion_controller.complete_one(job)).unwrap();
    });
    guard_entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let canonical = Arc::clone(&fixture.canonical);
    let parent_id = fixture.head.id.clone();
    let child = child_revision(&fixture.head, "new head after ready", false);
    let child_id = child.revision_id.clone();
    let (update_started_tx, update_started_rx) = mpsc::sync_channel(1);
    let (update_tx, update_rx) = mpsc::channel();
    let update = std::thread::spawn(move || {
        update_started_tx.send(()).unwrap();
        let result = canonical.commit_expected(
            crate::PERSONAL_OWNER_ROLE_ID,
            MemoryTier::Working,
            ExpectedHead::Revision(parent_id.as_str().into()),
            child,
        );
        update_tx.send(result).unwrap();
    });
    update_started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let update_was_blocked = matches!(
        update_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    );

    guard_release_tx.send(()).unwrap();
    assert!(update_was_blocked);
    assert!(matches!(
        completion_rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap(),
        ProjectionBuildOutcome::Ready
    ));
    update_rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    completion.join().unwrap();
    update.join().unwrap();

    controller.reconcile_once().unwrap();
    let view = controller.core.current_view_for_test().unwrap();
    assert!(view.entries.iter().any(|entry| {
        entry.source.revision_id == fixture.head.id.as_str()
            && matches!(
                entry.state,
                ProjectionState::AbsentByPolicy {
                    reason: AbsentReason::Superseded
                }
            )
    }));
    assert!(view.entries.iter().any(|entry| {
        entry.source.revision_id == child_id
            && matches!(
                entry.state,
                ProjectionState::Queued {
                    reason: QueueReason::Reconciliation
                }
            )
    }));

    let vault_path = fixture.directory.path().join("vault");
    drop(view);
    drop(controller);
    drop(fixture.canonical);
    drop(fixture.vault);
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let restarted = ProjectionCoordinator::open_existing(vault, canonical, provider("contract-a")).unwrap();
    let restarted_view = restarted.core.current_view_for_test().unwrap();
    assert!(restarted_view.entries.iter().any(|entry| {
        entry.source.revision_id == fixture.head.id.as_str()
            && matches!(
                entry.state,
                ProjectionState::AbsentByPolicy {
                    reason: AbsentReason::Superseded
                }
            )
    }));
    assert!(restarted_view.entries.iter().any(|entry| {
        entry.source.revision_id == child_id
            && matches!(
                entry.state,
                ProjectionState::Queued {
                    reason: QueueReason::Reconciliation
                }
            )
    }));
}

#[test]
fn provider_failure_is_durable_and_nonzero_shape_is_enforced() {
    let fixture = canonical_fixture("provider failure source");
    let controller = ProjectionCoordinator::bootstrap_for_owner(
        Arc::clone(&fixture.vault),
        Arc::clone(&fixture.canonical),
        VerifiedDocumentProviderSnapshot::verify(Arc::new(DeterministicProvider::failing("contract-a", 2))).unwrap(),
    )
    .map(|(controller, _)| controller)
    .unwrap();
    let job = controller.claim_one().unwrap().unwrap();
    assert!(matches!(
        controller.complete_one(job).unwrap(),
        ProjectionBuildOutcome::Failed(FailureCategory::ProviderUnavailable)
    ));
    assert!(matches!(
        controller.core.current_view_for_test().unwrap().entries[0].state,
        ProjectionState::Failed {
            failure_category: FailureCategory::ProviderUnavailable,
            retryable: true,
            ..
        }
    ));
}

#[test]
fn independent_claims_complete_in_reverse_order_without_global_head_loss() {
    let fixture = canonical_fixture("first source");
    let mut second = MemoryEntry::ephemeral(crate::PERSONAL_OWNER_ROLE_ID, "second source");
    second.tier = MemoryTier::Working;
    fixture
        .canonical
        .commit_expected(
            crate::PERSONAL_OWNER_ROLE_ID,
            MemoryTier::Working,
            ExpectedHead::Absent,
            CanonicalRevision::from_entry(&second).unwrap(),
        )
        .unwrap();
    let controller = bootstrap(&fixture, "contract-a");
    let first = controller.claim_one().unwrap().unwrap();
    let second = controller.claim_one().unwrap().unwrap();
    assert!(matches!(
        controller.complete_one(second).unwrap(),
        ProjectionBuildOutcome::Ready
    ));
    assert!(matches!(
        controller.complete_one(first).unwrap(),
        ProjectionBuildOutcome::Ready
    ));
    let view = controller.core.current_view_for_test().unwrap();
    assert_eq!(
        view.entries
            .iter()
            .filter(|entry| matches!(entry.state, ProjectionState::Ready { .. }))
            .count(),
        2
    );
}

#[test]
fn projection_trace_records_true_phase_order_and_excludes_private_canaries() {
    if std::env::var_os("PLICO_PROJECTION_TRACE_PHASE_CHILD").is_none() {
        return;
    }
    let mut fixture = canonical_fixture("PRIVATE_CONTENT_QUERY_TOKEN_CANARY");
    let controller = bootstrap(&fixture, "private-provider-contract-canary");
    let initial = controller.claim_one().unwrap().unwrap();
    assert!(matches!(
        controller.complete_one(initial).unwrap(),
        ProjectionBuildOutcome::Ready
    ));
    commit_child(&mut fixture, "PRIVATE_NEW_CONTENT_QUERY_TOKEN_CANARY", false);
    let request_id = Uuid::new_v4();
    let private_path = fixture.directory.path().display().to_string();
    let private_content_hash = fixture.head.canonical_content_hash.to_string();
    let private_root_hash = fixture.canonical.projection_snapshot().unwrap().root_hash;
    let private_compatibility_hash = controller.provider.identity().provider_compatibility_id().to_string();
    let (outcome, captured) = capture_projection_trace(|| {
        let wake = controller
            .wake_for_current(
                fixture.head.memory_id.clone(),
                fixture.head.id.as_str().into(),
                fixture.head.canonical_content_hash.clone(),
                Some(request_id),
            )
            .unwrap();
        assert!(matches!(controller.notify(wake), WakeDisposition::Queued));
        let job = controller.reconcile_and_claim_one().unwrap().unwrap();
        controller.complete_one(job).unwrap()
    });
    assert!(matches!(outcome, ProjectionBuildOutcome::Ready));
    let ready_view = controller.core.current_view_for_test().unwrap();
    let private_artifact_hash = ready_view
        .entries
        .iter()
        .find_map(|entry| match &entry.state {
            ProjectionState::Ready { artifact, .. } => Some(artifact.artifact_hash.clone()),
            _ => None,
        })
        .expect("expected a Ready projection artifact");
    captured.assert_subsequence(&[
        ("reconcile", "started"),
        ("manifest_transition", "validated"),
        ("root_publish", "published"),
        ("reconcile", "complete"),
        ("manifest_claim", "building"),
        ("claim_validation", "verified"),
        ("canonical_document", "verified"),
        ("provider_precheck", "verified"),
        ("provider_embed", "completed"),
        ("provider_postcheck", "verified"),
        ("output_validation", "verified"),
        ("final_canonical_guard", "acquired"),
        ("manifest_transition", "validated"),
        ("artifact_verify", "durable"),
        ("root_publish", "published"),
        ("complete", "ready"),
    ]);
    for field in [
        "operation",
        "phase",
        "result_category",
        "run_id",
        "request_id",
        "revision_id",
        "projection_id",
        "manifest_watermark",
        "canonical_revision_watermark",
    ] {
        assert!(captured.has_field(field), "missing trace field {field}");
    }
    assert!(captured.has_span_fields(&[
        "run_id",
        "request_id",
        "revision_id",
        "projection_id",
        "manifest_watermark",
        "canonical_revision_watermark",
    ]));
    let correlated_phases = [
        "claim_validation",
        "canonical_document",
        "provider_precheck",
        "provider_embed",
        "provider_postcheck",
        "output_validation",
        "final_canonical_guard",
        "manifest_transition",
        "artifact_verify",
        "root_publish",
        "complete",
    ];
    assert!(
        captured.events_are_in_correlated_projection_span(&correlated_phases),
        "projection events escaped correlated span: events={:?} spans={:?}",
        captured.events.lock().unwrap(),
        captured.spans.lock().unwrap()
    );
    assert!(captured.events.lock().unwrap().iter().any(|event| {
        field_is(&event.fields, "phase", "manifest_claim")
            && field_is(&event.fields, "result_category", "building")
            && event.fields.iter().any(|(field, _)| field == "run_id")
            && event.fields.iter().any(|(field, _)| field == "request_id")
    }));
    let turn_span = captured
        .spans
        .lock()
        .unwrap()
        .iter()
        .find_map(|(id, fields)| field_is(fields, "span_name", "projection_turn").then_some(*id))
        .expect("projection turn span was not captured");
    assert!(captured
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| {
            matches!(
                event
                    .fields
                    .iter()
                    .find(|(field, _)| field == "phase")
                    .map(|(_, value)| value.as_str()),
                Some("manifest_transition" | "root_publish")
            ) && field_is(&event.fields, "commit_kind", "control")
        })
        .all(|event| event.parent_span == Some(turn_span)));
    let forbidden_fields = [
        "content_hash",
        "root_hash",
        "artifact_hash",
        "provider_url",
        "body",
        "path",
        "query",
        "tags",
        "bearer",
    ];
    for field in captured.field_names() {
        assert!(
            !forbidden_fields.contains(&field.as_str()),
            "forbidden trace field {field}"
        );
    }
    let values = captured.flattened_values();
    assert!(
        !captured.contains_lower_hex_value(64),
        "full hash leaked in projection trace"
    );
    for sentinel in [
        "PRIVATE_CONTENT_QUERY_TOKEN_CANARY",
        "PRIVATE_NEW_CONTENT_QUERY_TOKEN_CANARY",
        private_path.as_str(),
        private_content_hash.as_str(),
        private_root_hash.as_str(),
        private_artifact_hash.as_str(),
        private_compatibility_hash.as_str(),
    ] {
        assert!(!values.contains(sentinel), "private trace canary leaked");
    }

    let failure_fixture = canonical_fixture("PRIVATE_FAILURE_CONTENT_CANARY");
    let failure_controller = ProjectionCoordinator::bootstrap_for_owner(
        Arc::clone(&failure_fixture.vault),
        Arc::clone(&failure_fixture.canonical),
        VerifiedDocumentProviderSnapshot::verify(Arc::new(DeterministicProvider::failing(
            "private-failure-contract",
            2,
        )))
        .unwrap(),
    )
    .map(|(controller, _)| controller)
    .unwrap();
    let failure_job = failure_controller.claim_one().unwrap().unwrap();
    let (failure_outcome, failure_trace) =
        capture_projection_trace(|| failure_controller.complete_one(failure_job).unwrap());
    assert!(matches!(
        failure_outcome,
        ProjectionBuildOutcome::Failed(FailureCategory::ProviderUnavailable)
    ));
    assert!(failure_trace.has_event("provider_embed", "unavailable"));
    assert!(failure_trace.has_event("complete", "failed"));
    let failure_values = failure_trace.flattened_values();
    for sentinel in [
        "PRIVATE_FAILURE_CONTENT_CANARY",
        "PRIVATE_PROVIDER_URL_CANARY",
        "PRIVATE_PROVIDER_BODY_CANARY",
        "PRIVATE_PROVIDER_PREFIX_CANARY",
    ] {
        assert!(
            !failure_values.contains(sentinel),
            "provider failure trace leaked private data"
        );
    }
    assert!(!failure_trace.contains_lower_hex_value(64));
}

#[test]
fn projection_trace_distinguishes_artifact_and_pointer_cutpoints() {
    if std::env::var_os("PLICO_PROJECTION_TRACE_CUTPOINT_CHILD").is_none() {
        return;
    }
    let artifact_fixture = canonical_fixture("artifact trace cutpoint");
    let artifact_controller = bootstrap(&artifact_fixture, "contract-a");
    let artifact_job = artifact_controller.claim_one().unwrap().unwrap();
    artifact_controller.core.inject_post_artifact_durability_failure_once();
    let (artifact_outcome, artifact_trace) =
        capture_projection_trace(|| artifact_controller.complete_one(artifact_job).unwrap());
    assert!(matches!(
        artifact_outcome,
        ProjectionBuildOutcome::Failed(FailureCategory::ArtifactStoreUnavailable)
    ));
    assert!(artifact_trace.has_event("artifact_persist", "durable_orphan"));
    assert!(!artifact_trace.has_event("artifact_verify", "durable"));
    assert!(!artifact_trace.events.lock().unwrap().iter().any(|event| {
        field_is(&event.fields, "phase", "root_publish")
            && field_is(&event.fields, "result_category", "published")
            && field_is(&event.fields, "commit_kind", "ready")
    }));
    assert!(artifact_trace.events.lock().unwrap().iter().any(|event| {
        field_is(&event.fields, "phase", "root_publish")
            && field_is(&event.fields, "result_category", "published")
            && field_is(&event.fields, "commit_kind", "failed")
    }));

    let pointer_fixture = canonical_fixture("pointer trace cutpoint");
    let pointer_controller = bootstrap(&pointer_fixture, "contract-a");
    let pointer_job = pointer_controller.claim_one().unwrap().unwrap();
    pointer_controller.core.inject_pre_pointer_failure_once();
    let (pointer_outcome, pointer_trace) = capture_projection_trace(|| pointer_controller.complete_one(pointer_job));
    assert!(matches!(
        pointer_outcome,
        Err(ProjectionControllerError::Projection(ProjectionError::Io(_)))
    ));
    assert!(pointer_trace.has_event("artifact_verify", "durable"));
    assert!(pointer_trace.has_event("root_publish", "pre_exchange_failed"));
    assert!(!pointer_trace.has_event("root_publish", "published"));

    let exchange_fixture = canonical_fixture("exchange trace cutpoint");
    let exchange_controller = bootstrap(&exchange_fixture, "contract-a");
    let exchange_job = exchange_controller.claim_one().unwrap().unwrap();
    exchange_controller.core.inject_post_exchange_sync_failure_once();
    let (exchange_outcome, exchange_trace) =
        capture_projection_trace(|| exchange_controller.complete_one(exchange_job));
    assert!(matches!(
        exchange_outcome,
        Err(ProjectionControllerError::Projection(
            ProjectionError::CommitIndeterminate
        ))
    ));
    assert!(exchange_trace.has_event("artifact_verify", "durable"));
    assert!(exchange_trace.has_event("root_publish", "indeterminate"));
    assert!(!exchange_trace.has_event("root_publish", "published"));
}

#[test]
fn projection_trace_phase_contract_is_process_isolated() {
    run_projection_trace_child(
        "kernel::ops::projection_controller::tests::projection_trace_records_true_phase_order_and_excludes_private_canaries",
        "PLICO_PROJECTION_TRACE_PHASE_CHILD",
    );
}

#[test]
fn projection_trace_cutpoint_contract_is_process_isolated() {
    run_projection_trace_child(
        "kernel::ops::projection_controller::tests::projection_trace_distinguishes_artifact_and_pointer_cutpoints",
        "PLICO_PROJECTION_TRACE_CUTPOINT_CHILD",
    );
}
