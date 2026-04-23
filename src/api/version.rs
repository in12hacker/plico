//! API Versioning — Semantic version types, feature flags, and deprecation notices.
//!
//! Extracted from `semantic.rs` for independent evolution.
//! Version management changes independently from protocol (ApiRequest/ApiResponse).

// ── Versioning Types (v17.0) ───────────────────────────────────────────

/// API version with semantic versioning (major.minor.patch).
///
/// # Examples
/// ```
/// use plico::api::version::ApiVersion;
/// let v = ApiVersion::parse("1.2.0").unwrap();
/// assert!(v.major == 1 && v.minor == 2 && v.patch == 0);
/// ```
///
/// Serializes/deserializes as a string like "1.2.0".
/// Can be deserialized from either "1.2.0" string or {major, minor, patch} struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApiVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl serde::Serialize for ApiVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&format!("{}.{}.{}", self.major, self.minor, self.patch))
    }
}

impl<'de> serde::Deserialize<'de> for ApiVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct VersionVisitor;
        impl<'de> serde::de::Visitor<'de> for VersionVisitor {
            type Value = ApiVersion;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a version string like '1.2.0' or an object with major, minor, patch")
            }
            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                ApiVersion::parse(s).map_err(serde::de::Error::custom)
            }
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut major = None;
                let mut minor = None;
                let mut patch = None;
                while let Some(key) = map.next_key()? {
                    match key {
                        "major" => major = Some(map.next_value()?),
                        "minor" => minor = Some(map.next_value()?),
                        "patch" => patch = Some(map.next_value()?),
                        _ => {}
                    }
                }
                Ok(ApiVersion {
                    major: major.unwrap_or(0),
                    minor: minor.unwrap_or(0),
                    patch: patch.unwrap_or(0),
                })
            }
        }
        deserializer.deserialize_any(VersionVisitor)
    }
}

impl ApiVersion {
    /// Version 1.0.0 — initial stable release.
    pub const V1: ApiVersion = ApiVersion { major: 1, minor: 0, patch: 0 };
    /// Current stable version.
    pub const CURRENT: ApiVersion = ApiVersion { major: 18, minor: 0, patch: 0 };
    /// Minimum supported version (for compatibility checks).
    pub const MIN_SUPPORTED: ApiVersion = ApiVersion { major: 1, minor: 0, patch: 0 };

    /// Parse a version string like "1.2.0" into an ApiVersion.
    pub fn parse(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(format!("invalid version format '{}', expected 'major.minor.patch'", s));
        }
        let major = parts[0].parse().map_err(|_| format!("invalid major version: {}", parts[0]))?;
        let minor = parts[1].parse().map_err(|_| format!("invalid minor version: {}", parts[1]))?;
        let patch = parts[2].parse().map_err(|_| format!("invalid patch version: {}", parts[2]))?;
        Ok(ApiVersion { major, minor, patch })
    }

    /// Check if this version supports a given feature.
    ///
    /// # Features
    /// - `"batch_operations"` — batch_create, batch_memory_store, batch_submit_intent, batch_query (v15.0+)
    /// - `"kg_causal"` — kg_causal_path, kg_impact_analysis, kg_temporal_changes (v16.0+)
    /// - `"deprecation_notices"` — response includes deprecation field (v17.0+)
    /// - `"tenant_management"` — create_tenant, list_tenants, tenant_share (v14.0+)
    /// - `"model_hot_swap"` — switch_embedding_model, switch_llm_model, check_model_health (v18.0+)
    pub fn supports(&self, feature: &str) -> bool {
        match feature {
            "batch_operations" => *self >= ApiVersion { major: 15, minor: 0, patch: 0 },
            "kg_causal" => *self >= ApiVersion { major: 16, minor: 0, patch: 0 },
            "deprecation_notices" => *self >= ApiVersion { major: 17, minor: 0, patch: 0 },
            "tenant_management" => *self >= ApiVersion { major: 14, minor: 0, patch: 0 },
            "model_hot_swap" => *self >= ApiVersion { major: 18, minor: 0, patch: 0 },
            _ => false,
        }
    }

    /// Check if this version is backward-compatible with another.
    /// Two versions are compatible if they have the same major version.
    pub fn is_compatible(&self, other: ApiVersion) -> bool {
        self.major == other.major
    }

    /// Returns true if this version is deprecated.
    pub fn is_deprecated(&self) -> bool {
        *self < (ApiVersion { major: 18, minor: 0, patch: 0 })
    }
}

impl Default for ApiVersion {
    fn default() -> Self {
        ApiVersion::CURRENT
    }
}

impl std::fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::str::FromStr for ApiVersion {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ApiVersion::parse(s)
    }
}

/// Deprecation notice included in API responses for deprecated endpoints.
///
/// When the server responds to a request using an older API version,
/// it may include a deprecation notice to inform the client of:
/// - When the endpoint was first deprecated
/// - When it will be removed entirely (sunset version)
/// - A migration message suggesting the replacement
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeprecationNotice {
    /// The API version when this endpoint/field was first deprecated.
    pub deprecated_since: ApiVersion,
    /// The API version when this endpoint will be removed entirely.
    pub sunset_version: ApiVersion,
    /// A human-readable migration message.
    pub message: String,
}

/// Feature flags for version-specific behavior.
#[derive(Debug, Clone, Default)]
pub struct VersionFeatures {
    /// True if the request supports batch operations (v15.0+).
    pub batch_operations: bool,
    /// True if the request supports KG causal reasoning (v16.0+).
    pub kg_causal: bool,
    /// True if the response should include deprecation notices (v17.0+).
    pub deprecation_notices: bool,
    /// True if the request supports tenant management (v14.0+).
    pub tenant_management: bool,
    /// True if the request supports model hot-swap (v18.0+).
    pub model_hot_swap: bool,
}

impl VersionFeatures {
    /// Derive feature flags from an API version.
    pub fn from_version(version: ApiVersion) -> Self {
        VersionFeatures {
            batch_operations: version.supports("batch_operations"),
            kg_causal: version.supports("kg_causal"),
            deprecation_notices: version.supports("deprecation_notices"),
            tenant_management: version.supports("tenant_management"),
            model_hot_swap: version.supports("model_hot_swap"),
        }
    }
}

/// Check if a request version supports a given feature.
/// Returns true for None (defaults to CURRENT, which supports all features).
pub fn version_supports(version: Option<ApiVersion>, feature: &str) -> bool {
    version.unwrap_or(ApiVersion::CURRENT).supports(feature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_version() {
        let v = ApiVersion::parse("1.2.3").unwrap();
        assert_eq!(v, ApiVersion { major: 1, minor: 2, patch: 3 });
    }

    #[test]
    fn parse_invalid_format() {
        assert!(ApiVersion::parse("1.2").is_err());
        assert!(ApiVersion::parse("abc").is_err());
        assert!(ApiVersion::parse("1.2.x").is_err());
    }

    #[test]
    fn display_roundtrip() {
        let v = ApiVersion { major: 18, minor: 0, patch: 0 };
        assert_eq!(v.to_string(), "18.0.0");
        let parsed: ApiVersion = "18.0.0".parse().unwrap();
        assert_eq!(parsed, v);
    }

    #[test]
    fn serialize_as_string() {
        let v = ApiVersion { major: 1, minor: 2, patch: 3 };
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, r#""1.2.3""#);
    }

    #[test]
    fn deserialize_from_string() {
        let v: ApiVersion = serde_json::from_str(r#""1.2.3""#).unwrap();
        assert_eq!(v, ApiVersion { major: 1, minor: 2, patch: 3 });
    }

    #[test]
    fn deserialize_from_object() {
        let v: ApiVersion = serde_json::from_str(r#"{"major":2,"minor":1,"patch":0}"#).unwrap();
        assert_eq!(v, ApiVersion { major: 2, minor: 1, patch: 0 });
    }

    #[test]
    fn deserialize_object_missing_fields_default_zero() {
        let v: ApiVersion = serde_json::from_str(r#"{"major":5}"#).unwrap();
        assert_eq!(v, ApiVersion { major: 5, minor: 0, patch: 0 });
    }

    #[test]
    fn ordering() {
        let v1 = ApiVersion { major: 1, minor: 0, patch: 0 };
        let v2 = ApiVersion { major: 2, minor: 0, patch: 0 };
        let v1_1 = ApiVersion { major: 1, minor: 1, patch: 0 };
        assert!(v1 < v2);
        assert!(v1 < v1_1);
        assert!(v1_1 < v2);
    }

    #[test]
    fn default_is_current() {
        assert_eq!(ApiVersion::default(), ApiVersion::CURRENT);
    }

    #[test]
    fn supports_feature_flags() {
        let v14 = ApiVersion { major: 14, minor: 0, patch: 0 };
        assert!(v14.supports("tenant_management"));
        assert!(!v14.supports("batch_operations"));

        let v15 = ApiVersion { major: 15, minor: 0, patch: 0 };
        assert!(v15.supports("batch_operations"));
        assert!(!v15.supports("kg_causal"));

        let v18 = ApiVersion { major: 18, minor: 0, patch: 0 };
        assert!(v18.supports("model_hot_swap"));
        assert!(v18.supports("batch_operations"));
    }

    #[test]
    fn supports_unknown_feature_returns_false() {
        assert!(!ApiVersion::CURRENT.supports("nonexistent_feature"));
    }

    #[test]
    fn is_compatible_same_major() {
        let a = ApiVersion { major: 18, minor: 0, patch: 0 };
        let b = ApiVersion { major: 18, minor: 5, patch: 3 };
        assert!(a.is_compatible(b));
    }

    #[test]
    fn is_compatible_different_major() {
        let a = ApiVersion { major: 17, minor: 0, patch: 0 };
        let b = ApiVersion { major: 18, minor: 0, patch: 0 };
        assert!(!a.is_compatible(b));
    }

    #[test]
    fn is_deprecated() {
        let old = ApiVersion { major: 17, minor: 0, patch: 0 };
        assert!(old.is_deprecated());
        let current = ApiVersion { major: 18, minor: 0, patch: 0 };
        assert!(!current.is_deprecated());
    }

    #[test]
    fn version_supports_none_uses_current() {
        assert!(version_supports(None, "model_hot_swap"));
    }

    #[test]
    fn version_features_from_version() {
        let features = VersionFeatures::from_version(ApiVersion { major: 18, minor: 0, patch: 0 });
        assert!(features.batch_operations);
        assert!(features.kg_causal);
        assert!(features.deprecation_notices);
        assert!(features.tenant_management);
        assert!(features.model_hot_swap);

        let old_features = VersionFeatures::from_version(ApiVersion { major: 13, minor: 0, patch: 0 });
        assert!(!old_features.batch_operations);
        assert!(!old_features.tenant_management);
    }

    #[test]
    fn deprecation_notice_serde() {
        let notice = DeprecationNotice {
            deprecated_since: ApiVersion { major: 15, minor: 0, patch: 0 },
            sunset_version: ApiVersion { major: 20, minor: 0, patch: 0 },
            message: "Use v18 API".into(),
        };
        let json = serde_json::to_string(&notice).unwrap();
        let rt: DeprecationNotice = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.deprecated_since, notice.deprecated_since);
        assert_eq!(rt.message, "Use v18 API");
    }
}
