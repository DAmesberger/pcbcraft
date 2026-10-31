//! The parametric source of a footprint and the evaluator seam.
//!
//! The core stores the source as plain data and never interprets the
//! sketch: [`SketchBlob`] is SolidCraft's canonical sketch JSON, opaque here.
//! A [`FootprintEvaluator`] (the integration crate's `SketchEvaluator`; a
//! fixed fake in core tests) applies [`SketchEdit`]s and solves the source
//! into the typed [`Pad`]s, graphics and [`Datum`] that every consumer
//! reads. Identity is never geometric: a role pad is named by its sketch
//! anchor point, an array pad by its [`GenerationKey`]; both map to a
//! stable [`PadId`].
use super::footprint::{Datum, FootprintDefinition, Pad, PadDefaults, PadKind, PadSide};
use super::graphics::MAX_GRAPHICS;
use super::graphics::{DrawingLayer, Fill, FootprintGraphic, Side};
use super::package::PackageDefinition;
use super::{MAX_LABEL, MAX_PADS};
use crate::diagnostics::{Error, Finding, Result};
use crate::geometry::Point;
use crate::ids::{GraphicId, PackageTerminalId, PadId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Canonical sketch JSON, in bytes.
pub const MAX_SKETCH_BYTES: usize = 1 << 20;
/// Points in one footprint sketch (checked by the evaluator).
pub const MAX_SKETCH_POINTS: usize = 4096;
/// Constraints in one footprint sketch (checked by the evaluator).
pub const MAX_SKETCH_CONSTRAINTS: usize = 8192;
/// Footprint parameters.
pub const MAX_PARAMETERS: usize = 256;
/// Pad arrays in one source.
pub const MAX_ARRAYS: usize = 32;
/// Bytes in one parameter expression.
pub const MAX_EXPR: usize = 256;
/// Edits in one `EditFootprint`.
pub const MAX_FOOTPRINT_EDITS: usize = 256;
/// Reserved prefix of injected package values.
pub const PACKAGE_PREFIX: &str = "pkg_";

/// How a footprint's solved data was derived. Stored with the result it
/// produced; published revisions are never re-solved.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FootprintSource {
    /// Which evaluator (and exact engine revision) wrote `sketch`.
    pub evaluator: EvaluatorRef,
    /// The SolidCraft sketch, including its stored solved positions.
    /// Dimensions bind to parameters through SolidCraft's own
    /// `Constraint.param`, so there is no binding table here.
    pub sketch: SketchBlob,
    pub parameters: Vec<FootprintParam>,
    /// One role per anchored pad (array instances excluded).
    pub pads: Vec<PadRole>,
    pub arrays: Vec<PadArray>,
    /// Shape graphics derived from the sketch or explicitly derived roles.
    pub graphics: Vec<GraphicRole>,
    /// Stable pad identity of every generated array instance.
    pub keys: Vec<KeyedPad>,
    /// Explicit per-instance changes, re-applied after each expansion.
    pub overrides: Vec<KeyedOverride>,
    /// The last evaluation (status, dof, quantized result hash) of this
    /// source, stored with the solved data it produced.
    pub solve: SolveSummary,
}

/// The evaluator that owns a source: an engine name and its exact revision.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EvaluatorRef {
    /// `"solidcraft-sketch"` for the integration crate's evaluator.
    pub engine: String,
    /// The engine revision compiled into the evaluator (a git hash).
    pub rev: String,
}

/// SolidCraft's canonical sketch JSON (`serde_json::to_string(&sketch)`
/// output only; anything else is refused by the evaluator's round trip).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct SketchBlob(pub String);

/// A SolidCraft sketch point id (for example `p3`); never reused.
#[derive(
    Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(transparent)]
pub struct SketchPointId(pub String);

/// A SolidCraft sketch curve id (for example `l2`).
#[derive(
    Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(transparent)]
pub struct SketchCurveId(pub String);

/// A SolidCraft sketch constraint id (for example `k10`).
#[derive(
    Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(transparent)]
pub struct SketchConstraintId(pub String);

/// Any sketch entity, by id.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SketchRef {
    Point(SketchPointId),
    Curve(SketchCurveId),
    Constraint(SketchConstraintId),
}

/// A footprint parameter or package-value name: an identifier
/// `[A-Za-z_][A-Za-z0-9_]*`. Names starting `pkg_` are reserved for
/// injected package values.
#[derive(
    Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(transparent)]
pub struct ParamName(pub String);

impl ParamName {
    /// Identifier syntax and length (the evaluator also refuses
    /// SolidCraft's reserved words).
    pub fn valid(&self) -> bool {
        let mut chars = self.0.chars();
        self.0.len() <= MAX_LABEL
            && chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    /// Whether the name is in the reserved package namespace.
    pub fn is_package(&self) -> bool {
        self.0.starts_with(PACKAGE_PREFIX)
    }
}

/// An expression in SolidCraft's grammar (`pkg_e * 2 + 0.1 mm`), at most
/// [`MAX_EXPR`] bytes. Interpreted only by the evaluator.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct Expr(pub String);

impl Expr {
    fn within_bounds(&self) -> bool {
        self.0.len() <= MAX_EXPR
    }
}

/// What a parameter's value measures.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParamUnit {
    /// A length in millimetres.
    #[default]
    Mm,
    /// An angle in degrees.
    Deg,
    /// A whole, unit-less count.
    Count,
}

/// One named footprint parameter (land sizes, row spans, counts).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FootprintParam {
    pub name: ParamName,
    pub expr: Expr,
    pub unit: ParamUnit,
    pub comment: String,
}

/// A typed value: what package values inject and parameters evaluate to.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ParamValue {
    /// Millimetres.
    Length(f64),
    /// Degrees.
    Angle(f64),
    Count(u32),
}

/// A package value injected into footprint expressions as `pkg_<name>`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackageValue {
    /// The full injected name, including the `pkg_` prefix.
    pub name: ParamName,
    pub value: ParamValue,
}

/// The typed `pkg_*` values a package offers footprints: today the body
/// envelope as `pkg_L`, `pkg_W`, `pkg_H` (lengths) and the terminal count
/// as `pkg_n`. Package parameters extend this list.
pub fn package_values(package: &PackageDefinition) -> Vec<PackageValue> {
    let [l, w, h] = package.body_size;
    let length = |name: &str, v: f64| PackageValue {
        name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
        value: ParamValue::Length(v),
    };
    vec![
        length("L", l),
        length("W", w),
        length("H", h),
        PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}n")),
            value: ParamValue::Count(u32::try_from(package.terminals.len()).unwrap_or(u32::MAX)),
        },
    ]
}

/// The sketch role of one anchored pad.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadRole {
    pub pad: PadId,
    /// The pad-centre point: the identity carrier.
    pub anchor: SketchPointId,
    pub geometry: PadGeometry,
    /// A construction line whose angle is the pad rotation; `None` is 0°.
    pub orientation: Option<SketchCurveId>,
    pub kind: PadKind,
    pub side: PadSide,
    pub terminal: Option<PackageTerminalId>,
    pub number: String,
    pub hole: Option<HoleRole>,
    pub mask: MaskRuleExpr,
    pub paste: PasteRuleExpr,
    pub vias: Option<ViaGridRole>,
}

/// Where a role pad's copper outline comes from.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PadGeometry {
    /// A typed template whose curves the role owns; edits replacing an
    /// owned curve are refused (`footprint.owned_curve`).
    Template {
        template: PadTemplate,
        owned_curves: Vec<SketchCurveId>,
    },
    /// The copper profile around the anchor, classified into a primitive
    /// or an exact custom outline.
    Profile,
}

/// The typed pad templates `AddPad` builds.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PadTemplate {
    #[default]
    Rect,
    RoundRect,
    Circle,
    Oblong,
}

/// Expressions sizing a new template pad, plus its electrical role.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadTemplateParams {
    /// Width (the diameter for circles).
    pub width: Expr,
    pub height: Expr,
    /// Rounded rectangles only, 0–0.5 of the short side.
    pub radius_ratio: Option<Expr>,
    pub kind: PadKind,
    pub side: PadSide,
    pub terminal: Option<PackageTerminalId>,
    pub number: String,
}

/// A role-owned drill: a construction circle or slot, never in the copper
/// pass.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HoleRole {
    pub curves: Vec<SketchCurveId>,
    pub plated: bool,
}

/// [`MaskRule`](super::footprint::MaskRule) with an expression.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MaskRuleExpr {
    #[default]
    Inherit,
    Tented,
    Expansion(Expr),
}

/// [`PasteRule`](super::footprint::PasteRule) with an expression; windows
/// are role-owned closed curves, each solved in its own profile pass.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PasteRuleExpr {
    #[default]
    Inherit,
    None,
    Reduction(Expr),
    Windows(Vec<SketchCurveId>),
}

/// A grid of pad-owned vias centred on the pad.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ViaGridRole {
    pub rows: Expr,
    pub cols: Expr,
    pub pitch: Expr,
    pub drill: Expr,
}

/// A pad array, unique in the footprint's local namespace.
#[derive(
    Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(transparent)]
pub struct ArrayId(pub u32);

/// Rigid copies of a solved seed pad, made outside the solver.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadArray {
    pub id: ArrayId,
    /// The role pad copied; it is instance (0, 0).
    pub seed: PadId,
    pub layout: ArrayLayout,
    pub numbering: NumberingScheme,
    pub skip: SkipRule,
    pub first_number: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ArrayLayout {
    Linear {
        count: Expr,
        pitch: Expr,
        direction: SketchCurveId,
    },
    /// Two facing rows (SOIC-like); row 1 runs back, turned 180°.
    DualRow {
        per_row: Expr,
        pitch: Expr,
        row_span: Expr,
        direction: SketchCurveId,
    },
    /// Four sides (QFN/QFP-like), placed per side so `nx ≠ ny` works.
    Quad {
        nx: Expr,
        ny: Expr,
        pitch: Expr,
        span_x: Expr,
        span_y: Expr,
        pin1: Pin1Position,
    },
    Grid {
        rows: Expr,
        cols: Expr,
        pitch_x: Expr,
        pitch_y: Expr,
        stagger: bool,
    },
}

/// Which side of a quad array pad 1 starts, counting counter-clockwise
/// from that side's first corner.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Pin1Position {
    #[default]
    Left,
    Bottom,
    Right,
    Top,
}

/// How array instances are numbered (display only; never identity).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum NumberingScheme {
    #[default]
    Sequential,
    CounterClockwise,
    Zigzag,
    PerRow,
    /// Row letters and column numbers ("A1"), skipping `skip_letters`
    /// (default "IOQSXZ", the JESD95-1 concept).
    GridAlpha {
        skip_letters: String,
    },
}

/// Instances an array leaves out.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SkipRule {
    /// Explicit `[i, j]` instances.
    pub explicit: Vec<[u16; 2]>,
    /// A centred void of `[rows, cols]`; it re-centres when the array grows.
    pub centre_void: Option<[u16; 2]>,
    /// Keep only this many perimeter rings.
    pub perimeter_depth: Option<u16>,
}

/// The identity of one generated array instance.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GenerationKey {
    Array { array: ArrayId, i: u16, j: u16 },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeyedPad {
    pub key: GenerationKey,
    pub pad: PadId,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeyedOverride {
    pub key: GenerationKey,
    pub value: PadOverride,
}

/// Explicit per-instance changes (for example a square pin 1 on a header).
/// An override whose key vanished blocks the apply until resolved.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadOverride {
    pub shape: Option<PadShapeExpr>,
    pub kind: Option<PadKind>,
    pub mask: Option<MaskRuleExpr>,
    pub paste: Option<PasteRuleExpr>,
}

/// A primitive pad shape with expression sizes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PadShapeExpr {
    Rect {
        width: Expr,
        height: Expr,
    },
    RoundRect {
        width: Expr,
        height: Expr,
        radius_ratio: Expr,
    },
    Circle {
        diameter: Expr,
    },
    Oblong {
        width: Expr,
        height: Expr,
    },
}

/// A derived shape graphic with a stable [`GraphicId`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GraphicRole {
    pub id: GraphicId,
    pub layer: DrawingLayer,
    pub side: Side,
    pub source: GraphicSource,
    pub width: Expr,
    pub fill: Fill,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphicSource {
    /// Role-owned sketch curves (silk or assembly drawn in the sketch).
    Curves(Vec<SketchCurveId>),
    /// Computed after the pad pass; exists only when explicitly added.
    Derived(DerivedGraphic),
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum DerivedGraphic {
    /// The pads' and envelope's box grown by `excess`, rounded up to
    /// `round` mm.
    CourtyardFromExtents { excess: Expr, round: Expr },
    /// The package envelope outline.
    AssemblyFromEnvelope,
}

/// The last evaluation of a source, stored with its result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SolveSummary {
    pub evaluator: EvaluatorRef,
    pub status: SolveStatus,
    /// Remaining degrees of freedom (shown, not blocking).
    pub dof: u32,
    /// Hex hash of the quantized (1e-6 mm) result; publish re-evaluates
    /// and refuses `footprint.stale` when it differs.
    pub result_hash: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SolveStatus {
    #[default]
    Solved,
    UnderConstrained,
    OverConstrained,
    Failed,
}

/// One change to a footprint draft. `EditFootprint` applies 1–256 of them
/// in order on a clone, evaluates once, and commits source and result
/// together, or nothing.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum FootprintEdit {
    Sketch(SketchEdit),
    SetParameter {
        name: ParamName,
        expr: Expr,
        unit: ParamUnit,
    },
    RenameParameter {
        from: ParamName,
        to: ParamName,
    },
    /// Refused while referenced.
    RemoveParameter {
        name: ParamName,
    },
    AddPad {
        template: PadTemplate,
        at: Point,
        params: PadTemplateParams,
    },
    /// Replace a role (named by `role.pad`) without touching its curves.
    SetPadRole {
        role: PadRole,
    },
    RemovePad {
        pad: PadId,
    },
    /// Turn a template pad into a profile pad; its curves become free.
    ConvertToCustom {
        pad: PadId,
    },
    AddArray {
        seed: PadId,
        layout: ArrayLayout,
        numbering: NumberingScheme,
        skip: SkipRule,
        first_number: String,
    },
    EditArray {
        array: PadArray,
    },
    RemoveArray {
        id: ArrayId,
    },
    SetOverride {
        key: GenerationKey,
        value: PadOverride,
    },
    ClearOverride {
        key: GenerationKey,
    },
    SetGraphicRole {
        role: GraphicRole,
    },
    RemoveGraphicRole {
        id: GraphicId,
    },
    /// The datum at a sketch point, oriented along an optional line.
    SetDatum {
        point: SketchPointId,
        orientation: Option<SketchCurveId>,
    },
    SetDefaults(PadDefaults),
    /// Drop the source; keep the solved data as an explicit footprint.
    Detach,
}

/// One sketch change. New entity ids are chosen by the sketch and returned
/// in [`SketchOutcome`], never invented by the caller. Trim, fillet,
/// offset, split and mirror arrive in F6 through the scratch-session bridge.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SketchEdit {
    AddPoint {
        at: Point,
    },
    AddLine {
        a: Point,
        b: Point,
        construction: bool,
    },
    /// Counter-clockwise from `start` to `end` about `center`.
    AddArc {
        center: Point,
        start: Point,
        end: Point,
        construction: bool,
    },
    AddCircle {
        center: Point,
        radius: f64,
        construction: bool,
    },
    AddRect {
        a: Point,
        b: Point,
        construction: bool,
    },
    AddPolyline {
        points: Vec<Point>,
        closed: bool,
        construction: bool,
    },
    AddConstraint {
        kind: GeomConstraint,
        refs: Vec<SketchRef>,
    },
    AddDimension {
        kind: DimKind,
        refs: Vec<SketchRef>,
        value: DimValue,
    },
    SetDimension {
        constraint: SketchConstraintId,
        value: DimValue,
    },
    Drag {
        point: SketchPointId,
        to: Point,
    },
    Move {
        ids: Vec<SketchRef>,
        delta: Point,
    },
    /// Deleting a pad anchor retires its pad.
    Delete {
        ids: Vec<SketchRef>,
    },
}

/// Geometric (dimensionless) constraints.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GeomConstraint {
    Coincident,
    Horizontal,
    Vertical,
    Parallel,
    Perpendicular,
    Equal,
    Tangent,
    Midpoint,
    PointOnCurve,
    Concentric,
    Symmetric,
    Fixed,
}

/// Driving dimensions.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DimKind {
    Distance,
    HorizontalDistance,
    VerticalDistance,
    Radius,
    Diameter,
    Angle,
}

/// A dimension's value: a fixed number (mm or degrees) or a parameter bound
/// through SolidCraft's `Constraint.param`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DimValue {
    Fixed(f64),
    Param(ParamName),
}

/// What a sketch edit produced.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SketchOutcome {
    pub sketch: SketchBlob,
    pub created: Vec<SketchRef>,
    pub deleted: Vec<SketchRef>,
}

/// What an evaluator is asked to solve.
#[derive(Clone, Copy, Debug)]
pub struct EvaluationRequest<'a> {
    pub source: &'a FootprintSource,
    pub package: &'a PackageDefinition,
    /// Typed `pkg_*` values, from [`package_values`].
    pub package_values: &'a [PackageValue],
    /// The draft before this edit (stored solved state, ids, defaults).
    pub previous: &'a FootprintDefinition,
}

/// A solved pad: `key` is `Some` for array instances. When the key is not
/// in `source.keys`, `pad.id` is a placeholder the engine replaces.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EvaluatedPad {
    pub key: Option<GenerationKey>,
    pub pad: Pad,
}

/// The result of evaluating a source, quantized to 1e-6 mm.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Evaluation {
    pub pads: Vec<EvaluatedPad>,
    /// Derived shape graphics, by role id.
    pub graphics: Vec<FootprintGraphic>,
    pub datum: Datum,
    pub summary: SolveSummary,
    /// The sketch with updated stored solved positions.
    pub sketch_out: SketchBlob,
    /// Array keys with no `PadId` yet; the engine allocates them.
    pub new_keys: Vec<GenerationKey>,
    /// Blocking (`blocks: Some(..)`) findings refuse the apply.
    pub findings: Vec<Finding>,
}

/// Interprets a [`FootprintSource`]. Pure with respect to the document:
/// the engine clones the draft, calls the evaluator and commits or drops
/// the clone. Undo and redo never call it.
pub trait FootprintEvaluator {
    /// The engine and exact revision this evaluator writes.
    fn evaluator(&self) -> EvaluatorRef;
    /// Apply one sketch edit to a canonical sketch.
    fn apply_sketch(&self, sketch: &SketchBlob, edit: &SketchEdit) -> Result<SketchOutcome>;
    /// Solve a source into typed pads, graphics and datum. Design problems
    /// are findings; `Err` is reserved for refusing the input outright.
    fn evaluate(&self, request: EvaluationRequest<'_>) -> Result<Evaluation>;
}

impl FootprintSource {
    pub(super) fn within_bounds(&self) -> bool {
        self.sketch.0.len() <= MAX_SKETCH_BYTES
            && self.evaluator.engine.len() <= MAX_LABEL
            && self.evaluator.rev.len() <= MAX_LABEL
            && self.parameters.len() <= MAX_PARAMETERS
            && self
                .parameters
                .iter()
                .all(|p| p.expr.within_bounds() && p.name.0.len() <= MAX_LABEL)
            && self.pads.len() <= MAX_PADS
            && self.arrays.len() <= MAX_ARRAYS
            && self.graphics.len() <= MAX_GRAPHICS
            && self.keys.len() <= MAX_PADS
            && self.overrides.len() <= MAX_PADS
    }
}

/// Store an evaluation into a footprint: pads (allocating `PadId`s for
/// new keys from `next_local`), derived graphics replaced by role id
/// (authored graphics kept), datum, summary and the solved sketch. Refuses
/// when the evaluation has a blocking finding; `definition` is unchanged
/// on error. Returns the newly allocated pad ids in key order.
pub fn commit(definition: &mut FootprintDefinition, evaluation: Evaluation) -> Result<Vec<PadId>> {
    if let Some(f) = evaluation.findings.iter().find(|f| f.blocks.is_some()) {
        return Err(Error::Conflict(f.message.clone()));
    }
    let mut next = definition.clone();
    let source = next
        .source
        .as_mut()
        .ok_or_else(|| Error::Invalid("the footprint has no source to evaluate".into()))?;
    let mut keys: BTreeMap<GenerationKey, PadId> =
        source.keys.iter().map(|k| (k.key, k.pad)).collect();
    let mut allocated = Vec::new();
    for key in &evaluation.new_keys {
        if keys.contains_key(key) {
            continue;
        }
        let id = PadId(next.next_local);
        next.next_local = next
            .next_local
            .checked_add(1)
            .ok_or_else(|| Error::Limit("footprint local identities exhausted".into()))?;
        keys.insert(*key, id);
        allocated.push(id);
    }
    let mut pads = Vec::with_capacity(evaluation.pads.len());
    for EvaluatedPad { key, mut pad } in evaluation.pads {
        if let Some(key) = key {
            pad.id = *keys
                .get(&key)
                .ok_or_else(|| Error::Invalid("an evaluated pad has an unknown key".into()))?;
        }
        pads.push(pad);
    }
    if pads.len() > MAX_PADS {
        return Err(Error::Limit("a footprint has at most 1024 pads".into()));
    }
    source.keys = keys
        .into_iter()
        .map(|(key, pad)| KeyedPad { key, pad })
        .collect();
    source.sketch = evaluation.sketch_out;
    source.solve = evaluation.summary;
    let derived: std::collections::BTreeSet<GraphicId> =
        source.graphics.iter().map(|r| r.id).collect();
    next.graphics.retain(|g| !derived.contains(&g.id));
    next.graphics.extend(evaluation.graphics);
    next.pads = pads;
    next.datum = evaluation.datum;

    *definition = next;
    Ok(allocated)
}

/// A fixed evaluator for core tests: sketch edits leave the sketch as is
/// and record nothing; evaluation returns `evaluation` verbatim.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(crate) struct FakeEvaluator {
    pub evaluation: Evaluation,
}

#[cfg(test)]
impl FootprintEvaluator for FakeEvaluator {
    fn evaluator(&self) -> EvaluatorRef {
        EvaluatorRef {
            engine: "fake".into(),
            rev: "0".into(),
        }
    }

    fn apply_sketch(&self, sketch: &SketchBlob, _edit: &SketchEdit) -> Result<SketchOutcome> {
        Ok(SketchOutcome {
            sketch: sketch.clone(),
            ..SketchOutcome::default()
        })
    }

    fn evaluate(&self, _request: EvaluationRequest<'_>) -> Result<Evaluation> {
        Ok(self.evaluation.clone())
    }
}

#[cfg(test)]
mod tests;
