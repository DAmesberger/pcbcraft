//! Project files on disk. A project is a ZIP container (see [`container`]).
//! Writes are atomic: a failed save leaves any existing file untouched and
//! no temporary file behind.
use crate::{Error, Result};
use sparksmith_core::format::{self, EditorFile, MAX_FILE_BYTES};
use std::fs::File;
use std::{
    io::{Read, Write},
    path::Path,
};

pub mod container;

pub fn check_project_path(path: &Path) -> Result<()> {
    // `.pcbcraft` (from before the rename) holds the same format: it opens
    // through the same reader and version check, and Save keeps its name.
    if path
        .extension()
        .is_none_or(|e| !format::EXTENSIONS.iter().any(|known| e == *known))
    {
        return Err(Error::Invalid("use a .sparksmith project filename".into()));
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
/// On Unix a new file gets the usual permissions (0666 less the umask) and a
/// replaced file keeps its own.
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
    write_with(path, replace, Access::Shared, |file| {
        Ok(file.write_all(bytes)?)
    })
}

/// Who may read a file the atomic writers make. Unix only; elsewhere the
/// platform's defaults apply.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Access {
    /// A new file gets 0666 less the umask, as any program's file would; a
    /// replaced file keeps its permissions.
    Shared,
    /// Owner only (0600), new or replaced: recovery snapshots.
    Private,
}

/// Let `fill` write a sibling temporary file, sync it, then rename it over
/// `path`. When `fill` fails, the temporary file is removed and `path` is
/// untouched. `fill` bounds what it writes. Permissions as for
/// [`atomic_write`].
pub fn atomic_write_with(path: &Path, fill: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    write_with(path, true, Access::Shared, fill)
}

/// [`atomic_write_with`], replacing `path` only when `replace` is set;
/// otherwise an existing `path` is an `AlreadyExists` error.
pub(crate) fn write_with(
    path: &Path,
    replace: bool,
    access: Access,
    fill: impl FnOnce(&mut File) -> Result<()>,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = temporary_in(parent, path, access)?;
    fill(temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;
    if replace {
        temporary.persist(path).map_err(|e| Error::Io(e.error))?;
    } else {
        temporary
            .persist_noclobber(path)
            .map_err(|e| Error::Io(e.error))?;
    }
    sync_directory(parent)
}

/// The sibling temporary file for `path`, with the permissions `path` should
/// end up with; tempfile's own default is owner-only.
#[cfg(unix)]
fn temporary_in(parent: &Path, path: &Path, access: Access) -> Result<tempfile::NamedTempFile> {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;
    let existing = match std::fs::metadata(path) {
        Ok(meta) => Some(meta.permissions().mode() & 0o777),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    // A new shared file is created with 0666 and the umask applies, as for
    // any newly created file. A replaced one is created owner-only and then
    // set exactly, so the umask cannot narrow what the file had.
    let mode = match (access, existing) {
        (Access::Shared, None) => 0o666,
        _ => 0o600,
    };
    let temporary = tempfile::Builder::new()
        .permissions(Permissions::from_mode(mode))
        .tempfile_in(parent)?;
    if let (Access::Shared, Some(existing)) = (access, existing) {
        temporary
            .as_file()
            .set_permissions(Permissions::from_mode(existing))?;
    }
    Ok(temporary)
}

#[cfg(not(unix))]
fn temporary_in(parent: &Path, _path: &Path, _access: Access) -> Result<tempfile::NamedTempFile> {
    Ok(tempfile::NamedTempFile::new_in(parent)?)
}

/// Make a rename in `directory` durable. A failure is [`Error::Unsynced`]:
/// the file is already in place, so it is not reported as refused input.
pub(crate) fn sync_directory(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            Error::Unsynced(format!(
                "The file was written, but synchronizing {} failed; power-loss durability is unconfirmed: {error}",
                directory.display()
            ))
        })?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
