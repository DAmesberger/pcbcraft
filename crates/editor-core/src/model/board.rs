//! Board shape data: a plain rectangle (`Project::board_size`) or a
//! parametric SolidCraft sketch with named edges, cutouts, milled slots
//! and mounting holes, and the placement constraints that tie parts (and
//! fills and keepouts) to named edges. Evaluation and constraint
//! resolution live in `layout::outline`; the core never interprets the
//! sketch (as for footprints, `library::footprint_source`).
use crate::geometry::Point;
use crate::ids::Id;
use crate::library::footprint_source::{
    EvaluatorRef, Expr, FootprintParam, SketchBlob, SketchCurveId, SketchPointId, SolveSummary,
};
use crate::library::{Outline, Segment};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Cutouts, slots, mounting holes and named edges on one board.
pub const MAX_CUTOUTS: usize = 256;
pub const MAX_MOUNTING_HOLES: usize = 256;
pub const MAX_NAMED_EDGES: usize = 1024;
/// Placement constraints on one board.
pub const MAX_EDGE_CONSTRAINTS: usize = 4096;

/// The names a rectangular board's edges carry, counter-clockwise from
/// the lower edge (origin at the lower-left corner).
pub const RECT_EDGES: [&str; 4] = ["bottom", "right", "top", "left"];

/// A named board edge (`left`, `usb_side`): identifier syntax, bounded.
#[derive(
    Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct EdgeName(pub String);

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

/// An inner loop of sketch curves and what it cuts.
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
    /// The outer outline, counter-clockwise, with arcs.
    pub outer: Outline,
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
    pub id: BoardFeatureId,
    pub kind: CutoutKind,
    pub outline: Outline,
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
            cutouts: Vec::new(),
            holes: Vec::new(),
            edges,
        }
    }

    pub fn edge(&self, name: &EdgeName) -> Option<&SolvedEdge> {
        self.edges.iter().find(|e| &e.name == name)
    }
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
    /// The closed loop of the outer outline.
    pub outline: Vec<SketchCurveId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cutouts: Vec<CutoutRole>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<MountingHoleRole>,
    pub edges: Vec<NamedEdge>,
    /// Next [`BoardFeatureId`].
    pub next_feature: u32,
    pub solve: SolveSummary,
    /// The shape the last solve produced; `Project::board_size` holds its
    /// bounding box.
    pub solved: SolvedBoard,
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

/// Where along a named edge a constraint measures from.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "at", content = "fraction")]
pub enum EdgeAnchor {
    /// The edge's first point (`a`).
    #[default]
    Start,
    Middle,
    End,
    /// A fraction 0–1 of the edge's length from `a`.
    Fraction(f64),
}

/// What a placement constraint moves.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum ConstraintTarget {
    /// A component placement, by instance.
    Placement(Id),
    Fill(Id),
    Keepout(Id),
}

/// Which point of the target lands on the constrained position.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintAlign {
    /// The placement origin (footprint datum).
    #[default]
    Origin,
    /// The courtyard's (or outline's) side nearest the edge.
    NearSide,
    /// The courtyard's (or outline's) centre.
    Centre,
}

/// A constraint to add: everything but its identity.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EdgeConstraintSpec {
    pub target: ConstraintTarget,
    pub edge: EdgeName,
    #[serde(default)]
    pub anchor: EdgeAnchor,
    /// Distance from the anchor along the edge (`a` to `b`), an
    /// expression over board parameters (`width / 2 - 3 mm`).
    pub along: Expr,
    /// Distance from the edge along its inward normal.
    pub offset: Expr,
    #[serde(default)]
    pub align: ConstraintAlign,
    /// The target turns with the edge's direction (plus `rotation_deg`);
    /// otherwise its own rotation is kept.
    #[serde(default)]
    pub follow_edge: bool,
    #[serde(default)]
    pub rotation_deg: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EdgeConstraint {
    pub id: Id,
    pub target: ConstraintTarget,
    pub edge: EdgeName,
    #[serde(default)]
    pub anchor: EdgeAnchor,
    pub along: Expr,
    pub offset: Expr,
    #[serde(default)]
    pub align: ConstraintAlign,
    #[serde(default)]
    pub follow_edge: bool,
    #[serde(default)]
    pub rotation_deg: f64,
}

impl EdgeConstraint {
    pub fn from_spec(id: Id, s: EdgeConstraintSpec) -> Self {
        Self {
            id,
            target: s.target,
            edge: s.edge,
            anchor: s.anchor,
            along: s.along,
            offset: s.offset,
            align: s.align,
            follow_edge: s.follow_edge,
            rotation_deg: s.rotation_deg,
        }
    }
}
