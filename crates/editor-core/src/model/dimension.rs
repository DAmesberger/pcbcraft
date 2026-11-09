//! Part dimensions: driving (or reference) dimensions and geometric
//! constraints between board geometry and the features of placed parts
//! (docs/board-sketch-references.md §5.2–§5.5). Data only; the parts solve
//! that turns them into placements lives in `layout::dimensions`, and the
//! sketch solver behind it in the host's board evaluator.
//!
//! Part dimensions replaced the edge constraints of the first board sketch
//! (docs/board-sketch-references.md §5.6).
use crate::ids::{Id, PadId};
use crate::library::footprint_source::{DimKind, DimValue, GeomConstraint, SketchRef};
use crate::model::board::ConstraintTarget;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Part dimensions on one board.
pub const MAX_PART_DIMENSIONS: usize = 4096;
/// References one part dimension names (1–3).
pub const MAX_DIMENSION_REFS: usize = 3;

/// A dimension or constraint between board, housing and part geometry.
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

/// One reference of a part dimension.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BoardRef {
    /// A board-sketch point or curve, by its stable sketch id.
    Board(SketchRef),
    /// A feature of a placed part (or of a fill or keepout's origin).
    Part(FeatureRef),
    /// A housing reference (§7.2; slice S7).
    Housing(HousingRefId),
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
    /// A face of the part's 3D model (slice S3).
    Face { placement: Id, face: ModelFaceRef },
    /// An edge of the part's 3D model (slice S3).
    Edge { placement: Id, edge: ModelEdgeRef },
    /// An end of an edge of the part's 3D model (slice S3).
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

/// A housing reference's identity in the project (slice S7).
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct HousingRefId(pub u32);

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

/// How a reference resolves against its source (§5.5). Derived, never
/// stored here: `Lost` and `Inexact` references are reported
/// (`board.reference_lost`, `board.reference_inexact`) and keep their last
/// geometry.
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
