//! Board shape: commands over the board sketch, the evaluator seam that
//! solves it (the integration crate's SolidCraft sketch engine, as for
//! footprints), and edge-relative placement constraints.
//!
//! The core never interprets the sketch. A [`BoardShapeEvaluator`] applies
//! sketch edits and solves the sketch with its parameters into a
//! [`SolvedBoard`], evaluating every constraint's expressions; the core
//! then places constrained targets with [`edge_pose`]. An unsolved or
//! over-constrained sketch is a finding, never a panic.
//!
//! Resolution runs after every engine operation ([`follow`]): when the
//! board, the constraints or a constrained target changed, every target is
//! placed where its constraint puts it, so parts move with the board's
//! parameters. A command that moves a constrained part elsewhere is refused
//! (`board.constrained`); a constraint whose edge or target is gone stays
//! stored and is reported by [`checks`] (`board.constraint_unresolved`).
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::geometry::{Bounds, Point, point_valid};
use crate::ids::Id;
use crate::library::footprint_source::{
    DimValue, EvaluatorRef, Expr, FootprintParam, ParamName, SketchBlob, SketchCurveId, SketchEdit,
    SketchOutcome, SketchPointId, SketchRef, SolveStatus, SolveSummary,
};
use crate::library::{MAX_OUTLINE_SEGMENTS, Outline};
use crate::model::board::{
    BoardFeatureId, BoardSketch, ConstraintAlign, ConstraintTarget, CutoutKind, CutoutRole,
    EdgeAnchor, EdgeConstraintSpec, EdgeName, MAX_CUTOUTS, MAX_EDGE_CONSTRAINTS,
    MAX_MOUNTING_HOLES, MAX_NAMED_EDGES, MountingHoleRole, NamedEdge, SolvedEdge, arc_geometry,
};
use crate::model::{BoardShape, EdgeConstraint, Project, SolvedBoard};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The largest bulge of a solved arc (as `library::Outline` allows).
const MAX_BULGE: f64 = 1e3;
/// Edits one `board.edit` applies.
pub const MAX_BOARD_EDITS: usize = 256;
/// Board parameters.
pub const MAX_BOARD_PARAMETERS: usize = 256;
/// Curves of one outline, cutout or slot loop.
pub const MAX_LOOP_CURVES: usize = MAX_OUTLINE_SEGMENTS;
/// Finding code: a constraint whose edge or target is gone.
pub const CONSTRAINT_UNRESOLVED: &str = "board.constraint_unresolved";
/// Finding code: a stored board sketch that did not solve.
pub const SHAPE_UNSOLVED: &str = "board.shape_unsolved";
/// Evaluator findings with this code prefix refuse the edit that caused
/// them (the board would have no valid shape); others are advisory.
pub const FATAL_PREFIX: &str = "board.shape_";
/// How far a resolved pose may differ from the stored one and still count
/// as the same, mm or degrees.
const SAME: f64 = 1e-9;
/// Chord tolerance for alignment extents, mm.
const EXTENT_CHORD_MM: f64 = 0.01;

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
    /// unknown parameter in a constraint). Codes starting
    /// [`FATAL_PREFIX`] refuse the edit.
    pub findings: Vec<Finding>,
    /// The sketch with its stored positions moved to the solve, so the
    /// next solve starts from it (`None`: keep the stored sketch).
    #[serde(default)]
    pub sketch_out: Option<SketchBlob>,
}

/// What [`BoardShapeEvaluator::fillet`] drew: the sketch and, per new
/// corner arc, one of the two lines it joins (so the arc joins that
/// line's loop).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Fillets {
    pub outcome: SketchOutcome,
    pub arcs: Vec<(SketchCurveId, SketchCurveId)>,
}

/// What [`BoardShapeEvaluator::slot`] drew: the sketch and the slot's
/// closed loop.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DrawnSlot {
    pub outcome: SketchOutcome,
    pub curves: Vec<SketchCurveId>,
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
    /// Round each corner point (where two lines meet) with a tangent arc
    /// of `radius`; the corner stays as the lines' virtual intersection,
    /// so dimensions to it keep holding. Empty `corners`: every corner of
    /// the outer outline.
    fn fillet(
        &self,
        _board: &BoardSketch,
        _corners: &[SketchPointId],
        _radius: &DimValue,
    ) -> Result<Fillets> {
        Err(Error::Unsupported(
            "board.fillet: this board evaluator cannot round corners".into(),
        ))
    }
    /// Draw a straight slot from cap centre `a` to cap centre `b` of
    /// `width` (its cap diameter).
    fn slot(
        &self,
        _board: &BoardSketch,
        _a: Point,
        _b: Point,
        _width: &DimValue,
    ) -> Result<DrawnSlot> {
        Err(Error::Unsupported(
            "board.slot: this board evaluator cannot draw slots".into(),
        ))
    }
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
    /// Round corners with tangent arcs of `radius` (a number or a board
    /// parameter); empty `corners` rounds every outer-outline corner.
    Fillet {
        #[serde(default)]
        corners: Vec<SketchPointId>,
        radius: DimValue,
    },
    /// A milled slot from cap centre `a` to `b`, `width` wide, as a
    /// [`CutoutKind::Slot`].
    AddSlot {
        a: Point,
        b: Point,
        width: DimValue,
        plated: bool,
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

// A new command chooses its permission class here.
declare_classes!(BoardCommand {
    SetRectangle "board.set_rectangle" Edit,
    StartSketch "board.start_sketch" Edit,
    Edit "board.edit" Edit,
    Constrain "board.constrain" Edit,
    EditConstraint "board.constraint_edit" Edit,
    DeleteConstraint "board.constraint_delete" Edit,
});

/// What a board command produced for its caller.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum BoardApplied {
    #[default]
    Nothing,
    /// The constraint `board.constrain` allocated.
    Constraint(Id),
    /// The sketch entities a `board.edit` created and kept.
    Edited { created: Vec<SketchRef> },
}

/// Apply a board command to a candidate. Constrained targets are placed by
/// [`follow`], which the engine runs after every operation.
pub fn apply(
    project: &mut Project,
    command: BoardCommand,
    evaluator: Option<&dyn BoardShapeEvaluator>,
) -> Result<BoardApplied> {
    match command {
        BoardCommand::SetRectangle { size } => {
            project.board_shape = BoardShape::Rectangle;
            super::apply_staged(project, super::LayoutCommand::SetBoardSize { size })?;
            Ok(BoardApplied::Nothing)
        }
        BoardCommand::StartSketch => {
            let evaluator = needs(evaluator)?;
            if !project.board_shape.is_rectangle() {
                return Err(Error::Conflict(
                    "board.sketch_exists: the board is already a sketch".into(),
                ));
            }
            let sketch = evaluator.rectangle(project.board_size)?;
            commit(project, sketch, evaluator)?;
            Ok(BoardApplied::Nothing)
        }
        BoardCommand::Edit { edits } => {
            let evaluator = needs(evaluator)?;
            if edits.is_empty() || edits.len() > MAX_BOARD_EDITS {
                return Err(Error::Limit(format!(
                    "board.edit: 1–{MAX_BOARD_EDITS} edits at a time"
                )));
            }
            let BoardShape::Sketch(sketch) = &project.board_shape else {
                return Err(Error::Conflict(
                    "board.not_sketch: start a board sketch first (board.start_sketch)".into(),
                ));
            };
            let mut sketch = (**sketch).clone();
            let mut created = Vec::new();
            let mut deleted = BTreeSet::new();
            for edit in edits {
                let changed = edit_one(&mut sketch, edit, evaluator)?;
                created.extend(changed.created);
                deleted.extend(changed.deleted);
            }
            retire(&mut sketch, &deleted)?;
            commit(project, sketch, evaluator)?;
            created.retain(|c| !deleted.contains(c));
            Ok(BoardApplied::Edited { created })
        }
        BoardCommand::Constrain { constraint } => {
            check_spec(project, &constraint)?;
            if project.edge_constraints.len() >= MAX_EDGE_CONSTRAINTS {
                return Err(Error::Limit(format!(
                    "board.constraint_limit: at most {MAX_EDGE_CONSTRAINTS} edge constraints"
                )));
            }
            if project
                .edge_constraints
                .iter()
                .any(|c| c.target == constraint.target)
            {
                return Err(Error::Conflict(
                    "board.constrained: the target already has an edge constraint".into(),
                ));
            }
            let id = project.allocate()?;
            project
                .edge_constraints
                .push(EdgeConstraint::from_spec(id, constraint));
            Ok(BoardApplied::Constraint(id))
        }
        BoardCommand::EditConstraint { constraint } => {
            check_spec(project, &spec_of(&constraint))?;
            if project
                .edge_constraints
                .iter()
                .any(|c| c.id != constraint.id && c.target == constraint.target)
            {
                return Err(Error::Conflict(
                    "board.constrained: the target already has an edge constraint".into(),
                ));
            }
            let slot = project
                .edge_constraints
                .iter_mut()
                .find(|c| c.id == constraint.id)
                .ok_or_else(|| missing(constraint.id))?;
            *slot = constraint;
            Ok(BoardApplied::Nothing)
        }
        BoardCommand::DeleteConstraint { id } => {
            let before = project.edge_constraints.len();
            project.edge_constraints.retain(|c| c.id != id);
            if project.edge_constraints.len() == before {
                return Err(missing(id));
            }
            Ok(BoardApplied::Nothing)
        }
    }
}

fn needs(evaluator: Option<&dyn BoardShapeEvaluator>) -> Result<&dyn BoardShapeEvaluator> {
    evaluator.ok_or_else(|| {
        Error::Unsupported(
            "board.evaluator: board sketches need the host's sketch evaluator".into(),
        )
    })
}

fn missing(id: Id) -> Error {
    Error::NotFound(format!("board.constraint_missing: no edge constraint {id}"))
}

fn spec_of(c: &EdgeConstraint) -> EdgeConstraintSpec {
    EdgeConstraintSpec {
        target: c.target,
        edge: c.edge.clone(),
        anchor: c.anchor,
        along: c.along.clone(),
        offset: c.offset.clone(),
        align: c.align,
        follow_edge: c.follow_edge,
        rotation_deg: c.rotation_deg,
    }
}

/// A new or edited constraint names a live target and edge, and asks only
/// for what its target supports.
fn check_spec(project: &Project, c: &EdgeConstraintSpec) -> Result<()> {
    match c.target {
        ConstraintTarget::Placement(id) => {
            if !project.placements.iter().any(|p| p.instance == id) {
                return Err(Error::NotFound(format!(
                    "board.target_missing: instance {id} is not placed on the board"
                )));
            }
        }
        ConstraintTarget::Fill(id) | ConstraintTarget::Keepout(id) => {
            let fill = matches!(c.target, ConstraintTarget::Fill(_));
            let exists = if fill {
                project.fills.iter().any(|f| f.id == id)
            } else {
                project.keepouts.iter().any(|k| k.id == id)
            };
            if !exists {
                return Err(Error::NotFound(format!(
                    "board.target_missing: no {} {id}",
                    if fill { "fill" } else { "keepout" }
                )));
            }
            if c.follow_edge || c.rotation_deg != 0.0 {
                return Err(Error::Invalid(
                    "board.constraint_rotation: fills and keepouts keep their orientation; \
                     only placements follow an edge's direction"
                        .into(),
                ));
            }
        }
    }
    if !c.edge.valid() {
        return Err(Error::Invalid(format!(
            "board.edge_name: `{}` is not an edge name",
            c.edge.0
        )));
    }
    if project.board().edge(&c.edge).is_none() {
        return Err(Error::NotFound(format!(
            "board.edge_missing: the board has no edge named `{}`",
            c.edge.0
        )));
    }
    anchor_valid(c.anchor)?;
    if !(c.along.valid() && c.offset.valid() && c.rotation_deg.is_finite()) {
        return Err(Error::Invalid(
            "board.constraint: `along` and `offset` need expressions and the rotation a number"
                .into(),
        ));
    }
    Ok(())
}

fn anchor_valid(anchor: EdgeAnchor) -> Result<()> {
    match anchor {
        EdgeAnchor::Fraction(f) if !(0.0..=1.0).contains(&f) => {
            Err(Error::Invalid("board.anchor: a fraction is 0–1".into()))
        }
        _ => Ok(()),
    }
}

fn curve_count(curves: &[SketchCurveId]) -> Result<()> {
    if curves.is_empty() || curves.len() > MAX_LOOP_CURVES {
        return Err(Error::Limit(format!(
            "board.loop: a loop has 1–{MAX_LOOP_CURVES} curves"
        )));
    }
    let unique: BTreeSet<&SketchCurveId> = curves.iter().collect();
    if unique.len() != curves.len() {
        return Err(Error::Invalid(
            "board.loop: a loop names a curve twice".into(),
        ));
    }
    Ok(())
}

fn next_feature(sketch: &mut BoardSketch) -> Result<BoardFeatureId> {
    let id = sketch.next_feature;
    sketch.next_feature = id
        .checked_add(1)
        .ok_or_else(|| Error::Limit("board.feature: identity space exhausted".into()))?;
    Ok(BoardFeatureId(id))
}

fn add_cutout(
    sketch: &mut BoardSketch,
    kind: CutoutKind,
    curves: Vec<SketchCurveId>,
) -> Result<()> {
    curve_count(&curves)?;
    if sketch.cutouts.len() >= MAX_CUTOUTS {
        return Err(Error::Limit(format!(
            "board.cutout_limit: at most {MAX_CUTOUTS} cutouts and slots"
        )));
    }
    let used: BTreeSet<&SketchCurveId> = sketch
        .outline
        .iter()
        .chain(sketch.cutouts.iter().flat_map(|c| &c.curves))
        .collect();
    if let Some(c) = curves.iter().find(|c| used.contains(c)) {
        return Err(Error::Conflict(format!(
            "board.loop: curve `{}` already bounds the outline or a cutout",
            c.0
        )));
    }
    let id = next_feature(sketch)?;
    sketch.cutouts.push(CutoutRole { id, kind, curves });
    Ok(())
}

/// The sketch entities one edit created and deleted.
#[derive(Default)]
struct SketchChanges {
    created: Vec<SketchRef>,
    deleted: Vec<SketchRef>,
}

impl SketchChanges {
    fn of(outcome: SketchOutcome) -> (SketchBlob, Self) {
        (
            outcome.sketch,
            Self {
                created: outcome.created,
                deleted: outcome.deleted,
            },
        )
    }
}

/// Apply one edit to the candidate sketch; returns the sketch entities it
/// created and deleted.
fn edit_one(
    sketch: &mut BoardSketch,
    edit: BoardEdit,
    evaluator: &dyn BoardShapeEvaluator,
) -> Result<SketchChanges> {
    match edit {
        BoardEdit::Sketch { edit } => {
            let (blob, changes) = SketchChanges::of(evaluator.apply_sketch(&sketch.sketch, &edit)?);
            sketch.sketch = blob;
            return Ok(changes);
        }
        BoardEdit::SetParameter { param } => {
            if !param.name.valid() || param.name.is_package() || !param.expr.valid() {
                return Err(Error::Invalid(format!(
                    "board.param: `{}` needs an identifier name (not `pkg_…`) and an expression",
                    param.name.0
                )));
            }
            match sketch.parameters.iter_mut().find(|p| p.name == param.name) {
                Some(slot) => *slot = param,
                None => {
                    if sketch.parameters.len() >= MAX_BOARD_PARAMETERS {
                        return Err(Error::Limit(format!(
                            "board.param: at most {MAX_BOARD_PARAMETERS} board parameters"
                        )));
                    }
                    sketch.parameters.push(param);
                }
            }
        }
        BoardEdit::RemoveParameter { name } => {
            let before = sketch.parameters.len();
            sketch.parameters.retain(|p| p.name != name);
            if sketch.parameters.len() == before {
                return Err(Error::NotFound(format!(
                    "board.param: no board parameter `{}`",
                    name.0
                )));
            }
        }
        BoardEdit::SetOutline { curves } => {
            curve_count(&curves)?;
            if let Some(c) = curves
                .iter()
                .find(|c| sketch.cutouts.iter().any(|k| k.curves.contains(c)))
            {
                return Err(Error::Conflict(format!(
                    "board.loop: curve `{}` bounds a cutout",
                    c.0
                )));
            }
            sketch.outline = curves;
        }
        BoardEdit::AddCutout { kind, curves } => add_cutout(sketch, kind, curves)?,
        BoardEdit::RemoveCutout { id } => {
            let before = sketch.cutouts.len();
            sketch.cutouts.retain(|c| c.id != id);
            if sketch.cutouts.len() == before {
                return Err(Error::NotFound(format!("board.cutout: no cutout {}", id.0)));
            }
        }
        BoardEdit::AddMountingHole {
            center,
            drill,
            plated,
            pad_diameter,
        } => {
            if sketch.holes.len() >= MAX_MOUNTING_HOLES {
                return Err(Error::Limit(format!(
                    "board.hole_limit: at most {MAX_MOUNTING_HOLES} mounting holes"
                )));
            }
            let id = next_feature(sketch)?;
            sketch.holes.push(MountingHoleRole {
                id,
                center,
                drill,
                plated,
                pad_diameter,
            });
        }
        BoardEdit::RemoveMountingHole { id } => {
            let before = sketch.holes.len();
            sketch.holes.retain(|h| h.id != id);
            if sketch.holes.len() == before {
                return Err(Error::NotFound(format!(
                    "board.hole: no mounting hole {}",
                    id.0
                )));
            }
        }
        BoardEdit::NameEdge { name, curve } => {
            if !name.valid() {
                return Err(Error::Invalid(format!(
                    "board.edge_name: `{}` is not an identifier",
                    name.0
                )));
            }
            if sketch.edges.iter().any(|e| e.name == name) {
                return Err(Error::Conflict(format!(
                    "board.edge_name: an edge is already named `{}`",
                    name.0
                )));
            }
            if sketch.edges.len() >= MAX_NAMED_EDGES {
                return Err(Error::Limit(format!(
                    "board.edge_limit: at most {MAX_NAMED_EDGES} named edges"
                )));
            }
            sketch.edges.push(NamedEdge { name, curve });
        }
        BoardEdit::UnnameEdge { name } => {
            let before = sketch.edges.len();
            sketch.edges.retain(|e| e.name != name);
            if sketch.edges.len() == before {
                return Err(Error::NotFound(format!(
                    "board.edge_name: no edge named `{}`",
                    name.0
                )));
            }
        }
        BoardEdit::Fillet { corners, radius } => {
            let drawn = evaluator.fillet(sketch, &corners, &radius)?;
            let (blob, changes) = SketchChanges::of(drawn.outcome);
            sketch.sketch = blob;
            for (arc, beside) in drawn.arcs {
                if sketch.outline.contains(&beside) {
                    sketch.outline.push(arc);
                } else if let Some(c) = sketch
                    .cutouts
                    .iter_mut()
                    .find(|c| c.curves.contains(&beside))
                {
                    c.curves.push(arc);
                }
            }
            return Ok(changes);
        }
        BoardEdit::AddSlot {
            a,
            b,
            width,
            plated,
        } => {
            if !(point_valid(a) && point_valid(b)) {
                return Err(Error::Invalid("board.slot: slot ends out of range".into()));
            }
            let drawn = evaluator.slot(sketch, a, b, &width)?;
            let (blob, changes) = SketchChanges::of(drawn.outcome);
            sketch.sketch = blob;
            add_cutout(sketch, CutoutKind::Slot { plated }, drawn.curves)?;
            return Ok(changes);
        }
    }
    Ok(SketchChanges::default())
}

/// Roles that name deleted sketch entities: a cutout, mounting hole or
/// edge name goes with its curve or point; the outer outline cannot.
fn retire(sketch: &mut BoardSketch, deleted: &BTreeSet<SketchRef>) -> Result<()> {
    if deleted.is_empty() {
        return Ok(());
    }
    let gone = |c: &SketchCurveId| deleted.contains(&SketchRef::Curve(c.clone()));
    if let Some(c) = sketch.outline.iter().find(|c| gone(c)) {
        return Err(Error::Conflict(format!(
            "board.outline_curve: curve `{}` bounds the board outline; set a new outline in the \
             same edit",
            c.0
        )));
    }
    sketch.cutouts.retain(|k| !k.curves.iter().any(gone));
    sketch.edges.retain(|e| !gone(&e.curve));
    sketch
        .holes
        .retain(|h| !deleted.contains(&SketchRef::Point(h.center.clone())));
    Ok(())
}

/// Solve a candidate sketch and store it with its solve; refuses a shape
/// that does not solve, and constraint expressions that no longer
/// evaluate.
fn commit(
    project: &mut Project,
    mut sketch: BoardSketch,
    evaluator: &dyn BoardShapeEvaluator,
) -> Result<()> {
    let evaluation = evaluator.evaluate(&sketch, &project.edge_constraints)?;
    if let Some(f) = evaluation
        .findings
        .iter()
        .find(|f| f.code.starts_with(FATAL_PREFIX))
    {
        return Err(Error::Invalid(format!("{}: {}", f.code, f.message)));
    }
    if matches!(
        evaluation.solve.status,
        SolveStatus::OverConstrained | SolveStatus::Failed
    ) {
        return Err(Error::Invalid(format!(
            "{SHAPE_UNSOLVED}: the board sketch does not solve"
        )));
    }
    expressions_evaluated(project, &evaluation)?;
    let size = sketch_size(&evaluation.solved)?;
    sketch.solve = evaluation.solve;
    sketch.solved = evaluation.solved;
    if let Some(blob) = evaluation.sketch_out {
        sketch.sketch = blob;
    }
    project.board_size = size;
    project.board_shape = BoardShape::Sketch(Box::new(sketch));
    Ok(())
}

/// Every constraint's expressions evaluated, or the first that did not.
fn expressions_evaluated(project: &Project, evaluation: &BoardEvaluation) -> Result<()> {
    let have: BTreeSet<Id> = evaluation
        .constraints
        .iter()
        .map(|v| v.constraint)
        .collect();
    if let Some(c) = project
        .edge_constraints
        .iter()
        .find(|c| !have.contains(&c.id))
    {
        let why = evaluation
            .findings
            .iter()
            .find(|f| f.message.contains(&format!("constraint {}", c.id)))
            .map_or_else(String::new, |f| format!(": {}", f.message));
        return Err(Error::Invalid(format!(
            "board.constraint_expr: edge constraint {} does not evaluate{why}",
            c.id
        )));
    }
    Ok(())
}

/// A sketch board's `board_size`: the box of its outline, which must lie
/// in the first quadrant (the board's lower-left frame starts at the
/// origin) within the board size range.
fn sketch_size(solved: &SolvedBoard) -> Result<Point> {
    let Bounds { min, max } = solved.bounds();
    if min.iter().any(|v| *v < -1e-6) {
        return Err(Error::Invalid(format!(
            "{FATAL_PREFIX}origin: the board outline must lie right of and above the origin \
             (it reaches {:.3}, {:.3})",
            min[0], min[1]
        )));
    }
    if !point_valid(max) || !max.iter().all(|v| (5.0..=1000.0).contains(v)) {
        return Err(Error::Invalid(format!(
            "{FATAL_PREFIX}size: the board outline must span 5–1000 mm (it spans {:.3} × {:.3})",
            max[0], max[1]
        )));
    }
    Ok(max)
}

/// A constrained position and direction.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EdgePose {
    pub position: Point,
    /// The edge's direction at that point, degrees.
    pub edge_deg: f64,
}

/// The point `along` mm from `anchor` on `edge` and `offset` mm along its
/// inward (left) normal. On an arc edge `along` is arc length and the
/// direction is the tangent there.
pub fn edge_pose(
    edge: &SolvedEdge,
    anchor: EdgeAnchor,
    along: f64,
    offset: f64,
) -> Result<EdgePose> {
    if !(along.is_finite() && offset.is_finite()) {
        return Err(Error::Invalid(
            "board.edge: degenerate edge or value".into(),
        ));
    }
    let arc = if edge.bulge == 0.0 {
        None
    } else {
        Some(
            arc_geometry(edge.a, edge.b, edge.bulge)
                .ok_or_else(|| Error::Invalid("board.edge: degenerate arc edge".into()))?,
        )
    };
    let len = match &arc {
        Some(arc) => arc.length(),
        None => (edge.b[0] - edge.a[0]).hypot(edge.b[1] - edge.a[1]),
    };
    if !(len.is_finite() && len > 1e-9) {
        return Err(Error::Invalid(
            "board.edge: degenerate edge or value".into(),
        ));
    }
    anchor_valid(anchor)?;
    let start = match anchor {
        EdgeAnchor::Start => 0.0,
        EdgeAnchor::Middle => len / 2.0,
        EdgeAnchor::End => len,
        EdgeAnchor::Fraction(f) => f * len,
    };
    let s = start + along;
    let (point, u) = match &arc {
        None => {
            let u = [(edge.b[0] - edge.a[0]) / len, (edge.b[1] - edge.a[1]) / len];
            ([edge.a[0] + u[0] * s, edge.a[1] + u[1] * s], u)
        }
        Some(arc) => {
            let turn = arc.sweep.signum();
            let angle = arc.start + turn * s / arc.radius;
            let (sin, cos) = angle.sin_cos();
            (arc.at(angle), [-sin * turn, cos * turn])
        }
    };
    let position = [point[0] - u[1] * offset, point[1] + u[0] * offset];
    if !point_valid(position) {
        return Err(Error::Invalid("board.edge: position out of range".into()));
    }
    Ok(EdgePose {
        position,
        edge_deg: u[1].atan2(u[0]).to_degrees(),
    })
}

/// A plain length in mm (`12.5`, `-3 mm`), which the core reads without
/// the host's evaluator.
fn plain_mm(expr: &Expr) -> Option<f64> {
    let text = expr.0.trim();
    let number = text.strip_suffix("mm").map_or(text, str::trim_end);
    if number.is_empty()
        || !number
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
    {
        return None;
    }
    number.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Every constraint's (along, offset) in mm: plain lengths read in core,
/// anything else through the host's evaluator.
fn values(
    project: &Project,
    evaluator: Option<&dyn BoardShapeEvaluator>,
) -> Result<BTreeMap<Id, (f64, f64)>> {
    let plain: Option<BTreeMap<Id, (f64, f64)>> = project
        .edge_constraints
        .iter()
        .map(|c| Some((c.id, (plain_mm(&c.along)?, plain_mm(&c.offset)?))))
        .collect();
    if let Some(plain) = plain {
        return Ok(plain);
    }
    let evaluator = evaluator.ok_or_else(|| {
        Error::Unsupported(
            "board.evaluator: edge constraints with expressions need the host's sketch evaluator"
                .into(),
        )
    })?;
    let board = match &project.board_shape {
        BoardShape::Rectangle => evaluator.rectangle(project.board_size)?,
        BoardShape::Sketch(sketch) => (**sketch).clone(),
    };
    let evaluation = evaluator.evaluate(&board, &project.edge_constraints)?;
    expressions_evaluated(project, &evaluation)?;
    Ok(evaluation
        .constraints
        .iter()
        .map(|v| (v.constraint, (v.along, v.offset)))
        .collect())
}

/// What each constrained target's pose depends on, to skip resolution
/// when an operation touched none of it.
fn inputs_changed(before: &Project, after: &Project) -> bool {
    if before.board_size != after.board_size
        || before.board_shape != after.board_shape
        || before.edge_constraints != after.edge_constraints
    {
        return true;
    }
    after.edge_constraints.iter().any(|c| match c.target {
        ConstraintTarget::Placement(id) => {
            let a = before.placements.iter().find(|p| p.instance == id);
            let b = after.placements.iter().find(|p| p.instance == id);
            a != b
        }
        ConstraintTarget::Fill(id) => {
            before.fills.iter().find(|f| f.id == id).map(|f| &f.outline)
                != after.fills.iter().find(|f| f.id == id).map(|f| &f.outline)
        }
        ConstraintTarget::Keepout(id) => {
            before
                .keepouts
                .iter()
                .find(|k| k.id == id)
                .map(|k| &k.outline)
                != after
                    .keepouts
                    .iter()
                    .find(|k| k.id == id)
                    .map(|k| &k.outline)
        }
    })
}

/// Unit direction of an edge pose and its inward normal.
fn frame(pose: &EdgePose) -> (Point, Point) {
    let (sin, cos) = pose.edge_deg.to_radians().sin_cos();
    ([cos, sin], [-sin, cos])
}

/// Where the reference point (`align`) of points `pts`, given relative to
/// the target's origin, lies relative to that origin.
fn reference(pts: &[Point], align: ConstraintAlign, u: Point, n: Point) -> Point {
    if align == ConstraintAlign::Origin || pts.is_empty() {
        return [0.0, 0.0];
    }
    let dot = |p: &Point, d: Point| p[0] * d[0] + p[1] * d[1];
    let (mut s0, mut s1, mut t0, mut t1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in pts {
        let (s, t) = (dot(p, u), dot(p, n));
        s0 = s0.min(s);
        s1 = s1.max(s);
        t0 = t0.min(t);
        t1 = t1.max(t);
    }
    let s = (s0 + s1) / 2.0;
    let t = match align {
        ConstraintAlign::NearSide => t0,
        _ => (t0 + t1) / 2.0,
    };
    [u[0] * s + n[0] * t, u[1] * s + n[1] * t]
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= SAME * a.abs().max(b.abs()).max(1.0)
}

fn same_angle(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(360.0);
    d <= 1e-9 || 360.0 - d <= 1e-9
}

/// Place every constrained target where its constraint puts it, after an
/// operation took the project from `before` to `project`. A constraint
/// whose edge or target is gone is left for [`checks`]. Refuses
/// (`board.constrained`) an operation that moved a constrained part away
/// from its constrained pose, and expressions that do not evaluate.
pub fn follow(
    before: &Project,
    project: &mut Project,
    evaluator: Option<&dyn BoardShapeEvaluator>,
) -> Result<()> {
    if project.edge_constraints.is_empty() || !inputs_changed(before, project) {
        return Ok(());
    }
    let values = values(project, evaluator)?;
    let board = project.board().into_owned();
    // Poses first, with every followed rotation, so alignment extents are
    // taken at the final orientation.
    let mut poses = Vec::new();
    for c in &project.edge_constraints {
        let (Some(edge), Some(&(along, offset))) = (board.edge(&c.edge), values.get(&c.id)) else {
            continue;
        };
        let pose = edge_pose(edge, c.anchor, along, offset).map_err(|e| {
            Error::Invalid(format!("board.constraint: edge constraint {}: {e}", c.id))
        })?;
        poses.push((c.clone(), pose));
    }
    // The rotations the operation left, before any follows its edge.
    let commanded: BTreeMap<Id, f64> = project
        .placements
        .iter()
        .map(|p| (p.instance, p.rotation_deg))
        .collect();
    for (c, pose) in &poses {
        if let ConstraintTarget::Placement(id) = c.target
            && c.follow_edge
            && let Some(p) = project.placements.iter_mut().find(|p| p.instance == id)
        {
            let rotation = (pose.edge_deg + c.rotation_deg).rem_euclid(360.0);
            if !same_angle(p.rotation_deg, rotation) {
                p.rotation_deg = rotation;
            }
        }
    }
    let aligned = poses.iter().any(|(c, _)| {
        matches!(c.target, ConstraintTarget::Placement(_)) && c.align != ConstraintAlign::Origin
    });
    let extents = if aligned {
        placement_extents(project)
    } else {
        BTreeMap::new()
    };
    for (c, pose) in &poses {
        let (u, n) = frame(pose);
        match c.target {
            ConstraintTarget::Placement(id) => {
                let Some(index) = project.placements.iter().position(|p| p.instance == id) else {
                    continue;
                };
                let Some(p) = project.placements.get_mut(index) else {
                    continue;
                };
                let local: Vec<Point> = extents
                    .get(&id)
                    .map(|pts| {
                        pts.iter()
                            .map(|q| [q[0] - p.position[0], q[1] - p.position[1]])
                            .collect()
                    })
                    .unwrap_or_default();
                let r = reference(&local, c.align, u, n);
                let position = [pose.position[0] - r[0], pose.position[1] - r[1]];
                if !point_valid(position) {
                    return Err(Error::Invalid(format!(
                        "board.constraint: edge constraint {} puts its part out of range",
                        c.id
                    )));
                }
                let old = before.placements.iter().find(|q| q.instance == id);
                let commanded = commanded.get(&id).copied().unwrap_or(p.rotation_deg);
                let moved = old.is_some_and(|q| q.position != p.position);
                let turned = old.is_some_and(|q| q.rotation_deg != commanded);
                let lands = near(position[0], p.position[0]) && near(position[1], p.position[1]);
                if moved && !lands
                    || c.follow_edge
                        && turned
                        && !same_angle(
                            (pose.edge_deg + c.rotation_deg).rem_euclid(360.0),
                            commanded,
                        )
                {
                    let name = project
                        .instance(id)
                        .map_or_else(|| id.to_string(), |i| i.designator.clone());
                    return Err(Error::Conflict(format!(
                        "board.constrained: {name} is placed by edge constraint {} (edge `{}`); \
                         edit or delete the constraint to move it",
                        c.id, c.edge.0
                    )));
                }
                if !lands {
                    p.position = position;
                }
            }
            ConstraintTarget::Fill(id) => {
                if let Some(f) = project.fills.iter_mut().find(|f| f.id == id) {
                    place_outline(&mut f.outline, c.align, pose, u, n)?;
                }
            }
            ConstraintTarget::Keepout(id) => {
                if let Some(k) = project.keepouts.iter_mut().find(|k| k.id == id) {
                    place_outline(&mut k.outline, c.align, pose, u, n)?;
                }
            }
        }
    }
    Ok(())
}

/// Translate an outline so its reference point (its start for
/// [`ConstraintAlign::Origin`]) lands on the pose.
fn place_outline(
    outline: &mut Outline,
    align: ConstraintAlign,
    pose: &EdgePose,
    u: Point,
    n: Point,
) -> Result<()> {
    let origin = outline.start;
    let pts: Vec<Point> = crate::model::board::flatten(outline, EXTENT_CHORD_MM)
        .into_iter()
        .map(|p| [p[0] - origin[0], p[1] - origin[1]])
        .collect();
    let r = reference(&pts, align, u, n);
    let delta = [
        pose.position[0] - r[0] - origin[0],
        pose.position[1] - r[1] - origin[1],
    ];
    if delta.iter().all(|d| d.abs() <= SAME) {
        return Ok(());
    }
    let shift = |p: Point| [p[0] + delta[0], p[1] + delta[1]];
    outline.start = shift(outline.start);
    for s in &mut outline.segments {
        match s {
            crate::library::Segment::Line { to } | crate::library::Segment::Arc { to, .. } => {
                *to = shift(*to);
            }
        }
    }
    let finite = point_valid(outline.start) && outline.segments.iter().all(|s| point_valid(s.to()));
    if !finite {
        return Err(Error::Invalid(
            "board.constraint: an edge constraint puts its outline out of range".into(),
        ));
    }
    Ok(())
}

/// Board-coordinate points of each placed part's extent: its closed
/// courtyard shapes, or its pads' copper when it has no courtyard.
fn placement_extents(project: &Project) -> BTreeMap<Id, Vec<Point>> {
    let mut out: BTreeMap<Id, Vec<Point>> = BTreeMap::new();
    for c in super::courtyard::placed_courtyards(project) {
        out.entry(c.instance)
            .or_default()
            .extend(c.shape.flatten(EXTENT_CHORD_MM));
    }
    let mut pads: BTreeMap<Id, Vec<Point>> = BTreeMap::new();
    for pad in super::pads(project) {
        if !out.contains_key(&pad.instance) {
            pads.entry(pad.instance)
                .or_default()
                .extend(pad.outline().flatten(EXTENT_CHORD_MM));
        }
    }
    out.extend(pads);
    out
}

/// Board-shape findings: a stored sketch that did not solve
/// ([`SHAPE_UNSOLVED`]) and constraints whose edge or target is gone
/// ([`CONSTRAINT_UNRESOLVED`]).
pub fn checks(project: &Project) -> Vec<Finding> {
    let mut out = Vec::new();
    if let BoardShape::Sketch(sketch) = &project.board_shape
        && matches!(
            sketch.solve.status,
            SolveStatus::OverConstrained | SolveStatus::Failed
        )
    {
        out.push(Finding::new(
            SHAPE_UNSOLVED,
            "The board sketch does not solve; the board keeps its last solved shape".into(),
            vec![ObjectRef::Board],
        ));
    }
    if project.edge_constraints.is_empty() {
        return out;
    }
    let board = project.board();
    for c in &project.edge_constraints {
        let (target, objects) = match c.target {
            ConstraintTarget::Placement(id) => (
                project
                    .placements
                    .iter()
                    .any(|p| p.instance == id)
                    .then(|| {
                        project
                            .instance(id)
                            .map_or_else(|| id.to_string(), |i| i.designator.clone())
                    }),
                vec![ObjectRef::Instance { id }],
            ),
            ConstraintTarget::Fill(id) => (
                project
                    .fills
                    .iter()
                    .find(|f| f.id == id)
                    .map(|f| format!("fill {}", f.name)),
                vec![ObjectRef::Board],
            ),
            ConstraintTarget::Keepout(id) => (
                project
                    .keepouts
                    .iter()
                    .find(|k| k.id == id)
                    .map(|k| format!("keepout {}", k.name)),
                vec![ObjectRef::Board],
            ),
        };
        let message = match (target, board.edge(&c.edge)) {
            (None, _) => format!(
                "Edge constraint {} places an object that is no longer on the board",
                c.id
            ),
            (Some(t), None) => format!(
                "{t} is constrained to edge `{}`, which the board no longer has",
                c.edge.0
            ),
            (Some(_), Some(_)) => continue,
        };
        out.push(Finding::new(CONSTRAINT_UNRESOLVED, message, objects));
    }
    out
}

/// Structural bounds: constraints within the limit, unique per target,
/// expressions well formed; a sketch board's roles bounded and unique, its
/// solved shape finite and `board_size` its box. Unknown edges or vanished
/// targets are check findings (`board.constraint_unresolved`), not invalid
/// documents.
pub fn validate(project: &Project) -> Result<()> {
    if project.edge_constraints.len() > MAX_EDGE_CONSTRAINTS {
        return Err(Error::Limit(format!(
            "board.constraint_limit: at most {MAX_EDGE_CONSTRAINTS} edge constraints"
        )));
    }
    let mut targets = BTreeSet::new();
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
            || anchor_valid(c.anchor).is_err()
            || c.edge.0.is_empty()
            || c.edge.0.len() > crate::ids::MAX_LABEL
        {
            return Err(Error::Invalid(
                "board.constraint: one constraint per target, with a named edge and well-formed expressions"
                    .into(),
            ));
        }
    }
    if let BoardShape::Sketch(sketch) = &project.board_shape {
        validate_sketch(sketch)?;
        let size = sketch_size(&sketch.solved)?;
        if !(near_mm(size[0], project.board_size[0]) && near_mm(size[1], project.board_size[1])) {
            return Err(Error::Invalid(
                "board.size_sketch: a sketch board's size is its outline's box; change the \
                 sketch's parameters (board.edit) or go back to a rectangle"
                    .into(),
            ));
        }
    }
    Ok(())
}

fn near_mm(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6
}

fn validate_sketch(sketch: &BoardSketch) -> Result<()> {
    let bad = |what: &str| Err(Error::Invalid(format!("board.sketch: {what}")));
    if sketch.sketch.0.len() > crate::library::footprint_source::MAX_SKETCH_BYTES {
        return Err(Error::Limit("board.sketch: the sketch is too large".into()));
    }
    curve_count(&sketch.outline)?;
    if sketch.cutouts.len() > MAX_CUTOUTS
        || sketch.holes.len() > MAX_MOUNTING_HOLES
        || sketch.edges.len() > MAX_NAMED_EDGES
        || sketch.parameters.len() > MAX_BOARD_PARAMETERS
    {
        return Err(Error::Limit("board.sketch: too many roles".into()));
    }
    let mut features = BTreeSet::new();
    let ids = sketch
        .cutouts
        .iter()
        .map(|c| c.id)
        .chain(sketch.holes.iter().map(|h| h.id));
    for id in ids {
        if id.0 >= sketch.next_feature || !features.insert(id) {
            return bad("cutout and hole identities are unique and allocated");
        }
    }
    let mut loops: BTreeSet<&SketchCurveId> = sketch.outline.iter().collect();
    for c in &sketch.cutouts {
        curve_count(&c.curves)?;
        for curve in &c.curves {
            if !loops.insert(curve) {
                return bad("a curve bounds two loops");
            }
        }
    }
    let mut names = BTreeSet::new();
    for e in &sketch.edges {
        if !e.name.valid() || !names.insert(&e.name) {
            return bad("edge names are unique identifiers");
        }
    }
    let mut params = BTreeSet::new();
    for p in &sketch.parameters {
        if !p.name.valid() || p.name.is_package() || !p.expr.valid() || !params.insert(&p.name) {
            return bad("board parameters have unique names and expressions");
        }
    }
    for h in &sketch.holes {
        if !h.drill.valid() || h.pad_diameter.as_ref().is_some_and(|d| !d.valid()) {
            return bad("mounting holes need drill expressions");
        }
    }
    let solved = &sketch.solved;
    let outlines = std::iter::once(&solved.outer).chain(solved.cutouts.iter().map(|c| &c.outline));
    for o in outlines {
        let finite = point_valid(o.start)
            && o.segments.iter().all(|s| match s {
                crate::library::Segment::Line { to } => point_valid(*to),
                crate::library::Segment::Arc { to, bulge } => {
                    point_valid(*to) && bulge.is_finite() && bulge.abs() <= MAX_BULGE
                }
            });
        if o.segments.is_empty() || o.segments.len() > MAX_OUTLINE_SEGMENTS || !finite {
            return bad("the solved outline is out of range");
        }
    }
    if solved.cutouts.len() > MAX_CUTOUTS
        || solved.holes.len() > MAX_MOUNTING_HOLES
        || solved.edges.len() > MAX_NAMED_EDGES
        || solved.holes.iter().any(|h| {
            !(point_valid(h.at) && h.drill.is_finite() && h.drill > 0.0)
                || h.pad_diameter
                    .is_some_and(|d| !(d.is_finite() && d > h.drill))
        })
        || solved
            .edges
            .iter()
            .any(|e| !point_valid(e.a) || !point_valid(e.b) || !e.bulge.is_finite())
    {
        return bad("the solved holes or edges are out of range");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
