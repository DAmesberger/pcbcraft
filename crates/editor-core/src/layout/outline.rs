//! Board shape: commands over the board sketch and the evaluator seam that
//! solves it (the integration crate's SolidCraft sketch engine, as for
//! footprints).
//!
//! The core never interprets the sketch. A [`BoardShapeEvaluator`] applies
//! sketch edits and solves the sketch with its parameters into a
//! [`SolvedBoard`]. An unsolved or over-constrained sketch is a finding,
//! never a panic. Parts held to the board by dimensions are solved after
//! the board (`layout::dimensions`).
//!
//! The outline is the sketch's one closed region, found by the evaluator
//! from whatever curves close; free-form curves reach the solved outline as
//! lines and arcs within the `board.outline_fit` rule. Curves that do not
//! make one board are reported ([`SHAPE_OPEN`], [`SHAPE_INVALID`]) and the
//! board keeps its last valid shape (docs/board-sketch-references.md §5.1).
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::geometry::{Bounds, Point, point_valid};
use crate::ids::Id;
use crate::library::MAX_OUTLINE_SEGMENTS;
use crate::library::footprint_source::{
    DimValue, EvaluatorRef, Expr, FootprintParam, ParamName, SketchBlob, SketchCurveId, SketchEdit,
    SketchOutcome, SketchPointId, SketchRef, SolveStatus, SolveSummary,
};
use crate::model::board::{
    BoardFeatureId, BoardSketch, CutoutKind, CutoutRole, EdgeName, MAX_CUTOUTS, MAX_FAULT_POINTS,
    MAX_FAULT_TEXT, MAX_MOUNTING_HOLES, MAX_NAMED_EDGES, MountingHoleRole, NamedEdge, ShapeFault,
    ShapeFaultKind,
};
use crate::model::{BoardShape, Project, SolvedBoard};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The largest bulge of a solved arc (as `library::Outline` allows).
const MAX_BULGE: f64 = 1e3;
/// Edits one `board.edit` applies.
pub const MAX_BOARD_EDITS: usize = 256;
/// Board parameters.
pub const MAX_BOARD_PARAMETERS: usize = 256;
/// Curves of one outline, cutout or slot loop.
pub const MAX_LOOP_CURVES: usize = MAX_OUTLINE_SEGMENTS;
/// Finding code: a stored board sketch that did not solve.
pub const SHAPE_UNSOLVED: &str = "board.shape_unsolved";
/// Finding code: the board sketch's curves do not close; it names the open
/// ends. Advisory in Board shape mode, an error everywhere else; the board
/// keeps its last valid shape (slice S1, §5.1). Not a refusal, although it
/// starts with [`FATAL_PREFIX`].
pub const SHAPE_OPEN: &str = "board.shape_open";
/// Finding code: the curves close but are not one board (more than one
/// region, a self-crossing outer loop, a touching hole); the board keeps
/// its last valid shape (slice S1, §5.1). Not a refusal, although it starts
/// with [`FATAL_PREFIX`].
pub const SHAPE_INVALID: &str = "board.shape_invalid";
/// Evaluator findings with this code prefix refuse the edit that caused
/// them (the board would have no valid shape); others are advisory.
/// [`SHAPE_OPEN`] and [`SHAPE_INVALID`] are exempt ([`refuses`]).
pub const FATAL_PREFIX: &str = "board.shape_";

/// Whether an evaluator finding refuses the edit: its code starts with
/// [`FATAL_PREFIX`] and it is not one of the reported shape faults, which
/// keep the last valid shape instead.
pub fn refuses(code: &str) -> bool {
    code.starts_with(FATAL_PREFIX) && code != SHAPE_OPEN && code != SHAPE_INVALID
}

/// What a board evaluation produced.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BoardEvaluation {
    pub solved: SolvedBoard,
    pub solve: SolveSummary,
    /// Design problems (unsolved, over-constrained). Codes that
    /// [`refuses`] names refuse the edit.
    pub findings: Vec<Finding>,
    /// Why the curves are not a board; `solved` is then the board's last
    /// valid shape.
    #[serde(default)]
    pub fault: Option<ShapeFault>,
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
    /// Solve the board with its parameters: the outline is the sketch's one
    /// closed region, its free-form curves fitted with lines and arcs within
    /// `board.outline_fit`. `Err` only refuses the input outright.
    fn evaluate(&self, board: &BoardSketch) -> Result<BoardEvaluation>;
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
    /// Draw a free-form curve.
    fn add_curve(&self, _board: &BoardSketch, _curve: &FreeFormCurve) -> Result<SketchOutcome> {
        Err(Error::Unsupported(
            "board.free_form: free-form board curves are not yet supported".into(),
        ))
    }
    /// Solve the dimensioned parts of a request: the parts solve
    /// (`layout::dimensions`, slice S2).
    fn solve_parts(
        &self,
        _request: &super::dimensions::PartsRequest<'_>,
    ) -> Result<super::dimensions::PartsSolution> {
        Err(super::dimensions::not_yet())
    }
    /// Trace a face, edge or vertex of a part file in the board plane
    /// (`layout::dimensions::model`, slice S3). `Err(NotFound)`: the model
    /// has no such face; `Err(Unsupported)`: it has no single trace in the
    /// board plane (a sloped face).
    fn trace_model(
        &self,
        _request: &super::dimensions::model::TraceRequest<'_>,
    ) -> Result<super::dimensions::model::Traced> {
        Err(Error::Unsupported(
            "layout.dimension: this board evaluator cannot read 3D models".into(),
        ))
    }
    /// Name the face or edge of a part file at a point picked in the 3D
    /// view (`layout::dimensions::model::pick`, slice S5): its body, its
    /// persistent name and the point in the model frame.
    fn pick_model(
        &self,
        _request: &super::dimensions::model::PickRequest<'_>,
    ) -> Result<super::dimensions::model::ModelPicked> {
        Err(Error::Unsupported(
            "layout.dimension: this board evaluator cannot read 3D models".into(),
        ))
    }
}

/// A free-form board curve to draw, in board millimetres (Solidcraft's
/// `CurveKind::{Spline, Ellipse, Conic}`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "curve", deny_unknown_fields)]
pub enum FreeFormCurve {
    /// Through `points` (fit points), or a clamped cubic B-spline over them
    /// as control points; `periodic` closes a fit spline smoothly.
    Spline {
        points: Vec<Point>,
        control: bool,
        periodic: bool,
        construction: bool,
    },
    /// Centre, the end of the major axis, and the minor radius (mm).
    Ellipse {
        center: Point,
        major: Point,
        minor_radius: f64,
        construction: bool,
    },
    /// From `a` to `b` shaped by `apex` and `rho` in (0, 1): 0.5 a
    /// parabola, less elliptical, more hyperbolic.
    Conic {
        a: Point,
        b: Point,
        apex: Point,
        rho: f64,
        construction: bool,
    },
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
    /// What the inner loop of these curves cuts: a slot role keyed by its
    /// curve ids, or back to a plain cutout (every hole is one without a
    /// role).
    SetCutoutKind {
        curves: Vec<SketchCurveId>,
        kind: CutoutKind,
    },
    /// A spline through (or controlled by) `points`.
    AddSpline {
        points: Vec<Point>,
        #[serde(default)]
        control: bool,
        #[serde(default)]
        periodic: bool,
        #[serde(default)]
        construction: bool,
    },
    /// An ellipse.
    AddEllipse {
        center: Point,
        major: Point,
        minor_radius: f64,
        #[serde(default)]
        construction: bool,
    },
    /// A conic from `a` to `b` shaped by `apex` and `rho`.
    AddConic {
        a: Point,
        b: Point,
        apex: Point,
        rho: f64,
        #[serde(default)]
        construction: bool,
    },
}

/// Board shape mutations; each is one undo step.
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
}

// A new command chooses its permission class here.
declare_classes!(BoardCommand {
    SetRectangle "board.set_rectangle" Edit "Set a rectangular outline",
    StartSketch "board.start_sketch" Edit "Start an outline sketch",
    Edit "board.edit" Edit "Edit the board outline",
});

/// What a board command produced for its caller.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum BoardApplied {
    #[default]
    Nothing,
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
    }
}

fn needs(evaluator: Option<&dyn BoardShapeEvaluator>) -> Result<&dyn BoardShapeEvaluator> {
    evaluator.ok_or_else(|| {
        Error::Unsupported(
            "board.evaluator: board sketches need the host's sketch evaluator".into(),
        )
    })
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

/// Give the inner loop of `curves` its kind: a slot keeps (or gets) a role
/// keyed by the whole curve set; a plain cutout needs none.
fn set_cutout_kind(
    sketch: &mut BoardSketch,
    kind: CutoutKind,
    curves: Vec<SketchCurveId>,
) -> Result<()> {
    curve_count(&curves)?;
    let set: BTreeSet<&SketchCurveId> = curves.iter().collect();
    let same = |role: &CutoutRole| role.curves.iter().collect::<BTreeSet<_>>() == set;
    if let Some(at) = sketch.cutouts.iter().position(same) {
        match kind {
            CutoutKind::Cutout => {
                sketch.cutouts.remove(at);
            }
            CutoutKind::Slot { .. } => {
                if let Some(role) = sketch.cutouts.get_mut(at) {
                    role.kind = kind;
                }
            }
        }
        return Ok(());
    }
    if kind == CutoutKind::Cutout {
        return Ok(());
    }
    if sketch.cutouts.len() >= MAX_CUTOUTS {
        return Err(Error::Limit(format!(
            "board.cutout_limit: at most {MAX_CUTOUTS} slots"
        )));
    }
    if let Some(c) = curves
        .iter()
        .find(|c| sketch.cutouts.iter().any(|k| k.curves.contains(c)))
    {
        return Err(Error::Conflict(format!(
            "board.loop: curve `{}` already bounds another slot",
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
            // A rounded corner of a slot stays in the slot's loop.
            for (arc, beside) in drawn.arcs {
                if let Some(c) = sketch
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
            set_cutout_kind(sketch, CutoutKind::Slot { plated }, drawn.curves)?;
            return Ok(changes);
        }
        BoardEdit::SetCutoutKind { curves, kind } => set_cutout_kind(sketch, kind, curves)?,
        BoardEdit::AddSpline {
            points,
            control,
            periodic,
            construction,
        } => {
            return add_curve(
                sketch,
                FreeFormCurve::Spline {
                    points,
                    control,
                    periodic,
                    construction,
                },
                evaluator,
            );
        }
        BoardEdit::AddEllipse {
            center,
            major,
            minor_radius,
            construction,
        } => {
            return add_curve(
                sketch,
                FreeFormCurve::Ellipse {
                    center,
                    major,
                    minor_radius,
                    construction,
                },
                evaluator,
            );
        }
        BoardEdit::AddConic {
            a,
            b,
            apex,
            rho,
            construction,
        } => {
            return add_curve(
                sketch,
                FreeFormCurve::Conic {
                    a,
                    b,
                    apex,
                    rho,
                    construction,
                },
                evaluator,
            );
        }
    }
    Ok(SketchChanges::default())
}

/// Draw a free-form curve into the candidate sketch.
fn add_curve(
    sketch: &mut BoardSketch,
    curve: FreeFormCurve,
    evaluator: &dyn BoardShapeEvaluator,
) -> Result<SketchChanges> {
    let (blob, changes) = SketchChanges::of(evaluator.add_curve(sketch, &curve)?);
    sketch.sketch = blob;
    Ok(changes)
}

/// Roles that name deleted sketch entities: a slot, mounting hole or edge
/// name goes with its curve or point. Deleting an outline curve opens the
/// outline, which the evaluation reports ([`SHAPE_OPEN`]).
fn retire(sketch: &mut BoardSketch, deleted: &BTreeSet<SketchRef>) -> Result<()> {
    if deleted.is_empty() {
        return Ok(());
    }
    let gone = |c: &SketchCurveId| deleted.contains(&SketchRef::Curve(c.clone()));
    sketch.cutouts.retain(|k| !k.curves.iter().any(gone));
    sketch.edges.retain(|e| !gone(&e.curve));
    sketch
        .holes
        .retain(|h| !deleted.contains(&SketchRef::Point(h.center.clone())));
    Ok(())
}

/// Solve a candidate sketch (with its free-form curves fitted within the
/// project's `board.outline_fit`) and store it with its solve; refuses a
/// shape that does not solve. Curves that are not a board are stored with their
/// [`ShapeFault`] and the last valid shape.
fn commit(
    project: &mut Project,
    mut sketch: BoardSketch,
    evaluator: &dyn BoardShapeEvaluator,
) -> Result<()> {
    sketch.outline_fit = outline_fit(project);
    let evaluation = evaluator.evaluate(&sketch)?;
    if let Some(f) = evaluation.findings.iter().find(|f| refuses(&f.code)) {
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
    let size = sketch_size(&evaluation.solved)?;
    super::copper_fits(project, size)?;
    sketch.solve = evaluation.solve;
    sketch.solved = evaluation.solved;
    sketch.fault = evaluation.fault.map(bounded);
    if let Some(blob) = evaluation.sketch_out {
        sketch.sketch = blob;
    }
    project.board_size = size;
    project.board_shape = BoardShape::Sketch(Box::new(sketch));
    Ok(())
}

/// The effective `board.outline_fit` rule, mm.
fn outline_fit(project: &Project) -> f64 {
    match project
        .rules
        .resolve(crate::rules::RuleKey::BoardOutlineFit)
        .value
    {
        crate::rules::RuleValue::Length(mm) => mm,
        _ => crate::rules::DEFAULT_OUTLINE_FIT_MM,
    }
}

/// Re-solve a sketch board whose stored fit is not the effective
/// `board.outline_fit` rule any more (the engine runs it after a rules
/// change), so the solved outline always follows the rule.
pub fn refit(project: &mut Project, evaluator: Option<&dyn BoardShapeEvaluator>) -> Result<()> {
    let BoardShape::Sketch(sketch) = &project.board_shape else {
        return Ok(());
    };
    if sketch.outline_fit == outline_fit(project) {
        return Ok(());
    }
    let sketch = (**sketch).clone();
    commit(project, sketch, needs(evaluator)?)
}

/// A fault cut to its stored bounds.
fn bounded(mut fault: ShapeFault) -> ShapeFault {
    fault.at.truncate(MAX_FAULT_POINTS);
    if fault.message.len() > MAX_FAULT_TEXT {
        let mut end = MAX_FAULT_TEXT;
        while !fault.message.is_char_boundary(end) {
            end -= 1;
        }
        fault.message.truncate(end);
    }
    fault
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

/// A part's name in a message: its designator.
pub(crate) fn part_name(project: &Project, instance: Id) -> String {
    project
        .instance(instance)
        .map_or_else(|| "The part".to_owned(), |i| i.designator.clone())
}

/// Board-shape findings: a stored sketch that did not solve
/// ([`SHAPE_UNSOLVED`]), curves that are not a board ([`SHAPE_OPEN`],
/// [`SHAPE_INVALID`]).
pub fn checks(project: &Project) -> Vec<Finding> {
    let mut out = Vec::new();
    if let BoardShape::Sketch(sketch) = &project.board_shape {
        if matches!(
            sketch.solve.status,
            SolveStatus::OverConstrained | SolveStatus::Failed
        ) {
            out.push(Finding::new(
                SHAPE_UNSOLVED,
                "The board sketch does not solve; the board keeps its last solved shape".into(),
                vec![ObjectRef::Board],
            ));
        }
        if let Some(fault) = &sketch.fault {
            let code = match fault.kind {
                ShapeFaultKind::Open => SHAPE_OPEN,
                ShapeFaultKind::Invalid => SHAPE_INVALID,
            };
            out.push(Finding::new(
                code,
                fault.message.clone(),
                vec![ObjectRef::Board],
            ));
        }
    }
    out
}

/// Structural bounds: a sketch board's roles bounded and unique, its
/// solved shape finite, fitted at the effective `board.outline_fit` and
/// `board_size` its box.
pub fn validate(project: &Project) -> Result<()> {
    if let BoardShape::Sketch(sketch) = &project.board_shape {
        validate_sketch(sketch)?;
        if sketch.outline_fit != outline_fit(project) {
            return Err(Error::Invalid(
                "board.outline_fit: the board was fitted at another board.outline_fit than the \
                 rules give"
                    .into(),
            ));
        }
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
    let mut loops: BTreeSet<&SketchCurveId> = BTreeSet::new();
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
    if !crate::rules::OUTLINE_FIT_RANGE_MM.contains(&sketch.outline_fit) {
        return bad("the outline fit is out of range");
    }
    if let Some(f) = &sketch.fault
        && (f.message.len() > MAX_FAULT_TEXT
            || f.at.len() > MAX_FAULT_POINTS
            || !f.at.iter().all(|p| point_valid(*p)))
    {
        return bad("the shape fault is out of range");
    }
    let solved = &sketch.solved;
    let outlines = std::iter::once((&solved.outer, &solved.outer_curves))
        .chain(solved.cutouts.iter().map(|c| (&c.outline, &c.curves)));
    for (o, curves) in outlines {
        if !(curves.is_empty() || curves.len() == o.segments.len()) {
            return bad("a solved loop names one curve per segment");
        }
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
mod shape_tests;
