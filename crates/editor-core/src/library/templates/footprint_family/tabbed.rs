//! Slice B2a (contract C0): the tabbed gull-wing (SOT, DPAK) footprint template. Owned by
//! B2a; the parent dispatches here, so the slice never edits
//! `footprint_family.rs`. Default values are estimated prototype defaults,
//! not copied tables or a compliance claim.
use super::{Builder, Pin1, e};
use crate::diagnostics::{Error, Result};
use crate::library::PackageDefinition;
use crate::library::footprint_source::Expr;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether the picker offers this template. `false` until B2a.
pub const AVAILABLE: bool = false;

/// The tabbed gull-wing (SOT, DPAK) land pattern's driving values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TabbedTemplate {
    /// Pad pitch along a row (`pitch`).
    pub pitch: Expr,
    /// Row centre to centre (`row_span`).
    pub row_span: Expr,
    /// Land length (`pad_l`).
    pub pad_l: Expr,
    /// Land width (`pad_w`).
    pub pad_w: Expr,
    /// Tab land length (`tab_l`).
    pub tab_l: Expr,
    /// Tab land width (`tab_w`).
    pub tab_w: Expr,
    /// Courtyard excess around pads and envelope (`courtyard_excess`).
    pub courtyard_excess: Expr,
}

impl TabbedTemplate {
    /// The template form prefilled from the package.
    pub fn defaults(_package: &PackageDefinition) -> Self {
        Self {
            pitch: e("1.9"),
            row_span: e("2.4"),
            pad_l: e("1.0"),
            pad_w: e("0.6"),
            tab_l: e("2.0"),
            tab_w: e("3.3"),
            courtyard_excess: e(super::TEMPLATE_COURTYARD_EXCESS),
        }
    }

    /// Every driving expression, for the syntax check.
    pub(super) fn exprs(&self) -> Vec<&Expr> {
        vec![
            &self.pitch,
            &self.row_span,
            &self.pad_l,
            &self.pad_w,
            &self.tab_l,
            &self.tab_w,
            &self.courtyard_excess,
        ]
    }
}

/// Build the template's pads and arrays. Contract stub until B2a.
pub(super) fn build(_b: &mut Builder<'_>, _t: &TabbedTemplate) -> Result<Option<Pin1>> {
    Err(Error::Unsupported(
        "footprint.template_unsupported: the tabbed gull-wing (SOT, DPAK) footprint template is not built yet (slice B2a)".into(),
    ))
}
