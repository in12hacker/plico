//! API Versioning — Semantic version types and feature flags.
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
    pub const CURRENT: ApiVersion = ApiVersion { major: 26, minor: 0, patch: 0 };

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

}
