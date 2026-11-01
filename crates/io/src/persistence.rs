//! Project files on disk. A project is a ZIP container (see [`container`]).
//! Writes are atomic: a failed save leaves any existing file untouched and
//! no temporary file behind.
use crate::{Error, Result};
use pcbcraft_core::format::{self, EditorFile, MAX_FILE_BYTES};
use std::fs::File;
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

/// Read and validate a project container. The pre-release format has one
/// version and no migrations; anything else is refused.
pub fn read_project(path: &Path) -> Result<EditorFile> {
    check_project_path(path)?;
    container::read_container(path)
}

/// Validate, encode and atomically replace a project container.
pub fn write_project(path: &Path, file: &EditorFile) -> Result<()> {
    check_project_path(path)?;
    container::write_container(path, file)
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
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::TooLarge("file exceeds 16 MiB".into()));
    }
    atomic_write_with(path, |file| Ok(file.write_all(bytes)?))
}

/// Let `fill` write a sibling temporary file, sync it, then rename it over
/// `path`. When `fill` fails, the temporary file is removed and `path` is
/// untouched. `fill` bounds what it writes.
pub fn atomic_write_with(path: &Path, fill: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    fill(temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| Error::Io(e.error))?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            Error::Invalid(format!(
                "The file was replaced, but synchronizing its directory failed; power-loss durability is unconfirmed: {error}"
            ))
        })?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
