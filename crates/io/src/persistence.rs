//! Project files on disk. Writes are atomic: a failed save leaves any existing
//! file untouched and no temporary file behind.
use crate::{Error, Result};
use pcbcraft_core::format::{self, EditorFile, MAX_FILE_BYTES};
use std::{
    io::{Read, Write},
    path::Path,
};

pub mod container;

pub fn check_project_path(path: &Path) -> Result<()> {
    if path.extension().is_none_or(|e| e != format::EXTENSION) {
        return Err(Error::Invalid("use a .pcbcraft project filename".into()));
    }
    if path.as_os_str().len() > 4096 {
        return Err(Error::Invalid("project path is too long".into()));
    }
    Ok(())
}

/// Read and validate a project file. The pre-release format has one version and no migrations.
pub fn read_project(path: &Path) -> Result<EditorFile> {
    check_project_path(path)?;
    Ok(format::decode(&read_bytes(path)?)?)
}

/// Validate, encode and atomically replace a project file.
pub fn write_project(path: &Path, file: &EditorFile) -> Result<()> {
    check_project_path(path)?;
    atomic_write(path, &format::encode(file)?)
}

pub fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_FILE_BYTES as u64 {
        return Err(Error::TooLarge("file exceeds 16 MiB".into()));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::TooLarge("file exceeds 16 MiB".into()));
    }
    Ok(bytes)
}

/// Write to a sibling temporary file, flush it, then rename it over `path`.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    write_via_temporary(path, bytes, true)
}

/// Like [`atomic_write`], but `path` must not exist: the finished temporary
/// file is linked into place without replacing anything, so a file created
/// at `path` meanwhile is never overwritten. That case is an `Io` error of
/// kind `AlreadyExists`, and no temporary file is left behind.
pub fn atomic_create(path: &Path, bytes: &[u8]) -> Result<()> {
    write_via_temporary(path, bytes, false)
}

fn write_via_temporary(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::TooLarge("file exceeds 16 MiB".into()));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    if replace {
        temporary.persist(path).map_err(|e| Error::Io(e.error))?;
    } else {
        temporary
            .persist_noclobber(path)
            .map_err(|e| Error::Io(e.error))?;
    }
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            Error::Invalid(format!(
                "The file was written, but synchronizing its directory failed; power-loss durability is unconfirmed: {error}"
            ))
        })?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
