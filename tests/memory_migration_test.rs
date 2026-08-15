#![cfg(feature = "offline-migration")]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::Arc;

use plico::api::public::{
    MemoryCreateInput, MemoryEntryInput, MemoryRecallInput, PublicCommand, PublicData, PublicRequest, PERSONAL_PROTOCOL,
};
use plico::fs::embedding::StubEmbeddingProvider;
use plico::kernel::{AIKernel, PublicRequestContext, PublicTransport};
use plico::llm::StubProvider;
use sha2::{Digest, Sha256};

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_legacy_vault(
    root: &std::path::Path,
    revision_id: uuid::Uuid,
    scope: serde_json::Value,
    include_peer_role: bool,
) {
    let mut roles = Vec::new();
    if include_peer_role {
        roles.extend(["role-a", "role-b"]);
    }
    write_legacy_vault_entries(root, &[(revision_id, "imported memory", scope)], &roles);
}

fn write_legacy_vault_entries(
    root: &std::path::Path,
    records: &[(uuid::Uuid, &str, serde_json::Value)],
    roles: &[&str],
) {
    fs::create_dir(root).unwrap();
    let mut token = serde_json::json!({
        "personal-owner": {
            "agent_id": "personal-owner",
            "token": "owner-secret",
            "issued_at": 1,
            "expires_at": null,
            "capabilities": []
        }
    });
    for role in roles {
        token[*role] = serde_json::json!({
            "agent_id": role,
            "token": format!("{role}-secret"),
            "issued_at": 1,
            "expires_at": null,
            "capabilities": []
        });
    }
    let tokens = root.join("agent_tokens.json");
    fs::write(&tokens, serde_json::to_vec(&token).unwrap()).unwrap();
    fs::set_permissions(&tokens, fs::Permissions::from_mode(0o600)).unwrap();
    let entries = records
        .iter()
        .map(|(revision_id, content, scope)| {
            serde_json::json!({
                "id": revision_id,
                "agent_id": "legacy-agent",
                "tenant_id": "default",
                "tier": "Working",
                "content": {"Text": content},
                "importance": 50,
                "access_count": 0,
                "last_accessed": 1,
                "created_at": 1,
                "tags": ["legacy"],
                "embedding": null,
                "ttl_ms": null,
                "original_ttl_ms": null,
                "scope": scope,
                "memory_type": "Semantic",
                "causal_parent": null,
                "supersedes": null,
                "superseded_by": null,
                "deleted_at": null
            })
        })
        .collect::<Vec<_>>();
    let entries = serde_json::to_vec(&entries).unwrap();
    let cid = hash(&entries);
    let envelope = serde_json::to_vec(&serde_json::json!({
        "cid": cid,
        "data": entries,
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
    let shard = root.join("cas").join(&cid[..2]);
    fs::create_dir_all(&shard).unwrap();
    fs::write(shard.join(&cid[2..]), envelope).unwrap();
    fs::write(
        root.join("memory_index.json"),
        serde_json::to_vec(&serde_json::json!({
            "agents": {"legacy-agent": [{"tier": "working", "cid": cid, "entry_count": records.len()}]}
        }))
        .unwrap(),
    )
    .unwrap();
}

fn request(command: PublicCommand) -> PublicRequest {
    PublicRequest {
        protocol: PERSONAL_PROTOCOL.into(),
        request_id: uuid::Uuid::new_v4(),
        auth: None,
        command,
    }
}

fn recall_count(kernel: &AIKernel, context: &PublicRequestContext, query: &str) -> usize {
    let response = kernel.handle_public_request(
        context,
        request(PublicCommand::MemoryRecall(MemoryRecallInput {
            query: query.into(),
            limit: 10,
        })),
    );
    match response.data.unwrap() {
        PublicData::MemoryRecall(result) => result.hits.len(),
        other => panic!("unexpected response: {other:?}"),
    }
}

#[test]
fn migrate_gen1_runtime_gen2_and_restart() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("vault");
    let imported_id = uuid::Uuid::new_v4();
    write_legacy_vault(&root, imported_id, serde_json::json!("Private"), false);
    let mut child = Command::new(env!("CARGO_BIN_EXE_plico-memory-migrate"))
        .args(["migrate", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"owner_bearer":"owner-secret","role_mappings":[{"legacy_agent_id":"legacy-agent","target_role_id":"personal-owner"}],"group_mappings":[]}"#,
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "published");
    assert_eq!(report["rollback_backup_created"], true);
    let backup = fs::read_dir(parent.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("vault.pre-ledger-backup."))
        })
        .unwrap();
    assert!(backup.join("memory_index.json").is_file());
    assert!(backup.join(".plico-migration-backup-evidence.json").is_file());
    assert!(!root.join("memory_index.json").exists());

    let context = PublicRequestContext::local_owner(PublicTransport::Embedded);
    let kernel = AIKernel::with_providers(
        root.clone(),
        Arc::new(StubEmbeddingProvider::new()),
        Arc::new(StubProvider::empty()),
    )
    .unwrap();
    let imported = kernel.handle_public_request(
        &context,
        request(PublicCommand::MemoryGet(MemoryEntryInput { entry_id: imported_id })),
    );
    assert!(imported.ok, "{imported:?}");
    let created = kernel.handle_public_request(
        &context,
        request(PublicCommand::MemoryCreate(MemoryCreateInput {
            content: "post migration commit".into(),
            tags: vec![],
        })),
    );
    assert!(created.ok, "{created:?}");
    let created_id = match created.data.unwrap() {
        PublicData::MemoryCreate(result) => result.entry.entry_id,
        other => panic!("unexpected response: {other:?}"),
    };
    drop(kernel);

    let restarted = AIKernel::with_providers(
        root,
        Arc::new(StubEmbeddingProvider::new()),
        Arc::new(StubProvider::empty()),
    )
    .unwrap();
    for entry_id in [imported_id, created_id] {
        let response = restarted.handle_public_request(
            &context,
            request(PublicCommand::MemoryGet(MemoryEntryInput { entry_id })),
        );
        assert!(response.ok, "{response:?}");
    }
}

#[test]
fn migrated_shared_cutoff_policy_is_visible_but_not_writable_by_peer_after_restart() {
    use plico::api::permission::PermissionAction;
    use plico::api::public::MemoryUpdateInput;

    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("vault");
    let imported_id = uuid::Uuid::new_v4();
    write_legacy_vault(&root, imported_id, serde_json::json!("Shared"), true);
    let mut child = Command::new(env!("CARGO_BIN_EXE_plico-memory-migrate"))
        .args(["migrate", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"owner_bearer":"owner-secret","role_mappings":[{"legacy_agent_id":"legacy-agent","target_role_id":"role-a"}],"group_mappings":[]}"#,
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let kernel = AIKernel::with_providers(
        root.clone(),
        Arc::new(StubEmbeddingProvider::new()),
        Arc::new(StubProvider::empty()),
    )
    .unwrap();
    kernel.permission_grant("role-b", PermissionAction::Read, None, None);
    kernel.permission_grant("role-b", PermissionAction::Write, None, None);
    let peer = PublicRequestContext::authenticated_role("role-b".into(), PublicTransport::Embedded);
    assert!(
        kernel
            .handle_public_request(
                &peer,
                request(PublicCommand::MemoryGet(MemoryEntryInput { entry_id: imported_id }))
            )
            .ok
    );
    let denied = kernel.handle_public_request(
        &peer,
        request(PublicCommand::MemoryUpdate(MemoryUpdateInput {
            entry_id: imported_id,
            content: "peer must not rewrite".into(),
        })),
    );
    assert_eq!(
        denied.error.unwrap().code,
        plico::api::public::PublicErrorCode::NotFound
    );

    let owner = PublicRequestContext::local_owner(PublicTransport::Embedded);
    let updated = kernel.handle_public_request(
        &owner,
        request(PublicCommand::MemoryUpdate(MemoryUpdateInput {
            entry_id: imported_id,
            content: "owner correction".into(),
        })),
    );
    let updated_id = match updated.data.unwrap() {
        PublicData::MemoryUpdate(result) => result.entry.entry_id,
        other => panic!("unexpected response: {other:?}"),
    };
    drop(kernel);

    let restarted = AIKernel::with_providers(
        root,
        Arc::new(StubEmbeddingProvider::new()),
        Arc::new(StubProvider::empty()),
    )
    .unwrap();
    restarted.permission_grant("role-b", PermissionAction::Read, None, None);
    let visible = restarted.handle_public_request(
        &peer,
        request(PublicCommand::MemoryGet(MemoryEntryInput { entry_id: updated_id })),
    );
    assert!(visible.ok, "{visible:?}");
}

#[test]
fn migrated_group_mapping_is_policy_enforced_after_restart() {
    use plico::api::permission::PermissionAction;
    use plico::api::public::MemoryUpdateInput;

    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("vault");
    let imported_id = uuid::Uuid::new_v4();
    write_legacy_vault(&root, imported_id, serde_json::json!({"Group": "research"}), true);
    let mut child = Command::new(env!("CARGO_BIN_EXE_plico-memory-migrate"))
        .args(["migrate", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"owner_bearer":"owner-secret","role_mappings":[{"legacy_agent_id":"legacy-agent","target_role_id":"role-a"}],"group_mappings":[{"legacy_group_id":"research","target_role_ids":["role-b"]}]}"#,
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let kernel = AIKernel::with_providers(
        root.clone(),
        Arc::new(StubEmbeddingProvider::new()),
        Arc::new(StubProvider::empty()),
    )
    .unwrap();
    for role in ["role-b", "role-c"] {
        kernel.permission_grant(role, PermissionAction::Read, None, None);
        kernel.permission_grant(role, PermissionAction::Write, None, None);
    }
    let mapped = PublicRequestContext::authenticated_role("role-b".into(), PublicTransport::Embedded);
    let outsider = PublicRequestContext::authenticated_role("role-c".into(), PublicTransport::Embedded);
    assert!(
        kernel
            .handle_public_request(
                &mapped,
                request(PublicCommand::MemoryGet(MemoryEntryInput { entry_id: imported_id }))
            )
            .ok
    );
    assert_eq!(
        kernel
            .handle_public_request(
                &outsider,
                request(PublicCommand::MemoryGet(MemoryEntryInput { entry_id: imported_id }))
            )
            .error
            .unwrap()
            .code,
        plico::api::public::PublicErrorCode::NotFound
    );
    assert_eq!(
        kernel
            .handle_public_request(
                &mapped,
                request(PublicCommand::MemoryUpdate(MemoryUpdateInput {
                    entry_id: imported_id,
                    content: "mapped reader must not rewrite".into(),
                }))
            )
            .error
            .unwrap()
            .code,
        plico::api::public::PublicErrorCode::NotFound
    );

    let owner = PublicRequestContext::local_owner(PublicTransport::Embedded);
    let updated_id = match kernel
        .handle_public_request(
            &owner,
            request(PublicCommand::MemoryUpdate(MemoryUpdateInput {
                entry_id: imported_id,
                content: "owner group correction".into(),
            })),
        )
        .data
        .unwrap()
    {
        PublicData::MemoryUpdate(result) => result.entry.entry_id,
        other => panic!("unexpected response: {other:?}"),
    };
    drop(kernel);

    let restarted = AIKernel::with_providers(
        root,
        Arc::new(StubEmbeddingProvider::new()),
        Arc::new(StubProvider::empty()),
    )
    .unwrap();
    restarted.permission_grant("role-b", PermissionAction::Read, None, None);
    let visible = restarted.handle_public_request(
        &mapped,
        request(PublicCommand::MemoryGet(MemoryEntryInput { entry_id: updated_id })),
    );
    assert!(visible.ok, "{visible:?}");
}

#[test]
fn mixed_policy_recall_authorizes_each_stream_before_ranking_and_after_restart() {
    use plico::api::permission::PermissionAction;

    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("vault");
    let private_id = uuid::Uuid::new_v4();
    let shared_id = uuid::Uuid::new_v4();
    let group_id = uuid::Uuid::new_v4();
    write_legacy_vault_entries(
        &root,
        &[
            (private_id, "private orchid", serde_json::json!("Private")),
            (shared_id, "shared saffron", serde_json::json!("Shared")),
            (group_id, "group cobalt", serde_json::json!({"Group": "research"})),
        ],
        &["role-a", "role-b", "role-c"],
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_plico-memory-migrate"))
        .args(["migrate", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"owner_bearer":"owner-secret","role_mappings":[{"legacy_agent_id":"legacy-agent","target_role_id":"role-a"}],"group_mappings":[{"legacy_group_id":"research","target_role_ids":["role-c"]}]}"#,
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let role_b = PublicRequestContext::authenticated_role("role-b".into(), PublicTransport::Embedded);
    let group_reader = PublicRequestContext::authenticated_role("role-c".into(), PublicTransport::Embedded);
    let owner = PublicRequestContext::local_owner(PublicTransport::Embedded);
    for _ in 0..2 {
        let kernel = AIKernel::with_providers(
            root.clone(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(StubProvider::empty()),
        )
        .unwrap();
        for role in ["role-b", "role-c"] {
            kernel.permission_grant(role, PermissionAction::Read, None, None);
        }

        assert_eq!(recall_count(&kernel, &role_b, "orchid"), 0);
        assert_eq!(recall_count(&kernel, &role_b, "saffron"), 1);
        assert_eq!(recall_count(&kernel, &role_b, "cobalt"), 0);
        assert_eq!(recall_count(&kernel, &group_reader, "orchid"), 0);
        assert_eq!(recall_count(&kernel, &group_reader, "cobalt"), 1);
        assert_eq!(recall_count(&kernel, &owner, "orchid"), 1);
        assert_eq!(recall_count(&kernel, &owner, "saffron"), 1);
        assert_eq!(recall_count(&kernel, &owner, "cobalt"), 1);
        drop(kernel);
    }
}
