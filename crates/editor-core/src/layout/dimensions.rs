//! Dimensioned parts (docs/board-sketch-references.md §5.2): the
//! `layout.dimension_*` commands, the parts solve that runs after the
//! board solve, and the reference findings.
//!
//! Only parts a [`PartDimension`] names are solved; every other part keeps
//! its stored pose. Dimensioned parts are grouped into connected components
//! (two parts are connected when a dimension names both), and each
//! component an operation touched is solved by the host's board evaluator
//! ([`super::outline::BoardShapeEvaluator::solve_parts`]) in a scratch
//! Solidcraft sketch: the named board (and housing) geometry held fixed, per
//! part an origin and a construction axis, its referenced features bound to
//! that frame, and the dimensions. A degree of freedom no dimension fixes
//! keeps its stored value. A move or rotate of a dimensioned part is a
//! solver drag; a move that cannot land is refused.
//!
//! Slice C0 fixes this contract; slice S2 implements it (until then every
//! path that would need the solve refuses with [`not_yet`]).
use crate::diagnostics::{Error, Finding, Result};
use crate::geometry::{Point, point_valid};
use crate::ids::Id;
use crate::library::footprint_source::{DimValue, SolveStatus};
use crate::model::Project;
use crate::model::board::{BoardSketch, ConstraintTarget};
use crate::model::dimension::{
    BoardRef, FeatureRef, MAX_DIMENSION_REFS, MAX_PART_DIMENSIONS, PartDimension,
    PartDimensionSpec, PartRelation,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub mod features;

/// Finding code: a reference whose source is gone (a deleted part, pad,
/// anchor, board curve or model face). It keeps its last geometry and
/// blocks fabrication output with no waiver (§5.5).
pub const REFERENCE_LOST: &str = "board.reference_lost";
/// Finding code: a reference found again but not provably the same
/// geometry (a renamed or unverified model face). It blocks fabrication
/// output with no waiver until acknowledged in an impact preview (§5.5).
pub const REFERENCE_INEXACT: &str = "board.reference_inexact";

/// Part-dimension mutations; each is one undo step.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum DimensionCommand {
    /// Add a part dimension and solve the parts it names; the outcome names
    /// it (`Outcome::DimensionAdded`). Refused when the parts would not
    /// solve.
    #[serde(rename = "layout.dimension_add")]
    Add { dimension: PartDimensionSpec },
    /// Replace a part dimension, by id, and re-solve.
    #[serde(rename = "layout.dimension_edit")]
    Edit { dimension: PartDimension },
    /// Delete a part dimension; the parts it named keep their poses.
    #[serde(rename = "layout.dimension_delete")]
    Delete { id: Id },
}

// A new command chooses its permission class here.
declare_classes!(DimensionCommand {
    Add "layout.dimension_add" Edit "Dimension a part",
    Edit "layout.dimension_edit" Edit "Edit a part dimension",
    Delete "layout.dimension_delete" Edit "Delete a part dimension",
});

/// What a dimension command produced for its caller.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum DimensionApplied {
    #[default]
    Nothing,
    /// The dimension `layout.dimension_add` allocated.
    Added(Id),
}

/// The refusal of every part-dimension path until slice S2.
pub fn not_yet() -> Error {
    Error::Unsupported("layout.dimension: part dimensions are not yet supported".into())
}

/// Apply a dimension command to a candidate. The parts it names are placed
/// by [`follow`], which the engine runs after every operation.
pub fn apply(
    _project: &mut Project,
    _command: DimensionCommand,
    _evaluator: Option<&dyn super::outline::BoardShapeEvaluator>,
) -> Result<DimensionApplied> {
    Err(not_yet())
}

/// Solve the dimensioned parts an operation touched (after the board
/// solve), taking the project from `before` to `project`. A part that a
/// command moved away from where its dimensions put it is dragged by the
/// solver; a move that cannot land is refused.
pub fn follow(
    _before: &Project,
    project: &mut Project,
    _evaluator: Option<&dyn super::outline::BoardShapeEvaluator>,
) -> Result<()> {
    if project.part_dimensions.is_empty() {
        return Ok(());
    }
    Err(not_yet())
}

/// Reference findings: [`REFERENCE_LOST`] and [`REFERENCE_INEXACT`], one per
/// unresolved reference.
pub fn checks(_project: &Project) -> Vec<Finding> {
    Vec::new()
}

/// Structural bounds: at most [`MAX_PART_DIMENSIONS`], 1–
/// [`MAX_DIMENSION_REFS`] references each, finite values and picks,
/// well-formed names and expressions. A reference whose source is gone is a
/// check finding ([`REFERENCE_LOST`]), not an invalid document.
pub fn validate(project: &Project) -> Result<()> {
    if project.part_dimensions.len() > MAX_PART_DIMENSIONS {
        return Err(Error::Limit(format!(
            "layout.dimension_limit: at most {MAX_PART_DIMENSIONS} part dimensions"
        )));
    }
    let mut ids = BTreeSet::new();
    for d in project.part_dimensions.iter() {
        if !ids.insert(d.id) || !well_formed(d) {
            return Err(Error::Invalid(format!(
                "layout.dimension: part dimension {} needs 1–{MAX_DIMENSION_REFS} well-formed \
                 references and a finite value",
                d.id
            )));
        }
    }
    Ok(())
}

fn well_formed(d: &PartDimension) -> bool {
    let value = match &d.value {
        None => true,
        Some(DimValue::Fixed(v)) => v.is_finite(),
        Some(DimValue::Param(name)) => name.valid(),
        Some(DimValue::Expr(expr)) => expr.valid(),
    };
    let kind = match d.kind {
        PartRelation::PointAlong { at } => at.is_finite() && (0.0..=1.0).contains(&at),
        PartRelation::Dim(_) | PartRelation::Geom(_) => true,
    };
    value
        && kind
        && (1..=MAX_DIMENSION_REFS).contains(&d.refs.len())
        && d.refs.iter().all(reference_well_formed)
}

fn reference_well_formed(r: &BoardRef) -> bool {
    let pick = |p: &[f64; 3]| p.iter().all(|v| v.is_finite());
    match r {
        BoardRef::Board(_) | BoardRef::Housing(_) => true,
        BoardRef::Part(f) => match f {
            FeatureRef::Origin { .. }
            | FeatureRef::Axis { .. }
            | FeatureRef::Pad { .. }
            | FeatureRef::Courtyard { .. } => true,
            FeatureRef::Anchor { name, .. } => name.valid(),
            FeatureRef::Face { face, .. } => {
                face.body.valid() && face.face.valid() && pick(&face.pick)
            }
            FeatureRef::Edge { edge, .. } | FeatureRef::Vertex { edge, .. } => {
                edge.body.valid() && edge.edge.valid() && pick(&edge.pick)
            }
        },
    }
}

/// A target's pose on the board: what the parts solve finds.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartPose {
    pub position: Point,
    pub rotation_deg: f64,
}

impl PartPose {
    pub fn valid(&self) -> bool {
        point_valid(self.position) && self.rotation_deg.is_finite()
    }
}

/// A feature's geometry in its target's frame (part frame, before the
/// placement mirrors and turns it), mm.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "shape", deny_unknown_fields)]
pub enum FeatureGeometry {
    Point {
        at: Point,
    },
    Line {
        a: Point,
        b: Point,
    },
    /// A bulge arc from `a` to `b` (as `library::Segment::Arc`).
    Arc {
        a: Point,
        b: Point,
        bulge: f64,
    },
    Circle {
        center: Point,
        radius: f64,
    },
}

/// One target of a parts solve, with every feature its dimensions name.
#[derive(Clone, Debug, PartialEq)]
pub struct SolvePart {
    pub target: ConstraintTarget,
    /// The stored pose: the start, and what an unconstrained degree of
    /// freedom keeps.
    pub pose: PartPose,
    /// On the bottom side: the part frame's y is mirrored.
    pub mirrored: bool,
    pub features: Vec<(FeatureRef, FeatureGeometry)>,
}

/// A command's move of a dimensioned target: the solver drags it toward
/// `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartDrag {
    pub target: ConstraintTarget,
    pub to: PartPose,
}

/// One connected component of dimensioned targets to solve.
#[derive(Clone, Copy, Debug)]
pub struct PartsRequest<'a> {
    /// The board after its solve; a rectangle board as the evaluator's
    /// rectangle sketch. Its points and curves are held fixed.
    pub board: &'a BoardSketch,
    pub parts: &'a [SolvePart],
    /// The dimensions among them, with their values unevaluated (the host
    /// evaluates expressions against the board parameters).
    pub dimensions: &'a [PartDimension],
    pub drag: Option<PartDrag>,
}

/// What a parts solve found.
#[derive(Clone, Debug, PartialEq)]
pub struct PartsSolution {
    /// One pose per requested target.
    pub poses: Vec<(ConstraintTarget, PartPose)>,
    /// Each dimension's measured value (mm or degrees), reference ones
    /// included.
    pub measured: Vec<(Id, f64)>,
    pub status: SolveStatus,
    pub findings: Vec<Finding>,
}

#[cfg(test)]
mod tests;
