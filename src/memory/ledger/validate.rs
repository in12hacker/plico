use std::collections::{HashMap, HashSet};

use super::model::{CanonicalRevision, LedgerError, PolicyMode, PolicyRecord, POLICY_SCHEMA, REVISION_SCHEMA};

pub(super) fn validate_revisions(records: &[CanonicalRevision]) -> Result<(), LedgerError> {
    let mut by_revision = HashMap::new();
    let mut heads = HashMap::new();
    for (offset, record) in records.iter().enumerate() {
        if record.schema != REVISION_SCHEMA || record.sequence != offset as u64 + 1 {
            return Err(LedgerError::Invalid {
                category: "invalid_revision_sequence",
            });
        }
        if uuid::Uuid::parse_str(record.revision_id.as_str()).is_err()
            || uuid::Uuid::parse_str(record.memory_id.as_str()).is_err()
            || record.committed_by_role.trim().is_empty()
        {
            return Err(LedgerError::Invalid {
                category: "empty_revision_identity",
            });
        }
        if record.content_hash
            != record
                .content
                .canonical_content_hash()
                .map_err(|category| LedgerError::Invalid { category })?
        {
            return Err(LedgerError::Invalid {
                category: "canonical_content_hash_mismatch",
            });
        }
        if by_revision.insert(record.revision_id.clone(), record).is_some() {
            return Err(LedgerError::Invalid {
                category: "duplicate_revision_id",
            });
        }
        match &record.parent_revision_id {
            None => {
                if record.memory_id.as_str() != record.revision_id.as_str() || record.deleted_at.is_some() {
                    return Err(LedgerError::Invalid {
                        category: "invalid_root_revision",
                    });
                }
                if heads
                    .insert(record.memory_id.clone(), record.revision_id.clone())
                    .is_some()
                {
                    return Err(LedgerError::Invalid {
                        category: "duplicate_memory_root",
                    });
                }
            }
            Some(parent_id) => {
                let parent = by_revision.get(parent_id).ok_or(LedgerError::Invalid {
                    category: "missing_parent_revision",
                })?;
                if parent.memory_id != record.memory_id
                    || parent.cognitive_tier != record.cognitive_tier
                    || parent.deleted_at.is_some()
                    || heads.get(&record.memory_id) != Some(parent_id)
                {
                    return Err(LedgerError::Invalid {
                        category: "invalid_parent_revision",
                    });
                }
                if record.deleted_at.is_some()
                    && (serde_json_canonicalizer::to_vec(&record.content).map_err(|_| LedgerError::Invalid {
                        category: "jcs_canonicalization_failed",
                    })? != serde_json_canonicalizer::to_vec(&parent.content).map_err(|_| LedgerError::Invalid {
                        category: "jcs_canonicalization_failed",
                    })? || record.content_hash != parent.content_hash
                        || record.tags != parent.tags
                        || record.memory_type != parent.memory_type
                        || record
                            .content
                            .canonical_content_hash()
                            .map_err(|category| LedgerError::Invalid { category })?
                            != parent.content_hash)
                {
                    return Err(LedgerError::Invalid {
                        category: "tombstone_payload_changed",
                    });
                }
                heads.insert(record.memory_id.clone(), record.revision_id.clone());
            }
        }
    }
    Ok(())
}

pub(super) fn validate_policies(records: &[PolicyRecord], revisions: &[CanonicalRevision]) -> Result<(), LedgerError> {
    let revision_ids: HashSet<_> = revisions.iter().map(|record| &record.revision_id).collect();
    let mut policy_ids = HashSet::new();
    for (offset, record) in records.iter().enumerate() {
        if record.schema != POLICY_SCHEMA
            || record.sequence != offset as u64 + 1
            || uuid::Uuid::parse_str(&record.policy_id).is_err()
            || record.origin_role_id.trim().is_empty()
            || record.committed_by_role.trim().is_empty()
            || !policy_ids.insert(&record.policy_id)
            || !revision_ids.contains(&record.effective_from_revision_id)
        {
            return Err(LedgerError::Invalid {
                category: "invalid_policy_record",
            });
        }
        if record.reader_roles.is_empty()
            || record.writer_roles.is_empty()
            || !is_sorted_unique(&record.reader_roles)
            || !is_sorted_unique(&record.writer_roles)
            || !record
                .reader_roles
                .iter()
                .any(|role| role == crate::PERSONAL_OWNER_ROLE_ID)
            || !record
                .writer_roles
                .iter()
                .any(|role| role == crate::PERSONAL_OWNER_ROLE_ID)
            || !matches!(record.mode, PolicyMode::Private | PolicyMode::ExplicitRoleSet)
        {
            return Err(LedgerError::UnsupportedPolicy {
                category: "policy_log_mode_not_implemented",
            });
        }
        let mut origin_roles = vec![record.origin_role_id.clone(), crate::PERSONAL_OWNER_ROLE_ID.to_string()];
        origin_roles.sort();
        origin_roles.dedup();
        if record.writer_roles != origin_roles
            || (record.mode == PolicyMode::Private && record.reader_roles != origin_roles)
            || (record.mode == PolicyMode::ExplicitRoleSet
                && !origin_roles.iter().all(|role| record.reader_roles.contains(role)))
        {
            return Err(LedgerError::Invalid {
                category: "invalid_policy_audience",
            });
        }
        let revision = revisions
            .iter()
            .find(|revision| revision.revision_id == record.effective_from_revision_id)
            .ok_or(LedgerError::Invalid {
                category: "missing_policy_revision",
            })?;
        if revision.memory_id != record.memory_id {
            return Err(LedgerError::Invalid {
                category: "policy_boundary_mismatch",
            });
        }
    }
    let sequence_by_revision: HashMap<_, _> = revisions
        .iter()
        .map(|revision| (&revision.revision_id, revision.sequence))
        .collect();
    let root_by_memory: HashMap<_, _> = revisions
        .iter()
        .filter(|revision| revision.parent_revision_id.is_none())
        .map(|revision| (&revision.memory_id, &revision.revision_id))
        .collect();
    let mut latest_policy: HashMap<&crate::memory::MemoryId, (&PolicyRecord, u64)> = HashMap::new();
    for policy in records {
        let effective_sequence =
            *sequence_by_revision
                .get(&policy.effective_from_revision_id)
                .ok_or(LedgerError::Invalid {
                    category: "missing_policy_revision",
                })?;
        if let Some((previous, previous_effective_sequence)) = latest_policy.get(&policy.memory_id) {
            if policy.origin_role_id != previous.origin_role_id {
                return Err(LedgerError::Invalid {
                    category: "policy_origin_changed",
                });
            }
            if effective_sequence < *previous_effective_sequence {
                return Err(LedgerError::Invalid {
                    category: "policy_effective_revision_rollback",
                });
            }
            if !previous
                .writer_roles
                .iter()
                .any(|writer| writer == &policy.committed_by_role)
            {
                return Err(LedgerError::Invalid {
                    category: "policy_writer_unauthorized",
                });
            }
        } else if root_by_memory.get(&policy.memory_id) != Some(&&policy.effective_from_revision_id) {
            return Err(LedgerError::Invalid {
                category: "initial_policy_not_effective_from_root",
            });
        }
        latest_policy.insert(&policy.memory_id, (policy, effective_sequence));
    }
    if root_by_memory
        .keys()
        .any(|memory_id| !latest_policy.contains_key(memory_id))
    {
        return Err(LedgerError::Invalid {
            category: "memory_stream_without_policy",
        });
    }
    Ok(())
}

fn is_sorted_unique(values: &[String]) -> bool {
    values.windows(2).all(|window| window[0] < window[1])
}

#[cfg(feature = "offline-migration")]
pub fn validate_migration_record_sets(
    revisions: &[CanonicalRevision],
    policies: &[PolicyRecord],
) -> Result<(), LedgerError> {
    validate_revisions(revisions)?;
    validate_policies(policies, revisions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{CanonicalRevision, MemoryContent, MemoryEntry, MemoryTier};

    fn revisions() -> Vec<CanonicalRevision> {
        let mut root = MemoryEntry::ephemeral("role-a", "root");
        root.tier = MemoryTier::Working;
        let mut root_revision = CanonicalRevision::from_entry(&root).unwrap();
        root_revision.sequence = 1;
        root_revision.committed_at = 10;

        let content = MemoryContent::Text("child".to_string());
        let mut child = root.clone();
        child.id = uuid::Uuid::new_v4().to_string();
        child.parent_revision_id = Some(root.id.as_str().into());
        child.canonical_content_hash = content.canonical_content_hash().unwrap();
        child.content = content;
        let mut child_revision = CanonicalRevision::from_entry(&child).unwrap();
        child_revision.sequence = 2;
        child_revision.committed_at = 20;
        vec![root_revision, child_revision]
    }

    fn policy(sequence: u64, revision: &CanonicalRevision, origin: &str, actor: &str) -> PolicyRecord {
        let mut roles = vec![origin.to_string(), crate::PERSONAL_OWNER_ROLE_ID.to_string()];
        roles.sort();
        roles.dedup();
        PolicyRecord {
            schema: POLICY_SCHEMA.to_string(),
            sequence,
            policy_id: uuid::Uuid::new_v4().to_string(),
            memory_id: revision.memory_id.clone(),
            effective_from_revision_id: revision.revision_id.clone(),
            origin_role_id: origin.to_string(),
            mode: PolicyMode::Private,
            reader_roles: roles.clone(),
            writer_roles: roles,
            committed_at: revision.committed_at,
            committed_by_role: actor.to_string(),
        }
    }

    #[test]
    fn initial_policy_must_start_at_stream_root() {
        let revisions = revisions();
        let policies = vec![policy(1, &revisions[1], "role-a", "role-a")];
        assert!(matches!(
            validate_policies(&policies, &revisions),
            Err(LedgerError::Invalid {
                category: "initial_policy_not_effective_from_root"
            })
        ));
    }

    #[test]
    fn later_policy_preserves_origin_and_requires_current_writer() {
        let revisions = revisions();
        let first = policy(1, &revisions[0], "role-a", "role-a");
        let changed_origin = policy(2, &revisions[1], "role-b", crate::PERSONAL_OWNER_ROLE_ID);
        assert!(matches!(
            validate_policies(&[first.clone(), changed_origin], &revisions),
            Err(LedgerError::Invalid {
                category: "policy_origin_changed"
            })
        ));

        let unauthorized = policy(2, &revisions[1], "role-a", "intruder");
        assert!(matches!(
            validate_policies(&[first.clone(), unauthorized], &revisions),
            Err(LedgerError::Invalid {
                category: "policy_writer_unauthorized"
            })
        ));

        let valid = policy(2, &revisions[1], "role-a", crate::PERSONAL_OWNER_ROLE_ID);
        validate_policies(&[first, valid], &revisions).unwrap();
    }
}
