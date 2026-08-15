//! Exact legacy memory snapshot types and fail-closed migration preflight.
//!
//! This module is deliberately isolated in the offline migrator. Runtime code
//! must never deserialize these types or use them as a compatibility reader.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyPersistenceIndex {
    pub agents: HashMap<String, Vec<LegacyPersistedTier>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyPersistedTier {
    pub tier: String,
    pub cid: String,
    pub entry_count: usize,
}

/// One decoded legacy CAS snapshot plus the index facts that referenced it.
#[derive(Debug, Clone)]
pub struct LegacySnapshot {
    index_agent_id: String,
    index_tier: String,
    cid: String,
    object_envelope_hash: String,
    declared_entry_count: usize,
    /// Exact bytes stored as the legacy CAS object's data payload.
    encoded_entries: Vec<u8>,
    entries: Vec<LegacyMemoryEntry>,
}

impl LegacySnapshot {
    /// Decode the exact old CAS envelope, verify its content address, then
    /// decode the exact legacy entry array carried in `data`.
    pub fn decode(
        index_agent_id: String,
        index_tier: String,
        cid: String,
        declared_entry_count: usize,
        encoded_object: Vec<u8>,
    ) -> Result<Self, LegacyPreflightError> {
        let object_envelope_hash = format!("{:x}", Sha256::digest(&encoded_object));
        let object: LegacyCasObject = serde_json::from_slice(&encoded_object)
            .map_err(|_| LegacyPreflightError::Rejected("invalid_legacy_cas_envelope"))?;
        let actual = format!("{:x}", Sha256::digest(&object.data));
        if cid != actual || object.cid != cid {
            return reject("legacy_cas_hash_mismatch");
        }
        let entries = serde_json::from_slice(&object.data)
            .map_err(|_| LegacyPreflightError::Rejected("invalid_legacy_snapshot_json"))?;
        Ok(Self {
            index_agent_id,
            index_tier,
            cid: actual,
            object_envelope_hash,
            declared_entry_count,
            encoded_entries: object.data,
            entries,
        })
    }

    #[cfg(test)]
    pub(crate) fn entries(&self) -> &[LegacyMemoryEntry] {
        &self.entries
    }

    #[cfg(test)]
    pub(crate) fn cid(&self) -> &str {
        &self.cid
    }
}

pub fn validate_legacy_object_reference(cid: &str, encoded_object: &[u8]) -> Result<(), LegacyPreflightError> {
    let object: LegacyCasObject = serde_json::from_slice(encoded_object)
        .map_err(|_| LegacyPreflightError::Rejected("invalid_object_reference_envelope"))?;
    if cid.len() != 64
        || !cid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || object.cid != cid
        || format!("{:x}", Sha256::digest(&object.data)) != cid
    {
        return reject("invalid_object_reference_cid");
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacyCasObject {
    cid: String,
    data: Vec<u8>,
    meta: LegacyCasObjectMeta,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacyCasObjectMeta {
    content_type: LegacyContentType,
    tags: Vec<String>,
    created_by: String,
    created_at: u64,
    intent: Option<String>,
    #[serde(default)]
    tenant_id: String,
    #[serde(default)]
    scope: LegacyObjectScope,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
enum LegacyContentType {
    Text,
    Image,
    Audio,
    Video,
    Structured,
    Binary,
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum LegacyObjectScope {
    Private,
    #[default]
    Shared,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LegacyMemoryEntry {
    pub id: String,
    pub agent_id: String,
    /// `None` means the field was absent in a pre-namespace snapshot. Keeping
    /// this distinction prevents silently mixing it with an explicit value.
    #[serde(default, skip_serializing_if = "LegacyTenantId::is_absent")]
    pub tenant_id: LegacyTenantId,
    pub tier: LegacyMemoryTier,
    pub content: LegacyMemoryContent,
    pub importance: u8,
    pub access_count: u32,
    pub last_accessed: u64,
    pub created_at: u64,
    pub tags: Vec<String>,
    pub embedding: Option<Vec<f32>>,
    #[serde(default)]
    pub ttl_ms: Option<u64>,
    #[serde(default)]
    pub original_ttl_ms: Option<u64>,
    /// Absence has the exact old serde meaning: `Private`.
    #[serde(default)]
    pub scope: LegacyMemoryScope,
    #[serde(default)]
    pub memory_type: LegacyMemoryType,
    #[serde(default)]
    pub causal_parent: Option<String>,
    #[serde(default)]
    pub supersedes: Option<String>,
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub deleted_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum LegacyMemoryTier {
    Ephemeral,
    Working,
    LongTerm,
    Procedural,
}

/// Distinguishes an absent pre-namespace field from an explicit string. JSON
/// `null` is rejected because the legacy field itself was a `String`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyTenantId(Option<String>);

impl LegacyTenantId {
    fn named(value: impl Into<String>) -> Self {
        Self(Some(value.into()))
    }

    pub(crate) fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }

    fn is_absent(&self) -> bool {
        self.0.is_none()
    }
}

impl<'de> Deserialize<'de> for LegacyTenantId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::named)
    }
}

impl Serialize for LegacyTenantId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self.0 {
            Some(value) => serializer.serialize_str(value),
            None => serializer.serialize_none(),
        }
    }
}

impl LegacyMemoryTier {
    fn index_name(self) -> &'static str {
        match self {
            Self::Ephemeral => "ephemeral",
            Self::Working => "working",
            Self::LongTerm => "long_term",
            Self::Procedural => "procedural",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub enum LegacyMemoryContent {
    Text(String),
    ObjectRef(String),
    Structured(serde_json::Value),
    Procedure(LegacyProcedure),
    Knowledge(LegacyKnowledgePiece),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LegacyProcedure {
    pub name: String,
    pub description: String,
    pub steps: Vec<LegacyProcedureStep>,
    pub learned_from: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LegacyProcedureStep {
    pub step_number: u32,
    pub description: String,
    pub action: String,
    pub expected_outcome: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LegacyKnowledgePiece {
    pub subject: String,
    pub statement: String,
    pub confidence: f32,
    pub source: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum LegacyMemoryScope {
    #[default]
    Private,
    Shared,
    Group(String),
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum LegacyMemoryType {
    Episodic,
    Semantic,
    Procedural,
    #[default]
    Untyped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyNamespace {
    PreNamespace,
    Named(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LegacyPreflightReport {
    namespace: LegacyNamespace,
    entry_count: usize,
    stream_count: usize,
    agent_ids: BTreeSet<String>,
    /// Legacy revision ID -> stable memory ID (the verified chain root ID).
    memory_ids: BTreeMap<String, String>,
    streams: Vec<LegacyStream>,
    entries: BTreeMap<String, LegacyMemoryEntry>,
    source_snapshots: Vec<LegacySourceSnapshot>,
    authorized_role_ids: Vec<String>,
    role_mappings: Vec<LegacyRoleMapping>,
    group_mappings: Vec<LegacyGroupMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyStream {
    pub memory_id: String,
    /// Parent-first order, ending at the sole verified head.
    pub revision_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LegacySourceSnapshot {
    pub legacy_agent_id: String,
    pub legacy_tier: String,
    pub cid: String,
    pub object_envelope_hash: String,
    pub entry_count: usize,
}

/// Owner-authorized local-role resolution captured before preflight.
///
/// All vectors must already be strict sorted/unique. Preflight rejects rather
/// than normalizes them so the signed source evidence has one interpretation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyAccessAuthorization {
    pub authorized_role_ids: Vec<String>,
    pub role_mappings: Vec<LegacyRoleMapping>,
    pub group_mappings: Vec<LegacyGroupMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRoleMapping {
    pub legacy_agent_id: String,
    pub target_role_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyGroupMapping {
    pub legacy_group_id: String,
    pub target_role_ids: Vec<String>,
}

impl LegacyPreflightReport {
    pub(crate) fn namespace(&self) -> &LegacyNamespace {
        &self.namespace
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.entry_count
    }

    pub(crate) fn stream_count(&self) -> usize {
        self.stream_count
    }

    pub(crate) fn streams(&self) -> &[LegacyStream] {
        &self.streams
    }

    pub(crate) fn entry(&self, revision_id: &str) -> Option<&LegacyMemoryEntry> {
        self.entries.get(revision_id)
    }

    pub(crate) fn source_snapshots(&self) -> &[LegacySourceSnapshot] {
        &self.source_snapshots
    }

    pub(crate) fn authorized_role_ids(&self) -> &[String] {
        &self.authorized_role_ids
    }

    pub(crate) fn role_mappings(&self) -> &[LegacyRoleMapping] {
        &self.role_mappings
    }

    pub(crate) fn group_mappings(&self) -> &[LegacyGroupMapping] {
        &self.group_mappings
    }

    pub(crate) fn ttl_field_count(&self) -> usize {
        self.entries
            .values()
            .map(|entry| usize::from(entry.ttl_ms.is_some()) + usize::from(entry.original_ttl_ms.is_some()))
            .sum()
    }

    pub(crate) fn embedded_vector_count(&self) -> usize {
        self.entries.values().filter(|entry| entry.embedding.is_some()).count()
    }

    pub(crate) fn object_reference_cids(&self) -> Vec<String> {
        let mut cids: Vec<_> = self
            .entries
            .values()
            .filter_map(|entry| match &entry.content {
                LegacyMemoryContent::ObjectRef(cid) => Some(cid.clone()),
                _ => None,
            })
            .collect();
        cids.sort();
        cids.dedup();
        cids
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LegacyPreflightError {
    #[error("legacy migration rejected: {0}")]
    Rejected(&'static str),
}

impl LegacyPreflightError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::Rejected(category) => category,
        }
    }
}

/// Validate already-decoded legacy snapshots without reading or writing disk.
pub fn preflight(
    index: &LegacyPersistenceIndex,
    snapshots: &[LegacySnapshot],
    authorization: &LegacyAccessAuthorization,
) -> Result<LegacyPreflightReport, LegacyPreflightError> {
    validate_index(index, snapshots)?;
    let mut entries = HashMap::<&str, &LegacyMemoryEntry>::new();
    let mut namespace: Option<Option<&str>> = None;
    let mut agent_ids = BTreeSet::new();

    for snapshot in snapshots {
        validate_snapshot_envelope(snapshot)?;
        for entry in &snapshot.entries {
            if uuid::Uuid::parse_str(&entry.id).is_err() {
                return reject("invalid_revision_id");
            }
            if entry.agent_id.trim().is_empty() {
                return reject("empty_legacy_agent_id");
            }
            if entry.tenant_id.as_deref().is_some_and(str::is_empty) {
                return reject("empty_legacy_namespace");
            }
            match namespace {
                None => namespace = Some(entry.tenant_id.as_deref()),
                Some(current) if current != entry.tenant_id.as_deref() => {
                    return reject("multiple_legacy_namespaces");
                }
                Some(_) => {}
            }
            if entry.causal_parent.is_some() {
                return reject("unsupported_causal_relation");
            }
            validate_canonicalizable_content(&entry.content)?;
            if entries.insert(entry.id.as_str(), entry).is_some() {
                return reject("duplicate_revision_id");
            }
            agent_ids.insert(entry.agent_id.clone());
        }
    }

    let mut child_counts = HashMap::<&str, usize>::new();
    for entry in entries.values() {
        if let Some(parent_id) = entry.supersedes.as_deref() {
            *child_counts.entry(parent_id).or_default() += 1;
        }
    }
    if child_counts.values().any(|count| *count > 1) {
        return reject("branched_supersession_chain");
    }
    for entry in entries.values() {
        validate_links(entry, &entries)?;
    }

    let mut memory_ids = BTreeMap::new();
    let mut visited = HashSet::new();
    let mut roots: Vec<_> = entries
        .values()
        .copied()
        .filter(|entry| entry.supersedes.is_none())
        .collect();
    roots.sort_by(|left, right| left.id.cmp(&right.id));
    let mut streams = Vec::with_capacity(roots.len());
    for root in &roots {
        let mut current = *root;
        let mut revision_ids = Vec::new();
        loop {
            if !visited.insert(current.id.as_str()) {
                return reject("supersession_cycle");
            }
            memory_ids.insert(current.id.clone(), root.id.clone());
            revision_ids.push(current.id.clone());
            match current.superseded_by.as_deref() {
                Some(child_id) => current = entries[child_id],
                None => break,
            }
        }
        if revision_ids[..revision_ids.len().saturating_sub(1)]
            .iter()
            .any(|revision_id| entries[revision_id.as_str()].deleted_at.is_some())
        {
            return reject("deleted_non_head_revision");
        }
        streams.push(LegacyStream {
            memory_id: root.id.clone(),
            revision_ids,
        });
    }
    if visited.len() != entries.len() {
        return reject("supersession_cycle");
    }

    validate_access_authorization(authorization, &agent_ids, entries.values().copied())?;

    let mut source_snapshots: Vec<_> = snapshots
        .iter()
        .map(|snapshot| LegacySourceSnapshot {
            legacy_agent_id: snapshot.index_agent_id.clone(),
            legacy_tier: snapshot.index_tier.clone(),
            cid: snapshot.cid.clone(),
            object_envelope_hash: snapshot.object_envelope_hash.clone(),
            entry_count: snapshot.declared_entry_count,
        })
        .collect();
    source_snapshots.sort_by(|left, right| {
        (&left.legacy_agent_id, &left.legacy_tier, &left.cid).cmp(&(
            &right.legacy_agent_id,
            &right.legacy_tier,
            &right.cid,
        ))
    });

    Ok(LegacyPreflightReport {
        namespace: match namespace.flatten() {
            Some(value) => LegacyNamespace::Named(value.to_string()),
            None => LegacyNamespace::PreNamespace,
        },
        entry_count: entries.len(),
        stream_count: roots.len(),
        agent_ids,
        memory_ids,
        streams,
        entries: entries
            .into_iter()
            .map(|(revision_id, entry)| (revision_id.to_string(), entry.clone()))
            .collect(),
        source_snapshots,
        authorized_role_ids: authorization.authorized_role_ids.clone(),
        role_mappings: authorization.role_mappings.clone(),
        group_mappings: authorization.group_mappings.clone(),
    })
}

fn validate_access_authorization<'a>(
    authorization: &LegacyAccessAuthorization,
    agent_ids: &BTreeSet<String>,
    entries: impl IntoIterator<Item = &'a LegacyMemoryEntry>,
) -> Result<(), LegacyPreflightError> {
    if !strict_sorted_nonempty(&authorization.authorized_role_ids) {
        return reject("invalid_authorized_role_set");
    }
    let authorized: HashSet<_> = authorization.authorized_role_ids.iter().map(String::as_str).collect();

    if !authorization
        .role_mappings
        .windows(2)
        .all(|pair| pair[0].legacy_agent_id < pair[1].legacy_agent_id)
    {
        return reject("invalid_role_mapping");
    }
    let mut mapped_agents = BTreeSet::new();
    for mapping in &authorization.role_mappings {
        if mapping.legacy_agent_id.trim().is_empty()
            || mapping.target_role_id.trim().is_empty()
            || !authorized.contains(mapping.target_role_id.as_str())
            || !mapped_agents.insert(mapping.legacy_agent_id.clone())
        {
            return reject("invalid_role_mapping");
        }
    }
    if &mapped_agents != agent_ids {
        return reject("incomplete_role_mapping");
    }

    let mut required_groups = BTreeSet::new();
    for entry in entries {
        if let LegacyMemoryScope::Group(group_id) = &entry.scope {
            if group_id.trim().is_empty() {
                return reject("unresolved_group_audience");
            }
            required_groups.insert(group_id.clone());
        }
    }
    if !authorization
        .group_mappings
        .windows(2)
        .all(|pair| pair[0].legacy_group_id < pair[1].legacy_group_id)
    {
        return reject("unresolved_group_audience");
    }
    let mut mapped_groups = BTreeSet::new();
    for mapping in &authorization.group_mappings {
        if mapping.legacy_group_id.trim().is_empty()
            || !strict_sorted_nonempty(&mapping.target_role_ids)
            || mapping
                .target_role_ids
                .iter()
                .any(|role| !authorized.contains(role.as_str()))
            || !mapped_groups.insert(mapping.legacy_group_id.clone())
        {
            return reject("unresolved_group_audience");
        }
    }
    if mapped_groups != required_groups {
        return reject("unresolved_group_audience");
    }
    Ok(())
}

fn strict_sorted_nonempty(values: &[String]) -> bool {
    !values.is_empty()
        && values.iter().all(|value| !value.trim().is_empty())
        && values.windows(2).all(|pair| pair[0] < pair[1])
}

fn validate_index(index: &LegacyPersistenceIndex, snapshots: &[LegacySnapshot]) -> Result<(), LegacyPreflightError> {
    let mut expected = HashMap::new();
    for (agent_id, tiers) in &index.agents {
        if agent_id.trim().is_empty() {
            return reject("invalid_snapshot_index_entry");
        }
        for tier in tiers {
            if !matches!(tier.tier.as_str(), "working" | "long_term" | "procedural") {
                return reject("unsupported_persisted_tier");
            }
            if tier.cid.len() != 64
                || !tier
                    .cid
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return reject("invalid_legacy_cid");
            }
            if expected.insert((agent_id.as_str(), tier.tier.as_str()), tier).is_some() {
                return reject("duplicate_persisted_tier");
            }
        }
    }

    let mut observed = HashSet::new();
    for snapshot in snapshots {
        let key = (snapshot.index_agent_id.as_str(), snapshot.index_tier.as_str());
        let persisted = expected
            .get(&key)
            .ok_or(LegacyPreflightError::Rejected("unindexed_legacy_snapshot"))?;
        if persisted.cid != snapshot.cid || persisted.entry_count != snapshot.declared_entry_count {
            return reject("snapshot_index_mismatch");
        }
        if !observed.insert(key) {
            return reject("duplicate_legacy_snapshot");
        }
    }
    if observed.len() != expected.len() {
        return reject("missing_legacy_snapshot");
    }
    Ok(())
}

fn validate_snapshot_envelope(snapshot: &LegacySnapshot) -> Result<(), LegacyPreflightError> {
    if snapshot.index_agent_id.trim().is_empty() || snapshot.cid.trim().is_empty() {
        return reject("invalid_snapshot_index_entry");
    }
    if snapshot.declared_entry_count != snapshot.entries.len() {
        return reject("snapshot_entry_count_mismatch");
    }
    if format!("{:x}", Sha256::digest(&snapshot.encoded_entries)) != snapshot.cid {
        return reject("legacy_cas_hash_mismatch");
    }
    if !matches!(snapshot.index_tier.as_str(), "working" | "long_term" | "procedural") {
        return reject("unsupported_persisted_tier");
    }
    if snapshot
        .entries
        .iter()
        .any(|entry| entry.agent_id != snapshot.index_agent_id || entry.tier.index_name() != snapshot.index_tier)
    {
        return reject("snapshot_boundary_mismatch");
    }
    Ok(())
}

const MAX_JCS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn validate_canonicalizable_content(content: &LegacyMemoryContent) -> Result<(), LegacyPreflightError> {
    match content {
        LegacyMemoryContent::Structured(value) => validate_jcs_value(value),
        LegacyMemoryContent::Knowledge(knowledge) if !knowledge.confidence.is_finite() => {
            reject("non_finite_knowledge_confidence")
        }
        LegacyMemoryContent::Text(_)
        | LegacyMemoryContent::ObjectRef(_)
        | LegacyMemoryContent::Procedure(_)
        | LegacyMemoryContent::Knowledge(_) => Ok(()),
    }
}

fn validate_jcs_value(value: &serde_json::Value) -> Result<(), LegacyPreflightError> {
    match value {
        serde_json::Value::Number(number)
            if number.as_u64().is_some_and(|value| value > MAX_JCS_SAFE_INTEGER)
                || number
                    .as_i64()
                    .is_some_and(|value| value.unsigned_abs() > MAX_JCS_SAFE_INTEGER) =>
        {
            reject("jcs_unsafe_integer")
        }
        serde_json::Value::Array(values) => {
            for value in values {
                validate_jcs_value(value)?;
            }
            Ok(())
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                validate_jcs_value(value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_links<'a>(
    entry: &'a LegacyMemoryEntry,
    entries: &HashMap<&'a str, &'a LegacyMemoryEntry>,
) -> Result<(), LegacyPreflightError> {
    if entry.supersedes.as_deref() == Some(entry.id.as_str())
        || entry.superseded_by.as_deref() == Some(entry.id.as_str())
    {
        return reject("self_supersession_link");
    }
    if let Some(parent_id) = entry.supersedes.as_deref() {
        let parent = entries
            .get(parent_id)
            .ok_or(LegacyPreflightError::Rejected("missing_supersession_revision"))?;
        if parent.superseded_by.as_deref() != Some(entry.id.as_str()) {
            return reject("asymmetric_supersession_link");
        }
        validate_link_boundary(entry, parent)?;
    }
    if let Some(child_id) = entry.superseded_by.as_deref() {
        let child = entries
            .get(child_id)
            .ok_or(LegacyPreflightError::Rejected("missing_supersession_revision"))?;
        if child.supersedes.as_deref() != Some(entry.id.as_str()) {
            return reject("asymmetric_supersession_link");
        }
        validate_link_boundary(entry, child)?;
    }
    Ok(())
}

fn validate_link_boundary(left: &LegacyMemoryEntry, right: &LegacyMemoryEntry) -> Result<(), LegacyPreflightError> {
    if left.agent_id != right.agent_id {
        return reject("cross_role_supersession_link");
    }
    if left.tenant_id != right.tenant_id {
        return reject("cross_namespace_supersession_link");
    }
    if left.tier != right.tier {
        return reject("cross_tier_supersession_link");
    }
    Ok(())
}

fn reject<T>(category: &'static str) -> Result<T, LegacyPreflightError> {
    Err(LegacyPreflightError::Rejected(category))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision_id(label: &str) -> String {
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, label.as_bytes()).to_string()
    }

    fn entry(label: &str) -> LegacyMemoryEntry {
        LegacyMemoryEntry {
            id: revision_id(label),
            agent_id: "conversation".into(),
            tenant_id: LegacyTenantId::named("default"),
            tier: LegacyMemoryTier::Working,
            content: LegacyMemoryContent::Text(format!("content-{label}")),
            importance: 50,
            access_count: 0,
            last_accessed: 10,
            created_at: 10,
            tags: vec![],
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: LegacyMemoryScope::Private,
            memory_type: LegacyMemoryType::Untyped,
            causal_parent: None,
            supersedes: None,
            superseded_by: None,
            deleted_at: None,
        }
    }

    fn snapshot(entries: Vec<LegacyMemoryEntry>) -> LegacySnapshot {
        let encoded_entries = serde_json::to_vec(&entries).unwrap();
        let cid = format!("{:x}", Sha256::digest(&encoded_entries));
        let encoded_object = serde_json::to_vec(&LegacyCasObject {
            cid: cid.clone(),
            data: encoded_entries,
            meta: LegacyCasObjectMeta {
                content_type: LegacyContentType::Structured,
                tags: vec!["memory".into()],
                created_by: "plico:memory-persister".into(),
                created_at: 1,
                intent: None,
                tenant_id: "default".into(),
                scope: LegacyObjectScope::Private,
            },
        })
        .unwrap();
        LegacySnapshot::decode(
            "conversation".into(),
            "working".into(),
            cid,
            entries.len(),
            encoded_object,
        )
        .unwrap()
    }

    fn authorization() -> LegacyAccessAuthorization {
        LegacyAccessAuthorization {
            authorized_role_ids: vec!["conversation-role".into(), "personal-owner".into()],
            role_mappings: vec![LegacyRoleMapping {
                legacy_agent_id: "conversation".into(),
                target_role_id: "conversation-role".into(),
            }],
            group_mappings: vec![],
        }
    }

    fn run_with_authorization(
        snapshots: &[LegacySnapshot],
        authorization: &LegacyAccessAuthorization,
    ) -> Result<LegacyPreflightReport, LegacyPreflightError> {
        let mut agents: HashMap<String, Vec<LegacyPersistedTier>> = HashMap::new();
        for snapshot in snapshots {
            agents
                .entry(snapshot.index_agent_id.clone())
                .or_default()
                .push(LegacyPersistedTier {
                    tier: snapshot.index_tier.clone(),
                    cid: snapshot.cid.clone(),
                    entry_count: snapshot.declared_entry_count,
                });
        }
        preflight(&LegacyPersistenceIndex { agents }, snapshots, authorization)
    }

    fn run(snapshots: &[LegacySnapshot]) -> Result<LegacyPreflightReport, LegacyPreflightError> {
        run_with_authorization(snapshots, &authorization())
    }

    #[test]
    fn accepts_isolated_and_linear_streams() {
        let isolated = entry("isolated");
        let mut root = entry("root");
        root.superseded_by = Some(revision_id("child"));
        let mut child = entry("child");
        child.supersedes = Some(revision_id("root"));

        let report = run(&[snapshot(vec![isolated, root, child])]).unwrap();
        assert_eq!(report.entry_count, 3);
        assert_eq!(report.stream_count, 2);
        assert_eq!(report.memory_ids[&revision_id("child")], revision_id("root"));
    }

    #[test]
    fn rejects_mixed_namespace_presence() {
        let explicit = entry("explicit");
        let mut absent = entry("absent");
        absent.tenant_id = LegacyTenantId::default();
        assert_eq!(
            run(&[snapshot(vec![explicit, absent])]).unwrap_err().category(),
            "multiple_legacy_namespaces"
        );
    }

    #[test]
    fn rejects_unmapped_group_and_causal_without_guessing() {
        let mut grouped = entry("grouped");
        grouped.scope = LegacyMemoryScope::Group("research".into());
        assert_eq!(
            run(&[snapshot(vec![grouped])]).unwrap_err().category(),
            "unresolved_group_audience"
        );

        let mut causal = entry("causal");
        causal.causal_parent = Some(revision_id("source"));
        assert_eq!(
            run(&[snapshot(vec![causal])]).unwrap_err().category(),
            "unsupported_causal_relation"
        );
    }

    #[test]
    fn rejects_bad_supersession_graphs() {
        let mut orphan = entry("orphan");
        orphan.supersedes = Some(revision_id("missing"));
        assert_eq!(
            run(&[snapshot(vec![orphan])]).unwrap_err().category(),
            "missing_supersession_revision"
        );

        let mut left = entry("left");
        left.superseded_by = Some(revision_id("right"));
        let mut right = entry("right");
        right.superseded_by = Some(revision_id("left"));
        assert_eq!(
            run(&[snapshot(vec![left, right])]).unwrap_err().category(),
            "asymmetric_supersession_link"
        );
    }

    #[test]
    fn legacy_dto_denies_new_or_unknown_fields() {
        let value = serde_json::json!({
            "id": "r1", "agent_id": "conversation", "tier": "Working",
            "content": {"Text": "x"}, "importance": 50, "access_count": 0,
            "last_accessed": 1, "created_at": 1, "tags": [], "embedding": null,
            "memory_id": "must-not-be-read-as-legacy"
        });
        assert!(serde_json::from_value::<LegacyMemoryEntry>(value).is_err());

        let null_namespace = serde_json::json!({
            "id": "r1", "agent_id": "conversation", "tenant_id": null,
            "tier": "Working", "content": {"Text": "x"}, "importance": 50,
            "access_count": 0, "last_accessed": 1, "created_at": 1,
            "tags": [], "embedding": null
        });
        assert!(serde_json::from_value::<LegacyMemoryEntry>(null_namespace).is_err());

        let index_with_unknown = serde_json::json!({"agents": {}, "schema_version": 1});
        assert!(serde_json::from_value::<LegacyPersistenceIndex>(index_with_unknown).is_err());
    }

    #[test]
    fn decode_rejects_bad_cas_hash() {
        let valid = snapshot(vec![entry("r1")]);
        let encoded = serde_json::to_vec(&LegacyCasObject {
            cid: valid.cid.clone(),
            data: valid.encoded_entries.clone(),
            meta: LegacyCasObjectMeta {
                content_type: LegacyContentType::Structured,
                tags: vec![],
                created_by: "test".into(),
                created_at: 1,
                intent: None,
                tenant_id: "default".into(),
                scope: LegacyObjectScope::Private,
            },
        })
        .unwrap();
        assert_eq!(
            LegacySnapshot::decode("conversation".into(), "working".into(), "0".repeat(64), 1, encoded,)
                .unwrap_err()
                .category(),
            "legacy_cas_hash_mismatch"
        );
    }

    #[test]
    fn rejects_structured_integer_outside_jcs_safe_range() {
        for integer in [
            serde_json::json!(9_007_199_254_740_992_u64),
            serde_json::json!(-9_007_199_254_740_992_i64),
        ] {
            let mut unsafe_entry = entry("unsafe");
            unsafe_entry.content = LegacyMemoryContent::Structured(serde_json::json!({"integer": integer}));
            assert_eq!(
                run(&[snapshot(vec![unsafe_entry])]).unwrap_err().category(),
                "jcs_unsafe_integer"
            );
        }
    }

    #[test]
    fn accepts_jcs_safe_integer_boundaries() {
        for integer in [
            serde_json::json!(9_007_199_254_740_991_u64),
            serde_json::json!(-9_007_199_254_740_991_i64),
        ] {
            let mut safe_entry = entry("safe");
            safe_entry.content = LegacyMemoryContent::Structured(serde_json::json!({"integer": integer}));
            run(&[snapshot(vec![safe_entry])]).unwrap();
        }
    }

    #[test]
    fn rejects_verified_cycle_and_branch() {
        let mut a = entry("a");
        a.supersedes = Some(revision_id("b"));
        a.superseded_by = Some(revision_id("b"));
        let mut b = entry("b");
        b.supersedes = Some(revision_id("a"));
        b.superseded_by = Some(revision_id("a"));
        assert_eq!(
            run(&[snapshot(vec![a, b])]).unwrap_err().category(),
            "supersession_cycle"
        );

        let mut root = entry("root");
        root.superseded_by = Some(revision_id("left"));
        let mut left = entry("left");
        left.supersedes = Some(revision_id("root"));
        let mut right = entry("right");
        right.supersedes = Some(revision_id("root"));
        assert_eq!(
            run(&[snapshot(vec![root, left, right])]).unwrap_err().category(),
            "branched_supersession_chain"
        );
    }

    #[test]
    fn rejects_duplicate_revision_id() {
        assert_eq!(
            run(&[snapshot(vec![entry("same"), entry("same")])])
                .unwrap_err()
                .category(),
            "duplicate_revision_id"
        );
    }

    #[test]
    fn accepts_only_an_explicit_sorted_group_mapping() {
        let mut grouped = entry("grouped");
        grouped.scope = LegacyMemoryScope::Group("research".into());
        let mut auth = authorization();
        auth.authorized_role_ids = vec![
            "conversation-role".into(),
            "personal-owner".into(),
            "research-role".into(),
        ];
        auth.group_mappings.push(LegacyGroupMapping {
            legacy_group_id: "research".into(),
            target_role_ids: vec!["research-role".into()],
        });
        assert!(run_with_authorization(&[snapshot(vec![grouped.clone()])], &auth).is_ok());

        auth.group_mappings[0].target_role_ids = vec!["research-role".into(), "conversation-role".into()];
        assert_eq!(
            run_with_authorization(&[snapshot(vec![grouped])], &auth)
                .unwrap_err()
                .category(),
            "unresolved_group_audience"
        );
    }
}
