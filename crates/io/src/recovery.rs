//! Independent, bounded recovery snapshots. Each live session owns a unique file;
//! restoring never writes to the original project path.
use crate::{Error, Result, persistence};
use pcbcraft_core::format::{EditorFile, encode_bounded};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const PREFIX: &str = "session-";
const SUFFIX: &str = ".pcbcraft-recovery";
const MAX_ENTRIES: usize = 128;
const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRecord {
    pub version: u32,
    pub document: EditorFile,
    pub original_path: Option<PathBuf>,
    pub saved_unix_ms: u64,
}

#[derive(Clone, Debug)]
pub struct RecoveryEntry {
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub saved_unix_ms: u64,
    pub project_name: String,
    /// Component instances the recovered project holds, so sessions of one
    /// unnamed project can be told apart.
    pub instances: usize,
    pub error: Option<String>,
}

pub struct RecoveryStore {
    directory: PathBuf,
    path: PathBuf,
}
impl RecoveryStore {
    pub fn new(directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        let directory = directory.canonicalize()?;
        // create_new-backed reservation prevents two sessions from sharing a name.
        let reservation = tempfile::Builder::new()
            .prefix(PREFIX)
            .suffix(SUFFIX)
            .tempfile_in(&directory)?;
        let (file, path) = reservation.keep().map_err(|e| Error::Io(e.error))?;
        drop(file);
        Ok(Self { directory, path })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn write(&self, document: &EditorFile, original_path: Option<&Path>) -> Result<()> {
        document.validate()?;
        if original_path.is_some_and(|p| p.as_os_str().len() > 4096) {
            return Err(Error::Invalid("recovery original path is too long".into()));
        }
        match std::fs::symlink_metadata(&self.path) {
            Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
                return Err(Error::Invalid(
                    "recovery path is not a regular session file".into(),
                ));
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        let saved_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Invalid("system clock precedes the Unix epoch".into()))?
            .as_millis();
        let saved_unix_ms = u64::try_from(saved_unix_ms)
            .map_err(|_| Error::Invalid("recovery timestamp exceeds supported range".into()))?;
        let record = RecoveryRecord {
            version: 1,
            document: document.clone(),
            original_path: original_path.map(Path::to_path_buf),
            saved_unix_ms,
        };
        persistence::atomic_write(&self.path, &encode_bounded(&record)?)
    }
    pub fn remove_own(&self) -> Result<()> {
        remove_entry(&self.directory, &self.path)
    }
}

pub fn read(path: &Path) -> Result<RecoveryRecord> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error::Invalid(
            "recovery entry must be a regular file, not a link".into(),
        ));
    }
    let record: RecoveryRecord = serde_json::from_slice(&persistence::read_bytes(path)?)?;
    if record.version != 1 {
        return Err(Error::Invalid("unsupported recovery record version".into()));
    }
    if record
        .original_path
        .as_ref()
        .is_some_and(|p| p.as_os_str().len() > 4096)
    {
        return Err(Error::Invalid("recovery original path is too long".into()));
    }
    record.document.validate()?;
    Ok(record)
}

fn session_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|name| {
            name.starts_with(PREFIX)
                && name.ends_with(SUFFIX)
                && name.len() > PREFIX.len() + SUFFIX.len()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        })
}

/// Corrupt records remain visible with an error so the user can discard one
/// explicitly without losing access to valid recovery candidates.
pub fn list(directory: &Path) -> Result<Vec<RecoveryEntry>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };
    let mut result = Vec::new();
    let mut scanned = 0usize;
    let mut bytes = 0u64;
    for entry in entries {
        let path = entry?.path();
        if !session_name(&path) {
            continue;
        }
        scanned += 1;
        if scanned > MAX_ENTRIES {
            return Err(Error::Invalid(
                "recovery directory exceeds the 128-entry scan limit".into(),
            ));
        }
        let meta = std::fs::symlink_metadata(&path)?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            continue;
        }
        if meta.len() == 0 {
            continue;
        } // reserved by a session awaiting its first dirty snapshot
        bytes = bytes.saturating_add(meta.len());
        if bytes > MAX_SCAN_BYTES {
            return Err(Error::Invalid(
                "recovery directory exceeds the 64 MiB scan limit".into(),
            ));
        }
        match read(&path) {
            Ok(record) => result.push(RecoveryEntry {
                path,
                original_path: record.original_path,
                saved_unix_ms: record.saved_unix_ms,
                instances: record.document.project.instances.len(),
                project_name: record.document.project.name,
                error: None,
            }),
            Err(error) => result.push(RecoveryEntry {
                path,
                original_path: None,
                saved_unix_ms: 0,
                project_name: "Unreadable recovery".into(),
                instances: 0,
                error: Some(error.to_string()),
            }),
        }
    }
    result.sort_by(|a, b| {
        b.saved_unix_ms
            .cmp(&a.saved_unix_ms)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(result)
}

/// Only an explicitly selected, direct-child session file can be discarded.
/// Symlinks, directories, unrelated filenames and paths outside the recovery
/// directory are rejected. Missing owned entries are harmless after cleanup.
pub fn remove_entry(directory: &Path, path: &Path) -> Result<()> {
    if !session_name(path) {
        return Err(Error::Invalid("not a recovery session filename".into()));
    }
    let directory = directory.canonicalize()?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("recovery path has no parent".into()))?
        .canonicalize()?;
    if parent != directory {
        return Err(Error::Invalid(
            "recovery entry is outside its directory".into(),
        ));
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error::Invalid(
            "recovery discard refuses links and directories".into(),
        ));
    }
    std::fs::remove_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests;
