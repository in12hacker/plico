//! API Versioning Tests (v17.0)
//!
//! Tests for the API versioning system including:
//! - ApiVersion parsing and comparison
//! - Feature support checks
//! - Deprecation notice handling
//! - Version compatibility

use plico::api::semantic::{
    ApiVersion, ApiRequest, ApiResponse, DeprecationNotice, VersionFeatures,
    version_supports, get_deprecation_notice,
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
    assert_eq!(ApiVersion::CURRENT.major, 17);
    assert_eq!(ApiVersion::MIN_SUPPORTED.major, 1);
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
fn test_version_compatibility() {
    let v1_0 = ApiVersion::parse("1.0.0").unwrap();
    let v1_5 = ApiVersion::parse("1.5.0").unwrap();
    let v2_0 = ApiVersion::parse("2.0.0").unwrap();
    // Same major version = compatible
    assert!(v1_0.is_compatible(v1_5));
    assert!(v1_5.is_compatible(v1_0));
    // Different major version = not compatible
    assert!(!v1_0.is_compatible(v2_0));
    assert!(!v2_0.is_compatible(v1_0));
}

#[test]
fn test_version_supports_feature() {
    let v15 = ApiVersion::parse("15.0.0").unwrap();
    let v16 = ApiVersion::parse("16.0.0").unwrap();
    let v17 = ApiVersion::parse("17.0.0").unwrap();
    let v14 = ApiVersion::parse("14.0.0").unwrap();
    let v13 = ApiVersion::parse("13.0.0").unwrap();

    // Batch operations introduced in v15
    assert!(!v13.supports("batch_operations"));
    assert!(!v14.supports("batch_operations"));
    assert!(v15.supports("batch_operations"));
    assert!(v16.supports("batch_operations"));
    assert!(v17.supports("batch_operations"));

    // KG causal introduced in v16
    assert!(!v14.supports("kg_causal"));
    assert!(!v15.supports("kg_causal"));
    assert!(v16.supports("kg_causal"));
    assert!(v17.supports("kg_causal"));

    // Deprecation notices introduced in v17
    assert!(!v15.supports("deprecation_notices"));
    assert!(!v16.supports("deprecation_notices"));
    assert!(v17.supports("deprecation_notices"));

    // Tenant management introduced in v14
    assert!(!v13.supports("tenant_management"));
    assert!(v14.supports("tenant_management"));
    assert!(v15.supports("tenant_management"));
}

#[test]
fn test_version_supports_unknown_feature() {
    let v = ApiVersion::parse("17.0.0").unwrap();
    assert!(!v.supports("unknown_feature"));
    assert!(!v.supports(""));
}

#[test]
fn test_version_supports_none_defaults_to_current() {
    // None version should default to CURRENT which supports all features
    assert!(version_supports(None, "batch_operations"));
    assert!(version_supports(None, "kg_causal"));
    assert!(version_supports(None, "deprecation_notices"));
    assert!(version_supports(None, "tenant_management"));
}

#[test]
fn test_version_is_deprecated() {
    assert!(ApiVersion::parse("1.0.0").unwrap().is_deprecated());
    assert!(ApiVersion::parse("16.0.0").unwrap().is_deprecated());
    assert!(!ApiVersion::parse("17.0.0").unwrap().is_deprecated());
    assert!(!ApiVersion::parse("17.5.0").unwrap().is_deprecated());
}

#[test]
fn test_deprecation_notice_structure() {
    let notice = DeprecationNotice {
        deprecated_since: ApiVersion::parse("17.0.0").unwrap(),
        sunset_version: ApiVersion::parse("18.0.0").unwrap(),
        message: "Upgrade to v17.0 or later".to_string(),
    };
    let json = serde_json::to_string(&notice).unwrap();
    assert!(json.contains("deprecated_since"));
    assert!(json.contains("sunset_version"));
    assert!(json.contains("message"));
    let decoded: DeprecationNotice = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.sunset_version.major, 18);
    assert_eq!(decoded.message, "Upgrade to v17.0 or later");
}

#[test]
fn test_version_features_from_version() {
    let v17 = ApiVersion::parse("17.0.0").unwrap();
    let features = VersionFeatures::from_version(v17);
    assert!(features.deprecation_notices);
    assert!(features.batch_operations);
    assert!(features.kg_causal);
    assert!(features.tenant_management);

    let v14 = ApiVersion::parse("14.0.0").unwrap();
    let features14 = VersionFeatures::from_version(v14);
    assert!(!features14.deprecation_notices);
    assert!(!features14.kg_causal);
    assert!(!features14.batch_operations); // batch_operations introduced in v15
    assert!(features14.tenant_management);

    let v13 = ApiVersion::parse("13.0.0").unwrap();
    let features13 = VersionFeatures::from_version(v13);
    assert!(!features13.deprecation_notices);
    assert!(!features13.kg_causal);
    assert!(!features13.batch_operations);
    assert!(!features13.tenant_management);
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
    assert!(resp.deprecation.is_none());
}

#[test]
fn test_api_response_with_deprecation() {
    let notice = DeprecationNotice {
        deprecated_since: ApiVersion::parse("17.0.0").unwrap(),
        sunset_version: ApiVersion::parse("18.0.0").unwrap(),
        message: "Deprecated".to_string(),
    };
    let resp = ApiResponse::ok().with_deprecation(notice);
    assert!(resp.deprecation.is_some());
    let d = resp.deprecation.unwrap();
    assert_eq!(d.sunset_version.major, 18);
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
    // Also support struct format for backward compatibility
    let json = r#"{"major":1,"minor":2,"patch":3}"#;
    let v: ApiVersion = serde_json::from_str(json).unwrap();
    assert_eq!(v.major, 1);
    assert_eq!(v.minor, 2);
    assert_eq!(v.patch, 3);
}

#[test]
fn test_get_deprecation_notice_returns_none_current_version() {
    let req = ApiRequest::Create {
        api_version: Some(ApiVersion::CURRENT),
        content: "test".to_string(),
        content_encoding: Default::default(),
        tags: vec![],
        agent_id: "a1".to_string(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    };
    // Currently no deprecation for any version
    assert!(get_deprecation_notice(&req).is_none());
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
    set.insert(v2); // same as v1
    set.insert(v3);
    assert_eq!(set.len(), 2); // v1 and v2 are same
}
