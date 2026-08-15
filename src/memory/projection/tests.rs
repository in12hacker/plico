use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;
use uuid::Uuid;

use super::coordinator_core::{
    ProjectionCoordinatorCore, ProjectionCoreOpenError, ProjectionDurableReceipt, ProjectionRebuildError,
    ProjectionRebuildSelector, ProjectionStatusObservation, ProjectionStatusState,
};
use super::current_view::rebuild_current_view;
use super::hash::{
    artifact_bytes_and_hash, builder_spec_bytes_and_hash, projection_id, root_bytes_and_hash, segment_bytes_and_hash,
    view_bytes_and_hash,
};
use super::model::{
    ArtifactDescriptor, BuilderSpec, CanonicalSourceIdentity, CanonicalWatermark, EmbeddingArtifact,
    EmbeddingInputContract, EmbeddingNormalization, EmbeddingOperationContract, FailureCategory, ManifestEvent,
    ManifestRecord, ProjectionError, ProjectionKind, ProjectionState, QueueReason, StaleReason, BUILDER_SPEC_SCHEMA,
    EMBEDDING_ARTIFACT_SCHEMA, MANIFEST_RECORD_SCHEMA,
};
use super::store::{
    validate_candidate, ProjectionCommitActor, ProjectionManifestStore, ProjectionStoreInspection,
    ProjectionUnknownSchemaTarget,
};
use super::validate::{validate_artifact, validate_builder_spec};
use crate::cas::PersonalVaultStorage;
use crate::memory::{
    CASCanonicalLedger, CanonicalLedger, CanonicalProjectionSnapshot, CanonicalRevision, ExpectedHead, MemoryContent,
    MemoryEntry, MemoryTier,
};

type ResetTraceFields = Vec<(String, String)>;
type ResetTraceSpans = Vec<(u64, ResetTraceFields)>;

#[derive(Clone, Default)]
struct CapturedResetTrace {
    events: Arc<Mutex<Vec<CapturedResetEvent>>>,
    spans: Arc<Mutex<ResetTraceSpans>>,
    active_spans: Arc<Mutex<Vec<u64>>>,
    next_span: Arc<AtomicU64>,
}

#[derive(Clone, Debug)]
struct CapturedResetEvent {
    parent_span: Option<u64>,
    fields: ResetTraceFields,
}

struct ResetTraceVisitor<'a>(&'a mut ResetTraceFields);

impl tracing::field::Visit for ResetTraceVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push((field.name().to_string(), format!("{value:?}")));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push((field.name().to_string(), value.to_string()));
    }
}

impl tracing::Subscriber for CapturedResetTrace {
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
        let mut fields = Vec::new();
        attributes.record(&mut ResetTraceVisitor(&mut fields));
        let id = self.next_span.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        self.spans.lock().unwrap().push((id, fields));
        tracing::span::Id::from_u64(id)
    }

    fn record(&self, span: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let mut fields = Vec::new();
        values.record(&mut ResetTraceVisitor(&mut fields));
        self.spans.lock().unwrap().push((span.into_u64(), fields));
    }

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = Vec::new();
        event.record(&mut ResetTraceVisitor(&mut fields));
        self.events.lock().unwrap().push(CapturedResetEvent {
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

fn reset_trace_field_is(fields: &ResetTraceFields, name: &str, value: &str) -> bool {
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

fn capture_reset_trace<R>(run: impl FnOnce() -> R) -> (R, CapturedResetTrace) {
    let _guard = crate::TRACE_CAPTURE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let captured = CapturedResetTrace::default();
    let result = tracing::subscriber::with_default(captured.clone(), || {
        tracing::callsite::rebuild_interest_cache();
        run()
    });
    (result, captured)
}

impl CapturedResetTrace {
    fn assert_no_untrusted_reset_correlation(&self) {
        let spans = self.spans.lock().unwrap();
        let events = self.events.lock().unwrap();
        assert!(
            spans
                .iter()
                .all(|(_, fields)| !reset_trace_field_is(fields, "operation", "projection_reset")),
            "untrusted reset marker produced a correlated span: {spans:?}"
        );
        assert!(
            events
                .iter()
                .all(|event| !reset_trace_field_is(&event.fields, "operation", "projection_reset")),
            "untrusted reset marker produced a correlated event: {events:?}"
        );
    }

    fn assert_contract(&self, expected: &[(&str, &str)], forbidden_values: &[String]) {
        let spans = self.spans.lock().unwrap().clone();
        let events = self.events.lock().unwrap().clone();
        let reset_spans = spans
            .iter()
            .filter(|(_, fields)| reset_trace_field_is(fields, "operation", "projection_reset"))
            .map(|(id, fields)| (*id, fields.clone()))
            .collect::<Vec<_>>();
        assert!(!reset_spans.is_empty(), "projection reset trace has no correlated span");
        let reset_span_ids = reset_spans
            .iter()
            .map(|(id, _)| *id)
            .collect::<std::collections::HashSet<_>>();
        let reset_events = events
            .iter()
            .filter(|event| {
                reset_trace_field_is(&event.fields, "operation", "projection_reset")
                    || event.parent_span.is_some_and(|id| reset_span_ids.contains(&id))
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut cursor = 0usize;
        for event in &reset_events {
            if cursor < expected.len()
                && reset_trace_field_is(&event.fields, "phase", expected[cursor].0)
                && reset_trace_field_is(&event.fields, "result_category", expected[cursor].1)
            {
                cursor += 1;
            }
        }
        assert_eq!(
            cursor,
            expected.len(),
            "missing reset trace subsequence: {reset_events:?}"
        );

        let mut operation_ids = std::collections::HashSet::new();
        for (_, fields) in &reset_spans {
            if let Some((_, value)) = fields.iter().find(|(field, _)| field == "reset_operation_id") {
                operation_ids.insert(value.trim_matches('"').to_string());
            }
        }
        for event in &reset_events {
            if let Some((_, value)) = event.fields.iter().find(|(field, _)| field == "reset_operation_id") {
                operation_ids.insert(value.trim_matches('"').to_string());
            }
        }
        assert_eq!(
            operation_ids.len(),
            1,
            "reset trace did not preserve one operation id: {operation_ids:?}"
        );
        assert!(Uuid::parse_str(operation_ids.iter().next().unwrap()).is_ok());

        let allowed_fields = [
            "operation",
            "phase",
            "outcome",
            "result_category",
            "reset_reason",
            "reset_operation_id",
            "selected_count",
            "manifest_generation",
            "event_watermark",
            "reconciled_revision_watermark",
            "reconciled_policy_watermark",
            "reconciled_relation_watermark",
        ];
        let records = reset_spans
            .iter()
            .map(|(_, fields)| fields)
            .chain(reset_events.iter().map(|event| &event.fields));
        let records = records.cloned().collect::<Vec<_>>();
        for fields in &records {
            for (field, _) in fields {
                assert!(
                    allowed_fields.contains(&field.as_str()),
                    "forbidden reset trace field {field}"
                );
            }
        }
        let values = records
            .iter()
            .flat_map(|fields| fields.iter().map(|(_, value)| value.as_str()))
            .collect::<Vec<_>>();
        for forbidden in forbidden_values {
            assert!(
                values.iter().all(|value| !value.contains(forbidden)),
                "private reset trace canary leaked: {forbidden}"
            );
        }
        assert!(
            values.iter().all(|value| !contains_lower_hex_run(value, 64)),
            "reset trace leaked a full lowercase hash"
        );
    }
}

fn run_reset_trace_child(test_name: &str, environment_flag: &str) {
    let executable = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(executable)
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env(environment_flag, "1")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "projection reset trace child failed");
            return;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("projection reset trace child exceeded deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Fixture {
    directory: TempDir,
    vault: Arc<PersonalVaultStorage>,
    canonical: CASCanonicalLedger,
    projection: Arc<ProjectionManifestStore>,
    snapshot: CanonicalProjectionSnapshot,
    source: CanonicalSourceIdentity,
    builder: BuilderSpec,
    builder_hash: String,
    clock: Arc<AtomicU64>,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(PersonalVaultStorage::open(&directory.path().join("vault"), None).unwrap());
    let canonical = CASCanonicalLedger::new(Arc::clone(&vault)).unwrap();
    let entry = MemoryEntry::ephemeral(crate::PERSONAL_OWNER_ROLE_ID, "projection fixture");
    let revision = CanonicalRevision::from_entry(&MemoryEntry {
        tier: MemoryTier::Working,
        ..entry
    })
    .unwrap();
    canonical
        .commit_expected(
            crate::PERSONAL_OWNER_ROLE_ID,
            MemoryTier::Working,
            ExpectedHead::Absent,
            revision,
        )
        .unwrap();
    let snapshot = canonical.projection_snapshot().unwrap();
    let canonical_revision = snapshot.revisions.first().unwrap();
    let source = CanonicalSourceIdentity {
        canonical_kind: "memory_revision".to_string(),
        memory_id: canonical_revision.memory_id.clone(),
        revision_id: canonical_revision.revision_id.clone(),
        revision_sequence: canonical_revision.sequence,
        content_hash: canonical_revision.content_hash.clone(),
    };
    let builder = fixture_builder();
    let builder_hash = builder_spec_bytes_and_hash(&builder).unwrap().1;
    let clock = Arc::new(AtomicU64::new(10));
    let clock_reader = Arc::clone(&clock);
    let projection = ProjectionManifestStore::bootstrap_new_with_clock(
        Arc::clone(&vault),
        &snapshot,
        Arc::new(move || clock_reader.load(std::sync::atomic::Ordering::SeqCst)),
    )
    .unwrap();
    Fixture {
        directory,
        vault,
        canonical,
        projection,
        snapshot,
        source,
        builder,
        builder_hash,
        clock,
    }
}

#[test]
fn expired_building_lease_is_requeued_durably_after_restart_without_sleep() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let claim = fixture.projection.claim_next(&fixture.snapshot).unwrap().unwrap();
    assert!(matches!(
        fixture.projection.current_view().unwrap().entries[0].state,
        ProjectionState::Building { .. }
    ));
    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    fixture.clock.store(100_000, std::sync::atomic::Ordering::SeqCst);
    let clock = Arc::clone(&fixture.clock);
    drop(claim);
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let clock_reader = Arc::clone(&clock);
    let restarted = ProjectionManifestStore::open_existing_and_repair_with_clock_for_test(
        Arc::clone(&vault),
        &snapshot,
        Arc::new(move || clock_reader.load(std::sync::atomic::Ordering::SeqCst)),
    )
    .unwrap();
    restarted.reconcile(&snapshot).unwrap();
    assert!(matches!(
        restarted.current_view().unwrap().entries[0].state,
        ProjectionState::Queued {
            reason: QueueReason::LeaseExpired
        }
    ));
    drop(restarted);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let replayed = reopen(vault, &snapshot);
    assert!(matches!(
        replayed.current_view().unwrap().entries[0].state,
        ProjectionState::Queued {
            reason: QueueReason::LeaseExpired
        }
    ));
}

fn reopen(vault: Arc<PersonalVaultStorage>, snapshot: &CanonicalProjectionSnapshot) -> Arc<ProjectionManifestStore> {
    ProjectionManifestStore::open_existing_and_repair(vault, snapshot).unwrap()
}

fn reopen_unrepaired(
    vault: Arc<PersonalVaultStorage>,
    snapshot: &CanonicalProjectionSnapshot,
) -> Arc<ProjectionManifestStore> {
    ProjectionManifestStore::open_existing_unrepaired_for_test(vault, snapshot).unwrap()
}

fn fixture_builder() -> BuilderSpec {
    BuilderSpec {
        schema: BUILDER_SPEC_SCHEMA.to_string(),
        projection_kind: ProjectionKind::MemoryEmbedding,
        builder_id: "plico.memory-embedding".to_string(),
        builder_version: "fixture-v1".to_string(),
        provider_family: "deterministic-stub".to_string(),
        provider_compatibility_id: "stub-output-v1".to_string(),
        model_id: "stub-2d".to_string(),
        raw_dimension: 2,
        dimension: 2,
        input_contract: EmbeddingInputContract::MemoryTextUtf8V1,
        operation_contract: EmbeddingOperationContract::DocumentV1,
        normalization: EmbeddingNormalization::ProviderNative,
        transform_contract_id: "provider-native-document-v1".to_string(),
        artifact_schema: EMBEDDING_ARTIFACT_SCHEMA.to_string(),
    }
}

fn watermark(snapshot: &CanonicalProjectionSnapshot) -> CanonicalWatermark {
    CanonicalWatermark {
        root_hash: snapshot.root_hash.clone(),
        generation: snapshot.root.generation,
        revision_watermark: snapshot.root.revision_watermark,
        policy_watermark: snapshot.root.policy_watermark,
        relation_watermark: snapshot.root.relation_watermark,
    }
}

fn activate_and_queue(fixture: &Fixture) {
    activate_builder_only(fixture);
    let genesis = fixture
        .projection
        .root_chain_for_test()
        .unwrap()
        .first()
        .unwrap()
        .reconciled_source
        .clone();
    let root = fixture.projection.root_hash().unwrap();
    fixture
        .projection
        .commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![
                ManifestEvent::ProjectionTransition {
                    projection_id: projection_id(&fixture.source.revision_id).unwrap(),
                    projection_kind: ProjectionKind::MemoryEmbedding,
                    projection_version: 1,
                    previous_sequence: None,
                    source: fixture.source.clone(),
                    desired_builder_spec_hash: fixture.builder_hash.clone(),
                    state: ProjectionState::Queued {
                        reason: QueueReason::Reconciliation,
                    },
                },
                ManifestEvent::ReconciliationAdvanced {
                    previous_source: genesis,
                    reconciled_source: watermark(&fixture.snapshot),
                    classified_revision_count: 1,
                },
            ],
            Vec::new(),
            &fixture.snapshot,
        )
        .unwrap();
}

fn activate_builder_only(fixture: &Fixture) {
    let root = fixture.projection.root_hash().unwrap();
    fixture
        .projection
        .commit_expected(
            &root,
            ProjectionCommitActor::PersonalOwner,
            vec![ManifestEvent::BuilderActivated {
                projection_kind: ProjectionKind::MemoryEmbedding,
                builder_spec: fixture.builder.clone(),
                builder_spec_hash: fixture.builder_hash.clone(),
                previous_builder_spec_hash: None,
            }],
            Vec::new(),
            &fixture.snapshot,
        )
        .unwrap();
}

fn claim_build(fixture: &Fixture, attempt_id: Uuid) {
    claim_build_attempt(fixture, attempt_id, 1);
}

fn claim_build_attempt(fixture: &Fixture, attempt_id: Uuid, attempt: u32) {
    let view = fixture.projection.current_view().unwrap();
    let entry = &view.entries[0];
    let root = fixture.projection.root_hash().unwrap();
    fixture
        .projection
        .commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![ManifestEvent::ProjectionTransition {
                projection_id: entry.projection_id,
                projection_kind: entry.projection_kind,
                projection_version: entry.projection_version + 1,
                previous_sequence: Some(entry.last_transition_sequence),
                source: entry.source.clone(),
                desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                state: ProjectionState::Building {
                    attempt,
                    attempt_id,
                    lease_expires_at: crate::util::now_ms() + 10_000,
                },
            }],
            Vec::new(),
            &fixture.snapshot,
        )
        .unwrap();
}

fn ready_event(fixture: &Fixture, attempt_id: Uuid) -> (ManifestEvent, EmbeddingArtifact) {
    ready_event_for_attempt(fixture, attempt_id, 1)
}

fn ready_event_for_attempt(fixture: &Fixture, attempt_id: Uuid, attempt: u32) -> (ManifestEvent, EmbeddingArtifact) {
    let view = fixture.projection.current_view().unwrap();
    let entry = &view.entries[0];
    let artifact = EmbeddingArtifact {
        schema: EMBEDDING_ARTIFACT_SCHEMA.to_string(),
        projection_id: entry.projection_id,
        source_revision_id: entry.source.revision_id.clone(),
        source_content_hash: entry.source.content_hash.clone(),
        builder_spec_hash: entry.desired_builder_spec_hash.clone(),
        dimension: 2,
        encoding: "f32-json/v1".to_string(),
        vector: vec![0.25, 0.75],
    };
    let (bytes, artifact_hash) = artifact_bytes_and_hash(&artifact).unwrap();
    let event = ManifestEvent::ProjectionTransition {
        projection_id: artifact.projection_id,
        projection_kind: ProjectionKind::MemoryEmbedding,
        projection_version: entry.projection_version + 1,
        previous_sequence: Some(entry.last_transition_sequence),
        source: entry.source.clone(),
        desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
        state: ProjectionState::Ready {
            attempt,
            attempt_id,
            artifact: ArtifactDescriptor {
                artifact_hash,
                byte_length: bytes.len() as u64,
                artifact_schema: EMBEDDING_ARTIFACT_SCHEMA.to_string(),
                dimension: 2,
                source_revision_id: entry.source.revision_id.clone(),
                source_content_hash: entry.source.content_hash.clone(),
                builder_spec_hash: entry.desired_builder_spec_hash.clone(),
            },
        },
    };
    (event, artifact)
}

fn make_ready(fixture: &Fixture) -> (String, String) {
    activate_and_queue(fixture);
    let attempt_id = Uuid::new_v4();
    claim_build(fixture, attempt_id);
    let (event, artifact) = ready_event(fixture, attempt_id);
    let artifact_hash = artifact_bytes_and_hash(&artifact).unwrap().1;
    let root = fixture.projection.root_hash().unwrap();
    let ready_root = fixture
        .projection
        .commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![event],
            vec![artifact],
            &fixture.snapshot,
        )
        .unwrap();
    (ready_root, artifact_hash)
}

fn changed_builder(fixture: &Fixture, model_id: &str, raw_dimension: u32, dimension: u32) -> (BuilderSpec, String) {
    let mut builder = fixture.builder.clone();
    builder.model_id = model_id.to_string();
    builder.raw_dimension = raw_dimension;
    builder.dimension = dimension;
    if raw_dimension == dimension {
        builder.normalization = EmbeddingNormalization::ProviderNative;
        builder.transform_contract_id = "provider-native-document-v1".to_string();
    } else {
        builder.normalization = EmbeddingNormalization::L2AfterMatryoshkaTruncationV1;
        builder.transform_contract_id = "plico-matryoshka-truncate-l2-v1".to_string();
    }
    let hash = builder_spec_bytes_and_hash(&builder).unwrap().1;
    (builder, hash)
}

fn switch_builder_and_queue(
    fixture: &Fixture,
    builder: BuilderSpec,
    builder_hash: String,
) -> Result<String, ProjectionError> {
    let view = fixture.projection.current_view().unwrap();
    let entry = &view.entries[0];
    let root = fixture.projection.root_hash().unwrap();
    fixture.projection.commit_expected(
        &root,
        ProjectionCommitActor::PersonalOwner,
        vec![
            ManifestEvent::BuilderActivated {
                projection_kind: ProjectionKind::MemoryEmbedding,
                builder_spec: builder,
                builder_spec_hash: builder_hash.clone(),
                previous_builder_spec_hash: Some(entry.desired_builder_spec_hash.clone()),
            },
            ManifestEvent::ProjectionTransition {
                projection_id: entry.projection_id,
                projection_kind: entry.projection_kind,
                projection_version: entry.projection_version + 1,
                previous_sequence: Some(entry.last_transition_sequence),
                source: entry.source.clone(),
                desired_builder_spec_hash: builder_hash,
                state: ProjectionState::Queued {
                    reason: QueueReason::BuilderChanged,
                },
            },
        ],
        Vec::new(),
        &fixture.snapshot,
    )
}

fn commit_root(canonical: &CASCanonicalLedger, content: MemoryContent, tier: MemoryTier) -> MemoryEntry {
    commit_root_for_role(canonical, crate::PERSONAL_OWNER_ROLE_ID, content, tier)
}

fn commit_root_for_role(
    canonical: &CASCanonicalLedger,
    role: &str,
    content: MemoryContent,
    tier: MemoryTier,
) -> MemoryEntry {
    let mut entry = MemoryEntry::long_term(role, content, Vec::new());
    entry.tier = tier;
    let revision = CanonicalRevision::from_entry(&entry).unwrap();
    canonical
        .commit_expected(role, tier, ExpectedHead::Absent, revision)
        .unwrap();
    entry
}

fn commit_child(
    canonical: &CASCanonicalLedger,
    parent: &MemoryEntry,
    content: MemoryContent,
    deleted: bool,
) -> MemoryEntry {
    let mut entry = parent.clone();
    entry.id = Uuid::new_v4().to_string();
    entry.parent_revision_id = Some(parent.id.as_str().into());
    entry.content = content;
    entry.canonical_content_hash = entry.content.canonical_content_hash().unwrap();
    entry.deleted_at = deleted.then_some(1);
    let revision = CanonicalRevision::from_entry(&entry).unwrap();
    canonical
        .commit_expected(
            crate::PERSONAL_OWNER_ROLE_ID,
            entry.tier,
            ExpectedHead::Revision(parent.id.as_str().into()),
            revision,
        )
        .unwrap();
    entry
}

struct CoreFixture {
    directory: TempDir,
    vault: Arc<PersonalVaultStorage>,
    canonical: Arc<CASCanonicalLedger>,
    core: Arc<ProjectionCoordinatorCore>,
    snapshot: CanonicalProjectionSnapshot,
    entries: Vec<MemoryEntry>,
    builder_hash: String,
}

fn core_fixture(specifications: &[(MemoryContent, MemoryTier)]) -> CoreFixture {
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(PersonalVaultStorage::open(&directory.path().join("vault"), None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let entries = specifications
        .iter()
        .map(|(content, tier)| commit_root(&canonical, content.clone(), *tier))
        .collect();
    let snapshot = canonical.projection_snapshot().unwrap();
    let builder = fixture_builder();
    let builder_hash = builder_spec_bytes_and_hash(&builder).unwrap().1;
    let core = Arc::new(
        canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::bootstrap_authorized(
                    Arc::clone(&vault),
                    builder,
                    ProjectionRebuildSelector::AllEligible,
                    &proof,
                )
                .map(|(core, _)| core)
            })
            .unwrap()
            .unwrap()
            .unwrap(),
    );
    CoreFixture {
        directory,
        vault,
        canonical,
        core,
        snapshot,
        entries,
        builder_hash,
    }
}

fn complete_one_core_projection(fixture: &CoreFixture) -> crate::memory::MemoryRevisionId {
    let claim = fixture.core.claim_next(&fixture.snapshot).unwrap().unwrap();
    let revision_id = claim.source().revision_id.clone();
    fixture
        .canonical
        .with_current_projection_source(claim.source(), |guard| {
            fixture.core.complete_ready(&claim, vec![0.25, 0.75], &guard)
        })
        .unwrap()
        .unwrap()
        .unwrap();
    revision_id
}

fn require_rebuild_receipt(
    result: Result<ProjectionDurableReceipt, ProjectionRebuildError>,
) -> ProjectionDurableReceipt {
    match result {
        Ok(receipt) => receipt,
        Err(_) => panic!("owner rebuild unexpectedly failed"),
    }
}

fn require_open_core(result: Result<ProjectionCoordinatorCore, ProjectionCoreOpenError>) -> ProjectionCoordinatorCore {
    match result {
        Ok(core) => core,
        Err(_) => panic!("projection core unexpectedly failed to open"),
    }
}

#[test]
fn projection_genesis_binds_canonical_genesis() {
    let fixture = fixture();
    let view = fixture.projection.current_view().unwrap();
    assert_eq!(
        view.reconciled_source,
        CanonicalWatermark {
            root_hash: fixture.snapshot.genesis_root_hash.clone(),
            generation: fixture.snapshot.genesis_root.generation,
            revision_watermark: 0,
            policy_watermark: 0,
            relation_watermark: 0,
        }
    );
    assert!(view.entries.is_empty());
}

#[test]
fn reconciliation_classifies_canonical_truth_and_survives_restart() {
    let directory = tempfile::tempdir().unwrap();
    let vault_path = directory.path().join("vault");
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = CASCanonicalLedger::new(Arc::clone(&vault)).unwrap();

    let updated_parent = commit_root(
        &canonical,
        MemoryContent::Text("old version".to_string()),
        MemoryTier::Working,
    );
    commit_child(
        &canonical,
        &updated_parent,
        MemoryContent::Text("current version".to_string()),
        false,
    );
    let deleted_parent = commit_root(
        &canonical,
        MemoryContent::Text("deleted stream".to_string()),
        MemoryTier::LongTerm,
    );
    commit_child(&canonical, &deleted_parent, deleted_parent.content.clone(), true);
    commit_root(
        &canonical,
        MemoryContent::Text("procedure tier".to_string()),
        MemoryTier::Procedural,
    );
    commit_root(
        &canonical,
        MemoryContent::Structured(serde_json::json!({"kind": "non-text"})),
        MemoryTier::LongTerm,
    );
    commit_root(&canonical, MemoryContent::Text("  \n".to_string()), MemoryTier::Working);
    commit_root(
        &canonical,
        MemoryContent::Text("active text".to_string()),
        MemoryTier::LongTerm,
    );
    let snapshot = canonical.projection_snapshot().unwrap();
    assert_eq!(snapshot.revisions.len(), 8);

    let builder = fixture_builder();
    let builder_hash = builder_spec_bytes_and_hash(&builder).unwrap().1;
    let projection = ProjectionManifestStore::bootstrap_new(Arc::clone(&vault), &snapshot).unwrap();
    let genesis = projection.current_view().unwrap().reconciled_source;
    let root = projection.root_hash().unwrap();
    projection
        .commit_expected(
            &root,
            ProjectionCommitActor::PersonalOwner,
            vec![ManifestEvent::BuilderActivated {
                projection_kind: ProjectionKind::MemoryEmbedding,
                builder_spec: builder,
                builder_spec_hash: builder_hash.clone(),
                previous_builder_spec_hash: None,
            }],
            Vec::new(),
            &snapshot,
        )
        .unwrap();
    let child_revision_ids: std::collections::HashSet<_> = snapshot
        .revisions
        .iter()
        .filter_map(|revision| revision.parent_revision_id.clone())
        .collect();
    let mut expected_reasons = std::collections::HashMap::new();
    let mut events = Vec::new();
    for revision in &snapshot.revisions {
        let reason = if child_revision_ids.contains(&revision.revision_id) {
            Some(super::model::AbsentReason::Superseded)
        } else if revision.deleted_at.is_some() {
            Some(super::model::AbsentReason::Deleted)
        } else if !matches!(revision.cognitive_tier, MemoryTier::Working | MemoryTier::LongTerm) {
            Some(super::model::AbsentReason::UnsupportedTier)
        } else {
            match &revision.content {
                MemoryContent::Text(text) if text.trim().is_empty() => Some(super::model::AbsentReason::BlankText),
                MemoryContent::Text(_) => None,
                _ => Some(super::model::AbsentReason::UnsupportedContent),
            }
        };
        expected_reasons.insert(revision.revision_id.clone(), reason);
        let source = CanonicalSourceIdentity {
            canonical_kind: "memory_revision".to_string(),
            memory_id: revision.memory_id.clone(),
            revision_id: revision.revision_id.clone(),
            revision_sequence: revision.sequence,
            content_hash: revision.content_hash.clone(),
        };
        events.push(ManifestEvent::ProjectionTransition {
            projection_id: projection_id(&revision.revision_id).unwrap(),
            projection_kind: ProjectionKind::MemoryEmbedding,
            projection_version: 1,
            previous_sequence: None,
            source,
            desired_builder_spec_hash: builder_hash.clone(),
            state: reason.map_or(
                ProjectionState::Queued {
                    reason: QueueReason::Reconciliation,
                },
                |reason| ProjectionState::AbsentByPolicy { reason },
            ),
        });
    }
    events.push(ManifestEvent::ReconciliationAdvanced {
        previous_source: genesis,
        reconciled_source: watermark(&snapshot),
        classified_revision_count: snapshot.root.revision_watermark,
    });
    let root = projection.root_hash().unwrap();
    projection
        .commit_expected(&root, ProjectionCommitActor::Worker, events, Vec::new(), &snapshot)
        .unwrap();
    drop(projection);
    drop(canonical);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let restarted = reopen(vault, &snapshot);
    let view = restarted.current_view().unwrap();
    for entry in &view.entries {
        match (expected_reasons[&entry.source.revision_id], &entry.state) {
            (Some(expected), ProjectionState::AbsentByPolicy { reason }) => assert_eq!(expected, *reason),
            (None, ProjectionState::Queued { .. }) => {}
            other => panic!("unexpected canonical classification: {other:?}"),
        }
    }
    for index in 0..view.entries.len() {
        let mut tampered = view.clone();
        tampered.entries[index].state = match tampered.entries[index].state {
            ProjectionState::AbsentByPolicy { .. } => ProjectionState::Queued {
                reason: QueueReason::Reconciliation,
            },
            _ => ProjectionState::AbsentByPolicy {
                reason: super::model::AbsentReason::Superseded,
            },
        };
        assert!(matches!(
            validate_candidate(&tampered, &[], &snapshot),
            Err(ProjectionError::Invalid { .. })
        ));
    }
}

#[test]
fn non_genesis_requires_an_active_builder_and_reconciliation_cannot_be_noop() {
    let fixture = fixture();
    let genesis = fixture.projection.current_view().unwrap().reconciled_source;
    let without_builder = ManifestRecord {
        schema: MANIFEST_RECORD_SCHEMA.to_string(),
        sequence: 1,
        committed_at: 1,
        committed_by_role: "projection-worker".to_string(),
        event: ManifestEvent::ReconciliationAdvanced {
            previous_source: genesis.clone(),
            reconciled_source: watermark(&fixture.snapshot),
            classified_revision_count: 1,
        },
    };
    assert!(matches!(
        rebuild_current_view(1, &genesis, &[without_builder]),
        Err(ProjectionError::Invalid {
            category: "missing_active_memory_embedding_builder"
        })
    ));

    let no_op = ManifestRecord {
        schema: MANIFEST_RECORD_SCHEMA.to_string(),
        sequence: 1,
        committed_at: 1,
        committed_by_role: "projection-worker".to_string(),
        event: ManifestEvent::ReconciliationAdvanced {
            previous_source: genesis.clone(),
            reconciled_source: genesis.clone(),
            classified_revision_count: genesis.revision_watermark,
        },
    };
    assert!(matches!(
        rebuild_current_view(1, &genesis, &[no_op]),
        Err(ProjectionError::Invalid {
            category: "invalid_reconciliation_advance"
        })
    ));
}

#[test]
fn ready_cannot_bypass_building() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let attempt_id = Uuid::new_v4();
    let (event, artifact) = ready_event(&fixture, attempt_id);
    let root = fixture.projection.root_hash().unwrap();
    let error = fixture
        .projection
        .commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![event],
            vec![artifact],
            &fixture.snapshot,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        ProjectionError::Invalid {
            category: "illegal_projection_transition"
        }
    ));
    assert_eq!(fixture.projection.root_hash().unwrap(), root);
}

#[test]
fn artifact_failure_leaves_orphan_without_manifest_visibility() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let attempt_id = Uuid::new_v4();
    claim_build(&fixture, attempt_id);
    let (event, artifact) = ready_event(&fixture, attempt_id);
    let root = fixture.projection.root_hash().unwrap();
    fixture.projection.inject_post_artifact_durability_failure_once();
    assert!(matches!(
        fixture.projection.commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![event],
            vec![artifact],
            &fixture.snapshot,
        ),
        Err(ProjectionError::ArtifactStoreUnavailable)
    ));
    assert_eq!(fixture.projection.root_hash().unwrap(), root);
    assert_eq!(fixture.projection.artifact_hashes().unwrap().len(), 1);
    assert!(matches!(
        fixture.projection.current_view().unwrap().entries[0].state,
        ProjectionState::Building { .. }
    ));
}

#[test]
fn manifest_objects_before_pointer_are_not_visible() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let attempt_id = Uuid::new_v4();
    claim_build(&fixture, attempt_id);
    let root = fixture.projection.root_hash().unwrap();
    let view = fixture.projection.current_view().unwrap();
    let entry = &view.entries[0];
    fixture.projection.inject_pre_pointer_failure_once();
    assert!(matches!(
        fixture.projection.commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![ManifestEvent::ProjectionTransition {
                projection_id: entry.projection_id,
                projection_kind: ProjectionKind::MemoryEmbedding,
                projection_version: entry.projection_version + 1,
                previous_sequence: Some(entry.last_transition_sequence),
                source: fixture.source.clone(),
                desired_builder_spec_hash: fixture.builder_hash.clone(),
                state: ProjectionState::Failed {
                    attempt: 1,
                    attempt_id,
                    failure_category: FailureCategory::ProviderUnavailable,
                    retryable: true,
                    retry_not_before: Some(100),
                },
            }],
            Vec::new(),
            &fixture.snapshot,
        ),
        Err(ProjectionError::Io(_))
    ));
    assert_eq!(fixture.projection.root_hash().unwrap(), root);
    assert!(matches!(
        fixture.projection.current_view().unwrap().entries[0].state,
        ProjectionState::Building { .. }
    ));
}

#[test]
fn exchange_sync_failure_is_indeterminate_and_restart_replays_new_root() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let attempt_id = Uuid::new_v4();
    claim_build(&fixture, attempt_id);
    let old_root = fixture.projection.root_hash().unwrap();
    let view = fixture.projection.current_view().unwrap();
    let entry = &view.entries[0];
    fixture.projection.inject_post_exchange_sync_failure_once();
    assert!(matches!(
        fixture.projection.commit_expected(
            &old_root,
            ProjectionCommitActor::Worker,
            vec![ManifestEvent::ProjectionTransition {
                projection_id: entry.projection_id,
                projection_kind: ProjectionKind::MemoryEmbedding,
                projection_version: entry.projection_version + 1,
                previous_sequence: Some(entry.last_transition_sequence),
                source: fixture.source.clone(),
                desired_builder_spec_hash: fixture.builder_hash.clone(),
                state: ProjectionState::Failed {
                    attempt: 1,
                    attempt_id,
                    failure_category: FailureCategory::ProviderUnavailable,
                    retryable: false,
                    retry_not_before: None,
                },
            }],
            Vec::new(),
            &fixture.snapshot,
        ),
        Err(ProjectionError::CommitIndeterminate)
    ));
    assert!(matches!(
        fixture.projection.root_hash(),
        Err(ProjectionError::WriterPoisoned)
    ));

    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let restarted = reopen(vault, &snapshot);
    assert_ne!(restarted.root_hash().unwrap(), old_root);
    assert!(matches!(
        restarted.current_view().unwrap().entries[0].state,
        ProjectionState::Failed { .. }
    ));
}

#[test]
fn ready_artifact_and_full_history_survive_restart() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let attempt_id = Uuid::new_v4();
    claim_build(&fixture, attempt_id);
    let (event, artifact) = ready_event(&fixture, attempt_id);
    let root = fixture.projection.root_hash().unwrap();
    let ready_root = fixture
        .projection
        .commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![event],
            vec![artifact],
            &fixture.snapshot,
        )
        .unwrap();
    assert!(matches!(
        fixture.projection.current_view().unwrap().entries[0].state,
        ProjectionState::Ready { .. }
    ));

    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let restarted = reopen(vault, &snapshot);
    assert_eq!(restarted.root_hash().unwrap(), ready_root);
    assert!(matches!(
        restarted.current_view().unwrap().entries[0].state,
        ProjectionState::Ready { .. }
    ));
}

#[test]
fn owner_rebuild_stale_and_queue_are_one_atomic_generation() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let attempt_id = Uuid::new_v4();
    claim_build(&fixture, attempt_id);
    let (event, artifact) = ready_event(&fixture, attempt_id);
    let root = fixture.projection.root_hash().unwrap();
    fixture
        .projection
        .commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![event],
            vec![artifact],
            &fixture.snapshot,
        )
        .unwrap();
    let view = fixture.projection.current_view().unwrap();
    let entry = &view.entries[0];
    let ProjectionState::Ready {
        artifact: descriptor, ..
    } = &entry.state
    else {
        panic!("fixture must be ready");
    };
    let root = fixture.projection.root_hash().unwrap();
    fixture
        .projection
        .commit_expected(
            &root,
            ProjectionCommitActor::PersonalOwner,
            vec![
                ManifestEvent::ProjectionTransition {
                    projection_id: entry.projection_id,
                    projection_kind: entry.projection_kind,
                    projection_version: entry.projection_version + 1,
                    previous_sequence: Some(entry.last_transition_sequence),
                    source: entry.source.clone(),
                    desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                    state: ProjectionState::Stale {
                        reason: StaleReason::OwnerRebuild,
                        artifact: descriptor.clone(),
                    },
                },
                ManifestEvent::ProjectionTransition {
                    projection_id: entry.projection_id,
                    projection_kind: entry.projection_kind,
                    projection_version: entry.projection_version + 2,
                    previous_sequence: Some(view.event_watermark + 1),
                    source: entry.source.clone(),
                    desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                    state: ProjectionState::Queued {
                        reason: QueueReason::OwnerRebuild,
                    },
                },
            ],
            Vec::new(),
            &fixture.snapshot,
        )
        .unwrap();
    assert!(matches!(
        fixture.projection.current_view().unwrap().entries[0].state,
        ProjectionState::Queued {
            reason: QueueReason::OwnerRebuild
        }
    ));
}

#[test]
fn builder_change_requeues_queued_building_and_nonretryable_failed_entries() {
    let queued = fixture();
    activate_and_queue(&queued);
    let (builder, hash) = changed_builder(&queued, "stub-2d-v2", 2, 2);
    switch_builder_and_queue(&queued, builder, hash.clone()).unwrap();
    let queued_view = queued.projection.current_view().unwrap();
    assert_eq!(queued_view.entries[0].desired_builder_spec_hash, hash);
    assert!(matches!(
        queued_view.entries[0].state,
        ProjectionState::Queued {
            reason: QueueReason::BuilderChanged
        }
    ));

    let building = fixture();
    activate_and_queue(&building);
    claim_build(&building, Uuid::new_v4());
    let (builder, hash) = changed_builder(&building, "stub-2d", 4, 2);
    switch_builder_and_queue(&building, builder, hash.clone()).unwrap();
    let building_view = building.projection.current_view().unwrap();
    assert_eq!(building_view.entries[0].desired_builder_spec_hash, hash);
    assert!(matches!(
        building_view.entries[0].state,
        ProjectionState::Queued {
            reason: QueueReason::BuilderChanged
        }
    ));

    let failed = fixture();
    activate_and_queue(&failed);
    let attempt_id = Uuid::new_v4();
    claim_build(&failed, attempt_id);
    let view = failed.projection.current_view().unwrap();
    let entry = &view.entries[0];
    let root = failed.projection.root_hash().unwrap();
    failed
        .projection
        .commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![ManifestEvent::ProjectionTransition {
                projection_id: entry.projection_id,
                projection_kind: entry.projection_kind,
                projection_version: entry.projection_version + 1,
                previous_sequence: Some(entry.last_transition_sequence),
                source: entry.source.clone(),
                desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                state: ProjectionState::Failed {
                    attempt: 1,
                    attempt_id,
                    failure_category: FailureCategory::InvalidProjection,
                    retryable: false,
                    retry_not_before: None,
                },
            }],
            Vec::new(),
            &failed.snapshot,
        )
        .unwrap();
    let (builder, hash) = changed_builder(&failed, "stub-2d-v3", 2, 2);
    switch_builder_and_queue(&failed, builder, hash.clone()).unwrap();
    let failed_view = failed.projection.current_view().unwrap();
    assert_eq!(failed_view.entries[0].desired_builder_spec_hash, hash);
    assert!(matches!(
        failed_view.entries[0].state,
        ProjectionState::Queued {
            reason: QueueReason::BuilderChanged
        }
    ));
}

#[test]
fn retryable_failure_obeys_backoff_but_owner_can_change_builder() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let attempt_id = Uuid::new_v4();
    claim_build(&fixture, attempt_id);
    let view = fixture.projection.current_view().unwrap();
    let entry = &view.entries[0];
    let root = fixture.projection.root_hash().unwrap();
    fixture
        .projection
        .commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![ManifestEvent::ProjectionTransition {
                projection_id: entry.projection_id,
                projection_kind: entry.projection_kind,
                projection_version: entry.projection_version + 1,
                previous_sequence: Some(entry.last_transition_sequence),
                source: entry.source.clone(),
                desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                state: ProjectionState::Failed {
                    attempt: 1,
                    attempt_id,
                    failure_category: FailureCategory::ProviderUnavailable,
                    retryable: true,
                    retry_not_before: Some(100),
                },
            }],
            Vec::new(),
            &fixture.snapshot,
        )
        .unwrap();

    let view = fixture.projection.current_view().unwrap();
    let entry = &view.entries[0];
    let failed_root = fixture.projection.root_hash().unwrap();
    let retry_error = fixture
        .projection
        .commit_expected(
            &failed_root,
            ProjectionCommitActor::Worker,
            vec![ManifestEvent::ProjectionTransition {
                projection_id: entry.projection_id,
                projection_kind: entry.projection_kind,
                projection_version: entry.projection_version + 1,
                previous_sequence: Some(entry.last_transition_sequence),
                source: entry.source.clone(),
                desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                state: ProjectionState::Queued {
                    reason: QueueReason::Retry,
                },
            }],
            Vec::new(),
            &fixture.snapshot,
        )
        .unwrap_err();
    assert!(matches!(
        retry_error,
        ProjectionError::Invalid {
            category: "invalid_projection_retry_transition"
        }
    ));
    assert_eq!(fixture.projection.root_hash().unwrap(), failed_root);

    let (builder, hash) = changed_builder(&fixture, "stub-2d-v2", 2, 2);
    let changed_root = switch_builder_and_queue(&fixture, builder, hash).unwrap();
    assert_ne!(changed_root, failed_root);
    assert!(matches!(
        fixture.projection.current_view().unwrap().entries[0].state,
        ProjectionState::Queued {
            reason: QueueReason::BuilderChanged
        }
    ));
}

#[test]
fn projection_namespace_has_one_writer_per_vault_lifecycle() {
    let fixture = fixture();
    let second = ProjectionManifestStore::bootstrap_new(Arc::clone(&fixture.vault), &fixture.snapshot);
    assert!(matches!(
        second,
        Err(ProjectionError::Invalid {
            category: "projection_namespace_already_claimed"
        })
    ));
}

#[test]
fn empty_generation_is_rejected_after_restart() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    fixture.projection.inject_empty_generation_root().unwrap();
    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    assert!(matches!(
        ProjectionManifestStore::inspect_existing_read_only(&vault, &snapshot),
        Ok(ProjectionStoreInspection::ResetRequired(
            crate::cas::ProjectionPairResetReason::ManifestIntegrityInvalid
        ))
    ));
}

#[test]
fn later_valid_root_cannot_hide_intermediate_source_tamper() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    fixture.projection.inject_intermediate_source_tamper().unwrap();
    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    assert!(matches!(
        ProjectionManifestStore::inspect_existing_read_only(&vault, &snapshot),
        Ok(ProjectionStoreInspection::ResetRequired(
            crate::cas::ProjectionPairResetReason::ManifestIntegrityInvalid
        ))
    ));
}

#[test]
fn hash_domains_are_distinct_and_unsafe_builder_identity_is_rejected() {
    let value = serde_json::json!({"same": "bytes"});
    let hashes = [
        builder_spec_bytes_and_hash(&value).unwrap().1,
        segment_bytes_and_hash(&value).unwrap().1,
        root_bytes_and_hash(&value).unwrap().1,
        view_bytes_and_hash(&value).unwrap().1,
        artifact_bytes_and_hash(&value).unwrap().1,
    ];
    assert_eq!(
        hashes.iter().collect::<std::collections::HashSet<_>>().len(),
        hashes.len()
    );

    let fixture = fixture();
    let mut unsafe_builder = fixture.builder.clone();
    unsafe_builder.provider_family = "https://provider.invalid".to_string();
    let unsafe_hash = builder_spec_bytes_and_hash(&unsafe_builder).unwrap().1;
    let root = fixture.projection.root_hash().unwrap();
    assert!(matches!(
        fixture.projection.commit_expected(
            &root,
            ProjectionCommitActor::PersonalOwner,
            vec![ManifestEvent::BuilderActivated {
                projection_kind: ProjectionKind::MemoryEmbedding,
                builder_spec: unsafe_builder,
                builder_spec_hash: unsafe_hash,
                previous_builder_spec_hash: None,
            }],
            Vec::new(),
            &fixture.snapshot,
        ),
        Err(ProjectionError::Invalid {
            category: "invalid_builder_spec"
        })
    ));
}

#[test]
fn matryoshka_artifact_requires_unit_norm_and_distinct_raw_dimension() {
    let fixture = fixture();
    let (builder, builder_hash) = changed_builder(&fixture, "stub-matryoshka", 4, 2);
    validate_builder_spec(&builder).unwrap();
    let projection_id = projection_id(&fixture.source.revision_id).unwrap();
    let valid = EmbeddingArtifact {
        schema: EMBEDDING_ARTIFACT_SCHEMA.to_string(),
        projection_id,
        source_revision_id: fixture.source.revision_id.clone(),
        source_content_hash: fixture.source.content_hash.clone(),
        builder_spec_hash: builder_hash.clone(),
        dimension: 2,
        encoding: "f32-json/v1".to_string(),
        vector: vec![0.6, 0.8],
    };
    let (valid_bytes, valid_hash) = artifact_bytes_and_hash(&valid).unwrap();
    let valid_descriptor = ArtifactDescriptor {
        artifact_hash: valid_hash,
        byte_length: valid_bytes.len() as u64,
        artifact_schema: EMBEDDING_ARTIFACT_SCHEMA.to_string(),
        dimension: 2,
        source_revision_id: fixture.source.revision_id.clone(),
        source_content_hash: fixture.source.content_hash.clone(),
        builder_spec_hash: builder_hash.clone(),
    };
    validate_artifact(&valid, &valid_descriptor, &builder).unwrap();

    let mut wrong_norm = valid;
    wrong_norm.vector = vec![1.0, 1.0];
    let (wrong_bytes, wrong_hash) = artifact_bytes_and_hash(&wrong_norm).unwrap();
    let wrong_descriptor = ArtifactDescriptor {
        artifact_hash: wrong_hash,
        byte_length: wrong_bytes.len() as u64,
        ..valid_descriptor
    };
    assert!(matches!(
        validate_artifact(&wrong_norm, &wrong_descriptor, &builder),
        Err(ProjectionError::Invalid {
            category: "artifact_normalization_mismatch"
        })
    ));

    let mut invalid_builder = builder;
    invalid_builder.raw_dimension = invalid_builder.dimension;
    assert!(matches!(
        validate_builder_spec(&invalid_builder),
        Err(ProjectionError::Invalid {
            category: "invalid_builder_spec"
        })
    ));
}

#[test]
fn valid_envelope_under_wrong_domain_hash_is_staled_and_cleaned() {
    let fixture = fixture();
    let (_, artifact_hash) = make_ready(&fixture);
    let mut replacement = EmbeddingArtifact {
        schema: EMBEDDING_ARTIFACT_SCHEMA.to_string(),
        projection_id: projection_id(&fixture.source.revision_id).unwrap(),
        source_revision_id: fixture.source.revision_id.clone(),
        source_content_hash: fixture.source.content_hash.clone(),
        builder_spec_hash: fixture.builder_hash.clone(),
        dimension: 2,
        encoding: "f32-json/v1".to_string(),
        vector: vec![0.25, 0.75],
    };
    replacement.vector = vec![0.5, 0.5];
    let replacement_bytes = artifact_bytes_and_hash(&replacement).unwrap().0;
    fixture
        .projection
        .inject_corrupt_artifact(&artifact_hash, &replacement_bytes)
        .unwrap();
    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let repairing = reopen_unrepaired(vault, &snapshot);
    repairing.repair_invalid_artifacts(&snapshot).unwrap().unwrap();
    assert!(matches!(
        repairing.current_view().unwrap().entries[0].state,
        ProjectionState::Stale {
            reason: StaleReason::ArtifactHashMismatch,
            ..
        }
    ));
    assert!(repairing.artifact_hashes().unwrap().is_empty());
}

#[test]
fn missing_ready_artifact_is_durably_repaired_to_stale() {
    let fixture = fixture();
    let (ready_root, artifact_hash) = make_ready(&fixture);
    fixture.projection.inject_missing_artifact(&artifact_hash).unwrap();

    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let repairing = reopen_unrepaired(Arc::clone(&vault), &snapshot);
    assert!(matches!(
        repairing.current_view(),
        Err(ProjectionError::ArtifactRepairRequired { count: 1 })
    ));
    assert!(matches!(
        repairing.root_hash(),
        Err(ProjectionError::ArtifactRepairRequired { count: 1 })
    ));
    assert!(matches!(
        repairing.commit_expected(
            &ready_root,
            ProjectionCommitActor::Worker,
            vec![ManifestEvent::ReconciliationAdvanced {
                previous_source: watermark(&snapshot),
                reconciled_source: watermark(&snapshot),
                classified_revision_count: 1,
            }],
            Vec::new(),
            &snapshot,
        ),
        Err(ProjectionError::ArtifactRepairRequired { count: 1 })
    ));
    repairing.repair_invalid_artifacts(&snapshot).unwrap().unwrap();
    assert!(matches!(
        repairing.current_view().unwrap().entries[0].state,
        ProjectionState::Stale {
            reason: StaleReason::ArtifactMissing,
            ..
        }
    ));
    drop(repairing);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let restarted = reopen(vault, &snapshot);
    assert!(matches!(
        restarted.current_view().unwrap().entries[0].state,
        ProjectionState::Stale {
            reason: StaleReason::ArtifactMissing,
            ..
        }
    ));
}

#[test]
fn artifact_repair_pre_pointer_failure_keeps_repair_required() {
    let fixture = fixture();
    let (_, artifact_hash) = make_ready(&fixture);
    fixture.projection.inject_missing_artifact(&artifact_hash).unwrap();
    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let repairing = reopen_unrepaired(vault, &snapshot);
    repairing.inject_pre_pointer_failure_once();
    assert!(matches!(
        repairing.repair_invalid_artifacts(&snapshot),
        Err(ProjectionError::Io(_))
    ));
    assert!(matches!(
        repairing.current_view(),
        Err(ProjectionError::ArtifactRepairRequired { count: 1 })
    ));
    repairing.repair_invalid_artifacts(&snapshot).unwrap().unwrap();
    assert!(matches!(
        repairing.current_view().unwrap().entries[0].state,
        ProjectionState::Stale {
            reason: StaleReason::ArtifactMissing,
            ..
        }
    ));
}

#[test]
fn artifact_repair_exchange_failure_is_indeterminate_and_restart_recovers() {
    let fixture = fixture();
    let (_, artifact_hash) = make_ready(&fixture);
    fixture.projection.inject_missing_artifact(&artifact_hash).unwrap();
    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let repairing = reopen_unrepaired(Arc::clone(&vault), &snapshot);
    repairing.inject_post_exchange_sync_failure_once();
    assert!(matches!(
        repairing.repair_invalid_artifacts(&snapshot),
        Err(ProjectionError::CommitIndeterminate)
    ));
    assert!(matches!(repairing.current_view(), Err(ProjectionError::WriterPoisoned)));
    drop(repairing);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let restarted = reopen(vault, &snapshot);
    assert!(matches!(
        restarted.current_view().unwrap().entries[0].state,
        ProjectionState::Stale {
            reason: StaleReason::ArtifactMissing,
            ..
        }
    ));
}

#[test]
fn corrupt_artifact_is_staled_cleaned_and_rebuilt_with_the_same_spec() {
    let fixture = fixture();
    let (_, artifact_hash) = make_ready(&fixture);
    fixture
        .projection
        .inject_corrupt_artifact(&artifact_hash, b"not-json")
        .unwrap();
    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let repairing = reopen_unrepaired(vault, &snapshot);
    repairing.repair_invalid_artifacts(&snapshot).unwrap().unwrap();
    assert!(matches!(
        repairing.current_view().unwrap().entries[0].state,
        ProjectionState::Stale {
            reason: StaleReason::ArtifactInvalid,
            ..
        }
    ));
    assert!(repairing.artifact_hashes().unwrap().is_empty());

    let view = repairing.current_view().unwrap();
    let entry = &view.entries[0];
    let root = repairing.root_hash().unwrap();
    repairing
        .commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![ManifestEvent::ProjectionTransition {
                projection_id: entry.projection_id,
                projection_kind: entry.projection_kind,
                projection_version: entry.projection_version + 1,
                previous_sequence: Some(entry.last_transition_sequence),
                source: entry.source.clone(),
                desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                state: ProjectionState::Queued {
                    reason: QueueReason::Reconciliation,
                },
            }],
            Vec::new(),
            &snapshot,
        )
        .unwrap();
    claim_build_attempt_for_store(&repairing, &snapshot, Uuid::new_v4(), 2);
    let view = repairing.current_view().unwrap();
    let entry = &view.entries[0];
    let attempt_id = match entry.state {
        ProjectionState::Building { attempt_id, .. } => attempt_id,
        _ => panic!("rebuild must be claimed"),
    };
    let artifact = EmbeddingArtifact {
        schema: EMBEDDING_ARTIFACT_SCHEMA.to_string(),
        projection_id: entry.projection_id,
        source_revision_id: entry.source.revision_id.clone(),
        source_content_hash: entry.source.content_hash.clone(),
        builder_spec_hash: entry.desired_builder_spec_hash.clone(),
        dimension: 2,
        encoding: "f32-json/v1".to_string(),
        vector: vec![0.25, 0.75],
    };
    let (bytes, rebuilt_hash) = artifact_bytes_and_hash(&artifact).unwrap();
    assert_eq!(rebuilt_hash, artifact_hash);
    let root = repairing.root_hash().unwrap();
    repairing
        .commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![ManifestEvent::ProjectionTransition {
                projection_id: entry.projection_id,
                projection_kind: entry.projection_kind,
                projection_version: entry.projection_version + 1,
                previous_sequence: Some(entry.last_transition_sequence),
                source: entry.source.clone(),
                desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                state: ProjectionState::Ready {
                    attempt: 2,
                    attempt_id,
                    artifact: ArtifactDescriptor {
                        artifact_hash: rebuilt_hash,
                        byte_length: bytes.len() as u64,
                        artifact_schema: EMBEDDING_ARTIFACT_SCHEMA.to_string(),
                        dimension: 2,
                        source_revision_id: entry.source.revision_id.clone(),
                        source_content_hash: entry.source.content_hash.clone(),
                        builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                    },
                },
            }],
            vec![artifact],
            &snapshot,
        )
        .unwrap();
    assert!(matches!(
        repairing.current_view().unwrap().entries[0].state,
        ProjectionState::Ready { attempt: 2, .. }
    ));
}

fn claim_build_attempt_for_store(
    store: &ProjectionManifestStore,
    snapshot: &CanonicalProjectionSnapshot,
    attempt_id: Uuid,
    attempt: u32,
) {
    let view = store.current_view().unwrap();
    let entry = &view.entries[0];
    let root = store.root_hash().unwrap();
    store
        .commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![ManifestEvent::ProjectionTransition {
                projection_id: entry.projection_id,
                projection_kind: entry.projection_kind,
                projection_version: entry.projection_version + 1,
                previous_sequence: Some(entry.last_transition_sequence),
                source: entry.source.clone(),
                desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                state: ProjectionState::Building {
                    attempt,
                    attempt_id,
                    lease_expires_at: crate::util::now_ms() + 10_000,
                },
            }],
            Vec::new(),
            snapshot,
        )
        .unwrap();
}

fn complete_ready_attempt_for_store(
    store: &ProjectionManifestStore,
    snapshot: &CanonicalProjectionSnapshot,
    attempt: u32,
) -> String {
    let view = store.current_view().unwrap();
    let entry = &view.entries[0];
    let attempt_id = match entry.state {
        ProjectionState::Building {
            attempt: current_attempt,
            attempt_id,
            ..
        } if current_attempt == attempt => attempt_id,
        _ => panic!("ready attempt must follow its building claim"),
    };
    let artifact = EmbeddingArtifact {
        schema: EMBEDDING_ARTIFACT_SCHEMA.to_string(),
        projection_id: entry.projection_id,
        source_revision_id: entry.source.revision_id.clone(),
        source_content_hash: entry.source.content_hash.clone(),
        builder_spec_hash: entry.desired_builder_spec_hash.clone(),
        dimension: 2,
        encoding: "f32-json/v1".to_string(),
        vector: vec![0.25, 0.75],
    };
    let (bytes, artifact_hash) = artifact_bytes_and_hash(&artifact).unwrap();
    let root = store.root_hash().unwrap();
    store
        .commit_expected(
            &root,
            ProjectionCommitActor::Worker,
            vec![ManifestEvent::ProjectionTransition {
                projection_id: entry.projection_id,
                projection_kind: entry.projection_kind,
                projection_version: entry.projection_version + 1,
                previous_sequence: Some(entry.last_transition_sequence),
                source: entry.source.clone(),
                desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                state: ProjectionState::Ready {
                    attempt,
                    attempt_id,
                    artifact: ArtifactDescriptor {
                        artifact_hash: artifact_hash.clone(),
                        byte_length: bytes.len() as u64,
                        artifact_schema: EMBEDDING_ARTIFACT_SCHEMA.to_string(),
                        dimension: 2,
                        source_revision_id: entry.source.revision_id.clone(),
                        source_content_hash: entry.source.content_hash.clone(),
                        builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                    },
                },
            }],
            vec![artifact],
            snapshot,
        )
        .unwrap();
    artifact_hash
}

#[test]
fn cleanup_failure_keeps_durable_stale_and_can_be_retried() {
    let fixture = fixture();
    let (_, artifact_hash) = make_ready(&fixture);
    fixture
        .projection
        .inject_corrupt_artifact(&artifact_hash, b"not-json")
        .unwrap();
    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let repairing = reopen_unrepaired(vault, &snapshot);
    repairing.inject_artifact_cleanup_failure_once();
    assert!(matches!(
        repairing.repair_invalid_artifacts(&snapshot),
        Err(ProjectionError::ArtifactMaintenanceRequired)
    ));
    assert!(matches!(
        repairing.current_view().unwrap().entries[0].state,
        ProjectionState::Stale {
            reason: StaleReason::ArtifactInvalid,
            ..
        }
    ));
    drop(repairing);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let restarted = reopen(vault, &snapshot);
    assert!(restarted.artifact_hashes().unwrap().is_empty());
}

#[test]
fn cleanup_and_rebuild_share_one_writer_critical_section() {
    let fixture = fixture();
    let (_, artifact_hash) = make_ready(&fixture);
    fixture
        .projection
        .inject_corrupt_artifact(&artifact_hash, b"not-json")
        .unwrap();
    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let store = reopen_unrepaired(vault, &snapshot);
    store.inject_artifact_cleanup_failure_once();
    assert!(matches!(
        store.repair_invalid_artifacts(&snapshot),
        Err(ProjectionError::ArtifactMaintenanceRequired)
    ));
    let view = store.current_view().unwrap();
    let entry = &view.entries[0];
    let expected_root = store.root_hash().unwrap();
    let queue_event = ManifestEvent::ProjectionTransition {
        projection_id: entry.projection_id,
        projection_kind: entry.projection_kind,
        projection_version: entry.projection_version + 1,
        previous_sequence: Some(entry.last_transition_sequence),
        source: entry.source.clone(),
        desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
        state: ProjectionState::Queued {
            reason: QueueReason::Reconciliation,
        },
    };

    let snapshot_ready = Arc::new(std::sync::Barrier::new(2));
    let continue_cleanup = Arc::new(std::sync::Barrier::new(2));
    store.inject_cleanup_barriers(Arc::clone(&snapshot_ready), Arc::clone(&continue_cleanup));
    let cleanup_store = Arc::clone(&store);
    let cleanup_snapshot = snapshot.clone();
    let cleanup = std::thread::spawn(move || cleanup_store.repair_invalid_artifacts(&cleanup_snapshot));
    snapshot_ready.wait();

    let commit_store = Arc::clone(&store);
    let commit_snapshot = snapshot.clone();
    let (sent, received) = std::sync::mpsc::channel();
    let commit = std::thread::spawn(move || {
        let result = commit_store.commit_expected(
            &expected_root,
            ProjectionCommitActor::Worker,
            vec![queue_event],
            Vec::new(),
            &commit_snapshot,
        );
        sent.send(result).unwrap();
    });
    assert!(matches!(
        received.recv_timeout(std::time::Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    continue_cleanup.wait();
    assert_eq!(cleanup.join().unwrap().unwrap(), None);
    received
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap()
        .unwrap();
    commit.join().unwrap();

    claim_build_attempt_for_store(&store, &snapshot, Uuid::new_v4(), 2);
    let rebuilt_hash = complete_ready_attempt_for_store(&store, &snapshot, 2);
    assert_eq!(rebuilt_hash, artifact_hash);
    assert_eq!(store.artifact_hashes().unwrap(), vec![artifact_hash]);
}

#[cfg(unix)]
#[test]
fn permissive_symlink_and_fifo_artifacts_are_staled_then_safely_removed() {
    for fixture_kind in 0..3 {
        let fixture = fixture();
        let (_, artifact_hash) = make_ready(&fixture);
        match fixture_kind {
            0 => fixture
                .projection
                .inject_permissive_artifact_mode(&artifact_hash)
                .unwrap(),
            1 => fixture.projection.inject_artifact_symlink(&artifact_hash).unwrap(),
            2 => fixture.projection.inject_artifact_fifo(&artifact_hash).unwrap(),
            _ => unreachable!(),
        }
        let vault_path = fixture.directory.path().join("vault");
        let snapshot = fixture.snapshot.clone();
        drop(fixture.projection);
        drop(fixture.canonical);
        drop(fixture.vault);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let repairing = reopen_unrepaired(vault, &snapshot);
        repairing.repair_invalid_artifacts(&snapshot).unwrap().unwrap();
        assert!(matches!(
            repairing.current_view().unwrap().entries[0].state,
            ProjectionState::Stale {
                reason: StaleReason::ArtifactInvalid,
                ..
            }
        ));
        assert!(repairing.artifact_hashes().unwrap().is_empty());
    }
}

#[test]
fn typed_status_requires_scoped_current_policy_proof_and_reports_unreconciled() {
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(PersonalVaultStorage::open(&directory.path().join("vault"), None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let entry = commit_root_for_role(
        &canonical,
        "role-a",
        MemoryContent::Text("authorized status".to_string()),
        MemoryTier::Working,
    );
    let snapshot = canonical.projection_snapshot().unwrap();
    let builder = fixture_builder();
    let core = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::bootstrap_authorized(
                Arc::clone(&vault),
                builder,
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
            .map(|(core, _)| core)
        })
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(canonical
        .with_authorized_current_revision("role-b", &entry.id.as_str().into(), |proof| core.status(&proof))
        .unwrap()
        .is_none());
    let observed = canonical
        .with_authorized_current_revision("role-a", &entry.id.as_str().into(), |proof| core.status(&proof))
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        observed,
        ProjectionStatusObservation::Observed {
            state: ProjectionStatusState::Queued {
                reason: QueueReason::Reconciliation
            },
            ..
        }
    ));

    let later = commit_root_for_role(
        &canonical,
        "role-a",
        MemoryContent::Text("not reconciled yet".to_string()),
        MemoryTier::LongTerm,
    );
    let unreconciled = canonical
        .with_authorized_current_revision("role-a", &later.id.as_str().into(), |proof| core.status(&proof))
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(unreconciled, ProjectionStatusObservation::Unreconciled { .. }));

    let still_observed = canonical
        .with_authorized_current_revision("role-a", &entry.id.as_str().into(), |proof| core.status(&proof))
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(still_observed, ProjectionStatusObservation::Observed { .. }));

    let deleted = commit_child(&canonical, &entry, entry.content.clone(), true);
    assert!(canonical
        .with_authorized_current_revision("role-a", &entry.id.as_str().into(), |proof| core.status(&proof))
        .unwrap()
        .is_none());

    let mut false_ancestor = watermark(&snapshot);
    false_ancestor.root_hash = "00".repeat(32);
    core.inject_status_reconciled_source_for_test(false_ancestor);
    let invalid = canonical
        .with_authorized_current_revision("role-a", &later.id.as_str().into(), |proof| core.status(&proof))
        .unwrap()
        .unwrap();
    assert!(matches!(
        invalid,
        Err(ProjectionError::Invalid {
            category: "projection_status_source_not_canonical_ancestor"
        })
    ));
    assert!(canonical
        .with_authorized_current_revision("role-a", &deleted.id.as_str().into(), |proof| core.status(&proof))
        .unwrap()
        .is_none());
}

#[test]
fn owner_rebuild_all_is_one_generation_skips_absent_and_survives_restart() {
    let fixture = core_fixture(&[
        (MemoryContent::Text("ready target".to_string()), MemoryTier::Working),
        (MemoryContent::Text("queued target".to_string()), MemoryTier::LongTerm),
        (
            MemoryContent::Text("procedural is absent".to_string()),
            MemoryTier::Procedural,
        ),
    ]);
    complete_one_core_projection(&fixture);
    let before = fixture.core.current_view_for_test().unwrap();
    let absent_before = before
        .entries
        .iter()
        .find(|entry| entry.source.revision_id == fixture.entries[2].id)
        .unwrap()
        .clone();

    assert!(fixture
        .canonical
        .with_owner_projection_maintenance("role-a", |proof| fixture
            .core
            .owner_rebuild_authorized(ProjectionRebuildSelector::AllEligible, &proof))
        .unwrap()
        .is_none());
    assert_eq!(fixture.core.current_view_for_test().unwrap(), before);

    let receipt = require_rebuild_receipt(
        fixture
            .canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                fixture
                    .core
                    .owner_rebuild_authorized(ProjectionRebuildSelector::AllEligible, &proof)
            })
            .unwrap()
            .unwrap(),
    );
    assert_eq!(receipt.selected_count, 2);
    assert_eq!(receipt.manifest_generation, before.generation + 1);
    assert_eq!(receipt.event_watermark, before.event_watermark + 3);
    assert_eq!(receipt.reconciled_source, before.reconciled_source);
    let after = fixture.core.current_view_for_test().unwrap();
    for entry in &after.entries {
        if entry.source.revision_id == absent_before.source.revision_id {
            assert_eq!(entry, &absent_before);
        } else {
            assert!(matches!(
                entry.state,
                ProjectionState::Queued {
                    reason: QueueReason::OwnerRebuild
                }
            ));
        }
    }

    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    let builder_hash = fixture.builder_hash.clone();
    drop(fixture.core);
    drop(fixture.canonical);
    drop(fixture.vault);
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let restarted = require_open_core(ProjectionCoordinatorCore::open_existing(
        vault,
        &snapshot,
        &builder_hash,
    ));
    assert_eq!(restarted.current_view_for_test().unwrap(), after);
}

#[test]
fn owner_rebuild_selector_errors_are_typed_and_zero_write() {
    let fixture = core_fixture(&[(MemoryContent::Text("old head".to_string()), MemoryTier::Working)]);
    let old = fixture.entries[0].clone();
    let current = commit_child(
        &fixture.canonical,
        &old,
        MemoryContent::Text("new head".to_string()),
        false,
    );
    let before = fixture.core.current_view_for_test().unwrap();
    let old_result = fixture
        .canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            fixture.core.owner_rebuild_authorized(
                ProjectionRebuildSelector::CurrentRevision(old.id.as_str().into()),
                &proof,
            )
        })
        .unwrap()
        .unwrap();
    assert!(matches!(old_result, Err(ProjectionRebuildError::NotFound)));
    assert_eq!(fixture.core.current_view_for_test().unwrap(), before);

    let deleted = commit_child(&fixture.canonical, &current, current.content.clone(), true);
    let deleted_result = fixture
        .canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            fixture.core.owner_rebuild_authorized(
                ProjectionRebuildSelector::CurrentRevision(deleted.id.as_str().into()),
                &proof,
            )
        })
        .unwrap()
        .unwrap();
    assert!(matches!(deleted_result, Err(ProjectionRebuildError::NotFound)));
    assert_eq!(fixture.core.current_view_for_test().unwrap(), before);

    let ineligible = core_fixture(&[(MemoryContent::Text("procedure".to_string()), MemoryTier::Procedural)]);
    let ineligible_before = ineligible.core.current_view_for_test().unwrap();
    let single = ineligible
        .canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ineligible.core.owner_rebuild_authorized(
                ProjectionRebuildSelector::CurrentRevision(ineligible.entries[0].id.as_str().into()),
                &proof,
            )
        })
        .unwrap()
        .unwrap();
    assert!(matches!(single, Err(ProjectionRebuildError::NotEligible)));
    let all = ineligible
        .canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ineligible
                .core
                .owner_rebuild_authorized(ProjectionRebuildSelector::AllEligible, &proof)
        })
        .unwrap()
        .unwrap();
    assert!(matches!(all, Err(ProjectionRebuildError::NothingToRebuild)));
    assert_eq!(ineligible.core.current_view_for_test().unwrap(), ineligible_before);
}

#[test]
fn owner_rebuild_pointer_failures_preserve_truth_across_restart() {
    for indeterminate in [false, true] {
        let fixture = core_fixture(&[(MemoryContent::Text("fault target".to_string()), MemoryTier::Working)]);
        let before = fixture.core.current_view_for_test().unwrap();
        if indeterminate {
            fixture.core.inject_post_exchange_sync_failure_once();
        } else {
            fixture.core.inject_pre_pointer_failure_once();
        }
        let result = fixture
            .canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                fixture
                    .core
                    .owner_rebuild_authorized(ProjectionRebuildSelector::AllEligible, &proof)
            })
            .unwrap()
            .unwrap();
        if indeterminate {
            assert!(matches!(
                result,
                Err(ProjectionRebuildError::Projection(ProjectionError::CommitIndeterminate))
            ));
        } else {
            assert!(matches!(
                result,
                Err(ProjectionRebuildError::Projection(ProjectionError::Io(_)))
            ));
            assert_eq!(fixture.core.current_view_for_test().unwrap(), before);
        }

        let vault_path = fixture.directory.path().join("vault");
        let snapshot = fixture.snapshot.clone();
        let builder_hash = fixture.builder_hash.clone();
        drop(fixture.core);
        drop(fixture.canonical);
        drop(fixture.vault);
        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let restarted = require_open_core(ProjectionCoordinatorCore::open_existing(
            vault,
            &snapshot,
            &builder_hash,
        ));
        let replayed = restarted.current_view_for_test().unwrap();
        if indeterminate {
            assert!(matches!(
                replayed.entries[0].state,
                ProjectionState::Queued {
                    reason: QueueReason::OwnerRebuild
                }
            ));
        } else {
            assert_eq!(replayed, before);
        }
    }
}

#[test]
fn owner_rebuild_holds_canonical_guard_until_manifest_publish() {
    let fixture = core_fixture(&[(MemoryContent::Text("guarded rebuild".to_string()), MemoryTier::Working)]);
    let parent = fixture.entries[0].clone();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
    fixture.canonical.inject_projection_guard_hook(entered_tx, release_rx);

    let maintenance_core = Arc::clone(&fixture.core);
    let maintenance_canonical = Arc::clone(&fixture.canonical);
    let (maintenance_tx, maintenance_rx) = std::sync::mpsc::sync_channel(1);
    let maintenance = std::thread::spawn(move || {
        let result = maintenance_canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                maintenance_core.owner_rebuild_authorized(ProjectionRebuildSelector::AllEligible, &proof)
            })
            .unwrap()
            .unwrap();
        maintenance_tx.send(result).unwrap();
    });
    entered_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();

    let update_canonical = Arc::clone(&fixture.canonical);
    let update_parent = parent.clone();
    let (update_started_tx, update_started_rx) = std::sync::mpsc::sync_channel(0);
    let (update_tx, update_rx) = std::sync::mpsc::sync_channel(1);
    let update = std::thread::spawn(move || {
        update_started_tx.send(()).unwrap();
        let child = commit_child(
            &update_canonical,
            &update_parent,
            MemoryContent::Text("update after rebuild".to_string()),
            false,
        );
        update_tx.send(child).unwrap();
    });
    update_started_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    assert!(matches!(
        update_rx.recv_timeout(std::time::Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    release_tx.send(()).unwrap();
    require_rebuild_receipt(maintenance_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap());
    let child = update_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
    maintenance.join().unwrap();
    update.join().unwrap();

    let snapshot = fixture.canonical.projection_snapshot().unwrap();
    fixture.core.reconcile(&snapshot).unwrap();
    let view = fixture.core.current_view_for_test().unwrap();
    assert!(matches!(
        view.entries
            .iter()
            .find(|entry| entry.source.revision_id == parent.id)
            .unwrap()
            .state,
        ProjectionState::AbsentByPolicy {
            reason: super::model::AbsentReason::Superseded
        }
    ));
    assert!(matches!(
        view.entries
            .iter()
            .find(|entry| entry.source.revision_id == child.id)
            .unwrap()
            .state,
        ProjectionState::Queued {
            reason: QueueReason::Reconciliation
        }
    ));
}

#[cfg(unix)]
#[test]
fn genesis_resume_cleans_manifest_and_all_artifact_orphans_without_exchange() {
    let directory = tempfile::tempdir().unwrap();
    let vault_path = directory.path().join("vault");
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    commit_root(
        &canonical,
        MemoryContent::Text("genesis orphan source".to_string()),
        MemoryTier::Working,
    );
    canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::bootstrap_genesis_only_for_test(Arc::clone(&vault), &proof)
        })
        .unwrap()
        .unwrap()
        .unwrap();
    let original_objects: Vec<_> = vault
        .projection_tree_fingerprint_for_test()
        .unwrap()
        .into_iter()
        .filter(|entry| entry.relative_path.starts_with("projection-store/manifest/objects/"))
        .collect();
    assert_eq!(original_objects.len(), 2);
    drop(canonical);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault
        .inject_projection_manifest_orphan_for_test(&"aa".repeat(32), b"manifest-orphan")
        .unwrap();
    vault
        .inject_projection_artifact_orphan_for_test(&"bb".repeat(32), b"artifact-orphan")
        .unwrap();
    let external_canary = vault
        .inject_projection_artifact_symlink_orphan_for_test(&"cc".repeat(32))
        .unwrap();
    vault
        .inject_projection_artifact_fifo_orphan_for_test(&"dd".repeat(32))
        .unwrap();
    vault
        .inject_projection_artifact_permissive_orphan_for_test(&"12".repeat(32), b"permissive-orphan")
        .unwrap();
    vault
        .inject_projection_artifact_orphan_for_test(
            &"34".repeat(32),
            &vec![7; super::model::MAX_EMBEDDING_ARTIFACT_BYTES as usize + 1],
        )
        .unwrap();
    let core = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::resume_genesis_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
            .map(|(core, _)| core)
        })
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 0);
    vault
        .assert_projection_external_canary_unchanged_for_test(&external_canary)
        .unwrap();
    let after = vault.projection_tree_fingerprint_for_test().unwrap();
    assert!(original_objects.iter().all(|entry| after.contains(entry)));
    assert!(after.iter().all(|entry| {
        !entry.relative_path.ends_with(&"aa".repeat(32))
            && !entry.relative_path.ends_with(&"bb".repeat(32))
            && !entry.relative_path.ends_with(&"cc".repeat(32))
            && !entry.relative_path.ends_with(&"dd".repeat(32))
            && !entry.relative_path.ends_with(&"12".repeat(32))
            && !entry.relative_path.ends_with(&"34".repeat(32))
    }));
    assert!(!core.current_view_for_test().unwrap().active_builder_specs.is_empty());
}

#[cfg(unix)]
#[test]
fn genesis_cleanup_sync_failure_keeps_pointer_and_restart_retries_empty_inventory() {
    for fail_manifest_sync in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let vault_path = directory.path().join("vault");
        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        commit_root(
            &canonical,
            MemoryContent::Text("genesis cleanup restart".to_string()),
            MemoryTier::LongTerm,
        );
        canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::bootstrap_genesis_only_for_test(Arc::clone(&vault), &proof)
            })
            .unwrap()
            .unwrap()
            .unwrap();
        drop(canonical);
        drop(vault);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        vault
            .inject_projection_manifest_orphan_for_test(&"ee".repeat(32), b"manifest-orphan")
            .unwrap();
        vault
            .inject_projection_artifact_orphan_for_test(&"ff".repeat(32), b"artifact-orphan")
            .unwrap();
        let pointer_before: Vec<_> = vault
            .projection_tree_fingerprint_for_test()
            .unwrap()
            .into_iter()
            .filter(|entry| entry.relative_path.starts_with("projection-store/manifest/roots/"))
            .collect();
        if fail_manifest_sync {
            vault.inject_projection_manifest_cleanup_sync_failure_once();
        } else {
            vault.inject_projection_artifact_cleanup_sync_failure_once();
        }
        let failed = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::resume_genesis_authorized(
                    Arc::clone(&vault),
                    fixture_builder(),
                    ProjectionRebuildSelector::AllEligible,
                    &proof,
                )
                .map(|(core, _)| core)
            })
            .unwrap()
            .unwrap();
        assert!(matches!(failed, Err(ProjectionError::ProjectionMaintenanceRequired)));
        let pointer_after: Vec<_> = vault
            .projection_tree_fingerprint_for_test()
            .unwrap()
            .into_iter()
            .filter(|entry| entry.relative_path.starts_with("projection-store/manifest/roots/"))
            .collect();
        assert_eq!(pointer_after, pointer_before);
        drop(canonical);
        drop(vault);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        let core = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::resume_genesis_authorized(
                    Arc::clone(&vault),
                    fixture_builder(),
                    ProjectionRebuildSelector::AllEligible,
                    &proof,
                )
                .map(|(core, _)| core)
            })
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!core.current_view_for_test().unwrap().active_builder_specs.is_empty());
        assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 0);
    }
}

#[test]
fn real_builder_activation_pre_pointer_orphans_are_cleaned_before_owner_resume() {
    let directory = tempfile::tempdir().unwrap();
    let vault_path = directory.path().join("vault");
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    commit_root(
        &canonical,
        MemoryContent::Text("real activation cutpoint".to_string()),
        MemoryTier::Working,
    );
    canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::bootstrap_genesis_only_for_test(Arc::clone(&vault), &proof)
        })
        .unwrap()
        .unwrap()
        .unwrap();
    drop(canonical);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let objects_before = vault
        .projection_tree_fingerprint_for_test()
        .unwrap()
        .into_iter()
        .filter(|entry| entry.relative_path.starts_with("projection-store/manifest/objects/"))
        .count();
    let (genesis_hash, activation_result) = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            let store = ProjectionManifestStore::open_existing_genesis_only(Arc::clone(&vault), proof.snapshot())?;
            let genesis_hash = store.root_hash()?;
            store.inject_pre_pointer_failure_once();
            let activation = store.activate_builder(fixture_builder(), proof.snapshot());
            Ok::<_, ProjectionError>((genesis_hash, activation))
        })
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(activation_result, Err(ProjectionError::Io(_))));
    let objects_after_fault = vault
        .projection_tree_fingerprint_for_test()
        .unwrap()
        .into_iter()
        .filter(|entry| entry.relative_path.starts_with("projection-store/manifest/objects/"))
        .count();
    assert!(objects_after_fault > objects_before);
    drop(canonical);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    let core = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::resume_genesis_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
            .map(|(core, _)| core)
        })
        .unwrap()
        .unwrap()
        .unwrap();
    let roots = core.root_chain_for_test().unwrap();
    assert!(roots.len() >= 2);
    assert_eq!(roots[1].previous_root_hash.as_deref(), Some(genesis_hash.as_str()));
    assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 0);
}

#[cfg(target_os = "linux")]
#[test]
fn owner_reset_replaces_only_manifest_incomplete_pair_and_returns_live_core() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let current_view_hash = fixture
        .projection
        .root_chain_for_test()
        .unwrap()
        .last()
        .unwrap()
        .current_view_hash
        .clone();
    let canonical_root_before = fixture.canonical.projection_snapshot().unwrap().root_hash;
    let vault_path = fixture.directory.path().join("vault");
    let builder_hash = fixture.builder_hash.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault
        .remove_projection_manifest_object_for_test(&current_view_hash)
        .unwrap();
    assert!(matches!(
        ProjectionCoordinatorCore::inspect_existing(&vault, &canonical.projection_snapshot().unwrap(), None).unwrap(),
        super::coordinator_core::ProjectionCoreInspection::ResetRequired(
            crate::cas::ProjectionPairResetReason::ManifestIncomplete
        )
    ));

    let (core, receipt) = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::reset_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
        })
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(receipt.selected_count, 1);
    assert!(receipt.manifest_generation >= 2);
    assert!(receipt.event_watermark >= 2);
    assert_eq!(
        receipt.reconciled_source,
        watermark(&canonical.projection_snapshot().unwrap())
    );
    assert!(matches!(
        core.current_view_for_test().unwrap().entries[0].state,
        ProjectionState::Queued {
            reason: QueueReason::Reconciliation
        }
    ));
    assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 0);
    assert_eq!(
        canonical.projection_snapshot().unwrap().root_hash,
        canonical_root_before
    );
    drop(core);
    drop(canonical);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = CASCanonicalLedger::new(Arc::clone(&vault)).unwrap();
    let restarted =
        match ProjectionCoordinatorCore::open_existing(vault, &canonical.projection_snapshot().unwrap(), &builder_hash)
        {
            Ok(core) => core,
            Err(_) => panic!("reset projection must reopen with the sealed builder"),
        };
    assert!(matches!(
        restarted.current_view_for_test().unwrap().entries[0].state,
        ProjectionState::Queued { .. }
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn owner_reset_recovers_prepared_applied_and_partial_cleanup_cutpoints() {
    type FaultInjector = fn(&PersonalVaultStorage);
    let cases: [(&str, FaultInjector, bool); 7] = [
        (
            "prepared_marker_before_pair_exchange",
            PersonalVaultStorage::inject_projection_reset_after_prepared_marker_persist_once,
            false,
        ),
        (
            "pair_exchanged_prepared",
            PersonalVaultStorage::inject_projection_reset_after_pair_exchange_once,
            false,
        ),
        (
            "transition_persisted_prepared",
            PersonalVaultStorage::inject_projection_reset_after_transition_persist_once,
            false,
        ),
        (
            "marker_exchanged_applied",
            PersonalVaultStorage::inject_projection_reset_after_marker_exchange_once,
            true,
        ),
        (
            "quarantine_pair_removed",
            PersonalVaultStorage::inject_projection_reset_after_quarantine_pair_removal_once,
            true,
        ),
        (
            "seal_removed",
            PersonalVaultStorage::inject_projection_reset_after_seal_removal_once,
            true,
        ),
        (
            "container_removed",
            PersonalVaultStorage::inject_projection_reset_after_container_removal_once,
            true,
        ),
    ];

    for (case, inject, expects_maintenance) in cases {
        let fixture = fixture();
        activate_and_queue(&fixture);
        let current_view_hash = fixture
            .projection
            .root_chain_for_test()
            .unwrap()
            .last()
            .unwrap()
            .current_view_hash
            .clone();
        let canonical_root_before = fixture.canonical.projection_snapshot().unwrap().root_hash;
        let vault_path = fixture.directory.path().join("vault");
        let builder_hash = fixture.builder_hash.clone();
        drop(fixture.projection);
        drop(fixture.canonical);
        drop(fixture.vault);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        vault
            .remove_projection_manifest_object_for_test(&current_view_hash)
            .unwrap();
        let canonical_tree_before = vault.canonical_tree_fingerprint_for_test().unwrap();
        let canonical_atimes_before = vault.canonical_tree_atimes_for_test(&canonical_tree_before).unwrap();
        inject(&vault);
        let reset = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::reset_authorized(
                    Arc::clone(&vault),
                    fixture_builder(),
                    ProjectionRebuildSelector::AllEligible,
                    &proof,
                )
            })
            .unwrap()
            .unwrap();
        assert!(
            matches!(reset, Err(ProjectionError::CommitIndeterminate)),
            "reset cutpoint {case} did not report an indeterminate commit"
        );
        let same_lifecycle_retry = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::recover_reset_authorized(Arc::clone(&vault), &proof)
            })
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                same_lifecycle_retry,
                Err(ProjectionError::Invalid {
                    category: "projection_namespace_already_claimed"
                })
            ),
            "reset cutpoint {case} released poisoned claims"
        );
        assert_eq!(
            canonical.projection_snapshot().unwrap().root_hash,
            canonical_root_before,
            "reset cutpoint {case} changed canonical storage"
        );
        assert_eq!(
            vault.canonical_tree_atimes_for_test(&canonical_tree_before).unwrap(),
            canonical_atimes_before,
            "reset cutpoint {case} changed canonical atime metadata"
        );
        assert_eq!(
            vault.canonical_tree_fingerprint_for_test().unwrap(),
            canonical_tree_before,
            "reset cutpoint {case} changed canonical tree bytes or metadata"
        );
        drop(canonical);
        drop(vault);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        let inspection =
            ProjectionCoordinatorCore::inspect_existing(&vault, &canonical.projection_snapshot().unwrap(), None)
                .unwrap();
        assert!(
            matches!(
                (expects_maintenance, inspection),
                (false, super::coordinator_core::ProjectionCoreInspection::ResetPending)
                    | (
                        true,
                        super::coordinator_core::ProjectionCoreInspection::MaintenanceRequired
                    )
            ),
            "reset cutpoint {case} exposed the wrong durable recovery phase"
        );
        let recovered = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::recover_reset_authorized(Arc::clone(&vault), &proof)?
                    .resume_authorized(fixture_builder(), ProjectionRebuildSelector::AllEligible, &proof)
                    .map(|(core, _)| core)
            })
            .unwrap()
            .unwrap();
        let core = match recovered {
            Ok(core) => core,
            Err(_) => panic!("reset cutpoint {case} did not recover after restart"),
        };
        assert!(matches!(
            core.current_view_for_test().unwrap().entries[0].state,
            ProjectionState::Queued { .. }
        ));
        assert_eq!(
            canonical.projection_snapshot().unwrap().root_hash,
            canonical_root_before,
            "reset cutpoint {case} changed canonical storage during recovery"
        );
        assert_eq!(
            vault.canonical_tree_atimes_for_test(&canonical_tree_before).unwrap(),
            canonical_atimes_before,
            "reset cutpoint {case} changed canonical atime metadata during recovery"
        );
        assert_eq!(
            vault.canonical_tree_fingerprint_for_test().unwrap(),
            canonical_tree_before,
            "reset cutpoint {case} changed canonical tree bytes or metadata during recovery"
        );
        assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 0);
        drop(core);
        drop(canonical);
        drop(vault);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = CASCanonicalLedger::new(Arc::clone(&vault)).unwrap();
        let restarted =
            ProjectionCoordinatorCore::open_existing(vault, &canonical.projection_snapshot().unwrap(), &builder_hash);
        assert!(restarted.is_ok(), "reset cutpoint {case} did not remain restartable");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn reset_marker_unlink_without_parent_sync_recovers_as_genesis_only() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let current_view_hash = fixture
        .projection
        .root_chain_for_test()
        .unwrap()
        .last()
        .unwrap()
        .current_view_hash
        .clone();
    let vault_path = fixture.directory.path().join("vault");
    let builder_hash = fixture.builder_hash.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault
        .remove_projection_manifest_object_for_test(&current_view_hash)
        .unwrap();
    let canonical_before = vault.canonical_tree_fingerprint_for_test().unwrap();
    let canonical_atimes_before = vault.canonical_tree_atimes_for_test(&canonical_before).unwrap();
    vault.inject_projection_reset_after_active_marker_unlink_once();
    let reset = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::reset_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
        })
        .unwrap()
        .unwrap();
    assert!(matches!(reset, Err(ProjectionError::CommitIndeterminate)));
    let same_lifecycle_retry = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::resume_genesis_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
            .map(|(core, _)| core)
        })
        .unwrap()
        .unwrap();
    match same_lifecycle_retry {
        Err(ProjectionError::Invalid { category }) => {
            assert_eq!(category, "projection_namespace_already_claimed")
        }
        Err(_) => panic!("same-lifecycle reset retry returned the wrong stable error"),
        Ok(_) => panic!("same-lifecycle reset retry reused poisoned projection claims"),
    }
    drop(canonical);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    assert!(matches!(
        ProjectionCoordinatorCore::inspect_existing(&vault, &canonical.projection_snapshot().unwrap(), None,),
        Ok(super::coordinator_core::ProjectionCoreInspection::GenesisOnly)
    ));
    let core = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::resume_genesis_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
            .map(|(core, _)| core)
        })
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        core.current_view_for_test().unwrap().entries[0].state,
        ProjectionState::Queued { .. }
    ));
    assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 0);
    assert_eq!(
        vault.canonical_tree_atimes_for_test(&canonical_before).unwrap(),
        canonical_atimes_before
    );
    assert_eq!(vault.canonical_tree_fingerprint_for_test().unwrap(), canonical_before);
    drop(core);
    drop(canonical);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = CASCanonicalLedger::new(Arc::clone(&vault)).unwrap();
    assert!(
        ProjectionCoordinatorCore::open_existing(vault, &canonical.projection_snapshot().unwrap(), &builder_hash,)
            .is_ok()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn owner_reset_safely_cleans_special_sparse_and_hardenable_quarantine_entries() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let current_view_hash = fixture
        .projection
        .root_chain_for_test()
        .unwrap()
        .last()
        .unwrap()
        .current_view_hash
        .clone();
    let vault_path = fixture.directory.path().join("vault");
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault
        .remove_projection_manifest_object_for_test(&current_view_hash)
        .unwrap();
    let canary = vault
        .inject_projection_artifact_symlink_orphan_for_test(&"71".repeat(32))
        .unwrap();
    vault
        .inject_projection_artifact_fifo_orphan_for_test(&"72".repeat(32))
        .unwrap();
    vault
        .inject_projection_artifact_permissive_orphan_for_test(&"74".repeat(32), b"wrong-mode")
        .unwrap();
    vault
        .inject_projection_artifact_sparse_orphan_for_test(&"75".repeat(32), 32 * 1024 * 1024)
        .unwrap();
    vault.set_projection_artifact_objects_mode_for_test(0o755).unwrap();
    let canonical_before = vault.canonical_tree_fingerprint_for_test().unwrap();
    let canonical_atimes_before = vault.canonical_tree_atimes_for_test(&canonical_before).unwrap();
    assert!(matches!(
        ProjectionCoordinatorCore::inspect_existing(&vault, &canonical.projection_snapshot().unwrap(), None,),
        Ok(super::coordinator_core::ProjectionCoreInspection::ResetRequired(
            crate::cas::ProjectionPairResetReason::StorageLayoutInvalid
        ))
    ));
    let (core, _) = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::reset_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
        })
        .unwrap()
        .unwrap()
        .unwrap();
    vault
        .assert_projection_external_canary_unchanged_for_test(&canary)
        .unwrap();
    assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 0);
    assert_eq!(
        vault.canonical_tree_atimes_for_test(&canonical_before).unwrap(),
        canonical_atimes_before
    );
    assert_eq!(vault.canonical_tree_fingerprint_for_test().unwrap(), canonical_before);
    assert!(matches!(
        core.current_view_for_test().unwrap().entries[0].state,
        ProjectionState::Queued { .. }
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn owner_reset_leaves_unopenable_zero_mode_quarantine_for_manual_intervention() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let current_view_hash = fixture
        .projection
        .root_chain_for_test()
        .unwrap()
        .last()
        .unwrap()
        .current_view_hash
        .clone();
    let vault_path = fixture.directory.path().join("vault");
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault
        .remove_projection_manifest_object_for_test(&current_view_hash)
        .unwrap();
    let canonical_before = vault.canonical_tree_fingerprint_for_test().unwrap();
    let canonical_atimes_before = vault.canonical_tree_atimes_for_test(&canonical_before).unwrap();
    vault.inject_projection_reset_after_marker_exchange_once();
    let reset = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::reset_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
        })
        .unwrap()
        .unwrap();
    assert!(matches!(reset, Err(ProjectionError::CommitIndeterminate)));
    drop(canonical);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    assert!(matches!(
        ProjectionCoordinatorCore::inspect_existing(&vault, &canonical.projection_snapshot().unwrap(), None,),
        Ok(super::coordinator_core::ProjectionCoreInspection::MaintenanceRequired)
    ));
    let live_before = vault.projection_tree_fingerprint_for_test().unwrap();
    vault
        .set_projection_quarantine_artifact_objects_mode_for_test(0o000)
        .unwrap();
    let recovery = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::recover_reset_authorized(Arc::clone(&vault), &proof)
        })
        .unwrap()
        .unwrap();
    assert!(matches!(recovery, Err(ProjectionError::ManualInterventionRequired)));
    assert_eq!(vault.projection_tree_fingerprint_for_test().unwrap(), live_before);
    assert_eq!(vault.canonical_tree_fingerprint_for_test().unwrap(), canonical_before);
    assert_eq!(
        vault.canonical_tree_atimes_for_test(&canonical_before).unwrap(),
        canonical_atimes_before
    );
    drop(canonical);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = CASCanonicalLedger::new(Arc::clone(&vault)).unwrap();
    assert!(matches!(
        ProjectionCoordinatorCore::inspect_existing(&vault, &canonical.projection_snapshot().unwrap(), None,),
        Ok(super::coordinator_core::ProjectionCoreInspection::ManualIntervention)
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn owner_reset_rejects_pair_present_without_seal_as_manual_intervention() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let current_view_hash = fixture
        .projection
        .root_chain_for_test()
        .unwrap()
        .last()
        .unwrap()
        .current_view_hash
        .clone();
    let vault_path = fixture.directory.path().join("vault");
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault
        .remove_projection_manifest_object_for_test(&current_view_hash)
        .unwrap();
    let canonical_before = vault.canonical_tree_fingerprint_for_test().unwrap();
    let canonical_atimes_before = vault.canonical_tree_atimes_for_test(&canonical_before).unwrap();
    vault.inject_projection_reset_after_marker_exchange_once();
    let reset = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::reset_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
        })
        .unwrap()
        .unwrap();
    assert!(matches!(reset, Err(ProjectionError::CommitIndeterminate)));
    let live_before = vault.projection_tree_fingerprint_for_test().unwrap();
    vault.remove_projection_reset_quarantine_seal_for_test().unwrap();
    drop(canonical);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    assert!(matches!(
        ProjectionCoordinatorCore::inspect_existing(&vault, &canonical.projection_snapshot().unwrap(), None,),
        Ok(super::coordinator_core::ProjectionCoreInspection::ManualIntervention)
    ));
    let recovery = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::recover_reset_authorized(Arc::clone(&vault), &proof)
        })
        .unwrap()
        .unwrap();
    assert!(matches!(recovery, Err(ProjectionError::ManualInterventionRequired)));
    assert_eq!(vault.projection_tree_fingerprint_for_test().unwrap(), live_before);
    assert_eq!(vault.canonical_tree_fingerprint_for_test().unwrap(), canonical_before);
    assert_eq!(
        vault.canonical_tree_atimes_for_test(&canonical_before).unwrap(),
        canonical_atimes_before
    );
    assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn owner_reset_rejects_self_consistent_cross_field_marker_tamper_without_exchange() {
    let fixture = fixture();
    activate_and_queue(&fixture);
    let current_view_hash = fixture
        .projection
        .root_chain_for_test()
        .unwrap()
        .last()
        .unwrap()
        .current_view_hash
        .clone();
    let vault_path = fixture.directory.path().join("vault");
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault
        .remove_projection_manifest_object_for_test(&current_view_hash)
        .unwrap();
    let canonical_before = vault.canonical_tree_fingerprint_for_test().unwrap();
    let canonical_atimes_before = vault.canonical_tree_atimes_for_test(&canonical_before).unwrap();
    vault.inject_projection_reset_after_marker_exchange_once();
    let reset = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::reset_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
        })
        .unwrap()
        .unwrap();
    assert!(matches!(reset, Err(ProjectionError::CommitIndeterminate)));
    let live_before = vault.projection_tree_fingerprint_for_test().unwrap();
    vault.tamper_projection_reset_marker_cross_field_for_test().unwrap();
    drop(canonical);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    assert!(matches!(
        ProjectionCoordinatorCore::inspect_existing(&vault, &canonical.projection_snapshot().unwrap(), None,),
        Ok(super::coordinator_core::ProjectionCoreInspection::ManualIntervention)
    ));
    let recovery = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::recover_reset_authorized(Arc::clone(&vault), &proof)
        })
        .unwrap()
        .unwrap();
    assert!(matches!(recovery, Err(ProjectionError::ManualInterventionRequired)));
    assert_eq!(vault.projection_tree_fingerprint_for_test().unwrap(), live_before);
    assert_eq!(vault.canonical_tree_fingerprint_for_test().unwrap(), canonical_before);
    assert_eq!(
        vault.canonical_tree_atimes_for_test(&canonical_before).unwrap(),
        canonical_atimes_before
    );
    assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn future_reset_seal_and_transition_schemas_are_unsupported_without_writes() {
    type FaultInjector = fn(&PersonalVaultStorage);
    type ProtocolTamper = fn(&PersonalVaultStorage) -> std::io::Result<()>;
    let cases: [(&str, FaultInjector, ProtocolTamper); 2] = [
        (
            "quarantine_seal",
            PersonalVaultStorage::inject_projection_reset_after_marker_exchange_once,
            PersonalVaultStorage::tamper_projection_reset_seal_future_schema_for_test,
        ),
        (
            "transition_evidence",
            PersonalVaultStorage::inject_projection_reset_after_transition_persist_once,
            PersonalVaultStorage::tamper_projection_reset_transition_future_schema_for_test,
        ),
    ];

    for (case, inject_fault, tamper) in cases {
        let fixture = fixture();
        activate_and_queue(&fixture);
        let current_view_hash = fixture
            .projection
            .root_chain_for_test()
            .unwrap()
            .last()
            .unwrap()
            .current_view_hash
            .clone();
        let vault_path = fixture.directory.path().join("vault");
        drop(fixture.projection);
        drop(fixture.canonical);
        drop(fixture.vault);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        vault
            .remove_projection_manifest_object_for_test(&current_view_hash)
            .unwrap();
        inject_fault(&vault);
        let reset = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::reset_authorized(
                    Arc::clone(&vault),
                    fixture_builder(),
                    ProjectionRebuildSelector::AllEligible,
                    &proof,
                )
            })
            .unwrap()
            .unwrap();
        assert!(matches!(reset, Err(ProjectionError::CommitIndeterminate)));
        tamper(&vault).unwrap();
        drop(canonical);
        drop(vault);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        let projection_before = vault.projection_reset_protocol_fingerprint_for_test().unwrap();
        let projection_atimes_before = vault.projection_tree_atimes_for_test(&projection_before).unwrap();
        let canonical_before = vault.canonical_tree_fingerprint_for_test().unwrap();
        let canonical_atimes_before = vault.canonical_tree_atimes_for_test(&canonical_before).unwrap();
        assert!(matches!(
            ProjectionCoordinatorCore::inspect_existing(&vault, &canonical.projection_snapshot().unwrap(), None),
            Ok(super::coordinator_core::ProjectionCoreInspection::UnsupportedFormat)
        ));
        let recovery = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::recover_reset_authorized(Arc::clone(&vault), &proof)
            })
            .unwrap()
            .unwrap();
        assert!(
            matches!(recovery, Err(ProjectionError::UnsupportedFormat { .. })),
            "future reset protocol case {case} was not rejected as unsupported"
        );
        assert_eq!(
            vault.projection_tree_atimes_for_test(&projection_before).unwrap(),
            projection_atimes_before,
            "future reset protocol case {case} changed projection atimes"
        );
        assert_eq!(
            vault.projection_reset_protocol_fingerprint_for_test().unwrap(),
            projection_before,
            "future reset protocol case {case} changed projection storage"
        );
        assert_eq!(
            vault.canonical_tree_atimes_for_test(&canonical_before).unwrap(),
            canonical_atimes_before,
            "future reset protocol case {case} changed canonical atimes"
        );
        assert_eq!(
            vault.canonical_tree_fingerprint_for_test().unwrap(),
            canonical_before,
            "future reset protocol case {case} changed canonical storage"
        );
        assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 1);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn reset_pre_mutation_storage_errors_are_retryable_without_protocol_writes() {
    type FaultInjector = fn(&PersonalVaultStorage);
    let cases: [(&str, FaultInjector, bool); 2] = [
        (
            "protocol_preflight_permission_denied",
            PersonalVaultStorage::inject_projection_reset_preflight_permission_denied_once,
            false,
        ),
        (
            "quarantine_cleanup_storage_io",
            PersonalVaultStorage::inject_projection_reset_quarantine_pre_mutation_io_once,
            true,
        ),
    ];

    for (case, inject_fault, resolve_transition_first) in cases {
        let fixture = fixture();
        activate_and_queue(&fixture);
        let current_view_hash = fixture
            .projection
            .root_chain_for_test()
            .unwrap()
            .last()
            .unwrap()
            .current_view_hash
            .clone();
        let vault_path = fixture.directory.path().join("vault");
        drop(fixture.projection);
        drop(fixture.canonical);
        drop(fixture.vault);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        vault
            .remove_projection_manifest_object_for_test(&current_view_hash)
            .unwrap();
        vault.inject_projection_reset_after_marker_exchange_once();
        let reset = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::reset_authorized(
                    Arc::clone(&vault),
                    fixture_builder(),
                    ProjectionRebuildSelector::AllEligible,
                    &proof,
                )
            })
            .unwrap()
            .unwrap();
        assert!(matches!(reset, Err(ProjectionError::CommitIndeterminate)));
        drop(canonical);
        drop(vault);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        if resolve_transition_first {
            vault.resolve_projection_reset_transition_for_test().unwrap();
        }
        let protocol_before = vault.projection_reset_protocol_fingerprint_for_test().unwrap();
        let protocol_atimes_before = vault.projection_tree_atimes_for_test(&protocol_before).unwrap();
        let canonical_before = vault.canonical_tree_fingerprint_for_test().unwrap();
        let canonical_atimes_before = vault.canonical_tree_atimes_for_test(&canonical_before).unwrap();
        inject_fault(&vault);
        let first = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::recover_reset_authorized(Arc::clone(&vault), &proof)
            })
            .unwrap()
            .unwrap();
        assert!(
            matches!(first, Err(ProjectionError::Io(_))),
            "reset fault {case} was not unavailable"
        );
        assert_eq!(
            vault.projection_tree_atimes_for_test(&protocol_before).unwrap(),
            protocol_atimes_before,
            "reset fault {case} changed protocol atimes"
        );
        assert_eq!(
            vault.projection_reset_protocol_fingerprint_for_test().unwrap(),
            protocol_before,
            "reset fault {case} changed reset protocol storage"
        );
        assert_eq!(
            vault.canonical_tree_atimes_for_test(&canonical_before).unwrap(),
            canonical_atimes_before,
            "reset fault {case} changed canonical atimes"
        );
        assert_eq!(vault.canonical_tree_fingerprint_for_test().unwrap(), canonical_before);

        let retry = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::recover_reset_authorized(Arc::clone(&vault), &proof)
            })
            .unwrap()
            .unwrap();
        assert!(retry.is_ok(), "reset fault {case} did not release claims for retry");
        assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 0);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn projection_reset_trace_contract_child() {
    if std::env::var_os("PLICO_PROJECTION_RESET_TRACE_CHILD").is_none() {
        return;
    }

    let fixture = fixture();
    commit_root(
        &fixture.canonical,
        MemoryContent::Text("PRIVATE_RESET_CONTENT_QUERY_TAG_CANARY".to_string()),
        MemoryTier::Working,
    );
    activate_and_queue(&fixture);
    let current_view_hash = fixture
        .projection
        .root_chain_for_test()
        .unwrap()
        .last()
        .unwrap()
        .current_view_hash
        .clone();
    let vault_path = fixture.directory.path().join("vault");
    let canonical_snapshot = fixture.canonical.projection_snapshot().unwrap();
    let private_content_hash = canonical_snapshot.revisions.last().unwrap().content_hash.clone();
    let forbidden_values = vec![
        "PRIVATE_RESET_CONTENT_QUERY_TAG_CANARY".to_string(),
        vault_path.display().to_string(),
        ".plico-projection-pair-staging.".to_string(),
        ".plico-projection-reset-marker-transition.".to_string(),
        canonical_snapshot.root_hash,
        private_content_hash.to_string(),
        current_view_hash.clone(),
        fixture.builder_hash.clone(),
        fixture.builder.model_id.clone(),
        fixture.builder.provider_compatibility_id.clone(),
    ];
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);
    let _directory = fixture.directory;

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault
        .remove_projection_manifest_object_for_test(&current_view_hash)
        .unwrap();
    let success_path = vault_path.clone();
    let (_, success_trace) = capture_reset_trace(move || {
        vault.inject_projection_reset_after_marker_exchange_once();
        let reset = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::reset_authorized(
                    Arc::clone(&vault),
                    fixture_builder(),
                    ProjectionRebuildSelector::AllEligible,
                    &proof,
                )
            })
            .unwrap()
            .unwrap();
        assert!(matches!(reset, Err(ProjectionError::CommitIndeterminate)));
        drop(canonical);
        drop(vault);

        let vault = Arc::new(PersonalVaultStorage::open(&success_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        let recovered = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::recover_reset_authorized(Arc::clone(&vault), &proof)
            })
            .unwrap()
            .unwrap()
            .unwrap();
        let core = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                recovered
                    .resume_authorized(fixture_builder(), ProjectionRebuildSelector::AllEligible, &proof)
                    .map(|(core, _)| core)
            })
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(
            core.current_view_for_test().unwrap().entries[0].state,
            ProjectionState::Queued { .. }
        ));
        assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 0);
    });
    success_trace.assert_contract(
        &[
            ("inspection", "reset_required"),
            ("prepared", "marker_durable"),
            ("pair_exchange", "namespace_exchanged_unverified"),
            ("pair_exchange", "published"),
            ("marker_transition", "transition_requires_recovery"),
            ("recovery", "applied_maintenance"),
            ("recovery", "maintenance_capability"),
            ("recovery", "applied_maintenance"),
            ("marker_transition", "transition_evidence_removed"),
            ("quarantine_cleanup", "pair_removed"),
            ("seal_cleanup", "seal_removed"),
            ("container_cleanup", "container_removed"),
            ("marker_clear", "marker_removed"),
            ("recovery", "genesis_only"),
            ("complete", "applied"),
        ],
        &forbidden_values,
    );

    let fixture = self::fixture();
    activate_and_queue(&fixture);
    let current_view_hash = fixture
        .projection
        .root_chain_for_test()
        .unwrap()
        .last()
        .unwrap()
        .current_view_hash
        .clone();
    let failure_path = fixture.directory.path().join("vault");
    let failure_forbidden = vec![
        failure_path.display().to_string(),
        ".plico-projection-pair-staging.".to_string(),
        ".plico-projection-reset-marker-transition.".to_string(),
        fixture.canonical.projection_snapshot().unwrap().root_hash,
        current_view_hash.clone(),
        fixture.builder_hash.clone(),
    ];
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);
    let _failure_directory = fixture.directory;
    let vault = Arc::new(PersonalVaultStorage::open(&failure_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault
        .remove_projection_manifest_object_for_test(&current_view_hash)
        .unwrap();
    vault.inject_projection_reset_after_marker_exchange_once();
    let reset = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::reset_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
        })
        .unwrap()
        .unwrap();
    assert!(matches!(reset, Err(ProjectionError::CommitIndeterminate)));
    drop(canonical);
    drop(vault);
    let vault = Arc::new(PersonalVaultStorage::open(&failure_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault.resolve_projection_reset_transition_for_test().unwrap();
    vault.inject_projection_reset_quarantine_pre_mutation_io_once();
    let (failure, failure_trace) = capture_reset_trace(|| {
        canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::recover_reset_authorized(Arc::clone(&vault), &proof)
            })
            .unwrap()
            .unwrap()
    });
    assert!(matches!(failure, Err(ProjectionError::Io(_))));
    failure_trace.assert_contract(
        &[
            ("recovery", "applied_maintenance"),
            ("recovery", "maintenance_capability"),
            ("recovery", "applied_maintenance"),
            ("quarantine_cleanup", "storage_io"),
        ],
        &failure_forbidden,
    );

    let fixture = self::fixture();
    activate_and_queue(&fixture);
    let current_view_hash = fixture
        .projection
        .root_chain_for_test()
        .unwrap()
        .last()
        .unwrap()
        .current_view_hash
        .clone();
    let untrusted_path = fixture.directory.path().join("vault");
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);
    let _untrusted_directory = fixture.directory;
    let vault = Arc::new(PersonalVaultStorage::open(&untrusted_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault
        .remove_projection_manifest_object_for_test(&current_view_hash)
        .unwrap();
    vault.inject_projection_reset_after_marker_exchange_once();
    let reset = canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::reset_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
        })
        .unwrap()
        .unwrap();
    assert!(matches!(reset, Err(ProjectionError::CommitIndeterminate)));
    drop(canonical);
    drop(vault);
    let vault = Arc::new(PersonalVaultStorage::open(&untrusted_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault
        .tamper_projection_reset_active_marker_future_schema_for_test()
        .unwrap();
    let (unsupported, untrusted_trace) = capture_reset_trace(|| {
        canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::recover_reset_authorized(Arc::clone(&vault), &proof)
            })
            .unwrap()
            .unwrap()
    });
    assert!(matches!(unsupported, Err(ProjectionError::UnsupportedFormat { .. })));
    untrusted_trace.assert_no_untrusted_reset_correlation();
}

#[cfg(target_os = "linux")]
#[test]
fn projection_reset_trace_contract_is_process_isolated() {
    run_reset_trace_child(
        "memory::projection::tests::projection_reset_trace_contract_child",
        "PLICO_PROJECTION_RESET_TRACE_CHILD",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn future_projection_schemas_are_unsupported_and_owner_reset_is_zero_write() {
    let cases = [
        ProjectionUnknownSchemaTarget::RootPointer,
        ProjectionUnknownSchemaTarget::CurrentRoot,
        ProjectionUnknownSchemaTarget::HistoricalRoot,
        ProjectionUnknownSchemaTarget::CurrentSegment,
        ProjectionUnknownSchemaTarget::CurrentRecord,
        ProjectionUnknownSchemaTarget::RecordBuilder,
        ProjectionUnknownSchemaTarget::CurrentView,
        ProjectionUnknownSchemaTarget::ViewBuilder,
        ProjectionUnknownSchemaTarget::ArtifactDescriptor,
        ProjectionUnknownSchemaTarget::Artifact,
    ];

    for target in cases {
        let fixture = fixture();
        match target {
            ProjectionUnknownSchemaTarget::ArtifactDescriptor | ProjectionUnknownSchemaTarget::Artifact => {
                make_ready(&fixture);
            }
            _ => activate_builder_only(&fixture),
        }
        fixture.projection.inject_unknown_schema_for_test(target).unwrap();
        let vault_path = fixture.directory.path().join("vault");
        drop(fixture.projection);
        drop(fixture.canonical);
        drop(fixture.vault);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        let projection_before = vault.projection_tree_fingerprint_for_test().unwrap();
        let projection_atimes_before = vault.projection_tree_atimes_for_test(&projection_before).unwrap();
        let canonical_before = vault.canonical_tree_fingerprint_for_test().unwrap();
        let canonical_atimes_before = vault.canonical_tree_atimes_for_test(&canonical_before).unwrap();
        match ProjectionCoordinatorCore::inspect_existing(&vault, &canonical.projection_snapshot().unwrap(), None) {
            Ok(super::coordinator_core::ProjectionCoreInspection::UnsupportedFormat) => {}
            Ok(super::coordinator_core::ProjectionCoreInspection::ResetRequired(reason)) => {
                let _classified_reason = reason;
                panic!("future schema target {target:?} was classified reset-required")
            }
            Ok(super::coordinator_core::ProjectionCoreInspection::Unavailable(_)) => {
                panic!("future schema target {target:?} was classified unavailable")
            }
            Ok(_) => panic!("future schema target {target:?} was not classified as unsupported"),
            Err(_) => panic!("future schema target {target:?} inspection failed"),
        }
        let reset = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::reset_authorized(
                    Arc::clone(&vault),
                    fixture_builder(),
                    ProjectionRebuildSelector::AllEligible,
                    &proof,
                )
            })
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                reset,
                Err(ProjectionError::UnsupportedFormat {
                    component: "projection_store"
                })
            ),
            "future schema target {target:?} was accepted for owner reset"
        );
        assert_eq!(
            vault.projection_tree_atimes_for_test(&projection_before).unwrap(),
            projection_atimes_before
        );
        assert_eq!(vault.projection_tree_fingerprint_for_test().unwrap(), projection_before);
        assert_eq!(
            vault.canonical_tree_atimes_for_test(&canonical_before).unwrap(),
            canonical_atimes_before
        );
        assert_eq!(vault.canonical_tree_fingerprint_for_test().unwrap(), canonical_before);
    }
}

#[cfg(unix)]
#[test]
fn genesis_fifo_resume_child() {
    if std::env::var_os("PLICO_GENESIS_FIFO_RESUME_CHILD").is_none() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let vault_path = directory.path().join("vault");
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    commit_root(
        &canonical,
        MemoryContent::Text("fifo deadline source".to_string()),
        MemoryTier::Working,
    );
    canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::bootstrap_genesis_only_for_test(Arc::clone(&vault), &proof)
        })
        .unwrap()
        .unwrap()
        .unwrap();
    drop(canonical);
    drop(vault);

    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
    vault
        .inject_projection_artifact_fifo_orphan_for_test(&"56".repeat(32))
        .unwrap();
    canonical
        .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
            ProjectionCoordinatorCore::resume_genesis_authorized(
                Arc::clone(&vault),
                fixture_builder(),
                ProjectionRebuildSelector::AllEligible,
                &proof,
            )
            .map(|(core, _)| core)
        })
        .unwrap()
        .unwrap()
        .unwrap();
}

#[cfg(unix)]
#[test]
fn genesis_fifo_resume_has_a_process_deadline() {
    let executable = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(executable)
        .arg("--exact")
        .arg("memory::projection::tests::genesis_fifo_resume_child")
        .env("PLICO_GENESIS_FIFO_RESUME_CHILD", "1")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("GenesisOnly FIFO resume exceeded its process deadline");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn referenced_fifo_repair_child() {
    if std::env::var_os("PLICO_REFERENCED_FIFO_CHILD").is_none() {
        return;
    }
    let fixture = fixture();
    let (_, artifact_hash) = make_ready(&fixture);
    fixture.projection.inject_artifact_fifo(&artifact_hash).unwrap();
    let vault_path = fixture.directory.path().join("vault");
    let snapshot = fixture.snapshot.clone();
    drop(fixture.projection);
    drop(fixture.canonical);
    drop(fixture.vault);
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
    let repairing = reopen_unrepaired(vault, &snapshot);
    repairing.repair_invalid_artifacts(&snapshot).unwrap().unwrap();
    assert!(matches!(
        repairing.current_view().unwrap().entries[0].state,
        ProjectionState::Stale {
            reason: StaleReason::ArtifactInvalid,
            ..
        }
    ));
    assert!(repairing.artifact_hashes().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
#[ignore = "process deadline gate; run explicitly with one test thread"]
fn referenced_fifo_open_repair_cleanup_has_a_process_deadline() {
    let executable = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(executable)
        .arg("--exact")
        .arg("memory::projection::tests::referenced_fifo_repair_child")
        .env("PLICO_REFERENCED_FIFO_CHILD", "1")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("referenced FIFO repair exceeded its process deadline");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
