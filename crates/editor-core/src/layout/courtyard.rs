//! Board courtyard checks between placements (slice F5, the old G4): two
//! parts' courtyards on the same board side must not overlap
//! (`board.courtyard_overlap`). Courtyards come only from authored or
//! explicitly derived footprint graphics on the courtyard layer; a part
//! without one is not checked here (the footprint lint reports it).
//!
//! Overlap is shared interior area of the flattened outlines (touching
//! edges is not overlap), so two parts may abut courtyard to courtyard
//! under the default rules. A positive `courtyard clearance` design rule
//! also reports courtyards closer than it (`board.courtyard_clearance`).
use super::check_pair_budget;
use super::grid::BoundsGrid;
use super::limits::{mm, need};
use super::placed::{BoardDrawing, PlacedItem, placed_graphics};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{Bounds, Point, Shape, polygons_overlap, polyline_polygon_distance};
use crate::ids::{GraphicId, Id};
use crate::model::{Layer, Project};
use crate::rules::RuleKey;
use std::collections::BTreeSet;

/// The finding code for overlapping courtyards.
pub const COURTYARD_OVERLAP: &str = "board.courtyard_overlap";
/// The finding code for courtyards closer than the courtyard clearance.
pub const COURTYARD_CLEARANCE: &str = "board.courtyard_clearance";
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

/// Chord tolerance for flattening arcs and circles, in mm.
const CHORD_MM: f64 = 0.01;

/// `board.courtyard_overlap` findings between different placements on the
/// same side: one per pair of parts, however many of their shapes
/// overlap. Only shapes whose bounds come within the clearance are
/// compared (a bounds grid offers them), at most the copper checks' pair
/// budget for this many shapes ([`check_pair_budget`], which only
/// courtyards piled on courtyards reach); reaching it adds one
/// [`COURTYARD_LIMIT`] finding, so unchecked courtyards are never silent.
pub fn findings(project: &Project) -> Vec<Finding> {
    bounded_findings(project, None)
}

pub(crate) fn limit_finding() -> Finding {
    Finding::new(
        COURTYARD_LIMIT,
        "Board checks incomplete: courtyards past the check limit are unchecked".into(),
        vec![ObjectRef::Board],
    )
}

/// [`findings`], comparing at most `budget` pairs (the pair budget when
/// `None`).
fn bounded_findings(project: &Project, budget: Option<usize>) -> Vec<Finding> {
    let yards: Vec<(PlacedCourtyard, Vec<Point>, Bounds)> = placed_courtyards(project)
        .into_iter()
        .filter_map(|c| {
            let ring = c.shape.flatten(CHORD_MM);
            let bounds = Bounds::of(ring.iter().copied())?;
            (ring.len() >= 3).then_some((c, ring, bounds))
        })
        .collect();
    let rules = &project.rules;
    let clearance = need(rules, &rules.effective(), RuleKey::CourtyardClearance);
    let reach = clearance.mm.max(0.0);
    let budget = budget.unwrap_or_else(|| check_pair_budget(yards.len()));
    let grid = BoundsGrid::new(yards.iter().map(|y| Some(y.2)), reach);
    let mut near = Vec::new();
    let mut compared = 0usize;
    let mut pairs: BTreeSet<(Id, Id, bool)> = BTreeSet::new();
    let mut out = Vec::new();
    for (i, (a, ra, ba)) in yards.iter().enumerate() {
        grid.near(ba, reach, &mut near);
        for (b, rb, bb) in near
            .iter()
            .filter(|j| **j > i)
            .filter_map(|j| yards.get(*j))
        {
            if a.instance == b.instance || a.layer != b.layer || !ba.inflated(reach).intersects(bb)
            {
                continue;
            }
            compared += 1;
            if compared > budget {
                out.push(limit_finding());
                return out;
            }
            let key = (
                a.instance.min(b.instance),
                a.instance.max(b.instance),
                a.layer == Layer::Top,
            );
            if pairs.contains(&key) {
                continue;
            }
            let overlap = polygons_overlap(ra, rb);
            let gap = if overlap || reach <= 0.0 {
                0.0
            } else {
                let mut closed = ra.clone();
                closed.extend(ra.first().copied());
                polyline_polygon_distance(&closed, rb)
            };
            let close = !overlap && reach > 0.0 && gap < reach - 1e-9;
            if !overlap && !close {
                continue;
            }
            pairs.insert(key);
            let name = |id: Id| {
                project
                    .instance(id)
                    .map(|i| i.designator.clone())
                    .unwrap_or_else(|| "component".into())
            };
            let side = if a.layer == Layer::Top {
                "top"
            } else {
                "bottom"
            };
            let (code, message) = if overlap {
                (
                    COURTYARD_OVERLAP,
                    format!(
                        "Courtyards overlap on the {side} side: {} / {}",
                        name(a.instance),
                        name(b.instance)
                    ),
                )
            } else {
                (
                    COURTYARD_CLEARANCE,
                    format!(
                        "Courtyards of {} and {} are {} mm apart on the {side} side; {}",
                        name(a.instance),
                        name(b.instance),
                        mm(gap),
                        clearance.text()
                    ),
                )
            };
            out.push(Finding::new(
                code,
                message,
                vec![
                    ObjectRef::Instance { id: a.instance },
                    ObjectRef::Instance { id: b.instance },
                ],
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests;
