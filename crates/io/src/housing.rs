//! The linked enclosure's file (docs/architecture/board-interface.md,
//! "Link updates"): read once, bounded, hashed, and copied into a scratch
//! folder that the engine opens. The user's enclosure file is only ever
//! read; the dry run and every read work on the copy.
use crate::{Error, Result};
use pcbcraft_core::ids::ContentHash;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The largest enclosure design read (it carries the linked board's
/// bodies, so it may be large).
pub const MAX_ENCLOSURE_BYTES: u64 = 512 * 1024 * 1024;

/// An enclosure read from disk: its bytes' hash and a scratch copy the
/// engine opens. The copy and its folder go when this is dropped.
pub struct EnclosureCopy {
    pub hash: ContentHash,
    /// The scratch copy.
    pub path: PathBuf,
    /// The folder the user keeps the enclosure in, against which its
    /// relative link paths resolve.
    pub folder: PathBuf,
    dir: tempfile::TempDir,
}

impl EnclosureCopy {
    /// The scratch folder, for files the dry run writes beside the copy.
    pub fn scratch(&self) -> &Path {
        self.dir.path()
    }
}

/// An enclosure design is a `.solidcraft` file.
pub fn check_path(path: &Path) -> Result<()> {
    let ok = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("solidcraft"));
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(
            "housing.path: the enclosure is a .solidcraft design".into(),
        ))
    }
}

/// The hash of the enclosure at `path` now, or `None` when it is not there.
pub fn hash(path: &Path) -> Result<Option<ContentHash>> {
    if !path.try_exists()? {
        return Ok(None);
    }
    Ok(Some(pcbcraft_core::interface::housing::file_hash(&read(
        path,
    )?)))
}

fn read(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_ENCLOSURE_BYTES {
        return Err(Error::TooLarge("the enclosure exceeds 512 MiB".into()));
    }
    let mut bytes = Vec::new();
    file.take(MAX_ENCLOSURE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_ENCLOSURE_BYTES {
        return Err(Error::TooLarge("the enclosure exceeds 512 MiB".into()));
    }
    Ok(bytes)
}

/// Read the enclosure at `path` once and copy it into a new scratch folder.
pub fn copy(path: &Path) -> Result<EnclosureCopy> {
    check_path(path)?;
    let bytes = read(path)?;
    let dir = tempfile::Builder::new()
        .prefix("sparksmith-enclosure-")
        .tempdir()?;
    let copy = dir.path().join("enclosure.solidcraft");
    std::fs::write(&copy, &bytes)?;
    let folder = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    Ok(EnclosureCopy {
        hash: pcbcraft_core::interface::housing::file_hash(&bytes),
        path: copy,
        folder,
        dir,
    })
}
