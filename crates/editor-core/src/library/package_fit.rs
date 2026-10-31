//! "Fit leads to pads": a pure rigid fit of tagged lead-contact points onto
//! pad centres, and the advisory placement findings. The integration crate
//! extracts the lead points; the core never reads geometry files.
//!
//! M1-M3 contract (`docs/m13-contract.md`): owned by the M3 placement task;
//! the solve refuses until then.
use super::package_model::ModelPlacement;
use crate::diagnostics::{Error, Result};
use crate::geometry::{Point, Point3};
use crate::ids::{PackageTerminalId, PadId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Lead contact off its land by more than this fraction of the land width
/// (estimate, advisory).
pub const LEAD_OFF_LAND_FRACTION: f64 = 0.25;

/// Which rotations the fit may use.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FitRotation {
    Free,
    Quarter,
}

/// One terminal's lead-contact centroid in the model frame and the centre
/// of the pad it lands on, in the datum frame.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LeadPair {
    pub terminal: PackageTerminalId,
    pub pad: PadId,
    pub lead: Point3,
    pub pad_centre: Point,
}

/// Residual of one terminal after the fit, in mm.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LeadResidual {
    pub terminal: PackageTerminalId,
    pub error_mm: f64,
}

/// The fit result: the placement that seats the leads, and its errors.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LeadFit {
    pub placement: ModelPlacement,
    pub rms_mm: f64,
    pub residuals: Vec<LeadResidual>,
    /// A mirrored assignment would fit better (pin order reversed).
    pub mirrored_better: bool,
}

/// 2D rigid least-squares fit of `pairs`, then seat on z = 0, starting from
/// `current` (its x/y rotations are kept).
pub fn fit_leads(
    _current: &ModelPlacement,
    _pairs: &[LeadPair],
    rotation: FitRotation,
) -> Result<LeadFit> {
    Err(Error::Unsupported(format!(
        "package.fit_unsupported: fitting leads ({rotation:?}) is not implemented yet"
    )))
}
