//! The `.pcbcraft` ZIP container (slice D0, founder decision F5).
//!
//! A container holds exactly:
//! - `project.json`: the editor file, every packed asset's bytes left empty;
//! - `assets/sha256/<hash>`: the raw bytes of each packed asset;
//! - in a recovery snapshot only, `recovery.json`: the recovery metadata.
//!
//! Entries are stored or deflated. Anything else (an unknown, duplicate,
//! directory, link or encrypted entry, another compression method, a
//! missing, unreferenced or mismatched asset) refuses the file and names
//! the entry: nothing is dropped silently. Every read is bounded by the
//! `pcbcraft_core::format::CONTAINER_*` limits, measured on the bytes that
//! actually decompress rather than on the sizes the archive declares.
//! Writes go through the atomic path of [`super::atomic_write_with`].
use crate::{Error, Result};
use pcbcraft_core::format::{
    self, CONTAINER_ASSET_PREFIX, CONTAINER_DOCUMENT, CONTAINER_MAX_ASSET_BYTES,
    CONTAINER_MAX_BYTES, CONTAINER_MAX_ENTRIES, CONTAINER_RECOVERY, EditorFile, MAX_FILE_BYTES,
};
use pcbcraft_core::ids::ContentHash;
use pcbcraft_core::library::bundle::AssetBytes;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// The first bytes of a ZIP local file header.
const ZIP_MAGIC: &[u8; 4] = b"PK\x03\x04";

/// Read and validate a project container.
pub fn read_container(path: &Path) -> Result<EditorFile> {
    Ok(read(path, Role::Project, Load::Everything)?.file)
}

/// Encode `file` (assets as raw entries) and atomically replace `path`.
pub fn write_container(path: &Path, file: &EditorFile) -> Result<()> {
    write(path, file, None, Method::Deflated)
}

/// Whether a container is a project or a recovery snapshot, which holds
/// one more entry.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Project,
    Recovery,
}

/// How much of a container to read.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Load {
    /// Every entry, every asset hash checked, the whole file validated.
    Everything,
    /// The document and metadata only, for listings: asset entries are
    /// checked by name and declared size but not read, and the document is
    /// parsed but not validated. Its packed assets keep empty bytes.
    Summary,
}

/// How asset entries are compressed. The document is always deflated.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    Deflated,
    /// Recovery snapshots store assets, so frequent snapshots stay cheap.
    Stored,
}

pub(crate) struct Contents {
    pub file: EditorFile,
    /// The `recovery.json` bytes of a recovery snapshot.
    pub recovery: Option<Vec<u8>>,
    /// Bytes decompressed while reading.
    pub read_bytes: u64,
}

pub(crate) fn read(path: &Path, role: Role, load: Load) -> Result<Contents> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    if length > CONTAINER_MAX_BYTES {
        return Err(Error::TooLarge(format!(
            "project container exceeds {} MiB",
            CONTAINER_MAX_BYTES / (1024 * 1024)
        )));
    }
    let mut magic = Vec::with_capacity(ZIP_MAGIC.len());
    (&mut file)
        .take(ZIP_MAGIC.len() as u64)
        .read_to_end(&mut magic)?;
    if magic != ZIP_MAGIC {
        return Err(Error::Invalid(
            concat!(
                "container.format: not a ",
                pcbcraft_core::product_name!(),
                " project container (a .pcbcraft file is a ZIP archive holding project.json)"
            )
            .into(),
        ));
    }
    file.rewind()?;
    let mut archive = ZipArchive::new(file).map_err(malformed)?;
    if archive.len() > CONTAINER_MAX_ENTRIES {
        return Err(Error::TooLarge(format!(
            "project container holds more than {CONTAINER_MAX_ENTRIES} entries"
        )));
    }
    if archive.has_duplicate_names() {
        return Err(Error::Invalid(
            "container.entry: the container names one entry twice".into(),
        ));
    }
    let mut document = None;
    let mut recovery = None;
    let mut assets: BTreeMap<ContentHash, AssetBytes> = BTreeMap::new();
    let mut declared: BTreeMap<ContentHash, u64> = BTreeMap::new();
    let mut total: u64 = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(malformed)?;
        let name = entry.name().map_err(malformed)?.into_owned();
        if entry.is_dir() || entry.is_symlink() {
            return Err(entry_error(&name, "is a directory or a link"));
        }
        if entry.encrypted() {
            return Err(entry_error(&name, "is encrypted"));
        }
        if !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(entry_error(
                &name,
                "uses a compression method other than stored or deflated",
            ));
        }
        let kind = if name == CONTAINER_DOCUMENT {
            Entry::Document
        } else if name == CONTAINER_RECOVERY && role == Role::Recovery {
            Entry::Recovery
        } else if let Some(hash) = asset_entry_hash(&name) {
            Entry::Asset(hash)
        } else {
            return Err(entry_error(&name, "is not part of the project format"));
        };
        let limit = match kind {
            Entry::Document => MAX_FILE_BYTES as u64,
            Entry::Recovery => format::CONTAINER_MAX_RECOVERY_BYTES as u64,
            Entry::Asset(_) => CONTAINER_MAX_ASSET_BYTES,
        };
        if entry.size() > limit {
            return Err(Error::TooLarge(format!(
                "container entry {name} exceeds {limit} bytes"
            )));
        }
        if let Entry::Asset(hash) = kind
            && load == Load::Summary
        {
            total = add_total(total, entry.size())?;
            declared.insert(hash, entry.size());
            continue;
        }
        let bytes = read_bounded(&mut entry, limit, &name)?;
        total = add_total(total, bytes.len() as u64)?;
        match kind {
            Entry::Document => document = Some(bytes),
            Entry::Recovery => recovery = Some(bytes),
            Entry::Asset(hash) => {
                // Hashed once here; the shared bytes remember it, so the
                // file's validation does not hash them again.
                let bytes = AssetBytes::new(bytes);
                if bytes.hash() != hash {
                    return Err(entry_error(&name, "does not match its content hash"));
                }
                declared.insert(hash, bytes.len() as u64);
                assets.insert(hash, bytes);
            }
        }
    }
    let document = document.ok_or_else(|| {
        Error::Invalid(format!(
            "container.entry: the container has no {CONTAINER_DOCUMENT}"
        ))
    })?;
    if role == Role::Recovery && recovery.is_none() {
        return Err(Error::Invalid(format!(
            "container.entry: the recovery snapshot has no {CONTAINER_RECOVERY}"
        )));
    }
    let read_bytes = (document.len() as u64)
        .saturating_add(recovery.as_ref().map_or(0, |r| r.len() as u64))
        .saturating_add(assets.values().map(|a| a.len() as u64).sum());
    let mut file = format::decode_container_document(&document)?;
    for asset in &mut file.project.library.assets {
        if declared.remove(&asset.hash).is_none() {
            return Err(Error::Invalid(format!(
                "container.asset_missing: asset {} is missing from the container (expected entry {})",
                asset.hash,
                asset_name(asset.hash)
            )));
        }
        if let Some(bytes) = assets.remove(&asset.hash) {
            asset.data = bytes;
        }
    }
    if let Some(hash) = declared.keys().next() {
        return Err(entry_error(
            &asset_name(*hash),
            "is not an asset the project packs",
        ));
    }
    if load == Load::Everything {
        file.validate()?;
    }
    Ok(Contents {
        file,
        recovery,
        read_bytes,
    })
}

/// Validate `file` and atomically write it as a container to `path`, with
/// `recovery` as the metadata entry of a recovery snapshot.
pub(crate) fn write(
    path: &Path,
    file: &EditorFile,
    recovery: Option<&[u8]>,
    assets: Method,
) -> Result<()> {
    let document = format::encode_container_document(file)?;
    if recovery.is_some_and(|r| r.len() > format::CONTAINER_MAX_RECOVERY_BYTES) {
        return Err(Error::TooLarge(
            "recovery metadata exceeds its entry limit".into(),
        ));
    }
    let mut total = add_total(0, document.len() as u64)?;
    total = add_total(total, recovery.map_or(0, |r| r.len() as u64))?;
    for asset in &file.project.library.assets {
        if asset.data.len() as u64 > CONTAINER_MAX_ASSET_BYTES {
            return Err(Error::TooLarge(format!(
                "asset {} exceeds {CONTAINER_MAX_ASSET_BYTES} bytes",
                asset.hash
            )));
        }
        total = add_total(total, asset.data.len() as u64)?;
    }
    super::atomic_write_with(path, |target| {
        let mut zip = ZipWriter::new(target);
        let deflated = options(CompressionMethod::Deflated);
        zip.start_file(CONTAINER_DOCUMENT, deflated)
            .map_err(malformed)?;
        zip.write_all(&document)?;
        if let Some(recovery) = recovery {
            zip.start_file(CONTAINER_RECOVERY, deflated)
                .map_err(malformed)?;
            zip.write_all(recovery)?;
        }
        let method = match assets {
            Method::Deflated => CompressionMethod::Deflated,
            Method::Stored => CompressionMethod::Stored,
        };
        for asset in &file.project.library.assets {
            zip.start_file(asset_name(asset.hash), options(method))
                .map_err(malformed)?;
            zip.write_all(&asset.data[..])?;
        }
        zip.finish().map_err(malformed)?;
        Ok(())
    })
}

/// The entry name of the asset with content hash `hash`.
pub fn asset_name(hash: ContentHash) -> String {
    format!("{CONTAINER_ASSET_PREFIX}{hash}")
}

/// The content hash an asset entry name carries, if `name` is one: the
/// prefix and exactly 64 lower-case hex digits.
pub fn asset_entry_hash(name: &str) -> Option<ContentHash> {
    name.strip_prefix(CONTAINER_ASSET_PREFIX)
        .and_then(ContentHash::parse)
}

enum Entry {
    Document,
    Recovery,
    Asset(ContentHash),
}

/// Reproducible entries: a fixed timestamp and ordinary file permissions.
fn options(method: CompressionMethod) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(method)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644)
}

/// Read at most `limit` decompressed bytes. Reading to the end lets the
/// archive check the entry's CRC.
fn read_bounded(entry: &mut impl Read, limit: u64, name: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    entry
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| entry_error(name, &format!("cannot be read ({error})")))?;
    if bytes.len() as u64 > limit {
        return Err(Error::TooLarge(format!(
            "container entry {name} exceeds {limit} bytes"
        )));
    }
    Ok(bytes)
}

fn add_total(total: u64, bytes: u64) -> Result<u64> {
    total
        .checked_add(bytes)
        .filter(|t| *t <= CONTAINER_MAX_BYTES)
        .ok_or_else(|| {
            Error::TooLarge(format!(
                "project container exceeds {} MiB uncompressed",
                CONTAINER_MAX_BYTES / (1024 * 1024)
            ))
        })
}

fn entry_error(name: &str, problem: &str) -> Error {
    let name: String = name.chars().take(160).collect();
    let kind = if name.starts_with(CONTAINER_ASSET_PREFIX) {
        "container.asset"
    } else {
        "container.entry"
    };
    Error::Invalid(format!("{kind}: container entry {name:?} {problem}"))
}

fn malformed(error: zip::result::ZipError) -> Error {
    match error {
        zip::result::ZipError::Io(error) => Error::Io(error),
        other => Error::Invalid(format!("container.malformed: {other}")),
    }
}

#[cfg(test)]
pub(crate) mod tests;
