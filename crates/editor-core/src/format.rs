//! The `.sparksmith` document format: version check, validation and bounded
//! encoding. This module is pure; reading and writing files belongs to
//! `sparksmith-io`.
//!
//! Versions (docs/design/file-format-plan.md §4.7): every incompatible change
//! to a stored type raises [`FILE_VERSION`], and `cargo xtask format-lock`
//! fails until it does. Optional capabilities are named [`FEATURES`]: a file
//! lists exactly the features it uses, so a design that uses none stays
//! readable by older builds, and a build refuses a feature it does not know,
//! naming it. Until the first release this build reads only
//! [`FILE_VERSION`]; an older file is refused with the command that converts
//! it (`sparksmith convert`, the chain in `sparksmith_io::convert`), and a
//! newer one is never guessed at.
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
/// Raised on every incompatible change to a stored type, with a converter
/// step from the previous version (`sparksmith_io::convert::STEPS`).
pub const FILE_VERSION: u32 = 2;
/// The optional capabilities this build reads, by the name a file lists
/// them under. None are defined yet; a feature is added here together with
/// its test in [`features_used`].
pub const FEATURES: &[&str] = &[];
pub const EXTENSION: &str = "sparksmith";
/// The extension before the rename from Pcbcraft (docs/rename-phase-b.md).
/// Such a file holds this same format, so it opens through the same reader
/// and version check, and Save keeps its name; Save As and new projects take
/// [`EXTENSION`].
pub const FORMER_EXTENSION: &str = "pcbcraft";
/// The extensions a project file may have, [`EXTENSION`] first.
pub const EXTENSIONS: [&str; 2] = [EXTENSION, FORMER_EXTENSION];

/// Whether `extension` (without the dot) names a project file, in any
/// letter case.
pub fn is_project_extension(extension: &str) -> bool {
    EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
}

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
    encode_bounded(&Stored::of(&stripped))
}

/// The features `file` uses, sorted: what its saved form lists. Each
/// feature in [`FEATURES`] adds its test here.
pub fn features_used(_file: &EditorFile) -> Vec<&'static str> {
    Vec::new()
}

/// The saved form of an [`EditorFile`]: its fields, with the features it
/// uses after the version. The features are derived from the document, so
/// the file in memory does not hold them a second time.
#[derive(Serialize)]
struct Stored<'a> {
    version: u32,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    features: Vec<&'static str>,
    project: &'a Project,
    drafts: &'a Drafts,
}

impl<'a> Stored<'a> {
    fn of(file: &'a EditorFile) -> Self {
        Self {
            version: file.version,
            features: features_used(file),
            project: &file.project,
            drafts: &file.drafts,
        }
    }
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
        check_version(u64::from(self.version))?;
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
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| Error::Invalid("editor file version is missing or invalid".into()))?;
    check_version(version)?;
    let listed = match value
        .as_object_mut()
        .and_then(|object| object.remove("features"))
    {
        None => Vec::new(),
        Some(features) => serde_json::from_value::<Vec<String>>(features)
            .map_err(|_| Error::Invalid("editor file features must be a list of names".into()))?,
    };
    if let Some(unknown) = listed.iter().find(|f| !FEATURES.contains(&f.as_str())) {
        let name = crate::product::NAME;
        return Err(Error::Unsupported(format!(
            "this file uses the feature \"{unknown}\", which this build of {name} does not know; open it with a newer {name}"
        )));
    }
    let file: EditorFile = serde_json::from_value(value)?;
    let used = features_used(&file);
    if listed != used {
        let names = |features: &[&str]| match features {
            [] => "none".to_owned(),
            some => some.join(", "),
        };
        let listed: Vec<&str> = listed.iter().map(String::as_str).collect();
        return Err(Error::Invalid(format!(
            "editor file lists the features {} but uses {}",
            names(&listed),
            names(&used)
        )));
    }
    Ok(file)
}

/// Refuses every version but [`FILE_VERSION`], saying how to go on.
fn check_version(version: u64) -> Result<()> {
    use crate::product::{NAME, SHORT_NAME};
    let current = u64::from(FILE_VERSION);
    if version < current {
        return Err(Error::Unsupported(format!(
            "this file is format version {version}, from an earlier {NAME}; this build reads version \
             {FILE_VERSION}. Convert it with `{SHORT_NAME} convert FILE` (the original is kept beside it), \
             then open it again"
        )));
    }
    if version > current {
        return Err(Error::Unsupported(format!(
            "this file is format version {version}, from a newer {NAME}; this build reads version {FILE_VERSION}"
        )));
    }
    Ok(())
}

/// The JSON Schema of the saved document, for `cargo xtask format-lock`.
pub fn stored_schema() -> Result<serde_json::Value> {
    Ok(serde_json::to_value(schemars::schema_for!(EditorFile))?)
}

/// Validate and serialize a document.
pub fn encode(file: &EditorFile) -> Result<Vec<u8>> {
    file.validate()?;
    encode_bounded(&Stored::of(file))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> Result<serde_json::Value> {
        let file = EditorFile::from_document(&Document::default());
        Ok(serde_json::from_slice(&encode(&file)?)?)
    }

    fn refusal(value: &serde_json::Value) -> Result<String> {
        match decode(&serde_json::to_vec(value)?) {
            Ok(_) => Ok("accepted".into()),
            Err(error) => Ok(error.to_string()),
        }
    }

    /// A file that uses no feature lists none, and reads back.
    #[test]
    fn a_file_without_features_lists_none() -> Result<()> {
        let value = saved()?;
        assert_eq!(value["version"], FILE_VERSION);
        assert!(value.get("features").is_none());
        let mut listed = value.clone();
        listed["features"] = serde_json::json!([]);
        assert_eq!(refusal(&listed)?, "accepted");
        Ok(())
    }

    /// An unknown feature is refused by name, never dropped.
    #[test]
    fn an_unknown_feature_is_refused_by_name() -> Result<()> {
        let mut value = saved()?;
        value["features"] = serde_json::json!(["teleport_vias"]);
        let message = refusal(&value)?;
        assert!(message.contains("\"teleport_vias\""), "{message}");
        assert!(message.contains("newer"), "{message}");
        value["features"] = serde_json::json!("teleport_vias");
        assert!(refusal(&value)?.contains("list of names"));
        Ok(())
    }

    /// An older version is refused with the command that converts it; a
    /// newer one says so.
    #[test]
    fn another_version_is_refused_with_the_way_on() -> Result<()> {
        let mut value = saved()?;
        value["version"] = serde_json::json!(FILE_VERSION - 1);
        let message = refusal(&value)?;
        assert!(
            message.contains(&format!("`{} convert FILE`", crate::product::SHORT_NAME)),
            "{message}"
        );
        value["version"] = serde_json::json!(FILE_VERSION + 1);
        let message = refusal(&value)?;
        assert!(message.contains("from a newer"), "{message}");
        assert!(!message.contains("convert"), "{message}");
        Ok(())
    }
}
