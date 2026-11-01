//! Slice B2c (contract C0): the grid array (BGA, LGA, WLCSP) footprint template. Owned by
//! B2c; the parent dispatches here, so the slice never edits
//! `footprint_family.rs`. Default values are estimated prototype defaults,
//! not copied tables or a compliance claim.
use super::{Builder, Pin1, e};
use crate::diagnostics::{Error, Result};
use crate::library::PackageDefinition;
use crate::library::footprint_source::Expr;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether the picker offers this template. `false` until B2c.
pub const AVAILABLE: bool = false;

/// The grid array (BGA, LGA, WLCSP) land pattern's driving values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GridTemplate {
    /// Ball pitch (`pitch`).
    pub pitch: Expr,
    /// Land diameter (`land`).
    pub land: Expr,
    /// Solder-mask expansion for NSMD lands (`mask_expansion`).
    pub mask_expansion: Expr,
    /// Courtyard excess around pads and envelope (`courtyard_excess`).
    pub courtyard_excess: Expr,
}

impl GridTemplate {
    /// The template form prefilled from the package.
    pub fn defaults(_package: &PackageDefinition) -> Self {
        Self {
            pitch: e("0.8"),
            land: e("0.4"),
            mask_expansion: e("0.05"),
            courtyard_excess: e(super::TEMPLATE_COURTYARD_EXCESS),
        }
    }

    /// Every driving expression, for the syntax check.
    pub(super) fn exprs(&self) -> Vec<&Expr> {
        vec![
            &self.pitch,
            &self.land,
            &self.mask_expansion,
            &self.courtyard_excess,
        ]
    }
}

/// Build the template's pads and arrays. Contract stub until B2c.
pub(super) fn build(_b: &mut Builder<'_>, _t: &GridTemplate) -> Result<Option<Pin1>> {
    Err(Error::Unsupported(
        "footprint.template_unsupported: the grid array (BGA, LGA, WLCSP) footprint template is not built yet (slice B2c)".into(),
    ))
}
