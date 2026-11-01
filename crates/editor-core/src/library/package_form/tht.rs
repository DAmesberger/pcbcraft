//! Slice B2d (contract C0): the through-hole axial/radial/DIP/TO package family: axial R/D, radial capacitors, DIP-8…40, TO-92, TO-220.
//! Owned by B2d: the form, its range rules and lead order live here;
//! the footprint template in `templates/footprint_family/tht.rs` and the
//! model builder in `pcbcraft-solidcraft` `models/tht.rs`.
use super::FamilyForm;
use crate::geometry::Point3;
use crate::library::footprint_source::PackageValue;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The through-hole construction.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "construction", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThtKind {
    /// Cylindrical body lying flat, leads bent down at `lead_pitch`.
    Axial {
        body_diameter: f64,
        body_length: f64,
    },
    /// Cylindrical body standing up.
    Radial {
        body_diameter: f64,
        body_height: f64,
    },
    /// Dual in-line: `pins` total in two rows `row_pitch` between centres.
    Dip {
        pins: u32,
        row_pitch: f64,
        body_length: f64,
        body_width: f64,
        body_height: f64,
    },
    /// TO-92 / TO-220 style: `pins` in one row.
    To {
        pins: u32,
        body_length: f64,
        body_width: f64,
        body_height: f64,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThtForm {
    pub kind: ThtKind,
    /// Lead pitch along a row (axial: lead to lead).
    pub lead_pitch: f64,
    pub lead_diameter: f64,
    /// Lead length below the seating plane.
    pub lead_below: f64,
}

impl FamilyForm for ThtForm {
    fn label(&self) -> &'static str {
        "Through-hole"
    }
    fn lengths(&self) -> Vec<f64> {
        vec![self.lead_pitch, self.lead_diameter, self.lead_below]
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
    /// Contract: the family is refused until B2d builds it.
    fn range_problems(&self) -> Vec<String> {
        vec!["The through-hole axial/radial/DIP/TO family is not built yet (slice B2d).".into()]
    }
}
