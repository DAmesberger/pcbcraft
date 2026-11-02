//! Assembly tables: pick-and-place (one CSV per side) and the BOM (one
//! row per placed part, and one row per purchased line grouped as
//! `project.bom` groups them).
//!
//! CONTRACT STUB: filled by the assembly task.
use super::{FabFile, FabricationOptions};
use crate::diagnostics::Result;
use crate::geometry::Point;
use crate::ids::Id;
use crate::model::{Layer, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where pick-and-place coordinates are measured from.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PlacementOrigin {
    /// The board origin of the design (Gerber coordinates are the same).
    #[default]
    BoardOrigin,
    /// The lower-left corner of the board outline's bounding box.
    OutlineLowerLeft,
    /// An explicit point in board millimetres.
    Point { at: Point },
}

/// How bottom-side rotations are written.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BottomRotation {
    /// Counter-clockwise as seen from the top, the board's own angle (the
    /// placement's `rotation_deg`).
    #[default]
    ViewedFromTop,
    /// Counter-clockwise as seen from below (the board flipped about its y
    /// axis): `(180 - rotation) mod 360`, x mirrored when
    /// [`PlacementConvention::mirror_bottom_x`] is set.
    ViewedFromBottom,
}

/// A rotation added to every part whose package label matches `package`
/// (`*` and `?` wildcards), for a fabricator whose zero orientation
/// differs from the library's.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RotationOffset {
    pub package: String,
    pub degrees: f64,
}

/// The pick-and-place convention. The default is Pcbcraft's: coordinates
/// of the placement origin (the footprint datum) from the board origin in
/// mm, rotation counter-clockwise in degrees as seen from the top with the
/// footprint as drawn in the library at 0°, bottom parts at their board
/// angle.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlacementConvention {
    #[serde(default)]
    pub origin: PlacementOrigin,
    #[serde(default)]
    pub bottom: BottomRotation,
    /// Write bottom-side x as seen from below (negated).
    #[serde(default)]
    pub mirror_bottom_x: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rotation_offsets: Vec<RotationOffset>,
}

/// One pick-and-place row.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PlacementRow {
    pub instance: Id,
    pub designator: String,
    pub value: String,
    pub package: String,
    pub x: f64,
    pub y: f64,
    pub rotation_deg: f64,
    pub side: Layer,
}

/// Every placed part under `convention`, in designator order.
pub fn rows(_project: &Project, _convention: &PlacementConvention) -> Result<Vec<PlacementRow>> {
    Ok(Vec::new())
}

/// The pick-and-place and BOM files.
pub fn files(
    _project: &Project,
    _options: &FabricationOptions,
    _stem: &str,
) -> Result<Vec<FabFile>> {
    Ok(Vec::new())
}
