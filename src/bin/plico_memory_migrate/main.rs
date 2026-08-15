//! Offline-only legacy memory inspection, planning and atomic publication.

mod legacy;
mod plan;

use std::io::Read;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use plico::api::OfflineCredentialSet;
use plico::cas::{
    OfflineMigrationVault, OfflineReferencedObjectFingerprint, OfflineSnapshotFingerprint, OfflineSourceFingerprint,
};
use plico::memory::ledger::{build_offline_migration_target, OfflineMigrationTargetInput};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use legacy::{
    preflight, validate_legacy_object_reference, LegacyAccessAuthorization, LegacyGroupMapping, LegacyPersistenceIndex,
    LegacyRoleMapping, LegacySnapshot,
};
use plan::{build_migration_record_plan, MigrationPlanInput};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Inspect,
    DryRun,
    Migrate,
}

#[derive(Debug)]
struct Cli {
    command: Command,
    vault_root: PathBuf,
}

#[derive(Deserialize, Zeroize)]
#[zeroize(drop)]
#[serde(deny_unknown_fields)]
struct RawAuthorizationEnvelope {
    owner_bearer: String,
    #[zeroize(skip)]
    role_mappings: Vec<LegacyRoleMapping>,
    #[zeroize(skip)]
    group_mappings: Vec<LegacyGroupMapping>,
}

struct AuthorizationEnvelope {
    owner_bearer: Zeroizing<String>,
    role_mappings: Vec<LegacyRoleMapping>,
    group_mappings: Vec<LegacyGroupMapping>,
}

#[derive(Debug, Serialize)]
struct SuccessReport {
    status: &'static str,
    operation: &'static str,
    lock_created: bool,
    source_entries: usize,
    source_streams: usize,
    source_namespace: &'static str,
    target_revisions: Option<usize>,
    target_policies: Option<usize>,
    target_relations: Option<u64>,
    target_root_hash: Option<String>,
    rollback_backup_created: bool,
}

fn main() {
    match run() {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string(&report).expect("success report is serializable")
            );
        }
        Err(category) => {
            eprintln!("{{\"status\":\"rejected\",\"category\":\"{category}\"}}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<SuccessReport, &'static str> {
    let cli = parse_cli(std::env::args().skip(1))?;
    let authorization = read_authorization()?;
    let vault = OfflineMigrationVault::open(&cli.vault_root).map_err(|error| error.category())?;
    let now_secs = unix_time()?.as_secs();
    let credential_bytes = Zeroizing::new(vault.read_agent_tokens_bytes().map_err(|error| error.category())?);
    let credential_bytes_hash = sha256(&credential_bytes);
    let credentials = OfflineCredentialSet::from_bytes(&credential_bytes).map_err(|_| "invalid_authorization")?;
    credentials
        .verify_owner(&authorization.owner_bearer, now_secs)
        .map_err(|_| "invalid_authorization")?;
    let credential_role_cutoff_hash = credentials
        .active_role_cutoff_hash(now_secs)
        .map_err(|_| "invalid_authorization")?;

    let index_bytes = vault.read_legacy_index_bytes().map_err(|_| "legacy_index_read")?;
    let index: LegacyPersistenceIndex = serde_json::from_slice(&index_bytes).map_err(|_| "invalid_legacy_index")?;
    let mut snapshots = Vec::new();
    let mut indexed: Vec<_> = index
        .agents
        .iter()
        .flat_map(|(agent_id, tiers)| tiers.iter().map(move |tier| (agent_id, tier)))
        .collect();
    indexed.sort_by(|(left_agent, left), (right_agent, right)| {
        (left_agent.as_str(), left.tier.as_str(), left.cid.as_str()).cmp(&(
            right_agent.as_str(),
            right.tier.as_str(),
            right.cid.as_str(),
        ))
    });
    for (agent_id, tier) in indexed {
        let bytes = vault
            .read_legacy_object_bytes(&tier.cid)
            .map_err(|_| "legacy_object_read")?;
        snapshots.push(
            LegacySnapshot::decode(
                agent_id.clone(),
                tier.tier.clone(),
                tier.cid.clone(),
                tier.entry_count,
                bytes,
            )
            .map_err(|error| error.category())?,
        );
    }
    let access = LegacyAccessAuthorization {
        authorized_role_ids: credentials.active_role_ids(now_secs),
        role_mappings: authorization.role_mappings,
        group_mappings: authorization.group_mappings,
    };
    let preflight = preflight(&index, &snapshots, &access).map_err(|error| error.category())?;
    let referenced_objects = preflight
        .object_reference_cids()
        .into_iter()
        .map(|cid| {
            let bytes = vault.read_legacy_object_bytes(&cid).map_err(|error| error.category())?;
            validate_legacy_object_reference(&cid, &bytes).map_err(|error| error.category())?;
            Ok(OfflineReferencedObjectFingerprint {
                cid,
                object_envelope_hash: sha256(&bytes),
            })
        })
        .collect::<Result<Vec<_>, &'static str>>()?;
    let source_index_hash = sha256(&index_bytes);
    let source_fingerprint = OfflineSourceFingerprint {
        source_index_hash: source_index_hash.clone(),
        snapshots: preflight
            .source_snapshots()
            .iter()
            .map(|snapshot| OfflineSnapshotFingerprint {
                legacy_agent_id: snapshot.legacy_agent_id.clone(),
                legacy_tier: snapshot.legacy_tier.clone(),
                cid: snapshot.cid.clone(),
                object_envelope_hash: snapshot.object_envelope_hash.clone(),
            })
            .collect(),
        referenced_objects,
    };
    vault
        .revalidate_source(&source_fingerprint)
        .map_err(|_| "legacy_source_changed")?;
    let revalidated_credential_bytes =
        Zeroizing::new(vault.read_agent_tokens_bytes().map_err(|error| error.category())?);
    revalidate_authorization(
        &revalidated_credential_bytes,
        &authorization.owner_bearer,
        &credential_bytes_hash,
        &credential_role_cutoff_hash,
    )
    .map_err(|()| "authorization_changed")?;
    let namespace = match preflight.namespace() {
        legacy::LegacyNamespace::PreNamespace => "pre_namespace",
        legacy::LegacyNamespace::Named(_) => "named_personal_vault",
    };

    match cli.command {
        Command::Inspect => Ok(SuccessReport {
            status: "verified",
            operation: "inspect",
            lock_created: vault.lock_created(),
            source_entries: preflight.entry_count(),
            source_streams: preflight.stream_count(),
            source_namespace: namespace,
            target_revisions: None,
            target_policies: None,
            target_relations: None,
            target_root_hash: None,
            rollback_backup_created: false,
        }),
        Command::DryRun => {
            let plan = build_migration_record_plan(MigrationPlanInput {
                preflight: &preflight,
                lock_created: vault.lock_created(),
                source_index_hash,
                credential_role_cutoff_hash,
                imported_at: unix_time()?
                    .as_millis()
                    .try_into()
                    .map_err(|_| "system_time_overflow")?,
                imported_by_role: plico::PERSONAL_OWNER_ROLE_ID.to_string(),
            })
            .map_err(|error| error.category())?;
            vault
                .revalidate_source(&source_fingerprint)
                .map_err(|_| "legacy_source_changed")?;
            Ok(SuccessReport {
                status: "verified",
                operation: "dry_run",
                lock_created: vault.lock_created(),
                source_entries: preflight.entry_count(),
                source_streams: preflight.stream_count(),
                source_namespace: namespace,
                target_revisions: Some(plan.revisions().len()),
                target_policies: Some(plan.policies().len()),
                target_relations: Some(plan.migration_manifest().target_relation_count),
                target_root_hash: None,
                rollback_backup_created: false,
            })
        }
        Command::Migrate => {
            let imported_at = unix_time()?
                .as_millis()
                .try_into()
                .map_err(|_| "system_time_overflow")?;
            let plan = build_migration_record_plan(MigrationPlanInput {
                preflight: &preflight,
                lock_created: vault.lock_created(),
                source_index_hash,
                credential_role_cutoff_hash: credential_role_cutoff_hash.clone(),
                imported_at,
                imported_by_role: plico::PERSONAL_OWNER_ROLE_ID.to_string(),
            })
            .map_err(|error| error.category())?;
            let mut target = vault.prepare_target().map_err(|error| error.category())?;
            let target_root_hash = build_offline_migration_target(
                &mut target,
                OfflineMigrationTargetInput {
                    source_manifest: plan.source_manifest().clone(),
                    migration_manifest: plan.migration_manifest().clone(),
                    revisions: plan.revisions().to_vec(),
                    policies: plan.policies().to_vec(),
                    committed_at: imported_at,
                    committed_by_role: plico::PERSONAL_OWNER_ROLE_ID.to_string(),
                },
            )
            .map_err(|_| "migration_target_invalid")?;
            let publication = vault
                .publish_target(target, &source_fingerprint, |bytes| {
                    revalidate_authorization(
                        bytes,
                        &authorization.owner_bearer,
                        &credential_bytes_hash,
                        &credential_role_cutoff_hash,
                    )
                })
                .map_err(|error| error.category())?;
            Ok(SuccessReport {
                status: "published",
                operation: "migrate",
                lock_created: vault.lock_created(),
                source_entries: preflight.entry_count(),
                source_streams: preflight.stream_count(),
                source_namespace: namespace,
                target_revisions: Some(plan.revisions().len()),
                target_policies: Some(plan.policies().len()),
                target_relations: Some(0),
                target_root_hash: Some(target_root_hash),
                rollback_backup_created: !publication.backup_name.is_empty(),
            })
        }
    }
}

fn parse_cli(args: impl IntoIterator<Item = String>) -> Result<Cli, &'static str> {
    let mut args = args.into_iter();
    let command = match args.next().as_deref() {
        Some("inspect") => Command::Inspect,
        Some("dry-run") => Command::DryRun,
        Some("migrate") => Command::Migrate,
        _ => return Err("usage"),
    };
    if args.next().as_deref() != Some("--root") {
        return Err("usage");
    }
    let vault_root = args.next().filter(|value| !value.is_empty()).ok_or("usage")?;
    if args.next().is_some() {
        return Err("usage");
    }
    Ok(Cli {
        command,
        vault_root: PathBuf::from(vault_root),
    })
}

fn read_authorization() -> Result<AuthorizationEnvelope, &'static str> {
    let mut bytes = Zeroizing::new(Vec::new());
    std::io::stdin()
        .take(1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|_| "authorization_input")?;
    if bytes.is_empty() || bytes.len() == 1024 * 1024 {
        return Err("authorization_input");
    }
    let mut raw: RawAuthorizationEnvelope = serde_json::from_slice(&bytes).map_err(|_| "authorization_input")?;
    if raw.owner_bearer.is_empty() {
        return Err("invalid_authorization");
    }
    Ok(AuthorizationEnvelope {
        owner_bearer: Zeroizing::new(std::mem::take(&mut raw.owner_bearer)),
        role_mappings: std::mem::take(&mut raw.role_mappings),
        group_mappings: std::mem::take(&mut raw.group_mappings),
    })
}

fn unix_time() -> Result<std::time::Duration, &'static str> {
    SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| "system_time")
}

fn revalidate_authorization(
    bytes: &[u8],
    bearer: &str,
    expected_bytes_hash: &str,
    expected_cutoff_hash: &str,
) -> Result<(), ()> {
    if sha256(bytes) != expected_bytes_hash {
        return Err(());
    }
    let credentials = OfflineCredentialSet::from_bytes(bytes).map_err(|_| ())?;
    let now = unix_time().map_err(|_| ())?.as_secs();
    credentials.verify_owner(bearer, now).map_err(|_| ())?;
    if credentials.active_role_cutoff_hash(now).map_err(|_| ())? != expected_cutoff_hash {
        return Err(());
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_accepts_only_exact_commands() {
        let inspect = parse_cli(["inspect".into(), "--root".into(), "/vault".into()]).unwrap();
        assert_eq!(inspect.command, Command::Inspect);
        assert_eq!(
            parse_cli(["migrate".into(), "--root".into(), "/vault".into()])
                .unwrap()
                .command,
            Command::Migrate
        );
        assert!(parse_cli(["inspect".into(), "--root".into(), "/vault".into(), "extra".into()]).is_err());
    }
}
