//! AI Object — the fundamental data unit of Plico
//!
//! In Plico, everything stored is an `AIObject`. Unlike a traditional file with a path,
//! an `AIObject` is identified by its content hash (CID). AI agents never reference
//! by location — only by content identity.
//!
//! # AIObjectMeta
//!
//! Instead of paths and filenames, AI agents describe objects with semantic tags,
//! content type, and origin metadata. The system infers structure from meaning.

use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};

/// The fundamental data unit in Plico's AI-native filesystem.
/// Its identity is determined entirely by content — not by path or name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIObject {
    /// Content Identifier — SHA-256 hash of `data`. This IS the object's address.
    /// Two objects with identical content will have identical CIDs (deduplication).
    pub cid: String,

    /// Raw content bytes. Can be text, image, audio, video, or any binary data.
    pub data: Vec<u8>,

    /// Semantic metadata — replaces filesystem paths, names, and directories.
    pub meta: AIObjectMeta,
}

impl AIObject {
    /// Create a new AIObject. CID is computed automatically from content.
    ///
    /// # Example
    ///
    /// ```
    /// use plico::{AIObject, AIObjectMeta};
    /// let obj = AIObject::new(
    ///     b"Agent task output: embedding batch result".to_vec(),
    ///     AIObjectMeta::text(["meeting", "project-x"]),
    /// );
    /// println!("CID: {}", obj.cid);
    /// ```
    pub fn new(data: Vec<u8>, meta: AIObjectMeta) -> Self {
        let cid = Self::compute_cid(&data);
        AIObject { cid, data, meta }
    }

    /// Compute SHA-256 CID from raw bytes.
    pub fn compute_cid(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    /// Verify that the stored CID matches the content (integrity check).
    pub fn verify_integrity(&self) -> bool {
        self.cid == Self::compute_cid(&self.data)
    }

    /// Content type as a readable string.
    pub fn content_type_str(&self) -> String {
        format!("{}", self.meta.content_type)
    }

    /// True if this is a text object.
    pub fn is_text(&self) -> bool {
        self.meta.content_type.is_text()
    }
}

/// Semantic metadata for an AIObject. Replaces: paths, filenames, directories,
/// MIME type inference, and owner information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIObjectMeta {
    /// Content type — MIME-like classification inferred from content or declared by AI.
    pub content_type: ContentType,

    /// Semantic tags — AI-assigned meaning descriptors.
    /// Examples: ["meeting", "2026-Q1", "project-x", "financial"]
    /// These replace filesystem paths. Objects are found by tag, not by path.
    pub tags: Vec<String>,

    /// Agent ID of the creator — who/what created this object.
    pub created_by: String,

    /// Unix timestamp (milliseconds) of creation.
    pub created_at: u64,

    /// Optional intent description — what this object is FOR.
    pub intent: Option<String>,

    /// Tenant ID — provides multi-tenant isolation.
    #[serde(default)]
    pub tenant_id: String,
}

impl AIObjectMeta {
    /// Default tenant ID when no tenant is specified.
    pub fn default_tenant() -> String {
        crate::DEFAULT_TENANT.to_string()
    }

    /// Create a text metadata block.
    pub fn text<const N: usize>(tags: [&str; N]) -> Self {
        Self {
            content_type: ContentType::Text,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            created_by: String::new(),
            created_at: now_ms(),
            intent: None,
            tenant_id: Self::default_tenant(),
        }
    }

    /// Add an intent description.
    pub fn with_intent(mut self, intent: impl Into<String>) -> Self {
        self.intent = Some(intent.into());
        self
    }

    /// Set the creating agent ID.
    pub fn with_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.created_by = agent_id.into();
        self
    }

    /// Set the tenant ID.
    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = tenant_id.into();
        self
    }
}

/// Content type classification — replaces MIME types with AI-semantic categories.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContentType {
    /// Plain text, markdown, structured text
    Text,
    /// Images (PNG, JPEG, WebP, GIF, SVG)
    Image,
    /// Audio files (MP3, WAV, FLAC)
    Audio,
    /// Video files (MP4, MKV, WebM)
    Video,
    /// Structured data (JSON, TOML, YAML, CSV)
    Structured,
    /// Binary / executable
    Binary,
    /// Unknown or mixed content
    Unknown,
}

impl ContentType {
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "txt" | "md" | "rst" | "log" => ContentType::Text,
            "json" | "toml" | "yaml" | "yml" | "csv" | "xml" => ContentType::Structured,
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" | "bmp" => ContentType::Image,
            "mp3" | "wav" | "flac" | "ogg" | "aac" => ContentType::Audio,
            "mp4" | "mkv" | "webm" | "avi" | "mov" => ContentType::Video,
            "exe" | "bin" | "so" | "dll" | "a" => ContentType::Binary,
            _ => ContentType::Unknown,
        }
    }

    pub fn is_text(&self) -> bool {
        matches!(self, ContentType::Text | ContentType::Structured)
    }

    pub fn is_multimedia(&self) -> bool {
        matches!(self, ContentType::Image | ContentType::Audio | ContentType::Video)
    }
}

impl std::fmt::Display for ContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentType::Text => write!(f, "text"),
            ContentType::Image => write!(f, "image"),
            ContentType::Audio => write!(f, "audio"),
            ContentType::Video => write!(f, "video"),
            ContentType::Structured => write!(f, "structured"),
            ContentType::Binary => write!(f, "binary"),
            ContentType::Unknown => write!(f, "unknown"),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cid_is_content_hash() {
        let data1 = b"hello world";
        let data2 = b"hello world";
        let data3 = b"hello worlc";

        let cid1 = AIObject::compute_cid(data1);
        let cid2 = AIObject::compute_cid(data2);
        let cid3 = AIObject::compute_cid(data3);

        // Same content → same CID (deduplication guarantee)
        assert_eq!(cid1, cid2);
        // Different content → different CID (collision-resistant)
        assert_ne!(cid1, cid3);
    }

    #[test]
    fn test_integrity_verification() {
        let obj = AIObject::new(b"test content".to_vec(), AIObjectMeta::text(["test"]));
        assert!(obj.verify_integrity());
    }

    #[test]
    fn test_content_type_from_extension() {
        assert_eq!(ContentType::from_extension("txt"), ContentType::Text);
        assert_eq!(ContentType::from_extension("JSON"), ContentType::Structured);
        assert_eq!(ContentType::from_extension("jpg"), ContentType::Image);
        assert_eq!(ContentType::from_extension("XYZ"), ContentType::Unknown);
    }
}
