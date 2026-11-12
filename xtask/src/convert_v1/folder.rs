//! A library folder: every revision record under `lineages/` is converted
//! as JSON, checked by reading it as the app does, and rewritten in the
//! folder's own encoding (pretty JSON, a final newline). Everything is
//! converted and checked before anything is written; the original folder
//! is copied to `DIR.pre-nm` first (never overwritten). Lineages,
//! revisions, assets and labels are unchanged, so every pin to a revision
//! still names it; only content hashes change, and projects recompute the
//! ones they remember when they are converted ([`super::identity`]).
use super::library;
use crate::Result;
use pcbcraft_core::library::bundle::RevisionRecord;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Bytes of one revision file (`library_folder::MAX_RECORD_BYTES`).
const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;

/// What converting a folder did.
#[derive(Debug, Default)]
pub struct Converted {
    /// Revision files rewritten.
    pub records: usize,
    /// Stored values rewritten in them.
    pub values: usize,
}

/// Converts the library folder at `root` in place.
pub fn convert(root: &Path, backup: &Path) -> Result<Converted> {
    let folder = pcbcraft_io::library_folder::open_folder(root)?;
    let mut rewritten: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    let mut out = Converted::default();
    for (relative, _, _) in pcbcraft_io::library_folder::record_files(&folder)? {
        let path = root.join(&relative);
        if fs::metadata(&path)?.len() > MAX_RECORD_BYTES {
            return Err(format!("{relative} exceeds {MAX_RECORD_BYTES} bytes").into());
        }
        let mut record: Value = serde_json::from_slice(&fs::read(&path)?)?;
        let object = record
            .as_object_mut()
            .ok_or_else(|| format!("{relative} is not a revision record"))?;
        let values = library::record(object).map_err(|e| format!("{relative}: {e}"))?;
        if values == 0 {
            continue;
        }
        let typed: RevisionRecord = serde_json::from_value(record)
            .map_err(|e| format!("{relative}: the converted record does not read: {e}"))?;
        let mut bytes = serde_json::to_vec_pretty(&typed)?;
        bytes.push(b'\n');
        rewritten.push((path, bytes));
        out.records += 1;
        out.values += values;
    }
    if rewritten.is_empty() {
        return Ok(out);
    }
    if !backup.exists() {
        copy_dir(root, backup)?;
    }
    for (path, bytes) in &rewritten {
        pcbcraft_io::persistence::atomic_write(path, bytes)?;
    }
    // Read back as the app does: every record parses and hashes.
    pcbcraft_io::library_folder::read_records(&folder)?;
    Ok(out)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
