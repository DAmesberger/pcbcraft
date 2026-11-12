//! Datum vocabulary shared by package datums (slice B2) and the board
//! interface (slices B1 and B3): what geometry a datum has and how a datum
//! is named inside its owner. Data and syntax only; the interface's rules
//! live in `crate::interface` (docs/architecture/board-interface.md).
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Longest datum name, in bytes.
pub const MAX_DATUM_NAME: usize = 32;

/// Names a package may not give a datum: the interface already uses them
/// for every placement (`i80.frame`, `i80.envelope`), so a package datum of
/// that name would make two datums with one token.
pub const RESERVED_DATUM_NAMES: [&str; 2] = ["frame", "envelope"];

/// The part-file name of a construction plane (or a sketch, for a free
/// profile) that declares an opening: `opening` or `opening-<suffix>`. It
/// becomes the package datum `opening` or `opening_<suffix>` (slice B2).
pub const PART_FILE_OPENING: &str = "opening";
/// The part-file name of a construction axis that declares an axis datum:
/// `axis` or `axis-<suffix>`, giving `axis` or `axis_<suffix>` (slice B2).
pub const PART_FILE_AXIS: &str = "axis";

/// The geometry a datum carries (docs/architecture/board-interface.md,
/// "Datum kinds").
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum DatumKind {
    /// An origin and two axes: the board origin, a part's placement frame.
    Frame,
    /// An origin and a normal: board top and bottom, a mating plane.
    Plane,
    /// A point and a direction, with an optional diameter: a mounting
    /// hole, an LED's light axis.
    Axis,
    /// Closed loops of lines and arcs on a plane: the board outline, a
    /// cutout, a connector opening's outline.
    Profile,
    /// A profile and a height along its normal: a keepout, a part's
    /// envelope, the space a plug needs.
    Volume,
    /// A name, a value and a unit: board thickness, a board parameter.
    Parameter,
}

/// A datum's name inside its owner (`opening`, `axis_led`, `top`): lower
/// case `[a-z][a-z0-9_]*`, at most [`MAX_DATUM_NAME`] bytes. It is part of
/// the datum's key and token, so it is identity, unlike the display name.
#[derive(
    Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(transparent)]
pub struct DatumName(pub String);

impl DatumName {
    /// The name's syntax: `[a-z][a-z0-9_]*`, at most [`MAX_DATUM_NAME`]
    /// bytes.
    pub fn valid(&self) -> bool {
        let mut chars = self.0.chars();
        self.0.len() <= MAX_DATUM_NAME
            && chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }

    /// Whether a package may declare a datum of this name: valid and not
    /// one of [`RESERVED_DATUM_NAMES`].
    pub fn valid_for_package(&self) -> bool {
        self.valid() && !RESERVED_DATUM_NAMES.contains(&self.0.as_str())
    }
}
