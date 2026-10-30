//! The `.pcbcraft` document format: version check, validation and bounded
//! encoding. This module is pure; reading and writing files belongs to
//! `pcbcraft-io`.
//!
//! Pcbcraft is pre-release: only the current format version is accepted and
//! there are no migrations. Any other version is refused, never guessed at.
use crate::diagnostics::{Error, Result};
use crate::library::Drafts;
use crate::model::{Document, Project};
use crate::validate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::Arc;

pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const FILE_VERSION: u32 = 1;
pub const EXTENSION: &str = "pcbcraft";

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EditorFile {
    pub version: u32,
    pub project: Project,
    /// Unfinished definitions, saved and recovered exactly as written.
    pub drafts: Drafts,
}

impl EditorFile {
    pub fn from_document(document: &Document) -> Self {
        Self {
            version: FILE_VERSION,
            project: (*document.project).clone(),
            drafts: (*document.drafts).clone(),
        }
    }
    pub fn into_document(self) -> Document {
        Document {
            project: Arc::new(self.project),
            drafts: Arc::new(self.drafts),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != FILE_VERSION {
            return Err(Error::Unsupported(format!(
                "unsupported editor file version {}",
                self.version
            )));
        }
        validate::document(&self.project, &self.drafts)
    }
}

/// Parse and validate a document of the current format version.
pub fn decode(bytes: &[u8]) -> Result<EditorFile> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::Limit("editor file exceeds 16 MiB".into()));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| Error::Invalid("editor file version is missing or invalid".into()))?;
    if version != u64::from(FILE_VERSION) {
        return Err(Error::Unsupported(format!(
            "unsupported editor file version {version}; this build reads version {FILE_VERSION}"
        )));
    }
    let file: EditorFile = serde_json::from_value(value)?;
    file.validate()?;
    Ok(file)
}

/// Validate and serialize a document.
pub fn encode(file: &EditorFile) -> Result<Vec<u8>> {
    file.validate()?;
    encode_bounded(file)
}

/// Bound serialization itself rather than allocating an arbitrarily large
/// buffer and checking it afterwards.
pub fn encode_bounded(value: &impl Serialize) -> Result<Vec<u8>> {
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_FILE_BYTES.saturating_sub(self.0.len()) {
                return Err(std::io::Error::other("file exceeds 16 MiB"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = Bounded(Vec::new());
    serde_json::to_writer_pretty(&mut buffer, value)
        .map_err(|_| Error::Limit("editor file exceeds 16 MiB".into()))?;
    Ok(buffer.0)
}
