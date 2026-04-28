//! API Versioning Tests (v26.0)
//!
//! Tests for the API versioning system including:
//! - ApiVersion parsing and comparison
//! - Serialization/deserialization

use plico::api::semantic::{
    ApiVersion, ApiRequest, ApiResponse,
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
