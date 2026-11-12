//! The datums a placed part's package revision declares, in the board
//! frame: what the `external_datums` rule exports for each placement and
//! what `interface.export` may name as a package datum.
//!
//! Package datums are slice B2 (docs/architecture/board-interface.md, "How
//! a connector's opening is defined"). Until a package revision holds
//! them, no placement has any, so the rule exports nothing and an export
//! of a package datum is refused as unknown. B2 fills [`placed`]; nothing
//! else here changes.
use super::DatumGeometry;
use crate::ids::Id;
use crate::library::datum::{DatumKind, DatumName};
use crate::model::Project;

/// One datum a placed part's package declares.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedDatum {
    pub name: DatumName,
    pub kind: DatumKind,
    /// Meant for the enclosure: the `external_datums` rule exports it.
    pub external: bool,
    /// In the board frame; `None` when it cannot be placed.
    pub geometry: Option<DatumGeometry>,
}

/// The datums `instance`'s package declares, by name, in the board frame.
/// Empty for a part that is not placed. No package declares any before
/// slice B2.
pub fn placed(project: &Project, instance: Id) -> Vec<PlacedDatum> {
    let _ = (project, instance);
    Vec::new()
}
