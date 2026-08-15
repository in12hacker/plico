//! Strict argument mapping for the 14 typed public operations.

use plico::api::public::*;

pub fn parse_command(mut args: Vec<String>) -> Result<PublicCommand, String> {
    let operation = args.remove(0);
    let command = match operation.as_str() {
        "capabilities.describe" => PublicCommand::CapabilitiesDescribe(EmptyInput::default()),
        "runtime.readiness" => PublicCommand::RuntimeReadiness(EmptyInput::default()),
        "object.put" => {
            let content = take_required(&mut args, "--content")?;
            let encoding = match take_optional(&mut args, "--encoding")?.as_deref() {
                None | Some("utf8") => ObjectEncoding::Utf8,
                Some("base64") => ObjectEncoding::Base64,
                Some(_) => return Err("--encoding must be utf8 or base64".to_string()),
            };
            PublicCommand::ObjectPut(ObjectPutInput {
                content,
                encoding,
                tags: take_repeated(&mut args, "--tag")?,
            })
        }
        "object.get" => PublicCommand::ObjectGet(ObjectGetInput {
            cid: take_required(&mut args, "--cid")?,
        }),
        "object.search" => PublicCommand::ObjectSearch(ObjectSearchInput {
            query: take_required(&mut args, "--query")?,
            limit: take_limit(&mut args)?,
            require_tags: take_repeated(&mut args, "--require-tag")?,
            exclude_tags: take_repeated(&mut args, "--exclude-tag")?,
        }),
        "memory.create" => PublicCommand::MemoryCreate(MemoryCreateInput {
            content: take_required(&mut args, "--content")?,
            tags: take_repeated(&mut args, "--tag")?,
        }),
        "memory.get" => PublicCommand::MemoryGet(MemoryEntryInput {
            entry_id: take_uuid(&mut args, "--entry-id")?,
        }),
        "memory.recall" => PublicCommand::MemoryRecall(MemoryRecallInput {
            query: take_required(&mut args, "--query")?,
            limit: take_limit(&mut args)?,
        }),
        "projection.status" => PublicCommand::ProjectionStatus(ProjectionStatusInput {
            kind: ProjectionKindInput::MemoryEmbedding,
            revision_id: take_uuid(&mut args, "--revision-id")?,
        }),
        "projection.rebuild" => {
            let revision_id = take_optional(&mut args, "--revision-id")?
                .map(|value| value.parse().map_err(|_| "--revision-id must be a UUID"))
                .transpose()?;
            let all_eligible = take_switch(&mut args, "--all-eligible")?;
            let selector = match (revision_id, all_eligible) {
                (Some(revision_id), false) => ProjectionRebuildSelectorInput::CurrentRevision { revision_id },
                (None, true) => ProjectionRebuildSelectorInput::AllEligible,
                _ => return Err("projection.rebuild requires exactly one of --revision-id or --all-eligible".into()),
            };
            PublicCommand::ProjectionRebuild(ProjectionRebuildInput {
                kind: ProjectionKindInput::MemoryEmbedding,
                selector,
            })
        }
        "memory.update" => PublicCommand::MemoryUpdate(MemoryUpdateInput {
            entry_id: take_uuid(&mut args, "--entry-id")?,
            content: take_required(&mut args, "--content")?,
        }),
        "memory.delete" => PublicCommand::MemoryDelete(MemoryEntryInput {
            entry_id: take_uuid(&mut args, "--entry-id")?,
        }),
        "session.start" => PublicCommand::SessionStart(SessionStartInput {
            last_seen_seq: take_optional(&mut args, "--last-seen-seq")?
                .map(|value| value.parse().map_err(|_| "--last-seen-seq must be u64"))
                .transpose()?,
        }),
        "session.end" => PublicCommand::SessionEnd(SessionEndInput {
            session_id: take_uuid(&mut args, "--session-id")?,
        }),
        _ => return Err(format!("unsupported operation '{operation}'")),
    };
    if !args.is_empty() {
        return Err(format!("unexpected arguments: {}", args.join(" ")));
    }
    command.validate().map_err(|error| error.message)?;
    Ok(command)
}

fn take_required(args: &mut Vec<String>, flag: &str) -> Result<String, String> {
    take_optional(args, flag)?.ok_or_else(|| format!("{flag} is required"))
}

fn take_optional(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
    let positions: Vec<usize> = args
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (value == flag).then_some(index))
        .collect();
    match positions.as_slice() {
        [] => Ok(None),
        [position] => {
            if *position + 1 >= args.len() {
                return Err(format!("{flag} requires a value"));
            }
            let value = args.remove(*position + 1);
            args.remove(*position);
            Ok(Some(value))
        }
        _ => Err(format!("{flag} may be supplied only once")),
    }
}

fn take_repeated(args: &mut Vec<String>, flag: &str) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    while let Some(position) = args.iter().position(|value| value == flag) {
        if position + 1 >= args.len() {
            return Err(format!("{flag} requires a value"));
        }
        values.push(args.remove(position + 1));
        args.remove(position);
    }
    Ok(values)
}

fn take_switch(args: &mut Vec<String>, flag: &str) -> Result<bool, String> {
    let positions: Vec<usize> = args
        .iter()
        .enumerate()
        .filter_map(|(index, value)| (value == flag).then_some(index))
        .collect();
    match positions.as_slice() {
        [] => Ok(false),
        [position] => {
            args.remove(*position);
            Ok(true)
        }
        _ => Err(format!("{flag} may be supplied only once")),
    }
}

fn take_limit(args: &mut Vec<String>) -> Result<usize, String> {
    take_optional(args, "--limit")?
        .map(|value| value.parse().map_err(|_| "--limit must be an integer".to_string()))
        .transpose()
        .map(|limit| limit.unwrap_or(DEFAULT_LIMIT))
}

fn take_uuid(args: &mut Vec<String>, flag: &str) -> Result<uuid::Uuid, String> {
    take_required(args, flag)?
        .parse()
        .map_err(|_| format!("{flag} must be a UUID"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_set_matches_public_catalog() {
        let commands = [
            "capabilities.describe",
            "runtime.readiness",
            "object.put",
            "object.get",
            "object.search",
            "memory.create",
            "memory.get",
            "memory.recall",
            "projection.status",
            "projection.rebuild",
            "memory.update",
            "memory.delete",
            "session.start",
            "session.end",
        ];
        assert_eq!(commands, PUBLIC_OPERATIONS);
    }

    #[test]
    fn identity_and_unknown_flags_fail_closed() {
        let error = parse_command(vec![
            "memory.create".into(),
            "--content".into(),
            "fact".into(),
            "--agent".into(),
            "forged".into(),
        ])
        .unwrap_err();
        assert!(error.contains("unexpected arguments"));
    }

    #[test]
    fn repeated_tags_are_preserved() {
        let command = parse_command(vec![
            "object.put".into(),
            "--content".into(),
            "fact".into(),
            "--tag".into(),
            "one".into(),
            "--tag".into(),
            "two".into(),
        ])
        .unwrap();
        let PublicCommand::ObjectPut(input) = command else {
            panic!("wrong command")
        };
        assert_eq!(input.tags, ["one", "two"]);
    }
}
