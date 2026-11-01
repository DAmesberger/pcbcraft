//! Board shape: commands over the board sketch, the evaluator seam that
//! solves it (the integration crate's SolidCraft sketch engine, as for
//! footprints), and edge-relative placement constraints.
//!
//! The core never interprets the sketch. A [`BoardShapeEvaluator`] applies
//! sketch edits and solves the sketch with its parameters into a
//! [`SolvedBoard`], evaluating every constraint's expressions; the core
//! then places constrained targets with [`edge_pose`]. An unsolved or
//! over-constrained sketch is a finding, never a panic.
use crate::diagnostics::{Error, Finding, Result};
use crate::geometry::{Point, point_valid};
use crate::ids::Id;
use crate::library::footprint_source::{
    EvaluatorRef, Expr, FootprintParam, ParamName, SketchBlob, SketchCurveId, SketchEdit,
    SketchOutcome, SketchPointId, SolveSummary,
};
use crate::model::board::{
    BoardFeatureId, BoardSketch, ConstraintTarget, CutoutKind, EdgeAnchor, EdgeConstraintSpec,
    EdgeName, MAX_EDGE_CONSTRAINTS, SolvedEdge,
};
use crate::model::{BoardShape, EdgeConstraint, Project, SolvedBoard};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The solved values of one constraint's expressions, mm.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ConstraintValues {
    pub constraint: Id,
    pub along: f64,
    pub offset: f64,
}

/// What a board evaluation produced.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BoardEvaluation {
    pub solved: SolvedBoard,
    pub solve: SolveSummary,
    /// One per constraint the request listed whose expressions evaluated.
    pub constraints: Vec<ConstraintValues>,
    /// Design problems (unsolved, over-constrained, open outline,
    /// unknown parameter in a constraint).
    pub findings: Vec<Finding>,
}

/// The host's board sketch engine.
pub trait BoardShapeEvaluator {
    fn evaluator(&self) -> EvaluatorRef;
    /// A sketch of a `size` rectangle (lower-left corner at the origin)
    /// with `width`/`height` parameters driving it and its edges named
    /// [`crate::model::board::RECT_EDGES`].
    fn rectangle(&self, size: Point) -> Result<BoardSketch>;
    /// Apply one sketch edit to a canonical sketch.
    fn apply_sketch(&self, sketch: &SketchBlob, edit: &SketchEdit) -> Result<SketchOutcome>;
    /// Solve the board and evaluate the constraints' expressions against
    /// its parameters. `Err` only refuses the input outright.
    fn evaluate(
        &self,
        board: &BoardSketch,
        constraints: &[EdgeConstraint],
    ) -> Result<BoardEvaluation>;
}

/// One board-sketch edit; `board.edit` applies a list as one undo step.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "op", deny_unknown_fields)]
pub enum BoardEdit {
    Sketch {
        edit: SketchEdit,
    },
    /// Add or replace a board parameter.
    SetParameter {
        param: FootprintParam,
    },
    RemoveParameter {
        name: ParamName,
    },
    /// The closed loop of curves that is the outer outline.
    SetOutline {
        curves: Vec<SketchCurveId>,
    },
    AddCutout {
        kind: CutoutKind,
        curves: Vec<SketchCurveId>,
    },
    RemoveCutout {
        id: BoardFeatureId,
    },
    AddMountingHole {
        center: SketchPointId,
        drill: Expr,
        plated: bool,
        #[serde(default)]
        pad_diameter: Option<Expr>,
    },
    RemoveMountingHole {
        id: BoardFeatureId,
    },
    NameEdge {
        name: EdgeName,
        curve: SketchCurveId,
    },
    UnnameEdge {
        name: EdgeName,
    },
}

/// Board shape and edge-constraint mutations; each is one undo step.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum BoardCommand {
    /// Back to a plain rectangle of `size` (drops the sketch).
    #[serde(rename = "board.set_rectangle")]
    SetRectangle { size: Point },
    /// Turn the rectangle into an editable sketch (needs the evaluator).
    #[serde(rename = "board.start_sketch")]
    StartSketch,
    /// Edit the board sketch and re-solve it; constrained targets follow.
    #[serde(rename = "board.edit")]
    Edit { edits: Vec<BoardEdit> },
    /// Add a placement constraint; the outcome names it
    /// (`Outcome::EdgeConstraintAdded`).
    #[serde(rename = "board.constrain")]
    Constrain { constraint: EdgeConstraintSpec },
    #[serde(rename = "board.constraint_edit")]
    EditConstraint { constraint: EdgeConstraint },
    #[serde(rename = "board.constraint_delete")]
    DeleteConstraint { id: Id },
}

/// Apply a board command to a candidate; returns the constraint id
/// `board.constrain` allocated.
pub fn apply(
    project: &mut Project,
    command: BoardCommand,
    evaluator: Option<&dyn BoardShapeEvaluator>,
) -> Result<Option<Id>> {
    match command {
        BoardCommand::SetRectangle { size } => {
            project.board_shape = BoardShape::Rectangle;
            super::apply_staged(project, super::LayoutCommand::SetBoardSize { size })?;
            Ok(None)
        }
        BoardCommand::StartSketch | BoardCommand::Edit { .. } => {
            let _ = evaluator.ok_or_else(|| {
                Error::Unsupported(
                    "board.evaluator: board sketches need the host's sketch evaluator".into(),
                )
            })?;
            Err(Error::Unsupported(
                "board.sketch: board sketches are not available in this build yet".into(),
            ))
        }
        BoardCommand::Constrain { constraint } => {
            let id = project.allocate()?;
            project
                .edge_constraints
                .push(EdgeConstraint::from_spec(id, constraint));
            Ok(Some(id))
        }
        BoardCommand::EditConstraint { constraint } => {
            let slot = project
                .edge_constraints
                .iter_mut()
                .find(|c| c.id == constraint.id)
                .ok_or_else(|| missing(constraint.id))?;
            *slot = constraint;
            Ok(None)
        }
        BoardCommand::DeleteConstraint { id } => {
            let before = project.edge_constraints.len();
            project.edge_constraints.retain(|c| c.id != id);
            if project.edge_constraints.len() == before {
                return Err(missing(id));
            }
            Ok(None)
        }
    }
}

fn missing(id: Id) -> Error {
    Error::NotFound(format!("board.constraint_missing: no edge constraint {id}"))
}

/// A constrained position and direction.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EdgePose {
    pub position: Point,
    /// The edge's direction at that point, degrees.
    pub edge_deg: f64,
}

/// The point `along` mm from `anchor` on a straight `edge` and `offset`
/// mm along its inward (left) normal. Arc edges are not supported yet.
pub fn edge_pose(
    edge: &SolvedEdge,
    anchor: EdgeAnchor,
    along: f64,
    offset: f64,
) -> Result<EdgePose> {
    if edge.bulge != 0.0 {
        return Err(Error::Unsupported(
            "board.arc_edge: constraints to arc edges are not available yet".into(),
        ));
    }
    let d = [edge.b[0] - edge.a[0], edge.b[1] - edge.a[1]];
    let len = d[0].hypot(d[1]);
    if !(len.is_finite() && len > 1e-9 && along.is_finite() && offset.is_finite()) {
        return Err(Error::Invalid(
            "board.edge: degenerate edge or value".into(),
        ));
    }
    let u = [d[0] / len, d[1] / len];
    let start = match anchor {
        EdgeAnchor::Start => 0.0,
        EdgeAnchor::Middle => len / 2.0,
        EdgeAnchor::End => len,
        EdgeAnchor::Fraction(f) if (0.0..=1.0).contains(&f) => f * len,
        EdgeAnchor::Fraction(_) => {
            return Err(Error::Invalid("board.anchor: a fraction is 0–1".into()));
        }
    };
    let s = start + along;
    let position = [
        edge.a[0] + u[0] * s - u[1] * offset,
        edge.a[1] + u[1] * s + u[0] * offset,
    ];
    if !point_valid(position) {
        return Err(Error::Invalid("board.edge: position out of range".into()));
    }
    Ok(EdgePose {
        position,
        edge_deg: u[1].atan2(u[0]).to_degrees(),
    })
}

/// Structural bounds: constraints within the limit, unique per target,
/// expressions well formed. Unknown edges or vanished targets are check
/// findings (`board.constraint_unresolved`), not invalid documents.
pub fn validate(project: &Project) -> Result<()> {
    if project.edge_constraints.len() > MAX_EDGE_CONSTRAINTS {
        return Err(Error::Limit(format!(
            "board.constraint_limit: at most {MAX_EDGE_CONSTRAINTS} edge constraints"
        )));
    }
    let mut targets = std::collections::BTreeSet::new();
    for c in &project.edge_constraints {
        let target = match c.target {
            ConstraintTarget::Placement(id) => (0u8, id),
            ConstraintTarget::Fill(id) => (1, id),
            ConstraintTarget::Keepout(id) => (2, id),
        };
        if !targets.insert(target)
            || !c.along.valid()
            || !c.offset.valid()
            || !c.rotation_deg.is_finite()
            || c.edge.0.is_empty()
            || c.edge.0.len() > crate::ids::MAX_LABEL
        {
            return Err(Error::Invalid(
                "board.constraint: one constraint per target, with a named edge and well-formed expressions"
                    .into(),
            ));
        }
    }
    Ok(())
}
