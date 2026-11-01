//! Slice B2d (contract C0): the through-hole axial, radial, DIP and TO footprint template. Owned by
//! B2d; the parent dispatches here, so the slice never edits
//! `footprint_family.rs`. Default values are estimated prototype defaults,
//! not copied tables or a compliance claim.
use super::{Builder, Pin1, e};
use crate::diagnostics::{Error, Result};
use crate::library::PackageDefinition;
use crate::library::footprint_source::Expr;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether the picker offers this template. `false` until B2d.
pub const AVAILABLE: bool = false;

/// The through-hole axial, radial, DIP and TO land pattern's driving values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThtTemplate {
    /// Lead pitch (`pitch`).
    pub pitch: Expr,
    /// Drill diameter (`drill`).
    pub drill: Expr,
    /// Copper land diameter (`land`).
    pub land: Expr,
    /// Courtyard excess around pads and envelope (`courtyard_excess`).
    pub courtyard_excess: Expr,
}

impl ThtTemplate {
    /// The template form prefilled from the package.
    pub fn defaults(_package: &PackageDefinition) -> Self {
        Self {
            pitch: e("2.54"),
            drill: e("0.8"),
            land: e("1.6"),
            courtyard_excess: e(super::TEMPLATE_COURTYARD_EXCESS),
        }
    }

    /// Every driving expression, for the syntax check.
    pub(super) fn exprs(&self) -> Vec<&Expr> {
        vec![&self.pitch, &self.drill, &self.land, &self.courtyard_excess]
    }
}

/// Build the template's pads and arrays. Contract stub until B2d.
pub(super) fn build(_b: &mut Builder<'_>, _t: &ThtTemplate) -> Result<Option<Pin1>> {
    Err(Error::Unsupported(
        "footprint.template_unsupported: the through-hole axial, radial, DIP and TO footprint template is not built yet (slice B2d)".into(),
    ))
}
