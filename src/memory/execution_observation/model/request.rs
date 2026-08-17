use serde::{Deserialize, Serialize};

use super::super::ids::{CanonicalUuid, ExecutionAttemptKeyV1, FixtureOriginV1, TerminalOutcomeV1};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppendStartedRequestV1 {
    pub(crate) schema: String,
    pub(crate) key: ExecutionAttemptKeyV1,
    pub(crate) fixture_origin: FixtureOriginV1,
    pub(crate) attestation_state: String,
    pub(crate) fixture_role_ref: Option<CanonicalUuid>,
    pub(crate) fixture_session_ref: Option<CanonicalUuid>,
    pub(crate) operation_contract_sha256: String,
    pub(crate) input_evidence_cids: Vec<String>,
    pub(crate) context_evidence_cids: Vec<String>,
    pub(crate) policy_sha256: String,
    pub(crate) runtime_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppendTerminalRequestV1 {
    pub(crate) schema: String,
    pub(crate) key: ExecutionAttemptKeyV1,
    pub(crate) attestation_state: String,
    pub(crate) outcome: TerminalOutcomeV1,
    pub(crate) output_evidence_cids: Vec<String>,
    pub(crate) execution_elapsed_ms: Option<u64>,
    pub(crate) policy_sha256: String,
    pub(crate) runtime_sha256: String,
}
