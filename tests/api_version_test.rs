//! API Versioning Tests (v26.0)
//!
//! Tests for the API versioning system including:
//! - ApiVersion parsing and comparison
//! - Feature support checks
//! - Version features

use plico::api::semantic::{
    ApiVersion, ApiRequest, ApiResponse, VersionFeatures,
    version_supports,
};

#[test]
fn test_api_version_parsing() {
    let v = ApiVersion::parse("1.2.3").unwrap();
    assert_eq!(v.major, 1);
    assert_eq!(v.minor, 2);
    assert_eq!(v.patch, 3);
}

#[test]
fn test_api_version_parse_invalid() {
    assert!(ApiVersion::parse("invalid").is_err());
    assert!(ApiVersion::parse("1.2").is_err());
    assert!(ApiVersion::parse("1.2.3.4").is_err());
    assert!(ApiVersion::parse("a.b.c").is_err());
}

#[test]
fn test_version_constants() {
    assert_eq!(ApiVersion::V1.major, 1);
    assert_eq!(ApiVersion::V1.minor, 0);
    assert_eq!(ApiVersion::V1.patch, 0);
    assert_eq!(ApiVersion::CURRENT.major, 26);
}

#[test]
fn test_version_display() {
    let v = ApiVersion::parse("1.2.0").unwrap();
    assert_eq!(format!("{}", v), "1.2.0");
}

#[test]
fn test_version_from_str() {
    let v: ApiVersion = "2.0.0".parse().unwrap();
    assert_eq!(v.major, 2);
    assert_eq!(v.minor, 0);
    assert_eq!(v.patch, 0);
}

#[test]
fn test_version_comparison() {
    let v1 = ApiVersion::parse("1.0.0").unwrap();
    let v2 = ApiVersion::parse("2.0.0").unwrap();
    let v3 = ApiVersion::parse("1.1.0").unwrap();
    assert!(v1 < v2);
    assert!(v1 < v3);
    assert!(v3 < v2);
    assert!(v2 > v1);
    assert!(v1 <= v1);
    assert!(v1 >= v1);
}

#[test]
fn test_version_supports_feature() {
    let v13 = ApiVersion::parse("13.0.0").unwrap();
    let v14 = ApiVersion::parse("14.0.0").unwrap();
    let v15 = ApiVersion::parse("15.0.0").unwrap();
    let v16 = ApiVersion::parse("16.0.0").unwrap();
    let v17 = ApiVersion::parse("17.0.0").unwrap();
    let v18 = ApiVersion::parse("18.0.0").unwrap();
    let v19 = ApiVersion::parse("19.0.0").unwrap();
    let v20 = ApiVersion::parse("20.0.0").unwrap();
    let v21 = ApiVersion::parse("21.0.0").unwrap();
    let v22 = ApiVersion::parse("22.0.0").unwrap();
    let v23 = ApiVersion::parse("23.0.0").unwrap();
    let v24 = ApiVersion::parse("24.0.0").unwrap();
    let v25 = ApiVersion::parse("25.0.0").unwrap();
    let v26 = ApiVersion::parse("26.0.0").unwrap();

    assert!(!v13.supports("batch_operations"));
    assert!(!v14.supports("batch_operations"));
    assert!(v15.supports("batch_operations"));
    assert!(v16.supports("batch_operations"));
    assert!(v17.supports("batch_operations"));
    assert!(v18.supports("batch_operations"));
    assert!(v19.supports("batch_operations"));
    assert!(v20.supports("batch_operations"));
    assert!(v21.supports("batch_operations"));
    assert!(v22.supports("batch_operations"));
    assert!(v23.supports("batch_operations"));
    assert!(v24.supports("batch_operations"));
    assert!(v25.supports("batch_operations"));
    assert!(v26.supports("batch_operations"));

    assert!(!v14.supports("kg_causal"));
    assert!(!v15.supports("kg_causal"));
    assert!(v16.supports("kg_causal"));
    assert!(v17.supports("kg_causal"));
    assert!(v18.supports("kg_causal"));
    assert!(v19.supports("kg_causal"));
    assert!(v20.supports("kg_causal"));
    assert!(v21.supports("kg_causal"));
    assert!(v22.supports("kg_causal"));
    assert!(v23.supports("kg_causal"));
    assert!(v24.supports("kg_causal"));
    assert!(v25.supports("kg_causal"));
    assert!(v26.supports("kg_causal"));

    assert!(!v16.supports("deprecation_notices"));
    assert!(v17.supports("deprecation_notices"));
    assert!(v18.supports("deprecation_notices"));
    assert!(v19.supports("deprecation_notices"));
    assert!(v20.supports("deprecation_notices"));
    assert!(v21.supports("deprecation_notices"));
    assert!(v22.supports("deprecation_notices"));
    assert!(v23.supports("deprecation_notices"));
    assert!(v24.supports("deprecation_notices"));
    assert!(v25.supports("deprecation_notices"));
    assert!(v26.supports("deprecation_notices"));

    assert!(!v13.supports("tenant_management"));
    assert!(v14.supports("tenant_management"));
    assert!(v15.supports("tenant_management"));
    assert!(v16.supports("tenant_management"));
    assert!(v17.supports("tenant_management"));
    assert!(v18.supports("tenant_management"));
    assert!(v19.supports("tenant_management"));
    assert!(v20.supports("tenant_management"));
    assert!(v21.supports("tenant_management"));
    assert!(v22.supports("tenant_management"));
    assert!(v23.supports("tenant_management"));
    assert!(v24.supports("tenant_management"));
    assert!(v25.supports("tenant_management"));
    assert!(v26.supports("tenant_management"));

    assert!(!v17.supports("model_hot_swap"));
    assert!(v18.supports("model_hot_swap"));
    assert!(v19.supports("model_hot_swap"));
    assert!(v20.supports("model_hot_swap"));
    assert!(v21.supports("model_hot_swap"));
    assert!(v22.supports("model_hot_swap"));
    assert!(v23.supports("model_hot_swap"));
    assert!(v24.supports("model_hot_swap"));
    assert!(v25.supports("model_hot_swap"));
    assert!(v26.supports("model_hot_swap"));
}

#[test]
fn test_version_supports_unknown_feature() {
    let v = ApiVersion::parse("26.0.0").unwrap();
    assert!(!v.supports("unknown_feature"));
    assert!(!v.supports(""));
}

#[test]
fn test_version_supports_none_defaults_to_current() {
    assert!(version_supports(None, "batch_operations"));
    assert!(version_supports(None, "kg_causal"));
    assert!(version_supports(None, "deprecation_notices"));
    assert!(version_supports(None, "tenant_management"));
    assert!(version_supports(None, "model_hot_swap"));
}

#[test]
fn test_version_features_from_version() {
    let v26 = ApiVersion::parse("26.0.0").unwrap();
    let features = VersionFeatures::from_version(v26);
    assert!(features.deprecation_notices);
    assert!(features.batch_operations);
    assert!(features.kg_causal);
    assert!(features.tenant_management);
    assert!(features.model_hot_swap);

    let v14 = ApiVersion::parse("14.0.0").unwrap();
    let features14 = VersionFeatures::from_version(v14);
    assert!(!features14.deprecation_notices);
    assert!(!features14.kg_causal);
    assert!(!features14.batch_operations);
    assert!(features14.tenant_management);
    assert!(!features14.model_hot_swap);

    let v13 = ApiVersion::parse("13.0.0").unwrap();
    let features13 = VersionFeatures::from_version(v13);
    assert!(!features13.deprecation_notices);
    assert!(!features13.kg_causal);
    assert!(!features13.batch_operations);
    assert!(!features13.tenant_management);
    assert!(!features13.model_hot_swap);
}

#[test]
fn test_api_request_with_version() {
    let json = r#"{"method":"create","api_version":"1.0.0","content":"test","tags":[],"agent_id":"a1"}"#;
    let req: ApiRequest = serde_json::from_str(json).unwrap();
    if let ApiRequest::Create { api_version, .. } = req {
        assert_eq!(api_version, Some(ApiVersion::parse("1.0.0").unwrap()));
    } else {
        panic!("expected Create");
    }
}

#[test]
fn test_api_request_without_version_defaults() {
    let json = r#"{"method":"create","content":"test","tags":[],"agent_id":"a1"}"#;
    let req: ApiRequest = serde_json::from_str(json).unwrap();
    if let ApiRequest::Create { api_version, .. } = req {
        assert!(api_version.is_none());
    } else {
        panic!("expected Create");
    }
}

#[test]
fn test_api_response_includes_version() {
    let resp = ApiResponse::ok();
    assert_eq!(resp.version, Some(ApiVersion::CURRENT));
}

#[test]
fn test_version_ord_trait() {
    use std::collections::BTreeSet;
    let mut set = BTreeSet::new();
    set.insert(ApiVersion::parse("2.0.0").unwrap());
    set.insert(ApiVersion::parse("1.0.0").unwrap());
    set.insert(ApiVersion::parse("1.1.0").unwrap());
    let mut iter = set.into_iter();
    assert_eq!(iter.next().unwrap().major, 1);
    assert_eq!(iter.next().unwrap().minor, 1);
    assert_eq!(iter.next().unwrap().major, 2);
}

#[test]
fn test_version_serialization_roundtrip() {
    let v = ApiVersion::parse("1.2.3").unwrap();
    let json = serde_json::to_string(&v).unwrap();
    assert_eq!(json, "\"1.2.3\"");
    let parsed: ApiVersion = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, v);
}

#[test]
fn test_version_struct_format_deserialization() {
    let json = r#"{"major":1,"minor":2,"patch":3}"#;
    let v: ApiVersion = serde_json::from_str(json).unwrap();
    assert_eq!(v.major, 1);
    assert_eq!(v.minor, 2);
    assert_eq!(v.patch, 3);
}

#[test]
fn test_api_response_error_includes_version() {
    let resp = ApiResponse::error("something went wrong");
    assert_eq!(resp.version, Some(ApiVersion::CURRENT));
    assert!(!resp.ok);
    assert!(resp.error.is_some());
}

#[test]
fn test_version_eq_and_hash() {
    let v1 = ApiVersion::parse("1.0.0").unwrap();
    let v2 = ApiVersion::parse("1.0.0").unwrap();
    let v3 = ApiVersion::parse("2.0.0").unwrap();
    assert_eq!(v1, v2);
    assert_ne!(v1, v3);

    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(v1);
    set.insert(v2);
    set.insert(v3);
    assert_eq!(set.len(), 2);
}
