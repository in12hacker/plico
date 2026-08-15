use super::*;

#[test]
fn operation_catalog_is_exact_and_unique() {
    let catalog = CapabilityCatalog::default();
    assert_eq!(catalog.operations.len(), 14);
    assert_eq!(catalog.operations, PUBLIC_OPERATIONS.map(str::to_string));
    let unique: std::collections::HashSet<_> = catalog.operations.iter().collect();
    assert_eq!(unique.len(), catalog.operations.len());
    assert_eq!(
        catalog.projections.memory_embedding.control_plane,
        CapabilitySupport::Supported
    );
    assert_eq!(
        catalog.projections.memory_embedding.retrieval,
        CapabilitySupport::Unsupported
    );
    assert_eq!(catalog.projections.memory_vector_recall, CapabilitySupport::Unsupported);
    assert_eq!(catalog.projections.memory_hybrid_recall, CapabilitySupport::Unsupported);
}

#[test]
fn projection_wire_uses_only_kind_and_selector_type() {
    let revision_id = uuid::Uuid::new_v4();
    let status: PublicRequest = serde_json::from_value(serde_json::json!({
        "protocol": PERSONAL_PROTOCOL,
        "request_id": uuid::Uuid::new_v4(),
        "operation": "projection.status",
        "input": { "kind": "memory_embedding", "revision_id": revision_id }
    }))
    .unwrap();
    assert_eq!(status.command.operation(), "projection.status");
    assert!(serde_json::from_value::<PublicRequest>(serde_json::json!({
        "protocol": PERSONAL_PROTOCOL,
        "request_id": uuid::Uuid::new_v4(),
        "operation": "projection.status",
        "input": { "projection_kind": "memory_embedding", "revision_id": revision_id }
    }))
    .is_err());
    assert!(serde_json::from_value::<PublicRequest>(serde_json::json!({
        "protocol": PERSONAL_PROTOCOL,
        "request_id": uuid::Uuid::new_v4(),
        "operation": "projection.rebuild",
        "input": {
            "kind": "memory_embedding",
            "selector": { "kind": "all_eligible" }
        }
    }))
    .is_err());
}

#[test]
fn public_auth_debug_redacts_bearer() {
    let secret = "PRIVATE_BEARER_CANARY";
    let rendered = format!("{:?}", PublicAuth { bearer: secret.into() });
    assert!(rendered.contains("<redacted>"));
    assert!(!rendered.contains(secret));
}

#[test]
fn request_rejects_unknown_and_identity_fields() {
    for extra in [
        r#", "agent_id":"owner""#,
        r#", "tenant_id":"forged-namespace""#,
        r#", "tier":"long_term""#,
    ] {
        let json = format!(
            r#"{{"protocol":"plico.personal.v2","request_id":"{}","operation":"memory.create","input":{{"content":"fact"{extra}}}}}"#,
            uuid::Uuid::new_v4()
        );
        assert!(serde_json::from_str::<PublicRequest>(&json).is_err());
    }
}

#[test]
fn old_wire_method_is_not_a_public_request() {
    let json = r#"{"method":"create","content":"legacy","agent_id":"cli"}"#;
    assert!(serde_json::from_str::<PublicRequest>(json).is_err());
}

#[test]
fn request_head_classifies_unknown_operation_without_dispatching_input() {
    let request_id = uuid::Uuid::new_v4();
    let unknown = format!(
        r#"{{"protocol":"plico.personal.v2","request_id":"{request_id}","operation":"legacy.create","input":{{"agent_id":"forged"}}}}"#
    );
    let head: PublicRequestHead = serde_json::from_str(&unknown).unwrap();
    assert!(head.validate_metadata().is_ok());
    assert!(!head.operation_supported());
    assert!(serde_json::from_str::<PublicRequest>(&unknown).is_err());

    let malformed_known = format!(
        r#"{{"protocol":"plico.personal.v2","request_id":"{request_id}","operation":"memory.create","input":{{"unknown":"field"}}}}"#
    );
    let head: PublicRequestHead = serde_json::from_str(&malformed_known).unwrap();
    assert!(head.validate_metadata().is_ok());
    assert!(head.operation_supported());
    assert!(serde_json::from_str::<PublicRequest>(&malformed_known).is_err());
}

#[test]
fn request_validation_enforces_protocol_limits_and_ids() {
    let request = PublicRequest::new(
        uuid::Uuid::nil(),
        None,
        PublicCommand::MemoryRecall(MemoryRecallInput {
            query: String::new(),
            limit: 0,
        }),
    );
    assert!(request.validate().is_err());

    let mut request = PublicRequest::new(
        uuid::Uuid::new_v4(),
        None,
        PublicCommand::MemoryRecall(MemoryRecallInput {
            query: "what changed?".to_string(),
            limit: 20,
        }),
    );
    assert!(request.validate().is_ok());
    request.protocol = "26.0.0".to_string();
    assert!(request.validate().is_err());
}

#[test]
fn object_base64_limits_apply_to_decoded_bytes() {
    let valid = PublicRequest::new(
        uuid::Uuid::new_v4(),
        None,
        PublicCommand::ObjectPut(ObjectPutInput {
            content: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"binary\0payload"),
            encoding: ObjectEncoding::Base64,
            tags: vec![],
        }),
    );
    assert!(valid.validate().is_ok());

    let invalid = PublicRequest::new(
        uuid::Uuid::new_v4(),
        None,
        PublicCommand::ObjectPut(ObjectPutInput {
            content: "not-base64".to_string(),
            encoding: ObjectEncoding::Base64,
            tags: vec![],
        }),
    );
    assert!(invalid.validate().is_err());
}

#[test]
fn typed_roundtrip_preserves_operation() {
    let request = PublicRequest::new(
        uuid::Uuid::new_v4(),
        Some(PublicAuth {
            bearer: "credential".to_string(),
        }),
        PublicCommand::ObjectSearch(ObjectSearchInput {
            query: "canonical memory".to_string(),
            limit: 10,
            require_tags: vec!["architecture".to_string()],
            exclude_tags: vec![],
        }),
    );
    let encoded = serde_json::to_vec(&request).unwrap();
    let decoded: PublicRequest = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded.command.operation(), "object.search");
    assert_eq!(decoded, request);
}

#[test]
fn success_and_failure_are_mutually_exclusive_by_constructor() {
    let request_id = uuid::Uuid::new_v4();
    let success = PublicResponse::success(
        request_id,
        PublicData::CapabilitiesDescribe(CapabilityCatalog::default()),
    );
    assert!(success.ok);
    assert!(success.data.is_some());
    assert!(success.error.is_none());
    assert!(success.validate().is_ok());
    let request = PublicRequest::new(
        request_id,
        None,
        PublicCommand::CapabilitiesDescribe(EmptyInput::default()),
    );
    assert!(success.validate_for(&request).is_ok());

    let failure = PublicResponse::failure(
        request_id,
        PublicError {
            code: PublicErrorCode::UnsupportedCapability,
            message: "operation is not public".to_string(),
            retryable: false,
            details: None,
        },
    );
    assert!(!failure.ok);
    assert!(failure.data.is_none());
    assert!(failure.error.is_some());
    assert!(failure.validate().is_ok());

    let inconsistent = PublicResponse {
        protocol: PERSONAL_PROTOCOL.to_string(),
        request_id,
        ok: true,
        data: None,
        error: None,
    };
    assert!(inconsistent.validate().is_err());

    let wrong_operation = PublicResponse::success(
        request_id,
        PublicData::RuntimeReadiness(ReadinessView {
            ready: false,
            canonical_store: ComponentState::Ready,
            canonical_memory_persistence: ComponentState::Unavailable,
            projection: ProjectionReadinessView {
                control_plane: ComponentState::Degraded,
                worker: ComponentState::Unavailable,
                control_plane_reason: Some(ProjectionUnavailableCategory::ProjectionNotInitialized),
                worker_reason: Some(ProjectionUnavailableCategory::ProjectionNotInitialized),
            },
            cognitive_worker: ComponentState::Unavailable,
            embedding_provider: ComponentState::Unavailable,
            configured_embedding_backend: "stub".to_string(),
            active_embedding_provider: "stub".to_string(),
        }),
    );
    assert!(wrong_operation.validate_for(&request).is_err());
}
