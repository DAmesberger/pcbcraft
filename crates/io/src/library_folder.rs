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
use crate::persistence::atomic_write;
use crate::{Error, Result};
use pcbcraft_core::ids::{
    ContentHash, DefinitionId, LibraryId, Revision, SourceLineageId, printable,
};
use pcbcraft_core::library::bundle::{
    AssetBytes, Bundle, MAX_ASSET_BYTES, MAX_ASSETS, MAX_BUNDLE_RECORDS, MAX_LINKS, PackedAsset,
    RefSite, RevisionRecord, SourceRevision, asset_hash, content_hash, references, validate_bundle,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
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
    check_manifest_parts(manifest.format, &manifest.required_features)
}

fn check_manifest_parts(format: u32, required: &[String]) -> std::result::Result<(), Disabled> {
    if format != FOLDER_FORMAT {
        return Err(Disabled::UnsupportedFormat(format));
    }
    let unknown: Vec<String> = required
        .iter()
        .filter(|f| !SUPPORTED_FEATURES.contains(&f.as_str()))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Err(Disabled::UnknownFeatures(unknown));
    }
    Ok(())
}

/// The directory that holds one subdirectory per lineage.
pub const LINEAGES_DIR: &str = "lineages";
/// The format `catalog.json` carries; any other is refused.
pub const CATALOG_FORMAT: u32 = 1;
/// Tags one lineage carries in `catalog.json`.
pub const MAX_TAGS: usize = 64;

/// Media type of one stored asset: `assets/sha256/<hash>.json` beside the
/// bytes. The folder layout names assets by hash only, and a bundle's
/// [`PackedAsset`] carries a media type, so it is kept in this sidecar.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetMeta {
    pub media_type: String,
}

/// `assets/sha256/<hash>.json`, relative to the folder root.
pub fn asset_meta_path(hash: ContentHash) -> String {
    format!("assets/sha256/{hash}.json")
}

/// The revision a canonical `r<NNNN>.json` file name stands for. Leading
/// zeros beyond four digits, revision 0 and other spellings are refused, so
/// one revision has exactly one file name.
pub fn parse_record_name(name: &str) -> Option<Revision> {
    let digits = name.strip_prefix('r')?.strip_suffix(".json")?;
    if digits.len() < 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: u32 = digits.parse().ok()?;
    (number >= 1 && format!("{number:04}") == digits).then_some(Revision(number))
}

/// Pretty JSON with a final newline: the git-friendly encoding of every
/// file this module writes. Key order is the types' fixed field order and
/// `BTreeMap` order.
pub(crate) fn pretty<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Read at most `max` bytes of a regular file; anything larger is refused.
pub(crate) fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let limit = u64::try_from(max).unwrap_or(u64::MAX);
    if file.metadata()?.len() > limit {
        return Err(Error::TooLarge(format!(
            "{} exceeds {max} bytes",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(Error::TooLarge(format!(
            "{} exceeds {max} bytes",
            path.display()
        )));
    }
    Ok(bytes)
}

/// A JSON error that names the file it came from (code `malformed_file`).
fn malformed(path: &str, why: impl std::fmt::Display) -> Error {
    Error::Json(<serde_json::Error as serde::de::Error>::custom(format!(
        "{path}: {why}"
    )))
}

fn conflict(message: String) -> Error {
    Error::Document(pcbcraft_core::Error::Conflict(message))
}

fn sync_directory(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(directory)
        .and_then(|d| d.sync_all())
        .map_err(|error| {
            Error::Invalid(format!(
                "a file was written, but synchronizing {} failed; power-loss durability is unconfirmed: {error}",
                directory.display()
            ))
        })?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

/// Atomically create `path` with `bytes`, never replacing an existing
/// file. `Ok(false)` when the file already exists (nothing is written).
fn create_new_atomic(path: &Path, bytes: &[u8]) -> Result<bool> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(path) {
        Ok(_) => {}
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(Error::Io(e.error)),
    }
    sync_directory(parent)?;
    Ok(true)
}

/// Atomically replace `path` (creating its directory).
fn replace_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    atomic_write(path, bytes)
}

/// Whether anything (file, directory or link) exists at `path`, without
/// following a link.
fn present(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// The fields every manifest format keeps, read leniently before the
/// strict parse, so a newer format that adds fields is reported as
/// unsupported rather than unreadable.
#[derive(Deserialize)]
struct ManifestProbe {
    format: u32,
    #[serde(default)]
    required_features: Vec<String>,
}

/// Why a manifest is not usable: unreadable, or of another format or
/// feature set (checked first, from the lenient probe).
enum ManifestError {
    Malformed(String),
    Refused(Disabled),
}

fn parse_manifest(bytes: &[u8]) -> std::result::Result<Manifest, ManifestError> {
    let probe: ManifestProbe =
        serde_json::from_slice(bytes).map_err(|e| ManifestError::Malformed(e.to_string()))?;
    check_manifest_parts(probe.format, &probe.required_features).map_err(ManifestError::Refused)?;
    let manifest: Manifest =
        serde_json::from_slice(bytes).map_err(|e| ManifestError::Malformed(e.to_string()))?;
    parse_checked(manifest).map_err(ManifestError::Malformed)
}

fn parse_checked(manifest: Manifest) -> std::result::Result<Manifest, String> {
    if !printable(&manifest.name) {
        return Err("the library name is blank, too long or has control characters".into());
    }
    if manifest
        .default_licence
        .as_deref()
        .is_some_and(|l| !printable(l))
        || manifest.required_features.iter().any(|f| !printable(f))
    {
        return Err("a licence or feature name is not printable text".into());
    }
    Ok(manifest)
}

/// Open a folder for the Browser: the folder, or why it is not searchable
/// (missing path, malformed manifest, other format, unknown feature).
pub fn inspect_folder(root: &Path) -> std::result::Result<Folder, Disabled> {
    let path = root.join(MANIFEST_FILE);
    if !path.exists() {
        return Err(Disabled::PathMissing);
    }
    let bytes =
        read_bounded(&path, MAX_RECORD_BYTES).map_err(|e| Disabled::Malformed(e.to_string()))?;
    let manifest = parse_manifest(&bytes).map_err(|e| match e {
        ManifestError::Malformed(why) => Disabled::Malformed(why),
        ManifestError::Refused(why) => why,
    })?;
    Ok(Folder {
        root: root.to_path_buf(),
        manifest,
    })
}

/// Open a folder: read and check its manifest. Refuses an unsupported
/// format or unknown required feature with `unsupported`, and a corrupt
/// manifest with `malformed_file`.
pub fn open_folder(root: &Path) -> Result<Folder> {
    let path = root.join(MANIFEST_FILE);
    let bytes = read_bounded(&path, MAX_RECORD_BYTES)?;
    let manifest = parse_manifest(&bytes).map_err(|e| match e {
        ManifestError::Malformed(why) => malformed(MANIFEST_FILE, why),
        ManifestError::Refused(why) => Error::Document(pcbcraft_core::Error::Unsupported(format!(
            "library folder {}: {why}",
            root.display()
        ))),
    })?;
    Ok(Folder {
        root: root.to_path_buf(),
        manifest,
    })
}

/// Create an empty folder library (manifest only) at `root`, which must
/// not already hold a manifest. The library id is drawn at random (D1).
pub fn create_folder(root: &Path, name: &str) -> Result<Folder> {
    if !printable(name) {
        return Err(Error::Invalid(
            "a library name must be printable text of at most 256 bytes".into(),
        ));
    }
    let path = root.join(MANIFEST_FILE);
    if present(&path) {
        return Err(conflict(format!(
            "{} already holds a library manifest",
            root.display()
        )));
    }
    let manifest = Manifest {
        format: FOLDER_FORMAT,
        library: new_library_id()?,
        name: name.to_owned(),
        required_features: Vec::new(),
        default_licence: None,
    };
    if !create_new_atomic(&path, &pretty(&manifest)?)? {
        return Err(conflict(format!(
            "{} already holds a library manifest",
            root.display()
        )));
    }
    Ok(Folder {
        root: root.to_path_buf(),
        manifest,
    })
}

/// Entries of a directory, sorted by name, without dot files (`.git`,
/// `.DS_Store`, editor swap files).
fn listing(directory: &Path) -> Result<Vec<(String, std::fs::FileType, PathBuf)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|raw| {
            Error::Invalid(format!(
                "{}: a file name is not UTF-8: {raw:?}",
                directory.display()
            ))
        })?;
        if name.starts_with('.') {
            continue;
        }
        out.push((name, entry.file_type()?, entry.path()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Read and check one record file: its name, its record's identity and
/// kind, and its bounds.
fn read_record_file(
    folder: &Folder,
    lineage: SourceLineageId,
    revision: Revision,
) -> Result<StoredRecord> {
    let path = record_path(lineage, revision);
    let bytes = read_bounded(&folder.root.join(&path), MAX_RECORD_BYTES)?;
    let record: RevisionRecord = serde_json::from_slice(&bytes).map_err(|e| malformed(&path, e))?;
    if record.lineage != lineage || record.revision != revision {
        return Err(malformed(
            &path,
            format!(
                "the file holds lineage {} revision {}",
                record.lineage, record.revision
            ),
        ));
    }
    if record.definition.kind() != record.kind {
        return Err(malformed(
            &path,
            "the record's kind disagrees with its body",
        ));
    }
    if record.links.len() > MAX_LINKS {
        return Err(Error::TooLarge(format!(
            "{path} lists more than {MAX_LINKS} links"
        )));
    }
    let content_hash = content_hash(&record)?;
    Ok(StoredRecord {
        path,
        record,
        content_hash,
    })
}

/// Every record file path of the folder with its lineage and revision,
/// sorted by path. Anything else under `lineages/` (except dot files) is
/// an error: the layout is this build's own, and a stray file usually
/// means a bad merge or copy.
pub fn record_files(folder: &Folder) -> Result<Vec<(String, SourceLineageId, Revision)>> {
    let directory = folder.root.join(LINEAGES_DIR);
    if !present(&directory) {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (name, kind, path) in listing(&directory)? {
        let lineage = SourceLineageId::parse(&name)
            .filter(|_| kind.is_dir())
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "{LINEAGES_DIR}/{name} is not a lineage directory (32 lower-case hex digits)"
                ))
            })?;
        for (file, file_kind, _) in listing(&path)? {
            let revision = parse_record_name(&file)
                .filter(|_| file_kind.is_file())
                .ok_or_else(|| {
                    Error::Invalid(format!(
                        "{LINEAGES_DIR}/{name}/{file} is not a revision file (r0001.json, …)"
                    ))
                })?;
            if out.len() >= MAX_FOLDER_RECORDS {
                return Err(Error::TooLarge(format!(
                    "a library folder holds at most {MAX_FOLDER_RECORDS} revisions"
                )));
            }
            out.push((record_path(lineage, revision), lineage, revision));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Every revision record of the folder, sorted by path, each with its
/// content hash. A file whose name disagrees with its record is an error.
pub fn read_records(folder: &Folder) -> Result<Vec<StoredRecord>> {
    record_files(folder)?
        .into_iter()
        .map(|(_, lineage, revision)| read_record_file(folder, lineage, revision))
        .collect()
}

/// The revisions one record names: exact revisions (`hard`, which must be
/// present and come first) and lineages without a revision (`soft`, such
/// as a symbol's owning component).
fn dependencies(stored: &StoredRecord) -> Result<(Vec<SourceRevision>, Vec<SourceLineageId>)> {
    let record = &stored.record;
    if record.links.is_empty() {
        // Every reference goes through a link, so there are none.
        return Ok((Vec::new(), Vec::new()));
    }
    let links: BTreeMap<DefinitionId, SourceLineageId> =
        record.links.iter().map(|l| (l.local, l.lineage)).collect();
    let resolve = |id: DefinitionId| {
        links.get(&id).copied().ok_or_else(|| {
            malformed(
                &stored.path,
                format!("reference {id} is not in the record's links"),
            )
        })
    };
    let mut hard = Vec::new();
    let mut soft = Vec::new();
    for site in references(&record.definition)? {
        match site {
            RefSite::Exact { target } => hard.push(SourceRevision {
                lineage: resolve(target.id)?,
                revision: target.revision,
            }),
            RefSite::Lineage { id, .. } => soft.push(resolve(id)?),
        }
    }
    Ok((hard, soft))
}

/// Records read so far by [`read_bundle`]: each file is read once, on
/// demand, and only the closure is read.
struct OnDemand<'a> {
    folder: &'a Folder,
    records: BTreeMap<SourceRevision, StoredRecord>,
}

impl OnDemand<'_> {
    /// The record at `at`, read from its file the first time.
    fn get(&mut self, at: SourceRevision) -> Result<&StoredRecord> {
        if !self.records.contains_key(&at) {
            // The closure is bounded; so are the files read for it.
            if self.records.len() > MAX_BUNDLE_RECORDS {
                return Err(Error::TooLarge(format!(
                    "a bundle carries at most {MAX_BUNDLE_RECORDS} revisions"
                )));
            }
            if !present(&self.folder.root.join(record_path(at.lineage, at.revision))) {
                return Err(Error::Document(pcbcraft_core::Error::NotFound(format!(
                    "the folder has no {}",
                    record_path(at.lineage, at.revision)
                ))));
            }
            let stored = read_record_file(self.folder, at.lineage, at.revision)?;
            self.records.insert(at, stored);
        }
        self.records
            .get(&at)
            .ok_or_else(|| Error::Invalid("a record read was lost".into()))
    }

    fn dependencies(
        &mut self,
        at: SourceRevision,
    ) -> Result<(Vec<SourceRevision>, Vec<SourceLineageId>)> {
        dependencies(self.get(at)?)
    }
}

/// The newest revision the folder holds of `lineage`, from its directory
/// listing (no record is read).
fn latest_revision(folder: &Folder, lineage: SourceLineageId) -> Result<Revision> {
    let name = lineage.to_string();
    let directory = folder.root.join(LINEAGES_DIR).join(&name);
    if !present(&directory) {
        return Err(Error::Document(pcbcraft_core::Error::NotFound(format!(
            "the folder has no revision of lineage {lineage}"
        ))));
    }
    let mut latest = None;
    for (file, kind, _) in listing(&directory)? {
        let revision = parse_record_name(&file)
            .filter(|_| kind.is_file())
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "{LINEAGES_DIR}/{name}/{file} is not a revision file (r0001.json, …)"
                ))
            })?;
        latest = latest.max(Some(revision));
    }
    latest.ok_or_else(|| {
        Error::Document(pcbcraft_core::Error::NotFound(format!(
            "the folder has no revision of lineage {lineage}"
        )))
    })
}

/// Append `start` and the exact revisions it reaches to `order`,
/// dependencies first (iterative depth-first post-order).
fn visit(
    start: SourceRevision,
    records: &mut OnDemand<'_>,
    done: &mut BTreeSet<SourceRevision>,
    order: &mut Vec<SourceRevision>,
    soft: &mut Vec<SourceLineageId>,
) -> Result<()> {
    if done.contains(&start) {
        return Ok(());
    }
    let mut on_path = BTreeSet::from([start]);
    let (hard, lineages) = records.dependencies(start)?;
    soft.extend(lineages);
    let mut stack = vec![(start, hard, 0usize)];
    while let Some((node, children, next)) = stack.last_mut() {
        let Some(child) = children.get(*next).copied() else {
            let node = *node;
            stack.pop();
            on_path.remove(&node);
            done.insert(node);
            order.push(node);
            if order.len() > MAX_BUNDLE_RECORDS {
                return Err(Error::TooLarge(format!(
                    "a bundle carries at most {MAX_BUNDLE_RECORDS} revisions"
                )));
            }
            continue;
        };
        *next += 1;
        if done.contains(&child) {
            continue;
        }
        if !on_path.insert(child) {
            return Err(Error::Invalid(format!(
                "{} is part of a reference cycle",
                record_path(child.lineage, child.revision)
            )));
        }
        let (hard, lineages) = records.dependencies(child)?;
        soft.extend(lineages);
        stack.push((child, hard, 0));
    }
    Ok(())
}

fn read_asset(folder: &Folder, hash: ContentHash) -> Result<PackedAsset> {
    let path = asset_path(hash);
    let data = read_bounded(&folder.root.join(&path), MAX_ASSET_BYTES)?;
    if asset_hash(&data) != hash {
        return Err(Error::Invalid(format!(
            "{path} no longer matches its content hash"
        )));
    }
    let meta_path = asset_meta_path(hash);
    let meta: AssetMeta = serde_json::from_slice(&read_bounded(
        &folder.root.join(&meta_path),
        MAX_RECORD_BYTES,
    )?)
    .map_err(|e| malformed(&meta_path, e))?;
    Ok(PackedAsset {
        hash,
        media_type: meta.media_type,
        data: AssetBytes::new(data),
    })
}

/// The bundle of `root` and its closure, read from the folder (records and
/// the assets they name). Records come dependencies first, root included.
/// A lineage named without a revision (a symbol's owning component) that
/// the exact closure does not already carry is added at its latest
/// revision, so every link resolves inside the bundle.
///
/// Only the closure is read: links are followed from the root and each
/// `lineages/<id>/rNNNN.json` is read when first reached; a lineage
/// directory is listed only to find a soft dependency's latest revision.
/// Still blocking file I/O: hosts call it off their UI thread.
pub fn read_bundle(folder: &Folder, root: SourceRevision) -> Result<Bundle> {
    let mut records = OnDemand {
        folder,
        records: BTreeMap::new(),
    };
    let mut done = BTreeSet::new();
    let mut order = Vec::new();
    let mut soft = Vec::new();
    visit(root, &mut records, &mut done, &mut order, &mut soft)?;
    while let Some(lineage) = soft.pop() {
        if done.iter().any(|r| r.lineage == lineage) {
            continue;
        }
        let revision = latest_revision(folder, lineage)?;
        visit(
            SourceRevision { lineage, revision },
            &mut records,
            &mut done,
            &mut order,
            &mut soft,
        )?;
    }
    let mut hashes = BTreeSet::new();
    let mut out = Vec::with_capacity(order.len());
    for key in order {
        let record = records
            .records
            .remove(&key)
            .map(|s| s.record)
            .ok_or_else(|| Error::Invalid("closure lost a record".into()))?;
        hashes.extend(record.assets.iter().copied());
        out.push(record);
    }
    if hashes.len() > MAX_ASSETS {
        return Err(Error::TooLarge(format!(
            "a bundle packs at most {MAX_ASSETS} assets"
        )));
    }
    let assets = hashes
        .into_iter()
        .map(|hash| read_asset(folder, hash))
        .collect::<Result<Vec<_>>>()?;
    Ok(Bundle {
        library: folder.manifest.library,
        name: folder.manifest.name.clone(),
        root,
        records: out,
        assets,
    })
}

/// Write a bundle's records and assets atomically, file by file (assets
/// first; orphans are tolerated). An existing record with an equal hash is
/// left alone; one with another hash refuses the whole write with
/// `conflict` before anything is written. A record is created with
/// no-clobber semantics, so a concurrent writer can never be overwritten.
pub fn write_bundle(folder: &Folder, bundle: &Bundle) -> Result<WriteReport> {
    validate_bundle(bundle)?;
    if bundle.library != folder.manifest.library {
        return Err(Error::Invalid(format!(
            "the bundle targets library {}, but {} is library {}",
            bundle.library,
            folder.root.display(),
            folder.manifest.library
        )));
    }
    // Plan: encode every record once, refusing a revision given twice with
    // different content.
    let mut planned: BTreeMap<String, (SourceLineageId, Revision, ContentHash, Vec<u8>)> =
        BTreeMap::new();
    for record in &bundle.records {
        if record.revision.0 == 0 {
            return Err(Error::Invalid("revision numbers start at 1".into()));
        }
        let path = record_path(record.lineage, record.revision);
        let hash = content_hash(record)?;
        if let Some(previous) = planned.get(&path) {
            if previous.2 != hash {
                return Err(Error::Invalid(format!(
                    "the bundle carries {path} twice with different content"
                )));
            }
            continue;
        }
        let bytes = pretty(record)?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(Error::TooLarge(format!(
                "{path} exceeds {MAX_RECORD_BYTES} bytes"
            )));
        }
        planned.insert(path, (record.lineage, record.revision, hash, bytes));
    }
    if record_files(folder)?.len() + planned.len() > MAX_FOLDER_RECORDS {
        return Err(Error::TooLarge(format!(
            "a library folder holds at most {MAX_FOLDER_RECORDS} revisions"
        )));
    }
    // Check: existing records must hash equal (rN is immutable).
    let mut report = WriteReport::default();
    let mut pending = Vec::new();
    for (path, (lineage, revision, hash, bytes)) in &planned {
        if present(&folder.root.join(path)) {
            let existing = read_record_file(folder, *lineage, *revision).map_err(|e| {
                conflict(format!(
                    "{path} already exists and cannot be read ({e}); revisions are immutable"
                ))
            })?;
            if existing.content_hash != *hash {
                return Err(conflict(format!(
                    "{path} already exists with other content; revisions are immutable, publish a new revision"
                )));
            }
            report.unchanged.push(path.clone());
        } else {
            pending.push((path, *hash, bytes));
        }
    }
    // Check: every asset a record names is in the bundle or the folder.
    let packed: BTreeMap<ContentHash, &PackedAsset> =
        bundle.assets.iter().map(|a| (a.hash, a)).collect();
    for record in &bundle.records {
        for hash in &record.assets {
            if !packed.contains_key(hash) {
                read_asset(folder, *hash).map_err(|e| {
                    Error::Invalid(format!(
                        "{} names asset {hash}, which neither the bundle nor the folder holds ({e})",
                        record_path(record.lineage, record.revision)
                    ))
                })?;
            }
        }
    }
    let mut asset_writes = Vec::new();
    for asset in &bundle.assets {
        let meta_path = folder.root.join(asset_meta_path(asset.hash));
        let meta = AssetMeta {
            media_type: asset.media_type.clone(),
        };
        if present(&meta_path) {
            let existing: AssetMeta =
                serde_json::from_slice(&read_bounded(&meta_path, MAX_RECORD_BYTES)?)
                    .map_err(|e| malformed(&asset_meta_path(asset.hash), e))?;
            if existing != meta {
                return Err(conflict(format!(
                    "asset {} is stored as {}, not {}",
                    asset.hash, existing.media_type, meta.media_type
                )));
            }
        }
        let data_path = folder.root.join(asset_path(asset.hash));
        let intact = present(&data_path)
            && read_bounded(&data_path, MAX_ASSET_BYTES)
                .is_ok_and(|bytes| asset_hash(&bytes) == asset.hash);
        if !intact || !present(&meta_path) {
            asset_writes.push((asset, data_path, meta_path, meta, intact));
        }
    }
    // Write: assets, then records.
    for (asset, data_path, meta_path, meta, intact) in asset_writes {
        if !intact {
            // Content-addressed: replacing missing or damaged bytes with the
            // bytes that hash to the name is always safe.
            replace_atomic(&data_path, &asset.data[..])?;
        }
        if !present(&meta_path) {
            create_new_atomic(&meta_path, &pretty(&meta)?)?;
        }
        report.assets.push(asset.hash);
    }
    for (path, hash, bytes) in pending {
        if create_new_atomic(&folder.root.join(path), bytes)? {
            report.written.push(path.clone());
        } else {
            // Created by someone else since the check.
            let (lineage, revision) = planned
                .get(path)
                .map(|p| (p.0, p.1))
                .ok_or_else(|| Error::Invalid("plan lost a record".into()))?;
            if read_record_file(folder, lineage, revision)?.content_hash != hash {
                return Err(conflict(format!(
                    "{path} was created concurrently with other content"
                )));
            }
            report.unchanged.push(path.clone());
        }
    }
    report.unchanged.sort();
    Ok(report)
}

fn check_labels(labels: &CatalogLabels) -> Result<()> {
    if labels.lineages.len() > MAX_FOLDER_RECORDS {
        return Err(Error::TooLarge(format!(
            "{CATALOG_FILE} labels at most {MAX_FOLDER_RECORDS} lineages"
        )));
    }
    for (lineage, label) in &labels.lineages {
        if label.tags.len() > MAX_TAGS {
            return Err(Error::TooLarge(format!(
                "lineage {lineage} has more than {MAX_TAGS} tags"
            )));
        }
        if label.shelf.as_deref().is_some_and(|s| !printable(s))
            || label.tags.iter().any(|t| !printable(t))
        {
            return Err(Error::Invalid(format!(
                "lineage {lineage} has a blank, overlong or non-printable shelf or tag"
            )));
        }
    }
    Ok(())
}

/// Read `catalog.json`; a missing file is empty labels.
pub fn read_labels(folder: &Folder) -> Result<CatalogLabels> {
    let path = folder.root.join(CATALOG_FILE);
    if !present(&path) {
        return Ok(CatalogLabels {
            format: CATALOG_FORMAT,
            lineages: BTreeMap::new(),
        });
    }
    let labels: CatalogLabels = serde_json::from_slice(&read_bounded(&path, MAX_RECORD_BYTES)?)
        .map_err(|e| malformed(CATALOG_FILE, e))?;
    if labels.format != CATALOG_FORMAT {
        return Err(Error::Document(pcbcraft_core::Error::Unsupported(format!(
            "{CATALOG_FILE} has unsupported format {}",
            labels.format
        ))));
    }
    check_labels(&labels)?;
    Ok(labels)
}

/// Atomically replace `catalog.json` (labels are mutable, D13).
pub fn write_labels(folder: &Folder, labels: &CatalogLabels) -> Result<()> {
    if labels.format != CATALOG_FORMAT {
        return Err(Error::Invalid(format!(
            "{CATALOG_FILE} is written in format {CATALOG_FORMAT}"
        )));
    }
    check_labels(labels)?;
    let bytes = pretty(labels)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(Error::TooLarge(format!("{CATALOG_FILE} is too large")));
    }
    replace_atomic(&folder.root.join(CATALOG_FILE), &bytes)
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

#[cfg(test)]
#[path = "library_folder_tests.rs"]
pub(crate) mod folder_tests;
