//! The `.sparksmith` document format: version check, validation and bounded
//! encoding. This module is pure; reading and writing files belongs to
//! `sparksmith-io`.
//!
//! Sparksmith is pre-release: only the current format version is accepted and
//! there are no migrations. Any other version is refused, never guessed at.
use crate::diagnostics::{Error, Result};
use crate::library::Drafts;
use crate::library::bundle::AssetBytes;
use crate::model::{Document, Project};
use crate::validate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::Arc;

pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const FILE_VERSION: u32 = 1;
pub const EXTENSION: &str = "sparksmith";

/// The `.sparksmith` file is a ZIP container (D0, founder decision F5). It
/// holds [`CONTAINER_DOCUMENT`], the editor file with every packed asset's
/// bytes left out, and one raw entry per asset at
/// [`CONTAINER_ASSET_PREFIX`]`<sha256>`. Recovery snapshots use the same
/// container with one more entry, [`CONTAINER_RECOVERY`]. Nothing else may
/// appear in a container: an unknown entry is refused, never dropped.
pub const CONTAINER_DOCUMENT: &str = "project.json";
pub const CONTAINER_ASSET_PREFIX: &str = "assets/sha256/";
/// The recovery metadata entry of a recovery snapshot.
pub const CONTAINER_RECOVERY: &str = "recovery.json";
/// Bytes of one container asset (target).
pub const CONTAINER_MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
/// Bytes of one container, uncompressed (target).
pub const CONTAINER_MAX_BYTES: u64 = 512 * 1024 * 1024;
/// Entries one container holds.
pub const CONTAINER_MAX_ENTRIES: usize = 4096;
/// Bytes of the recovery metadata entry.
pub const CONTAINER_MAX_RECOVERY_BYTES: usize = 64 * 1024;
const _: () = assert!(crate::library::bundle::MAX_ASSETS + 2 <= CONTAINER_MAX_ENTRIES);

/// Validate `file` and encode the container's document: the editor file
/// with every packed asset's bytes empty, since each asset is an entry of
/// its own. The document stays bounded by [`MAX_FILE_BYTES`].
pub fn encode_container_document(file: &EditorFile) -> Result<Vec<u8>> {
    file.validate()?;
    let mut stripped = file.clone();
    for asset in &mut stripped.project.library.assets {
        asset.data = AssetBytes::default();
    }
    encode_bounded(&stripped)
}

/// Parse the container's document, refusing any other format version. It
/// is not validated: its packed assets have no bytes until the container's
/// asset entries fill them in, after which the caller validates the whole
/// file. A document that carries asset bytes inline is refused.
pub fn decode_container_document(bytes: &[u8]) -> Result<EditorFile> {
    let file = parse_versioned(bytes)?;
    if let Some(asset) = file
        .project
        .library
        .assets
        .iter()
        .find(|a| !a.data.is_empty())
    {
        return Err(Error::Invalid(format!(
            "{CONTAINER_DOCUMENT} carries the bytes of asset {} inline; asset bytes belong in {CONTAINER_ASSET_PREFIX}{}",
            asset.hash, asset.hash
        )));
    }
    Ok(file)
}

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
    let file = parse_versioned(bytes)?;
    file.validate()?;
    Ok(file)
}

/// Parse a bounded document of the current format version, unvalidated.
fn parse_versioned(bytes: &[u8]) -> Result<EditorFile> {
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
    Ok(serde_json::from_value(value)?)
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
