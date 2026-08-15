//! Exact MCP tool catalog for the typed personal-vault protocol.

use plico::api::public::{PublicCommand, DEFAULT_LIMIT, MAX_LIMIT, MAX_TAGS};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolInputError {
    UnknownTool,
    InvalidArguments,
}

pub fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "capabilities.describe",
            "Describe the stable personal-vault protocol and its limits.",
            empty_schema(),
        ),
        tool(
            "runtime.readiness",
            "Report readiness without probing providers or mutating canonical state.",
            empty_schema(),
        ),
        tool(
            "object.put",
            "Commit one canonical object to the personal vault.",
            object_schema(
                json!({
                    "content": { "type": "string", "minLength": 1 },
                    "encoding": { "type": "string", "enum": ["utf8", "base64"], "default": "utf8" },
                    "tags": tags_schema(),
                }),
                &["content"],
            ),
        ),
        tool(
            "object.get",
            "Read a visible canonical object by content identifier.",
            object_schema(
                json!({
                    "cid": {
                        "type": "string",
                        "pattern": "^[0-9a-f]{64}$"
                    }
                }),
                &["cid"],
            ),
        ),
        tool(
            "object.search",
            "Search visible objects and return typed retrieval diagnostics.",
            object_schema(
                json!({
                    "query": { "type": "string", "minLength": 1 },
                    "limit": limit_schema(),
                    "require_tags": tags_schema(),
                    "exclude_tags": tags_schema(),
                }),
                &["query"],
            ),
        ),
        tool(
            "memory.create",
            "Commit a canonical working-memory entry.",
            object_schema(
                json!({
                    "content": { "type": "string", "minLength": 1 },
                    "tags": tags_schema(),
                }),
                &["content"],
            ),
        ),
        tool(
            "memory.get",
            "Read one visible working-memory entry.",
            memory_entry_schema(),
        ),
        tool(
            "memory.recall",
            "Recall visible working memories using the currently supported lexical path.",
            object_schema(
                json!({
                    "query": { "type": "string", "minLength": 1 },
                    "limit": limit_schema(),
                }),
                &["query"],
            ),
        ),
        tool(
            "projection.status",
            "Read the memory-embedding projection state for one canonical revision.",
            object_schema(
                json!({
                    "kind": { "const": "memory_embedding" },
                    "revision_id": uuid_schema(),
                }),
                &["kind", "revision_id"],
            ),
        ),
        tool(
            "projection.rebuild",
            "Run an owner-authorized memory-embedding projection rebuild.",
            object_schema(
                json!({
                    "kind": { "const": "memory_embedding" },
                    "selector": {
                        "oneOf": [
                            {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "type": { "const": "current_revision" },
                                    "revision_id": uuid_schema(),
                                },
                                "required": ["type", "revision_id"]
                            },
                            {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": { "type": { "const": "all_eligible" } },
                                "required": ["type"]
                            }
                        ]
                    }
                }),
                &["kind", "selector"],
            ),
        ),
        tool(
            "memory.update",
            "Durably replace the canonical content of one working-memory entry.",
            object_schema(
                json!({
                    "entry_id": uuid_schema(),
                    "content": { "type": "string", "minLength": 1 },
                }),
                &["entry_id", "content"],
            ),
        ),
        tool(
            "memory.delete",
            "Durably delete one working-memory entry.",
            memory_entry_schema(),
        ),
        tool(
            "session.start",
            "Start a durable personal-vault session from an optional event watermark.",
            object_schema(
                json!({
                    "last_seen_seq": { "type": "integer", "minimum": 0 }
                }),
                &[],
            ),
        ),
        tool(
            "session.end",
            "Durably end a personal-vault session.",
            object_schema(json!({ "session_id": uuid_schema() }), &["session_id"]),
        ),
    ]
}

pub fn command_from_tool(name: &str, arguments: Value) -> Result<PublicCommand, ToolInputError> {
    let command = match name {
        "capabilities.describe" => decode(arguments).map(PublicCommand::CapabilitiesDescribe),
        "runtime.readiness" => decode(arguments).map(PublicCommand::RuntimeReadiness),
        "object.put" => decode(arguments).map(PublicCommand::ObjectPut),
        "object.get" => decode(arguments).map(PublicCommand::ObjectGet),
        "object.search" => decode(arguments).map(PublicCommand::ObjectSearch),
        "memory.create" => decode(arguments).map(PublicCommand::MemoryCreate),
        "memory.get" => decode(arguments).map(PublicCommand::MemoryGet),
        "memory.recall" => decode(arguments).map(PublicCommand::MemoryRecall),
        "projection.status" => decode(arguments).map(PublicCommand::ProjectionStatus),
        "projection.rebuild" => decode(arguments).map(PublicCommand::ProjectionRebuild),
        "memory.update" => decode(arguments).map(PublicCommand::MemoryUpdate),
        "memory.delete" => decode(arguments).map(PublicCommand::MemoryDelete),
        "session.start" => decode(arguments).map(PublicCommand::SessionStart),
        "session.end" => decode(arguments).map(PublicCommand::SessionEnd),
        _ => Err(ToolInputError::UnknownTool),
    }?;
    command.validate().map_err(|_| ToolInputError::InvalidArguments)?;
    Ok(command)
}

fn decode<T: DeserializeOwned>(arguments: Value) -> Result<T, ToolInputError> {
    serde_json::from_value(arguments).map_err(|_| ToolInputError::InvalidArguments)
}

fn tool(name: &'static str, description: &'static str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

fn empty_schema() -> Value {
    object_schema(json!({}), &[])
}

fn memory_entry_schema() -> Value {
    object_schema(json!({ "entry_id": uuid_schema() }), &["entry_id"])
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn tags_schema() -> Value {
    json!({
        "type": "array",
        "items": { "type": "string", "minLength": 1 },
        "maxItems": MAX_TAGS,
        "default": [],
    })
}

fn limit_schema() -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "maximum": MAX_LIMIT,
        "default": DEFAULT_LIMIT,
    })
}

fn uuid_schema() -> Value {
    json!({ "type": "string", "format": "uuid" })
}

#[cfg(test)]
mod tests {
    use plico::api::public::PUBLIC_OPERATIONS;

    use super::*;

    #[test]
    fn catalog_is_exactly_the_public_operation_catalog() {
        let definitions = tool_definitions();
        let names: Vec<&str> = definitions
            .iter()
            .map(|definition| definition["name"].as_str().unwrap())
            .collect();

        assert_eq!(names, PUBLIC_OPERATIONS);
    }

    #[test]
    fn every_schema_rejects_unknown_or_claimed_identity_fields() {
        for definition in tool_definitions() {
            let schema = &definition["inputSchema"];
            assert_eq!(schema["additionalProperties"], false);
            for forbidden in ["auth", "bearer", "role_id", "agent_id", "tenant_id", "namespace"] {
                assert!(
                    schema["properties"].get(forbidden).is_none(),
                    "{} exposed forbidden field {forbidden}",
                    definition["name"]
                );
            }
        }
    }

    #[test]
    fn command_mapping_uses_typed_input_and_rejects_extra_fields() {
        let command = command_from_tool("object.search", json!({ "query": "memory hierarchy", "limit": 4 })).unwrap();
        assert_eq!(command.operation(), "object.search");

        assert_eq!(
            command_from_tool(
                "object.search",
                json!({ "query": "memory hierarchy", "role_id": "forged" }),
            ),
            Err(ToolInputError::InvalidArguments)
        );
        assert_eq!(
            command_from_tool("object.search", json!({ "query": "memory hierarchy", "limit": 0 })),
            Err(ToolInputError::InvalidArguments)
        );
        assert_eq!(command_from_tool("plico", json!({})), Err(ToolInputError::UnknownTool));
    }

    #[test]
    fn all_public_commands_have_a_decodable_tool_input() {
        let samples = [
            ("capabilities.describe", json!({})),
            ("runtime.readiness", json!({})),
            ("object.put", json!({ "content": "x" })),
            ("object.get", json!({ "cid": "a".repeat(64) })),
            ("object.search", json!({ "query": "x" })),
            ("memory.create", json!({ "content": "x" })),
            ("memory.get", json!({ "entry_id": uuid::Uuid::new_v4() })),
            ("memory.recall", json!({ "query": "x" })),
            (
                "projection.status",
                json!({ "kind": "memory_embedding", "revision_id": uuid::Uuid::new_v4() }),
            ),
            (
                "projection.rebuild",
                json!({ "kind": "memory_embedding", "selector": { "type": "all_eligible" } }),
            ),
            (
                "memory.update",
                json!({ "entry_id": uuid::Uuid::new_v4(), "content": "x" }),
            ),
            ("memory.delete", json!({ "entry_id": uuid::Uuid::new_v4() })),
            ("session.start", json!({})),
            ("session.end", json!({ "session_id": uuid::Uuid::new_v4() })),
        ];

        for (name, input) in samples {
            let command = command_from_tool(name, input).unwrap();
            assert_eq!(command.operation(), name);
        }
    }
}
