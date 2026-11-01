//! Board courtyard checks between placements (slice F5, the old G4): two
//! parts' courtyards on the same board side must not overlap
//! (`board.courtyard_overlap`). Courtyards come only from authored or
//! explicitly derived footprint graphics on the courtyard layer; a part
//! without one is not checked here (the footprint lint reports it).
//!
//! Overlap is shared interior area of the flattened outlines (touching
//! edges is not overlap), so two parts may abut courtyard to courtyard.
use super::placed::{BoardDrawing, PlacedItem, placed_graphics};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{Bounds, Point, Shape, polygons_overlap};
use crate::ids::{GraphicId, Id};
use crate::model::{Layer, Project};
use std::collections::BTreeSet;

/// The finding code for overlapping courtyards.
pub const COURTYARD_OVERLAP: &str = "board.courtyard_overlap";
/// The finding code the board checks use when a bound leaves geometry
/// unchecked.
pub const COURTYARD_LIMIT: &str = "board.check_limit";

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

/// Courtyard shapes compared pairwise; more are left unchecked (a board
/// this size needs a spatial index first).
pub const MAX_CHECKED_COURTYARDS: usize = 4096;
/// Overlap findings one check reports.
pub const MAX_OVERLAP_FINDINGS: usize = 256;
/// Chord tolerance for flattening arcs and circles, in mm.
const CHORD_MM: f64 = 0.01;

/// `board.courtyard_overlap` findings between different placements on the
/// same side: one per pair of parts, however many of their shapes
/// overlap. Bounded by [`MAX_CHECKED_COURTYARDS`] and
/// [`MAX_OVERLAP_FINDINGS`]; reaching either adds one
/// [`COURTYARD_LIMIT`] finding, so unchecked courtyards are never silent.
pub fn findings(project: &Project) -> Vec<Finding> {
    bounded_findings(project, MAX_CHECKED_COURTYARDS, MAX_OVERLAP_FINDINGS)
}

pub(crate) fn limit_finding() -> Finding {
    Finding::new(
        COURTYARD_LIMIT,
        "Board checks incomplete: courtyards past the check limit are unchecked".into(),
        vec![ObjectRef::Board],
    )
}

fn bounded_findings(project: &Project, max_yards: usize, max_findings: usize) -> Vec<Finding> {
    let all = placed_courtyards(project);
    let truncated = all.len() > max_yards;
    let yards: Vec<(PlacedCourtyard, Vec<Point>, Bounds)> = all
        .into_iter()
        .take(max_yards)
        .filter_map(|c| {
            let ring = c.shape.flatten(CHORD_MM);
            let bounds = Bounds::of(ring.iter().copied())?;
            (ring.len() >= 3).then_some((c, ring, bounds))
        })
        .collect();
    let mut pairs: BTreeSet<(Id, Id, bool)> = BTreeSet::new();
    let mut out = Vec::new();
    for (i, (a, ra, ba)) in yards.iter().enumerate() {
        for (b, rb, bb) in yards.iter().skip(i + 1) {
            if a.instance == b.instance || a.layer != b.layer || !ba.intersects(bb) {
                continue;
            }
            let key = (
                a.instance.min(b.instance),
                a.instance.max(b.instance),
                a.layer == Layer::Top,
            );
            if pairs.contains(&key) || !polygons_overlap(ra, rb) {
                continue;
            }
            if out.len() >= max_findings {
                out.push(limit_finding());
                return out;
            }
            pairs.insert(key);
            let name = |id: Id| {
                project
                    .instance(id)
                    .map(|i| i.designator.clone())
                    .unwrap_or_else(|| "component".into())
            };
            out.push(Finding::new(
                COURTYARD_OVERLAP,
                format!(
                    "Courtyards overlap on the {} side: {} / {}",
                    if a.layer == Layer::Top {
                        "top"
                    } else {
                        "bottom"
                    },
                    name(a.instance),
                    name(b.instance)
                ),
                vec![
                    ObjectRef::Instance { id: a.instance },
                    ObjectRef::Instance { id: b.instance },
                ],
            ));
        }
    }
    if truncated {
        out.push(limit_finding());
    }
    out
}

#[cfg(test)]
mod tests;
