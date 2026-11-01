//! Slice B2a (contract C0): the tabbed gull-wing package family: SOT-23-3/5/6, SOT-223, SOT-89, TO-252/263 (DPAK).
//! Owned by B2a: the form, its range rules and lead order live here;
//! the footprint template in `templates/footprint_family/tabbed.rs` and the
//! model builder in `pcbcraft-solidcraft` `models/tabbed.rs`.
use super::FamilyForm;
use crate::geometry::Point3;
use crate::library::footprint_source::PackageValue;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A tab lead on the +y side (SOT-223, DPAK), electrically a terminal.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Tab {
    pub width: f64,
    pub length: f64,
}

/// Gull-wing leads with unequal counts on the two long sides and an
/// optional tab.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TabbedForm {
    /// Leads on the −y and +y sides.
    pub leads_per_side: [u32; 2],
    pub pitch: f64,
    pub body_length: f64,
    pub body_width: f64,
    pub body_height: f64,
    pub standoff: f64,
    pub span: f64,
    pub lead_width: f64,
    pub lead_thickness: f64,
    pub foot_length: f64,
    pub tab: Option<Tab>,
}

impl FamilyForm for TabbedForm {
    fn label(&self) -> &'static str {
        "Tabbed gull-wing"
    }
    fn lengths(&self) -> Vec<f64> {
        vec![
            self.pitch,
            self.body_length,
            self.body_width,
            self.body_height,
            self.span,
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
    /// Contract: the family is refused until B2a builds it.
    fn range_problems(&self) -> Vec<String> {
        vec!["The tabbed gull-wing family is not built yet (slice B2a).".into()]
    }
}
