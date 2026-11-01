//! Slice D0 (contract C0): the `.pcbcraft` ZIP container (founder
//! decision F5): `project.json` (the document as today) and
//! `assets/sha256/<hash>` (raw bytes), stored or deflated, written through
//! the same atomic path as today. A missing or mismatched asset refuses
//! reopen and names it; recovery files use the same container. Limits are
//! `pcbcraft_core::format::CONTAINER_*`. `read_project` / `write_project`
//! switch to these at D0; until then they are unused.
use crate::{Error, Result};
use pcbcraft_core::format::EditorFile;
use std::path::Path;

/// Read and validate a container. Contract stub until D0.
pub fn read_container(_path: &Path) -> Result<EditorFile> {
    Err(unsupported())
}

/// Encode `file` (assets as raw entries) and atomically replace `path`.
/// Contract stub until D0.
pub fn write_container(_path: &Path, _file: &EditorFile) -> Result<()> {
    Err(unsupported())
}

fn unsupported() -> Error {
    Error::Invalid(
        "container.unsupported: the ZIP project container is not available yet (slice D0)".into(),
    )
}
