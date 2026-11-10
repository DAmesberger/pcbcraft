//! A `.pcbcraft` project container: its document is converted as JSON,
//! then read back through `pcbcraft-io` as the app reads it (typed and
//! validated, so a conversion that missed a field fails here and writes
//! nothing), its remembered hashes are recomputed ([`identity`]), and it is
//! written as the app writes it.
use super::{identity, library, schematic};
use crate::Result;
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const DOCUMENT: &str = "project.json";
/// Bytes of a container's document, as `format::MAX_FILE_BYTES` bounds it.
const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;

/// What converting one container did.
#[derive(Debug, Default)]
pub struct Converted {
    /// Stored values rewritten.
    pub values: usize,
    pub identity: identity::Refreshed,
}

/// The document's stored values converted as JSON; the number changed.
pub fn convert_document(file: &mut Value) -> Result<usize> {
    if file.get("version") != Some(&Value::from(1)) {
        return Err("not a version 1 Pcbcraft document".into());
    }
    let mut n = 0;
    let project = file
        .get_mut("project")
        .and_then(Value::as_object_mut)
        .ok_or("the document has no project")?;
    n += schematic::convert_project(project)?;
    n += library::project(project)?;
    if let Some(library) = project.get_mut("library").and_then(Value::as_object_mut) {
        n += library::library(library)?;
    }
    if let Some(drafts) = file.get_mut("drafts").and_then(Value::as_object_mut) {
        n += library::drafts(drafts)?;
    }
    Ok(n)
}

/// Converts one container in place, keeping the original as
/// `FILE.pre-nm` (never overwritten).
pub fn convert(path: &Path, backup: &Path) -> Result<Converted> {
    let original = fs::read(path)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&original[..]))?;
    let mut document = Vec::new();
    archive
        .by_name(DOCUMENT)?
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut document)?;
    if document.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(format!("{DOCUMENT} exceeds {MAX_DOCUMENT_BYTES} bytes").into());
    }
    let mut file: Value = serde_json::from_slice(&document)?;
    let values = convert_document(&mut file)?;

    // The converted JSON in a container beside the file, read as the app
    // reads it.
    let staged = sibling(path, ".convert-tmp.pcbcraft");
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i)?;
        if entry.name()? == DOCUMENT {
            drop(entry);
            out.start_file(DOCUMENT, options)?;
            out.write_all(&serde_json::to_vec(&file)?)?;
        } else {
            out.raw_copy_file(entry)?;
        }
    }
    fs::write(&staged, out.finish()?.into_inner())?;
    let read = pcbcraft_io::read_project(&staged);
    fs::remove_file(&staged)?;
    let mut typed = read.map_err(|e| format!("the converted document does not read: {e}"))?;
    let identity = identity::refresh(&mut typed)?;
    let converted = Converted { values, identity };
    if converted.values + converted.identity.changed() == 0 {
        return Ok(converted);
    }
    if !backup.exists() {
        fs::write(backup, &original)?;
    }
    pcbcraft_io::write_project(path, &typed)?;
    Ok(converted)
}

/// `path` with `suffix` appended to its file name.
pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}
