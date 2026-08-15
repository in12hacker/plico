"""Exact public-envelope, validation, and secret-handling client contracts."""

from __future__ import annotations

import json

import pytest

from plico_benchmarks.core.client import (
    PROTOCOL,
    PUBLIC_OPERATION_CATALOG,
    PUBLIC_OPERATIONS,
    PlicoClient,
    PlicoProtocolError,
)


def test_public_protocol_is_v2_exact_14_without_v1_status_alias():
    assert PROTOCOL == "plico.personal.v2"
    assert len(PUBLIC_OPERATION_CATALOG) == len(set(PUBLIC_OPERATION_CATALOG)) == 14
    assert "projection.status" in PUBLIC_OPERATIONS
    assert "projection.rebuild" in PUBLIC_OPERATIONS
    assert "memory.index_status" not in PUBLIC_OPERATIONS


def test_request_sends_exact_typed_envelope_and_validates_response(monkeypatch):
    client = PlicoClient(bearer_token="private-bearer")
    sent: list[bytes] = []
    monkeypatch.setattr(client, "ensure_connected", lambda: None)
    monkeypatch.setattr(client, "_send", sent.append)

    def response() -> bytes:
        request = json.loads(sent[0])
        return json.dumps(
            {
                "protocol": PROTOCOL,
                "request_id": request["request_id"],
                "ok": True,
                "data": {
                    "operation": "runtime.readiness",
                    "result": {"ready": True},
                },
            }
        ).encode()

    monkeypatch.setattr(client, "_recv", response)

    assert client.runtime_readiness() == {"ready": True}
    envelope = json.loads(sent[0])
    assert set(envelope) == {"protocol", "request_id", "auth", "operation", "input"}
    assert envelope["protocol"] == PROTOCOL
    assert envelope["auth"] == {"bearer": "private-bearer"}
    assert envelope["operation"] == "runtime.readiness"
    assert envelope["input"] == {}
    assert "agent_id" not in envelope
    assert "method" not in envelope


def test_uds_request_omits_payload_auth_and_records_frame_bytes(monkeypatch):
    client = PlicoClient(uds_path="/owner-only/plico.sock")
    sent: list[bytes] = []
    monkeypatch.setattr(client, "ensure_connected", lambda: None)
    monkeypatch.setattr(client, "_send", sent.append)

    def response() -> bytes:
        request = json.loads(sent[0])
        return json.dumps(
            {
                "protocol": PROTOCOL,
                "request_id": request["request_id"],
                "ok": True,
                "data": {
                    "operation": "runtime.readiness",
                    "result": {"ready": True},
                },
            },
            separators=(",", ":"),
        ).encode()

    monkeypatch.setattr(client, "_recv", response)

    assert client.runtime_readiness() == {"ready": True}
    assert "auth" not in json.loads(sent[0])
    assert client.transport == "uds"
    assert client.last_exchange_bytes == {
        "request": len(sent[0]) + 4,
        "response": len(response()) + 4,
    }


@pytest.mark.parametrize("operation", sorted(PUBLIC_OPERATIONS))
def test_public_catalog_has_no_legacy_operation_names(operation):
    assert "." in operation
    assert operation not in {"create", "read", "search", "health_report"}


@pytest.mark.parametrize("identity_field", ["agent_id", "role_id", "tenant_id", "scope"])
def test_identity_and_namespace_fields_are_rejected_before_transport(identity_field, monkeypatch):
    client = PlicoClient(bearer_token="private-bearer")
    monkeypatch.setattr(
        client,
        "ensure_connected",
        lambda: (_ for _ in ()).throw(AssertionError("must not connect")),
    )

    with pytest.raises(ValueError, match="outside its schema"):
        client.request("memory.create", {"content": "fact", "tags": [], identity_field: "claim"})


def test_domain_error_does_not_echo_peer_message_details_or_bearer():
    response = {
        "protocol": PROTOCOL,
        "request_id": "00000000-0000-4000-8000-000000000001",
        "ok": False,
        "error": {
            "code": "INVALID_ARGUMENT",
            "message": "private-bearer and personal content",
            "retryable": False,
            "details": {"token": "private-bearer"},
        },
    }

    with pytest.raises(PlicoProtocolError) as failure:
        PlicoClient._validated_result(
            response,
            "00000000-0000-4000-8000-000000000001",
            "memory.create",
        )

    rendered = str(failure.value)
    assert "INVALID_ARGUMENT" in rendered
    assert failure.value.code == "INVALID_ARGUMENT"
    assert "private-bearer" not in rendered
    assert "personal content" not in rendered


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("protocol", "legacy.v0", "unexpected protocol"),
        ("request_id", "00000000-0000-4000-8000-000000000002", "request_id"),
    ],
)
def test_response_metadata_mismatch_fails_closed(field, value, message):
    response = {
        "protocol": PROTOCOL,
        "request_id": "00000000-0000-4000-8000-000000000001",
        "ok": True,
        "data": {"operation": "runtime.readiness", "result": {"ready": True}},
    }
    response[field] = value

    with pytest.raises(PlicoProtocolError, match=message):
        PlicoClient._validated_result(
            response,
            "00000000-0000-4000-8000-000000000001",
            "runtime.readiness",
        )


def test_response_operation_mismatch_fails_closed():
    response = {
        "protocol": PROTOCOL,
        "request_id": "00000000-0000-4000-8000-000000000001",
        "ok": True,
        "data": {"operation": "object.get", "result": {}},
    }

    with pytest.raises(PlicoProtocolError, match="mismatched typed data"):
        PlicoClient._validated_result(
            response,
            "00000000-0000-4000-8000-000000000001",
            "runtime.readiness",
        )


def test_response_cannot_mix_success_data_and_error():
    response = {
        "protocol": PROTOCOL,
        "request_id": "00000000-0000-4000-8000-000000000001",
        "ok": True,
        "data": {"operation": "runtime.readiness", "result": {"ready": True}},
        "error": {"code": "INTERNAL"},
    }

    with pytest.raises(PlicoProtocolError, match="invalid success envelope"):
        PlicoClient._validated_result(
            response,
            "00000000-0000-4000-8000-000000000001",
            "runtime.readiness",
        )


def test_projection_conveniences_emit_the_exact_typed_v2_inputs(monkeypatch):
    client = PlicoClient(bearer_token="private-bearer")
    requests = []
    monkeypatch.setattr(
        client,
        "request",
        lambda operation, input_data: requests.append((operation, input_data)) or {},
    )

    revision_id = "00000000-0000-4000-8000-000000000003"
    client.projection_status(revision_id)
    client.projection_rebuild_current(revision_id)
    client.projection_rebuild_all_eligible()

    assert requests == [
        (
            "projection.status",
            {"kind": "memory_embedding", "revision_id": revision_id},
        ),
        (
            "projection.rebuild",
            {
                "kind": "memory_embedding",
                "selector": {"type": "current_revision", "revision_id": revision_id},
            },
        ),
        (
            "projection.rebuild",
            {"kind": "memory_embedding", "selector": {"type": "all_eligible"}},
        ),
    ]
