//! Slice B2b (contract C0): the molded two-terminal package family: SOD-123/323/523, SMA/SMB/SMC, molded tantalum A–E, MELF.
//! Owned by B2b: the form, its range rules and lead order live here;
//! the footprint template in `templates/footprint_family/molded.rs` and the
//! model builder in `pcbcraft-solidcraft` `models/molded.rs`.
use super::FamilyForm;
use crate::geometry::Point3;
use crate::library::footprint_source::PackageValue;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How the two terminals are formed.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MoldedLead {
    /// Flat leads under the body (SOD-123F, SMA flat).
    Flat,
    /// J-leads folded under the body (SMA/SMB/SMC, tantalum).
    JLead,
    /// Cylindrical end caps (MELF, MiniMELF).
    Melf,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MoldedForm {
    pub lead: MoldedLead,
    pub body_length: f64,
    pub body_width: f64,
    pub body_height: f64,
    /// Overall length, terminal tip to tip.
    pub span: f64,
    pub terminal_length: f64,
    pub terminal_width: f64,
    /// A polarity band at terminal 1, as data.
    pub polarised: bool,
}

impl FamilyForm for MoldedForm {
    fn label(&self) -> &'static str {
        "Molded two-terminal"
    }
    fn lengths(&self) -> Vec<f64> {
        vec![
            self.body_length,
            self.body_width,
            self.body_height,
            self.span,
            self.terminal_length,
            self.terminal_width,
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
    /// Contract: the family is refused until B2b builds it.
    fn range_problems(&self) -> Vec<String> {
        vec!["The molded two-terminal family is not built yet (slice B2b).".into()]
    }
}
