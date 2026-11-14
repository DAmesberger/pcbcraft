//! A `.sparksmith` project container: its document goes through the
//! version chain as JSON ([`crate::convert::upgrade`]), is read back as the
//! app reads it (typed and validated, so a step that missed a field fails
//! here and writes nothing), has its remembered hashes recomputed
//! ([`identity`]), and is written as the app writes it.
use super::{identity, library, schematic};
use crate::convert::{Options, Result, Upgraded, upgrade};
use serde_json::Value;
use sparksmith_core::format::{CONTAINER_DOCUMENT, FILE_VERSION, MAX_FILE_BYTES};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// What converting one container did.
#[derive(Debug, Default)]
pub struct Converted {
    /// The version the file had; [`FILE_VERSION`] when it was current.
    pub from: u32,
    /// Stored values rewritten.
    pub values: usize,
    pub identity: identity::Refreshed,
    /// What a step says beyond counts, one line each.
    pub notes: Vec<String>,
    /// Where the original was kept.
    pub backup: Option<PathBuf>,
}

/// Step 1 to 2 of a document: stored f64 millimetres and degrees become
/// integer nanometres and millidegrees (U1 schematic, U2 library
/// geometry) and fab profiles become files ([`super::rules`]); the number
/// of values changed. Values already in the new form are left alone, since
/// version 1 files were written both before and after those slices.
pub fn step(file: &mut Value, options: &Options) -> Result<usize> {
    let mut n = 0;
    let project = file
        .get_mut("project")
        .and_then(Value::as_object_mut)
        .ok_or("the document has no project")?;
    n += schematic::convert_project(project)?;
    n += super::rules::convert_project_rules(project, options.profile.as_ref())?;
    n += library::project(project)?;
    if let Some(library) = project.get_mut("library").and_then(Value::as_object_mut) {
        n += library::library(library)?;
    }
    if let Some(drafts) = file.get_mut("drafts").and_then(Value::as_object_mut) {
        n += library::drafts(drafts)?;
    }
    Ok(n)
}

/// Converts one container in place to [`FILE_VERSION`], keeping the
/// original as `FILE.v<N>` (never overwritten). A current file is left
/// alone.
pub fn convert(path: &Path, options: &Options) -> Result<Converted> {
    let original = fs::read(path)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&original[..]))?;
    let mut document = Vec::new();
    archive
        .by_name(CONTAINER_DOCUMENT)?
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut document)?;
    if document.len() > MAX_FILE_BYTES {
        return Err(format!("{CONTAINER_DOCUMENT} exceeds {MAX_FILE_BYTES} bytes").into());
    }
    let mut file: Value = serde_json::from_slice(&document)?;
    let notes = match file.get("version").and_then(Value::as_u64) {
        Some(version) if version < 3 => crate::convert::v2::notes(&file),
        _ => Vec::new(),
    };
    let Upgraded { from, values } = upgrade(&mut file, options)?;
    if from == FILE_VERSION {
        return Ok(Converted {
            from,
            ..Converted::default()
        });
    }

    // The converted JSON in a container beside the file, read as the app
    // reads it.
    let staged = sibling(path, ".convert-tmp.sparksmith");
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let zip_options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i)?;
        if entry.name()? == CONTAINER_DOCUMENT {
            drop(entry);
            out.start_file(CONTAINER_DOCUMENT, zip_options)?;
            out.write_all(&serde_json::to_vec(&file)?)?;
        } else {
            out.raw_copy_file(entry)?;
        }
    }
    fs::write(&staged, out.finish()?.into_inner())?;
    let read = crate::read_project(&staged);
    fs::remove_file(&staged)?;
    let mut typed = read.map_err(|e| format!("the converted document does not read: {e}"))?;
    let identity = identity::refresh(&mut typed)?;
    let backup = sibling(path, &format!(".v{from}"));
    if !backup.exists() {
        fs::write(&backup, &original)?;
    }
    crate::write_project(path, &typed)?;
    Ok(Converted {
        from,
        values,
        identity,
        notes,
        backup: Some(backup),
    })
}

/// `path` with `suffix` appended to its file name.
pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}
