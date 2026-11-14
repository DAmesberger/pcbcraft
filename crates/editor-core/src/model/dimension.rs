//! Part dimensions: driving (or reference) dimensions and geometric
//! constraints between board geometry and the features of placed parts
//! (docs/board-sketch-references.md §5.2–§5.5). Data only; the parts solve
//! that turns them into placements lives in `layout::dimensions`, and the
//! sketch solver behind it in the host's board evaluator.
//!
//! Part dimensions replaced the edge constraints of the first board sketch
//! (docs/board-sketch-references.md §5.6).
use crate::geometry::Point;
use crate::ids::{Id, PackageRef, PadId};
use crate::library::footprint_source::{DimKind, DimValue, GeomConstraint, SketchRef};
use crate::model::board::ConstraintTarget;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// References one part dimension names (1–3).
pub const MAX_DIMENSION_REFS: usize = 3;

/// A dimension or constraint between board, assembly and part geometry.
/// Only parts it names are solved; every other part keeps its stored pose.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartDimension {
    pub id: Id,
    pub kind: PartRelation,
    /// 1–[`MAX_DIMENSION_REFS`] references, in the order `kind` reads them.
    pub refs: Vec<BoardRef>,
    /// The driving value; `None` for a geometric constraint (and for a
    /// reference dimension, which only measures).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<DimValue>,
    /// Measures only; never drives.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reference: bool,
}

/// A part dimension to add: everything but its identity.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartDimensionSpec {
    pub kind: PartRelation,
    pub refs: Vec<BoardRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<DimValue>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reference: bool,
}

impl PartDimension {
    pub fn from_spec(id: Id, spec: PartDimensionSpec) -> Self {
        Self {
            id,
            kind: spec.kind,
            refs: spec.refs,
            value: spec.value,
            reference: spec.reference,
        }
    }
}

/// What a part dimension holds: a driving dimension, a dimensionless
/// geometric constraint, or a point at a fraction along a curve.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PartRelation {
    Dim(DimKind),
    Geom(GeomConstraint),
    /// The point lies on the curve at `at` (0–1) of its length from its
    /// start; Solidcraft's `PointAlong`.
    PointAlong {
        at: f64,
    },
}

impl PartRelation {
    /// Whether the relation sets a direction (an angle, parallel,
    /// perpendicular, horizontal, vertical or tangent), so it may turn the
    /// parts it names. Every other relation places a point and never turns
    /// a part: a part's rotation no direction holds keeps its stored value,
    /// so two distances that only a turn would satisfy are a conflict.
    pub fn orients(self) -> bool {
        matches!(
            self,
            Self::Dim(DimKind::Angle)
                | Self::Geom(
                    GeomConstraint::Horizontal
                        | GeomConstraint::Vertical
                        | GeomConstraint::Parallel
                        | GeomConstraint::Perpendicular
                        | GeomConstraint::Tangent
                )
        )
    }
}

/// One reference of a part dimension.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BoardRef {
    /// A board-sketch point or curve, by its stable sketch id.
    Board(SketchRef),
    /// A feature of a placed part (or of a fill or keepout's origin).
    Part(FeatureRef),
    /// A followed face (§7.2; slice S7).
    FollowedFace(FollowedFaceId),
}

/// A feature of a placed part, rebuilt from the library
/// snapshot (or the part's model design) for every solve and never stored.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "feature", deny_unknown_fields)]
pub enum FeatureRef {
    /// The placement origin (footprint datum), or a fill's or keepout's
    /// outline start.
    Origin { target: ConstraintTarget },
    /// An axis of the target's frame through its origin.
    Axis {
        target: ConstraintTarget,
        axis: PartAxis,
    },
    Pad {
        placement: Id,
        pad: PadId,
        at: PadPoint,
    },
    /// A side of the courtyard's box in the part frame; `None`: its centre.
    Courtyard {
        placement: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        side: Option<BoxSide>,
    },
    /// An anchor authored by name in the part's model design.
    Anchor { placement: Id, name: AnchorName },
    /// A face of the part's part file, by body, persistent face name and a
    /// pick point on it (model frame, mm), traced in the board plane by the
    /// host ([`ModelTrace`]). A name the part does not have is refused with
    /// its faces square to the board, each by name with a point on it.
    Face { placement: Id, face: ModelFaceRef },
    /// An edge of the part's 3D model.
    Edge { placement: Id, edge: ModelEdgeRef },
    /// An end of an edge of the part's 3D model.
    Vertex {
        placement: Id,
        edge: ModelEdgeRef,
        end: End,
    },
}

impl FeatureRef {
    /// The target whose pose the feature moves with.
    pub fn target(&self) -> ConstraintTarget {
        match self {
            Self::Origin { target } | Self::Axis { target, .. } => *target,
            Self::Pad { placement, .. }
            | Self::Courtyard { placement, .. }
            | Self::Anchor { placement, .. }
            | Self::Face { placement, .. }
            | Self::Edge { placement, .. }
            | Self::Vertex { placement, .. } => ConstraintTarget::Placement(*placement),
        }
    }

    /// Whether the feature is a face, edge or vertex of the part's 3D
    /// model: the host traces it ([`ModelTrace`]).
    pub fn is_model(&self) -> bool {
        matches!(
            self,
            Self::Face { .. } | Self::Edge { .. } | Self::Vertex { .. }
        )
    }
}

/// An axis of a part frame.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PartAxis {
    X,
    Y,
}

/// A side of an axis-aligned box in the part frame (before the placement
/// turns or mirrors it).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoxSide {
    Left,
    Right,
    Bottom,
    Top,
}

/// Where on a pad a reference measures.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PadPoint {
    Centre,
    /// A side of the pad's box in the part frame.
    Side(BoxSide),
}

/// An end of an edge.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum End {
    Start,
    End,
}

/// A followed face's identity in the project (slice S7).
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct FollowedFaceId(pub u32);

macro_rules! model_name {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd,
            Ord, Hash,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Not blank, at most [`crate::ids::MAX_LABEL`] bytes, no control
            /// characters.
            pub fn valid(&self) -> bool {
                crate::ids::printable(&self.0)
            }
        }
    };
}

model_name!(
    /// An anchor authored by name in a part's model design.
    AnchorName
);
model_name!(
    /// A body of a model design, by its name there.
    BodyName
);
model_name!(
    /// A face's persistent name in a model design (`F2:side:l4`).
    FaceName
);
model_name!(
    /// An edge's persistent name in a model design.
    ModelEdgeName
);

/// A face of the package's own 3D model, in the model's frame (§6.2):
/// re-found by name, checked against `pick` (a point on it, mm).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelFaceRef {
    pub body: BodyName,
    pub face: FaceName,
    pub pick: [f64; 3],
}

/// An edge of the package's own 3D model, in the model's frame.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelEdgeRef {
    pub body: BodyName,
    pub edge: ModelEdgeName,
    pub pick: [f64; 3],
}

/// How a reference resolves against its source (§5.5). Derived: a 3D
/// model reference's status is the host's, kept in its [`ModelTrace`];
/// every other one is read from the document. `Lost` and `Inexact`
/// references are reported (`board.reference_lost`,
/// `board.reference_inexact`), stop driving and keep their last geometry.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceStatus {
    Resolved,
    /// Found, but not provably the same geometry (a renamed or unverified
    /// face); acknowledged in an impact preview, it becomes `Resolved`.
    Inexact,
    /// Its source is gone.
    Lost,
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

impl FeatureGeometry {
    /// Every coordinate finite and in range, a radius positive.
    pub fn valid(&self) -> bool {
        use crate::geometry::point_valid;
        match *self {
            Self::Point { at } => point_valid(at),
            Self::Line { a, b } => point_valid(a) && point_valid(b),
            Self::Arc { a, b, bulge } => point_valid(a) && point_valid(b) && bulge.is_finite(),
            Self::Circle { center, radius } => {
                point_valid(center) && radius.is_finite() && radius > 0.0
            }
        }
    }
}

/// Bytes of a trace's note.
pub const MAX_TRACE_NOTE: usize = 512;

/// What the host found for one face, edge or vertex of a part's 3D model
/// (docs/board-sketch-references.md §5.4, §5.5): its trace in the board
/// plane and how it resolves. Written by the host's trace
/// (`layout::dimensions::model::refresh`) whenever a dimension or the
/// package it was traced in changes, and kept with the document like the
/// solved board, so checks, fabrication preflight and the canvas read it
/// without Solidcraft. A lost or inexact reference keeps the geometry it
/// had last.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelTrace {
    /// The reference: a `face`, `edge` or `vertex` feature.
    pub feature: FeatureRef,
    /// The package revision it was traced in; `None` once the part is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageRef>,
    /// The model design it was traced on (`layout::dimensions::model::
    /// design_key`): a new package revision with the same design keeps
    /// its names exact, so only another design can make a name inexact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design: Option<crate::ids::ContentHash>,
    pub status: ReferenceStatus,
    /// Its trace in the part frame; for a lost reference, the last one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<FeatureGeometry>,
    /// Why it is inexact or lost.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}
