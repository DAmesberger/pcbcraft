//! Slice B2b (contract C0): the molded two-terminal (SOD, SMA, tantalum, MELF) footprint template. Owned by
//! B2b; the parent dispatches here, so the slice never edits
//! `footprint_family.rs`. Default values are estimated prototype defaults,
//! not copied tables or a compliance claim.
use super::{Builder, Pin1, e};
use crate::diagnostics::{Error, Result};
use crate::library::PackageDefinition;
use crate::library::footprint_source::Expr;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether the picker offers this template. `false` until B2b.
pub const AVAILABLE: bool = false;

/// The molded two-terminal (SOD, SMA, tantalum, MELF) land pattern's driving values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MoldedTemplate {
    /// Land length along the body axis (`pad_l`).
    pub pad_l: Expr,
    /// Land width (`pad_w`).
    pub pad_w: Expr,
    /// Gap between the lands (`gap`).
    pub gap: Expr,
    /// Courtyard excess around pads and envelope (`courtyard_excess`).
    pub courtyard_excess: Expr,
}

impl MoldedTemplate {
    /// The template form prefilled from the package.
    pub fn defaults(_package: &PackageDefinition) -> Self {
        Self {
            pad_l: e("1.2"),
            pad_w: e("1.2"),
            gap: e("2.0"),
            courtyard_excess: e(super::TEMPLATE_COURTYARD_EXCESS),
        }
    }

    /// Every driving expression, for the syntax check.
    pub(super) fn exprs(&self) -> Vec<&Expr> {
        vec![&self.pad_l, &self.pad_w, &self.gap, &self.courtyard_excess]
    }
}

/// Build the template's pads and arrays. Contract stub until B2b.
pub(super) fn build(_b: &mut Builder<'_>, _t: &MoldedTemplate) -> Result<Option<Pin1>> {
    Err(Error::Unsupported(
        "footprint.template_unsupported: the molded two-terminal (SOD, SMA, tantalum, MELF) footprint template is not built yet (slice B2b)".into(),
    ))
}
