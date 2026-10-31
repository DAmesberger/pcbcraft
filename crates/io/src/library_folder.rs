//! Local library folders (S3, D2), shared as plain files or git:
//!
//! ```text
//! <folder>/pcbcraft-library.json       manifest: format, identity, name, required features
//! <folder>/lineages/<id>/r0001.json    one immutable `RevisionRecord` per revision
//! <folder>/assets/sha256/<hash>        asset bytes by content hash
//! <folder>/catalog.json                mutable shelves and tags (D13), not revisioned
//! ```
//!
//! Output is git-friendly: stable key order, pretty JSON with a final
//! newline, one file per revision. An existing `rNNNN.json` is never
//! overwritten. Meaning (records, hashes, bundles) stays in
//! `pcbcraft_core::library::bundle`; this module only moves bytes.
use crate::{Error, Result};
use pcbcraft_core::ids::{ContentHash, LibraryId, Revision, SourceLineageId};
use pcbcraft_core::library::bundle::{Bundle, RevisionRecord, SourceRevision};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The manifest's file name.
pub const MANIFEST_FILE: &str = "pcbcraft-library.json";
/// The mutable label file's name.
pub const CATALOG_FILE: &str = "catalog.json";
/// The one folder format this build reads and writes (pre-release: no
/// migrations; any other version is refused).
pub const FOLDER_FORMAT: u32 = 1;
/// Required features this build understands. A manifest naming any other
/// is refused, never read partially.
pub const SUPPORTED_FEATURES: &[&str] = &[];
/// Revision files one folder may hold.
pub const MAX_FOLDER_RECORDS: usize = 20_000;
/// Bytes of one revision or manifest file.
pub const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;

/// `pcbcraft-library.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub library: LibraryId,
    pub name: String,
    #[serde(default)]
    pub required_features: Vec<String>,
    #[serde(default)]
    pub default_licence: Option<String>,
}

/// Shelf and tags of one lineage (D13).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Labels {
    #[serde(default)]
    pub shelf: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// `catalog.json`: labels by lineage, sorted by id.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CatalogLabels {
    pub format: u32,
    pub lineages: BTreeMap<SourceLineageId, Labels>,
}

/// An opened folder whose manifest passed the version and feature checks.
#[derive(Clone, Debug, PartialEq)]
pub struct Folder {
    pub root: PathBuf,
    pub manifest: Manifest,
}

/// One record read from disk, with the hash of its content.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredRecord {
    /// Relative to the folder root, `/`-separated.
    pub path: String,
    pub record: RevisionRecord,
    pub content_hash: ContentHash,
}

/// What `write_bundle` did with each record and asset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WriteReport {
    /// Records written, as relative paths.
    pub written: Vec<String>,
    /// Records already present with an equal hash.
    pub unchanged: Vec<String>,
    /// Assets written by hash.
    pub assets: Vec<ContentHash>,
}

/// Why a configured source is not searchable. The Browser lists the source
/// with this reason instead of dropping it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disabled {
    /// Turned off in settings.
    Off,
    PathMissing,
    /// The manifest is unreadable or malformed.
    Malformed(String),
    UnsupportedFormat(u32),
    UnknownFeatures(Vec<String>),
}

impl std::fmt::Display for Disabled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => f.write_str("turned off"),
            Self::PathMissing => f.write_str("path missing"),
            Self::Malformed(why) => write!(f, "unreadable manifest: {why}"),
            Self::UnsupportedFormat(v) => write!(f, "unsupported format v{v}"),
            Self::UnknownFeatures(names) => {
                write!(f, "requires unsupported features: {}", names.join(", "))
            }
        }
    }
}

/// A random 128-bit id from the operating system (D1).
fn random_u128() -> Result<[u8; 16]> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::Invalid(format!("the system random source failed: {e}")))?;
    Ok(bytes)
}

pub fn new_library_id() -> Result<LibraryId> {
    random_u128().map(LibraryId::from_bytes)
}

pub fn new_lineage_id() -> Result<SourceLineageId> {
    random_u128().map(SourceLineageId::from_bytes)
}

/// `lineages/<id>/r<NNNN>.json`, relative to the folder root.
pub fn record_path(lineage: SourceLineageId, revision: Revision) -> String {
    format!("lineages/{lineage}/r{:04}.json", revision.0)
}

/// `assets/sha256/<hash>`, relative to the folder root.
pub fn asset_path(hash: ContentHash) -> String {
    format!("assets/sha256/{hash}")
}

/// Check a parsed manifest: format version and required features.
pub fn check_manifest(manifest: &Manifest) -> std::result::Result<(), Disabled> {
    if manifest.format != FOLDER_FORMAT {
        return Err(Disabled::UnsupportedFormat(manifest.format));
    }
    let unknown: Vec<String> = manifest
        .required_features
        .iter()
        .filter(|f| !SUPPORTED_FEATURES.contains(&f.as_str()))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Err(Disabled::UnknownFeatures(unknown));
    }
    Ok(())
}

/// Open a folder: read and check its manifest. Refuses an unsupported
/// format or unknown required feature with `unsupported`.
pub fn open_folder(root: &Path) -> Result<Folder> {
    let path = root.join(MANIFEST_FILE);
    let bytes = crate::persistence::read_bytes(&path)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(Error::TooLarge("library manifest is too large".into()));
    }
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    check_manifest(&manifest).map_err(|why| {
        Error::Document(pcbcraft_core::Error::Unsupported(format!(
            "library folder {}: {why}",
            root.display()
        )))
    })?;
    Ok(Folder {
        root: root.to_path_buf(),
        manifest,
    })
}

/// Create an empty folder library (manifest only) at `root`, which must
/// not already hold a manifest.
pub fn create_folder(root: &Path, name: &str) -> Result<Folder> {
    let _ = (root, name);
    Err(crate::unsupported("creating library folders"))
}

/// Every revision record of the folder, sorted by path, each with its
/// content hash. A file whose name disagrees with its record is an error.
pub fn read_records(folder: &Folder) -> Result<Vec<StoredRecord>> {
    let _ = folder;
    Err(crate::unsupported("reading library folder records"))
}

/// The bundle of `root` and its closure, read from the folder (records and
/// the assets they name).
pub fn read_bundle(folder: &Folder, root: SourceRevision) -> Result<Bundle> {
    let _ = (folder, root);
    Err(crate::unsupported("reading library bundles"))
}

/// Write a bundle's records and assets atomically, file by file (assets
/// first; orphans are tolerated). An existing record with an equal hash is
/// left alone; one with another hash refuses the whole write with
/// `conflict` before anything is written.
pub fn write_bundle(folder: &Folder, bundle: &Bundle) -> Result<WriteReport> {
    let _ = (folder, bundle);
    Err(crate::unsupported("writing library bundles"))
}

/// Read `catalog.json`; a missing file is empty labels.
pub fn read_labels(folder: &Folder) -> Result<CatalogLabels> {
    let _ = folder;
    Err(crate::unsupported("reading library labels"))
}

/// Atomically replace `catalog.json`.
pub fn write_labels(folder: &Folder, labels: &CatalogLabels) -> Result<()> {
    let _ = (folder, labels);
    Err(crate::unsupported("writing library labels"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_checks_refuse_other_versions_and_unknown_features() {
        let mut manifest = Manifest {
            format: FOLDER_FORMAT,
            library: LibraryId(7),
            name: "Mine".into(),
            required_features: vec![],
            default_licence: None,
        };
        assert_eq!(check_manifest(&manifest), Ok(()));
        manifest.format = 2;
        assert_eq!(
            check_manifest(&manifest),
            Err(Disabled::UnsupportedFormat(2))
        );
        manifest.format = FOLDER_FORMAT;
        manifest.required_features = vec!["future".into()];
        assert_eq!(
            check_manifest(&manifest),
            Err(Disabled::UnknownFeatures(vec!["future".into()]))
        );
    }

    #[test]
    fn paths_are_stable() {
        let lineage = SourceLineageId(1);
        assert_eq!(
            record_path(lineage, Revision(3)),
            "lineages/00000000000000000000000000000001/r0003.json"
        );
    }
}
