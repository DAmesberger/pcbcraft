//! Slice A2 (contract C0): read-only preview of a library-folder item.
//! `library.open_preview` (automation) reads the bundle through io and
//! returns the same report `library.inspect` gives a project item, so
//! agents and the Component workspace inspect a folder item without
//! importing it. "Edit a copy" is `library.import` plus a draft (F4).
use super::ItemReport;
use crate::diagnostics::{Error, Result};
use crate::ids::{Revision, SourceLineageId};
use crate::library::Bundle;

/// Inspect the root (or `lineage`) of a folder bundle. Contract stub
/// until A2.
pub fn inspect_bundle(
    _bundle: &Bundle,
    _lineage: Option<SourceLineageId>,
    _revision: Option<Revision>,
) -> Result<ItemReport> {
    Err(Error::Unsupported(
        "library.preview_unsupported: previewing library-folder items is not available yet (slice A2)"
            .into(),
    ))
}
