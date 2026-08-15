use sha2::{Digest, Sha256};

use super::*;
use crate::legacy::{
    preflight, LegacyAccessAuthorization, LegacyGroupMapping, LegacyPersistedTier, LegacyPersistenceIndex,
    LegacyRoleMapping, LegacySnapshot,
};

fn revision_id(label: &str) -> String {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, label.as_bytes()).to_string()
}

fn entry(label: &str, agent: &str, scope: serde_json::Value) -> LegacyMemoryEntry {
    serde_json::from_value(serde_json::json!({
        "id": revision_id(label),
        "agent_id": agent,
        "tenant_id": "default",
        "tier": "Working",
        "content": {"Text": format!("content-{label}")},
        "importance": 61,
        "access_count": 3,
        "last_accessed": 7,
        "created_at": 5,
        "tags": ["kept"],
        "embedding": [0.5, 0.25],
        "ttl_ms": 90,
        "original_ttl_ms": 100,
        "scope": scope,
        "memory_type": "Semantic",
        "causal_parent": null,
        "supersedes": null,
        "superseded_by": null,
        "deleted_at": null
    }))
    .unwrap()
}

fn snapshot(agent: &str, entries: Vec<LegacyMemoryEntry>) -> LegacySnapshot {
    let bytes = serde_json::to_vec(&entries).unwrap();
    let cid = format!("{:x}", Sha256::digest(&bytes));
    let envelope = serde_json::to_vec(&serde_json::json!({
        "cid": cid,
        "data": bytes,
        "meta": {
            "content_type": "Structured",
            "tags": ["memory"],
            "created_by": "plico:memory-persister",
            "created_at": 1,
            "intent": null,
            "tenant_id": "default",
            "scope": "private"
        }
    }))
    .unwrap();
    LegacySnapshot::decode(agent.to_string(), "working".to_string(), cid, entries.len(), envelope).unwrap()
}

fn preflight_vault(snapshots: &[LegacySnapshot], authorization: &LegacyAccessAuthorization) -> LegacyPreflightReport {
    let mut agents = std::collections::HashMap::new();
    for snapshot in snapshots {
        let entries = snapshot.entries();
        let agent = entries.first().unwrap().agent_id.clone();
        agents.entry(agent).or_insert_with(Vec::new).push(LegacyPersistedTier {
            tier: "working".into(),
            cid: snapshot.cid().to_string(),
            entry_count: entries.len(),
        });
    }
    preflight(&LegacyPersistenceIndex { agents }, snapshots, authorization).unwrap()
}

fn authorization(agents: &[(&str, &str)]) -> LegacyAccessAuthorization {
    let mut roles: Vec<_> = agents.iter().map(|(_, role)| (*role).to_string()).collect();
    roles.push(PERSONAL_OWNER_ROLE_ID.to_string());
    roles.sort();
    roles.dedup();
    let mut role_mappings: Vec<_> = agents
        .iter()
        .map(|(agent, role)| LegacyRoleMapping {
            legacy_agent_id: (*agent).to_string(),
            target_role_id: (*role).to_string(),
        })
        .collect();
    role_mappings.sort_by(|left, right| left.legacy_agent_id.cmp(&right.legacy_agent_id));
    LegacyAccessAuthorization {
        authorized_role_ids: roles,
        role_mappings,
        group_mappings: vec![],
    }
}

fn build(report: &LegacyPreflightReport) -> MigrationRecordPlan {
    build_migration_record_plan(MigrationPlanInput {
        preflight: report,
        lock_created: false,
        source_index_hash: "a".repeat(64),
        credential_role_cutoff_hash: "b".repeat(64),
        imported_at: 1_000,
        imported_by_role: PERSONAL_OWNER_ROLE_ID.into(),
    })
    .unwrap()
}

#[test]
fn private_linear_chain_preserves_content_and_origin() {
    let mut root = entry("root", "conversation", serde_json::json!("Private"));
    let mut child = entry("child", "conversation", serde_json::json!("Private"));
    root.superseded_by = Some(child.id.clone());
    child.supersedes = Some(root.id.clone());
    let auth = authorization(&[("conversation", "conversation-role")]);
    let report = preflight_vault(&[snapshot("conversation", vec![root, child])], &auth);
    let plan = build(&report);

    assert_eq!(plan.revisions().len(), 2);
    assert_eq!(plan.policies().len(), 1);
    assert_eq!(
        plan.revisions()[1].parent_revision_id,
        Some(plan.revisions()[0].revision_id.clone())
    );
    assert_eq!(plan.policies()[0].origin_role_id, "conversation-role");
    assert_eq!(plan.policies()[0].committed_by_role, PERSONAL_OWNER_ROLE_ID);
}

#[test]
fn shared_audience_is_frozen_to_all_active_roles_at_cutoff() {
    let mut auth = authorization(&[("assistant", "assistant-role"), ("conversation", "conversation-role")]);
    auth.authorized_role_ids.insert(2, "existing-reader-role".into());
    let report = preflight_vault(
        &[
            snapshot(
                "assistant",
                vec![entry("other", "assistant", serde_json::json!("Private"))],
            ),
            snapshot(
                "conversation",
                vec![entry("shared", "conversation", serde_json::json!("Shared"))],
            ),
        ],
        &auth,
    );
    let plan = build(&report);
    let shared = plan
        .policies()
        .iter()
        .find(|policy| policy.origin_role_id == "conversation-role")
        .unwrap();
    assert_eq!(
        shared.reader_roles,
        vec![
            "assistant-role",
            "conversation-role",
            "existing-reader-role",
            PERSONAL_OWNER_ROLE_ID,
        ]
    );
    assert_eq!(shared.writer_roles, vec!["conversation-role", PERSONAL_OWNER_ROLE_ID]);
}

#[test]
fn group_mapping_is_preserved_as_explicit_audience() {
    let mut auth = authorization(&[("conversation", "conversation-role")]);
    auth.authorized_role_ids.push("research-role".into());
    auth.group_mappings.push(LegacyGroupMapping {
        legacy_group_id: "research".into(),
        target_role_ids: vec!["research-role".into()],
    });
    let report = preflight_vault(
        &[snapshot(
            "conversation",
            vec![entry(
                "grouped",
                "conversation",
                serde_json::json!({"Group": "research"}),
            )],
        )],
        &auth,
    );
    let plan = build(&report);
    assert_eq!(
        plan.policies()[0].reader_roles,
        vec!["conversation-role", PERSONAL_OWNER_ROLE_ID, "research-role"]
    );
    let mapping = &plan.migration_manifest().stream_mappings[0].policy_mappings[0];
    assert_eq!(
        mapping.source_scope,
        ManifestSourceScope::Group {
            legacy_group_id: "research".into()
        }
    );
}

#[test]
fn deleted_head_appends_import_time_tombstone() {
    let mut deleted = entry("deleted", "conversation", serde_json::json!("Private"));
    deleted.deleted_at = Some(55);
    let auth = authorization(&[("conversation", "conversation-role")]);
    let report = preflight_vault(&[snapshot("conversation", vec![deleted.clone()])], &auth);
    let plan = build(&report);

    assert_eq!(plan.revisions().len(), 2);
    assert_eq!(plan.revisions()[0].deleted_at, None);
    assert_eq!(
        plan.revisions()[1].parent_revision_id.as_ref().unwrap().as_str(),
        deleted.id
    );
    assert_eq!(plan.revisions()[1].deleted_at, Some(1_000));
    assert_eq!(plan.revisions()[1].committed_at, 1_000);
    let mapping = &plan.migration_manifest().stream_mappings[0];
    assert_eq!(mapping.legacy_deleted_at, Some(55));
    assert_eq!(
        mapping.target_head_revision_id,
        mapping.tombstone_revision_id.clone().unwrap()
    );
}

#[test]
fn scope_changes_emit_ordered_policy_events() {
    let mut root = entry("private", "conversation", serde_json::json!("Private"));
    let mut child = entry("shared", "conversation", serde_json::json!("Shared"));
    root.superseded_by = Some(child.id.clone());
    child.supersedes = Some(root.id.clone());
    let auth = authorization(&[("conversation", "conversation-role")]);
    let report = preflight_vault(&[snapshot("conversation", vec![root, child])], &auth);
    let plan = build(&report);

    assert_eq!(plan.policies().len(), 2);
    assert_eq!(plan.policies()[0].sequence, 1);
    assert_eq!(plan.policies()[1].sequence, 2);
    assert_eq!(plan.policies()[0].origin_role_id, "conversation-role");
    assert_eq!(plan.policies()[1].origin_role_id, "conversation-role");
    assert_eq!(plan.migration_manifest().stream_mappings[0].policy_mappings.len(), 2);
}

#[test]
fn manifest_counts_discarded_derived_fields() {
    let auth = authorization(&[("conversation", "conversation-role")]);
    let report = preflight_vault(
        &[snapshot(
            "conversation",
            vec![entry("counted", "conversation", serde_json::json!("Private"))],
        )],
        &auth,
    );
    let plan = build(&report);
    let manifest = plan.migration_manifest();
    assert_eq!(manifest.target_revision_count, 1);
    assert_eq!(manifest.target_policy_count, 1);
    assert_eq!(manifest.target_relation_count, 0);
    assert_eq!(manifest.dispositions.importance_values_discarded, 1);
    assert_eq!(manifest.dispositions.access_statistic_fields_discarded, 2);
    assert_eq!(manifest.dispositions.ttl_fields_discarded, 2);
    assert_eq!(manifest.dispositions.embedded_vectors_discarded, 1);
    assert_eq!(manifest.dispositions.projection_rebuilds_required, 1);
}

#[test]
fn invalid_commit_identity_and_source_hash_fail_closed() {
    let auth = authorization(&[("conversation", "conversation-role")]);
    let report = preflight_vault(
        &[snapshot(
            "conversation",
            vec![entry("input", "conversation", serde_json::json!("Private"))],
        )],
        &auth,
    );
    for (hash, time, actor) in [
        ("bad".into(), 1_000, PERSONAL_OWNER_ROLE_ID.into()),
        ("a".repeat(64), 0, PERSONAL_OWNER_ROLE_ID.into()),
        ("a".repeat(64), 1_000, "untrusted".into()),
    ] {
        assert!(build_migration_record_plan(MigrationPlanInput {
            preflight: &report,
            lock_created: false,
            source_index_hash: hash,
            credential_role_cutoff_hash: "b".repeat(64),
            imported_at: time,
            imported_by_role: actor,
        })
        .is_err());
    }
}

#[test]
fn empty_legacy_vault_builds_verifiable_empty_target() {
    let auth = LegacyAccessAuthorization {
        authorized_role_ids: vec![PERSONAL_OWNER_ROLE_ID.into()],
        role_mappings: vec![],
        group_mappings: vec![],
    };
    let report = preflight(
        &LegacyPersistenceIndex {
            agents: Default::default(),
        },
        &[],
        &auth,
    )
    .unwrap();
    let plan = build(&report);
    assert!(plan.revisions().is_empty());
    assert!(plan.policies().is_empty());
    assert_eq!(plan.source_manifest().source_entry_count, 0);
    assert_eq!(plan.source_manifest().source_stream_count, 0);
    assert_eq!(plan.migration_manifest().target_revision_count, 0);
    assert_eq!(plan.migration_manifest().target_policy_count, 0);
    assert!(plan.migration_manifest().stream_mappings.is_empty());
}
