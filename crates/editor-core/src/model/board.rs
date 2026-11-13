//! Board shape data: a plain rectangle (`Project::board_size`) or a
//! parametric SolidCraft sketch with named edges, cutouts, milled slots
//! and mounting holes, and the placement constraints that tie parts (and
//! fills and keepouts) to named edges. Evaluation and constraint
//! resolution live in `layout::outline`; the core never interprets the
//! sketch (as for footprints, `library::footprint_source`).
use crate::geometry::{Bounds, Point};
use crate::ids::Id;
use crate::library::footprint_source::{
    EvaluatorRef, Expr, FootprintParam, ParamName, SketchBlob, SketchCurveId, SketchPointId,
    SolveSummary,
};
use crate::library::{Outline, Segment};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Cutouts, slots, mounting holes and named edges on one board.
pub const MAX_CUTOUTS: usize = 256;
pub const MAX_MOUNTING_HOLES: usize = 256;
pub const MAX_NAMED_EDGES: usize = 1024;

/// The names a rectangular board's edges carry, counter-clockwise from
/// the lower edge (origin at the lower-left corner).
pub const RECT_EDGES: [&str; 4] = ["bottom", "right", "top", "left"];

/// A named board edge (`left`, `usb_side`): identifier syntax, bounded.
#[derive(
    Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct EdgeName(pub String);

impl EdgeName {
    /// Identifier syntax `[A-Za-z_][A-Za-z0-9_]*`, at most
    /// [`crate::ids::MAX_LABEL`] bytes.
    pub fn valid(&self) -> bool {
        let mut chars = self.0.chars();
        self.0.len() <= crate::ids::MAX_LABEL
            && chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
}

/// Board-local cutout, slot or hole identity; never reused.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct BoardFeatureId(pub u32);

/// What an inner closed loop of the board sketch is.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CutoutKind {
    /// Material removed through the board (routed).
    Cutout,
    /// A milled slot, plated or not.
    Slot { plated: bool },
}

/// The kind of an inner loop: every hole of the board sketch is a cutout
/// unless a role keyed by its curves (the whole set, in any order) makes it
/// a slot.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CutoutRole {
    pub id: BoardFeatureId,
    pub kind: CutoutKind,
    pub curves: Vec<SketchCurveId>,
}

/// A sketch curve the user named, so placements can refer to it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NamedEdge {
    pub name: EdgeName,
    pub curve: SketchCurveId,
}

/// A drilled mounting hole at a sketch point.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MountingHoleRole {
    pub id: BoardFeatureId,
    pub center: SketchPointId,
    pub drill: Expr,
    pub plated: bool,
    /// Copper ring diameter for a plated hole (`None`: no copper).
    #[serde(default)]
    pub pad_diameter: Option<Expr>,
}

/// The solved shape: what checks, fills, 2D and 3D read.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SolvedBoard {
    /// The outer outline, counter-clockwise, with arcs. Free-form curves
    /// arrive fitted with lines and arcs within the `board.outline_fit` rule.
    pub outer: Outline,
    /// The sketch curve behind each segment of [`Self::outer`] (empty for a
    /// plain rectangle).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outer_curves: Vec<SketchCurveId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cutouts: Vec<SolvedCutout>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<SolvedMountingHole>,
    /// Every named edge's solved segment.
    pub edges: Vec<SolvedEdge>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SolvedCutout {
    /// The [`CutoutRole`] of the hole: its stable id, kept as its curves
    /// change. `None` only in an evaluator's raw solve, before the board
    /// commit identifies a new loop (`layout::outline::identify_cutouts`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<BoardFeatureId>,
    pub kind: CutoutKind,
    pub outline: Outline,
    /// The sketch curve behind each segment of `outline`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<SketchCurveId>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SolvedMountingHole {
    pub id: BoardFeatureId,
    pub at: Point,
    pub drill: f64,
    pub plated: bool,
    #[serde(default)]
    pub pad_diameter: Option<f64>,
}

/// One named edge solved: from `a` to `b` along the board's
/// counter-clockwise direction (the inside is on the left), straight or
/// an arc in bulge form.
///
/// A filleted corner keeps its virtual (unrounded) corner point, so a
/// straight edge runs corner to corner as drawn before the fillets: its
/// `a` and `b` are the virtual corners, not the ends of the trimmed line
/// in [`SolvedBoard::outer`]. Edge constraints measure `along` (and their
/// `Start`, `Middle`, `End` and fraction anchors) on this segment, so a
/// part keeps its place when a corner radius changes.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SolvedEdge {
    pub name: EdgeName,
    pub a: Point,
    pub b: Point,
    #[serde(default)]
    pub bulge: f64,
}

impl SolvedBoard {
    /// A `size` rectangle with its lower-left corner at the origin and
    /// the edges named [`RECT_EDGES`].
    pub fn rectangle(size: Point) -> Self {
        let [w, h] = size;
        let corners = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
        let edges = RECT_EDGES
            .iter()
            .enumerate()
            .map(|(k, name)| SolvedEdge {
                name: EdgeName((*name).into()),
                a: corners[k],
                b: corners[(k + 1) % 4],
                bulge: 0.0,
            })
            .collect();
        Self {
            outer: Outline {
                start: corners[0],
                segments: (1..=4)
                    .map(|k| Segment::Line { to: corners[k % 4] })
                    .collect(),
            },
            outer_curves: Vec::new(),
            cutouts: Vec::new(),
            holes: Vec::new(),
            edges,
        }
    }

    pub fn edge(&self, name: &EdgeName) -> Option<&SolvedEdge> {
        self.edges.iter().find(|e| &e.name == name)
    }

    /// The exact box of the outer outline, arcs included.
    pub fn bounds(&self) -> Bounds {
        outline_bounds(&self.outer)
    }

    /// Whether `p` lies inside the outer outline and outside every cutout
    /// (outlines flattened within `tolerance` mm; mounting holes ignored).
    pub fn contains(&self, p: Point, tolerance: f64) -> bool {
        crate::geometry::polygon_contains(&flatten(&self.outer, tolerance), p)
            && !self
                .cutouts
                .iter()
                .any(|c| crate::geometry::polygon_contains(&flatten(&c.outline, tolerance), p))
    }
}

/// A bulge arc from `a` to `b` as a circle: its centre, radius, the angle
/// of `a` about the centre (radians) and the signed sweep (radians,
/// positive counter-clockwise). `None` for a straight or degenerate span.
pub fn arc_geometry(a: Point, b: Point, bulge: f64) -> Option<ArcGeometry> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let chord = d[0].hypot(d[1]);
    if !(bulge.is_finite() && bulge != 0.0 && chord.is_finite() && chord > 1e-12) {
        return None;
    }
    let sweep = 4.0 * bulge.atan();
    let half = sweep / 2.0;
    let radius = chord / (2.0 * half.sin().abs());
    // The centre sits on the chord's perpendicular bisector, left of the
    // chord for a counter-clockwise arc of less than half a turn.
    let k = (chord / 2.0) / half.tan();
    let n = [-d[1] / chord, d[0] / chord];
    let center = [
        (a[0] + b[0]) / 2.0 + n[0] * k,
        (a[1] + b[1]) / 2.0 + n[1] * k,
    ];
    let start = (a[1] - center[1]).atan2(a[0] - center[0]);
    (radius.is_finite() && center.iter().all(|v| v.is_finite())).then_some(ArcGeometry {
        center,
        radius,
        start,
        sweep,
    })
}

/// A circle span: see [`arc_geometry`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArcGeometry {
    pub center: Point,
    pub radius: f64,
    /// Angle of the start point about the centre, radians.
    pub start: f64,
    /// Signed sweep, radians (positive counter-clockwise).
    pub sweep: f64,
}

impl ArcGeometry {
    pub fn at(&self, angle: f64) -> Point {
        [
            self.center[0] + self.radius * angle.cos(),
            self.center[1] + self.radius * angle.sin(),
        ]
    }

    pub fn length(&self) -> f64 {
        self.radius * self.sweep.abs()
    }
}

/// The exact box of an outline, arcs included.
pub fn outline_bounds(outline: &Outline) -> Bounds {
    let mut bounds = Bounds::point(outline.start);
    let mut from = outline.start;
    for s in &outline.segments {
        let to = s.to();
        bounds = bounds.including(to);
        if let Segment::Arc { bulge, .. } = s
            && let Some(arc) = arc_geometry(from, to, *bulge)
        {
            // Every axis extreme the sweep passes.
            for k in 0..4 {
                let axis = f64::from(k) * std::f64::consts::FRAC_PI_2;
                let along = if arc.sweep > 0.0 {
                    (axis - arc.start).rem_euclid(std::f64::consts::TAU)
                } else {
                    (arc.start - axis).rem_euclid(std::f64::consts::TAU)
                };
                if along <= arc.sweep.abs() {
                    bounds = bounds.including(arc.at(axis));
                }
            }
        }
        from = to;
    }
    bounds
}

/// The outline as a closed polygon (the start not repeated), arcs as
/// chords whose sagitta is within `tolerance` mm (at most 256 per arc).
pub fn flatten(outline: &Outline, tolerance: f64) -> Vec<Point> {
    let mut out = vec![outline.start];
    let mut from = outline.start;
    for s in &outline.segments {
        let to = s.to();
        if let Segment::Arc { bulge, .. } = s
            && let Some(arc) = arc_geometry(from, to, *bulge)
        {
            let tol = tolerance.max(1e-6).min(arc.radius);
            let step = 2.0 * (1.0 - tol / arc.radius).clamp(-1.0, 1.0).acos();
            let n = if step > 1e-9 {
                (arc.sweep.abs() / step).ceil().clamp(1.0, 256.0)
            } else {
                256.0
            };
            // A whole number in 1–256.
            let n = n as usize;
            for k in 1..n {
                out.push(arc.at(arc.start + arc.sweep * k as f64 / n as f64));
            }
        }
        out.push(to);
        from = to;
    }
    if out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// The parametric board: SolidCraft's sketch, its parameters, the roles
/// of its loops and curves, and the last solve.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoardSketch {
    pub evaluator: EvaluatorRef,
    pub sketch: SketchBlob,
    /// Board parameters (`width`, `height`, `corner_r`); dimensions bind
    /// to them through SolidCraft's own `Constraint.param`.
    pub parameters: Vec<FootprintParam>,
    /// Slot roles of inner loops. The outline itself is no list: it is the
    /// sketch's one closed region (docs/board-sketch-references.md §5.1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cutouts: Vec<CutoutRole>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<MountingHoleRole>,
    pub edges: Vec<NamedEdge>,
    /// Next [`BoardFeatureId`].
    pub next_feature: u32,
    pub solve: SolveSummary,
    /// The shape the last valid solve produced; `Project::board_size` holds
    /// its bounding box.
    pub solved: SolvedBoard,
    /// The arc-fit tolerance `solved` was made with, mm: the effective
    /// `board.outline_fit` rule (`layout::outline::validate` keeps the two
    /// equal; changing the rule re-solves the board).
    #[serde(default = "default_outline_fit")]
    pub outline_fit: f64,
    /// Why the sketch's curves are not a board now (open ends, a crossing,
    /// a touching hole); `solved` is then the last valid shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fault: Option<ShapeFault>,
    /// Board parameters that drive sketch dimensions but move no outline
    /// curve or mounting hole (`board.param_idle`), as the last solve found
    /// them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub idle: Vec<ParamName>,
}

fn default_outline_fit() -> f64 {
    crate::rules::DEFAULT_OUTLINE_FIT_MM
}

/// Points one [`ShapeFault`] names at most.
pub const MAX_FAULT_POINTS: usize = 64;
/// Length of a [`ShapeFault`]'s message at most, bytes.
pub const MAX_FAULT_TEXT: usize = 1024;

/// What is wrong with the board sketch's curves.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShapeFaultKind {
    /// Curve ends touch nothing (`board.shape_open`).
    Open,
    /// The curves close but are not one board (`board.shape_invalid`).
    Invalid,
}

/// Why the board sketch's curves are not a board: reported as a finding,
/// while the board keeps its last valid shape.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ShapeFault {
    pub kind: ShapeFaultKind,
    pub message: String,
    /// Where: the open ends, or the crossing or touching point.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub at: Vec<Point>,
}

/// The board outline. A rectangle is `Project::board_size`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "shape", content = "sketch")]
pub enum BoardShape {
    #[default]
    Rectangle,
    Sketch(Box<BoardSketch>),
}

impl BoardShape {
    pub fn is_rectangle(&self) -> bool {
        matches!(self, Self::Rectangle)
    }
}

/// What a part dimension moves (`model::dimension`).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum ConstraintTarget {
    /// A component placement, by instance.
    Placement(Id),
    Fill(Id),
    Keepout(Id),
}
