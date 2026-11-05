//! Part bodies against the board outline: `board.body_outside_outline` warns
//! when a placed part's body, seen from above, reaches past the outer outline.
//! The body is the package envelope in its model frame, the box 2D and the
//! courtyard use. A connector may overhang the edge on purpose, so this is a
//! warning, never a block. A body over a cutout is not reported: parts sit
//! over cutouts and slots by design.
use super::CopperRing;
use super::board_edge::{BoardEdges, EdgeVerdict};
use super::placed::placement_transform;
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::Shape;
use crate::instances;
use crate::model::Project;

/// The finding code for a body that reaches past the board outline.
pub const BODY_OUTSIDE_OUTLINE: &str = "board.body_outside_outline";

/// One finding per placed part whose body reaches past the outer outline.
pub fn findings(project: &Project) -> Vec<Finding> {
    let edges = BoardEdges::of(project);
    let mut out = Vec::new();
    for placement in &project.placements {
        let Some(instance) = project.instance(placement.instance) else {
            continue;
        };
        let Some(physical) = instances::resolve(&project.library, instance)
            .ok()
            .and_then(|r| r.physical)
        else {
            continue;
        };
        let footprint = &physical.footprint.definition;
        let outline = footprint.envelope_outline(&physical.package.definition);
        let transform = placement_transform(placement, &footprint.datum);
        let ring = outline.corners().map(|p| transform.apply(p)).to_vec();
        let body = CopperRing::of_shape(&Shape::polygon(ring), false);
        if edges.verdict(&body, 0.0) == EdgeVerdict::Outside {
            out.push(Finding::new(
                BODY_OUTSIDE_OUTLINE,
                format!(
                    "The body of {} extends past the board outline",
                    instance.designator
                ),
                vec![ObjectRef::Instance { id: instance.id }],
            ));
        }
    }
    out
}
