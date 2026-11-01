//! Slice B2c (contract C0): the grid array package family: BGA, LGA, WLCSP with a depopulation mask and JEDEC A1 numbering as data.
//! Owned by B2c: the form, its range rules and lead order live here;
//! the footprint template in `templates/footprint_family/grid.rs` and the
//! model builder in `pcbcraft-solidcraft` `models/grid.rs`.
use super::FamilyForm;
use crate::geometry::Point3;
use crate::library::footprint_source::PackageValue;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GridContact {
    Ball,
    Land,
}

/// Bytes of a grid row-letter alphabet.
pub const MAX_ROW_LETTERS: usize = 64;
/// Positions one depopulation mask lists.
pub const MAX_DEPOPULATED: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GridForm {
    pub contact: GridContact,
    pub rows: u32,
    pub cols: u32,
    /// Pitch along a row and between rows.
    pub pitch: [f64; 2],
    pub body_length: f64,
    pub body_width: f64,
    pub body_height: f64,
    /// Ball or land diameter.
    pub contact_diameter: f64,
    /// Ball height (standoff); 0 for lands.
    pub contact_height: f64,
    /// Empty (row, column) positions, 0-based.
    pub depopulated: Vec<[u32; 2]>,
    /// The row-letter alphabet as data (JEDEC skips I, O, Q, S, X, Z).
    pub row_letters: String,
}

impl FamilyForm for GridForm {
    fn label(&self) -> &'static str {
        "Grid array"
    }
    fn lengths(&self) -> Vec<f64> {
        vec![
            self.pitch[0],
            self.pitch[1],
            self.body_length,
            self.body_width,
            self.body_height,
            self.contact_diameter,
            self.contact_height,
        ]
    }
    fn envelope(&self) -> Option<[f64; 3]> {
        None
    }
    fn lead_count(&self) -> u64 {
        0
    }
    fn lead_points(&self) -> Vec<Point3> {
        Vec::new()
    }
    fn values(&self) -> Vec<PackageValue> {
        Vec::new()
    }
    /// Contract: the family is refused until B2c builds it.
    fn range_problems(&self) -> Vec<String> {
        vec!["The grid array family is not built yet (slice B2c).".into()]
    }
}
