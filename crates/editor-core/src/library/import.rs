//! Import, export (promote) and duplicate over a project library (S3, D1,
//! D6). Pure functions over `Library`; the engine stages them on a
//! candidate document through `library.import`, `library.place_from`,
//! `library.promote` and `library.duplicate`. Identity allocation is passed
//! in so this module needs no `model`.
use super::Library;
use super::bundle::{Bundle, OriginRecord};
use crate::diagnostics::{Error, Result};
use crate::ids::{DefinitionId, DefinitionRef, LibraryId, SourceLineageId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Fresh source lineage ids one promote may consume.
pub const MAX_FRESH_LINEAGES: usize = 256;

/// One record of an import and the project revision it maps to.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImportedRevision {
    pub record: OriginRecord,
    /// An equal origin was already in the project; nothing was appended.
    pub reused: bool,
}

/// What an import did, in bundle order. `root` is the project copy of the
/// bundle's root.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImportReport {
    pub root: DefinitionRef,
    pub revisions: Vec<ImportedRevision>,
}

/// Import a validated bundle: dedupe by origin, map each new source lineage
/// to a lineage from `allocate` and each source revision to the next project
/// revision of that lineage, rewrite every reference, append the revisions,
/// pack the assets and record the origins. A failure leaves `library`
/// unchanged only because the caller stages on a candidate.
pub fn import(
    library: &mut Library,
    bundle: &Bundle,
    allocate: &mut dyn FnMut() -> Result<DefinitionId>,
) -> Result<ImportReport> {
    let _ = (library, bundle, allocate);
    Err(Error::Unsupported(
        "library.import: importing library bundles is not implemented yet".into(),
    ))
}

/// What `library.place_from` places.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "from", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlaceSource {
    /// A component revision the project already holds.
    Project { component: crate::ids::ComponentRef },
    /// The root component of a bundle, imported first (or reused when an
    /// equal origin is already in the project).
    Bundle { bundle: Box<Bundle> },
}

/// What `library.place_from` does when the project holds a different
/// revision of the bundle root's lineage. It never upgrades silently.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OtherRevision {
    /// Refuse with `conflict` code `library.other_revision`, naming both
    /// revisions, so the host can offer Keep both / Update project copy /
    /// Cancel.
    #[default]
    Refuse,
    /// Import the bundle's revision beside the existing one and place it.
    KeepBoth,
}

/// What promoting `root` to a library would write and record.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PromotePlan {
    /// Every record of the closure. Members whose origin is already in the
    /// target map back to their source lineage; io skips a record the
    /// folder holds with an equal hash and refuses one with another hash.
    pub bundle: Bundle,
    /// The origins `library.promote` records for the project revisions.
    pub origins: Vec<OriginRecord>,
    /// Fresh lineage ids the plan used, in `fresh` order.
    pub used: Vec<SourceLineageId>,
}

/// Compute the export of `root` and its closure for `target` (pure and
/// deterministic: equal inputs give equal plans). New lineages take ids
/// from `fresh` in order; too few is `invalid_input`.
pub fn export(
    library: &Library,
    root: DefinitionRef,
    target: LibraryId,
    name: &str,
    fresh: &[SourceLineageId],
) -> Result<PromotePlan> {
    let _ = (library, root, target, name, fresh);
    Err(Error::Unsupported(
        "library.promote: exporting to a library folder is not implemented yet".into(),
    ))
}

/// D6: copy a component revision with its symbols and bindings into new
/// draft lineages from `allocate` (component first), references rewritten
/// to the copies; footprints, packages and fields stay shared. The copies
/// record their source in `DefinitionMeta::provenance` text. Returns the new
/// lineages, component first.
pub fn duplicate(
    library: &Library,
    drafts: &mut super::Drafts,
    component: crate::ids::ComponentRef,
    name: &str,
    allocate: &mut dyn FnMut() -> Result<DefinitionId>,
) -> Result<Vec<DefinitionId>> {
    let _ = (library, drafts, component, name, allocate);
    Err(Error::Unsupported(
        "library.duplicate: duplicating a component is not implemented yet".into(),
    ))
}
