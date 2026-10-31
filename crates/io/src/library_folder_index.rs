//! The per-source catalog index cache (S3): `<cache>/pcbcraft/index/
//! <LibraryId>.json`, holding the manifest hash, each record's content hash
//! and the `SourceCatalog` built by `pcbcraft_core::catalog::source_catalog`.
//! Never authoritative: a version mismatch, an unreadable file or a changed
//! record rebuilds it (only the changed lineages where possible), and a
//! deleted index rebuilds identically. Never written into a project or a
//! library folder. Run off the UI thread.
use crate::Result;
use crate::library_folder::Folder;
use pcbcraft_core::catalog::SourceCatalog;
use pcbcraft_core::ids::{ContentHash, LibraryId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The cache file's own format; any other is rebuilt, never migrated.
pub const INDEX_FORMAT: u32 = 1;

/// One cached index.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceIndex {
    pub format: u32,
    pub library: LibraryId,
    /// SHA-256 of the manifest bytes.
    pub manifest_hash: ContentHash,
    /// Content hash per record path, sorted.
    pub files: BTreeMap<String, ContentHash>,
    pub catalog: SourceCatalog,
}

/// How a load went, for the Browser and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexState {
    /// The cache matched the folder.
    Cached,
    /// The cache was missing, stale or unreadable and was rebuilt.
    Rebuilt,
}

/// `<cache_dir>/index/<library>.json`.
pub fn index_path(cache_dir: &Path, library: LibraryId) -> PathBuf {
    cache_dir.join("index").join(format!("{library}.json"))
}

/// The folder's catalog from the cache, rebuilt and rewritten when it does
/// not match the folder.
pub fn load_or_build(cache_dir: &Path, folder: &Folder) -> Result<(SourceCatalog, IndexState)> {
    let _ = (cache_dir, folder);
    Err(crate::unsupported("library folder indexes"))
}
