//! The published board interface's files (docs/architecture/
//! board-interface.md, "The export format"): the `.solidcraft` design an
//! assembly links and its sidecar `<name>.interface.json` beside it. Both
//! are replaced atomically, so a linked design never reads half a publish.
use crate::persistence::{atomic_write, read_bytes, sync_directory};
use crate::{Error, Result};
use sparksmith_core::ids::InterfaceSourceId;
use sparksmith_core::interface::Sidecar;
use std::path::{Path, PathBuf};

/// The sidecar's file name ending.
pub const SIDECAR_SUFFIX: &str = ".interface.json";

/// A published design is a `.solidcraft` file with a name.
pub fn check_design_path(path: &Path) -> Result<()> {
    let named = path
        .file_stem()
        .is_some_and(|stem| !stem.is_empty() && !stem.to_string_lossy().starts_with('.'));
    let extension = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("solidcraft"));
    if named && extension {
        Ok(())
    } else {
        Err(Error::Invalid(
            "interface.path: publish the interface to a named .solidcraft file".into(),
        ))
    }
}

/// `<folder>/<name>.interface.json` for the design `<folder>/<name>.solidcraft`.
pub fn sidecar_path(design: &Path) -> PathBuf {
    let stem = design
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    design.with_file_name(format!("{stem}{SIDECAR_SUFFIX}"))
}

/// The board's interface identity for its first publish, from the
/// operating system's random source.
pub fn new_source_id() -> Result<InterfaceSourceId> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::Invalid(format!("the system random source failed: {e}")))?;
    Ok(InterfaceSourceId::from_bytes(bytes))
}

/// The sidecar published beside `design`, or `None` when there is none.
/// One that is there but unreadable, or of a format this build does not
/// read, is refused.
pub fn read_sidecar(design: &Path) -> Result<Option<Sidecar>> {
    let path = sidecar_path(design);
    if !path.try_exists()? {
        return Ok(None);
    }
    let bytes = read_bytes(&path)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::Invalid("interface.format: the sidecar is not UTF-8 text".into()))?;
    Ok(Some(Sidecar::read(&text)?))
}

/// A sibling temporary path for writing `design` before it replaces the
/// published one: hidden, in the same folder, so the rename is atomic.
pub fn temporary_design(design: &Path) -> Result<PathBuf> {
    check_design_path(design)?;
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::Invalid(format!("the system random source failed: {e}")))?;
    let tag: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let stem = design
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(design.with_file_name(format!(".{stem}.publishing-{tag}.solidcraft")))
}

/// Put a publish in place: the sidecar `text` replaces the one beside
/// `design` (written through its own temporary file), then the finished
/// design at `temporary` replaces `design`. Each replacement is atomic.
/// When writing the sidecar fails, the temporary design is removed and
/// nothing published changes.
pub fn put_in_place(design: &Path, temporary: &Path, text: &str) -> Result<()> {
    if let Err(error) = atomic_write(&sidecar_path(design), text.as_bytes()) {
        let _ = std::fs::remove_file(temporary);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(temporary, design) {
        let _ = std::fs::remove_file(temporary);
        return Err(error.into());
    }
    let parent = design
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    sync_directory(parent)
}
