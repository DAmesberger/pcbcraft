//! Board courtyard checks between placements (slice F5, the old G4): two
//! parts' courtyards on the same board side must not overlap
//! (`board.courtyard_overlap`). Courtyards come only from authored or
//! explicitly derived footprint graphics on the courtyard layer; a part
//! without one is not checked here (the footprint lint reports it).
//!
//! Contract (docs/f45-contract.md): [`placed_courtyards`] is final;
//! [`findings`] is filled in by the templates task and reports nothing
//! until then.
use super::placed::{BoardDrawing, PlacedItem, placed_graphics};
use crate::diagnostics::Finding;
use crate::geometry::Shape;
use crate::ids::{GraphicId, Id};
use crate::model::{Layer, Project};

/// The finding code for overlapping courtyards.
pub const COURTYARD_OVERLAP: &str = "board.courtyard_overlap";

/// One closed courtyard shape of one placed part, in board coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedCourtyard {
    pub instance: Id,
    pub graphic: GraphicId,
    /// The board side the courtyard is on.
    pub layer: Layer,
    pub shape: Shape,
}

/// Every closed courtyard shape of every placed part, in placement order.
pub fn placed_courtyards(project: &Project) -> Vec<PlacedCourtyard> {
    placed_graphics(project)
        .into_iter()
        .filter_map(|g| match (g.layer, g.item) {
            (BoardDrawing::Courtyard(layer), PlacedItem::Shape { shape, .. })
                if shape.is_closed() =>
            {
                Some(PlacedCourtyard {
                    instance: g.instance,
                    graphic: g.graphic,
                    layer,
                    shape,
                })
            }
            _ => None,
        })
        .collect()
}

/// `board.courtyard_overlap` findings between different placements on the
/// same side. Bounded by the placed-graphic limits.
pub fn findings(_project: &Project) -> Vec<Finding> {
    Vec::new()
}
