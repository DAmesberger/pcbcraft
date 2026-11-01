//! Slice B2b: the molded two-terminal (SOD, SMA, tantalum, MELF) footprint
//! template. The parent dispatches here, so the slice never edits
//! `footprint_family.rs`. Two lands on the x axis, pad 1 at −x (the band
//! end), and for a polarised part the pin-1 dot. Default values are
//! estimated prototype lands (toe 0.4 mm, heel 0.2 mm, side 0.1 mm), not
//! copied tables or a compliance claim.
use super::{Beyond, Builder, Pin1, e, locate, mm, origin, param, pin1_dot, smd};
use crate::diagnostics::Result;
use crate::library::PackageDefinition;
use crate::library::footprint_source::{Expr, PadTemplate, ParamUnit};
use crate::library::package_form::PackageForm;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    /// Mark pad 1 with the pin-1 dot (polarised parts).
    #[serde(default)]
    pub polarity: bool,
}

impl MoldedTemplate {
    /// The template form prefilled from the package: expressions over the
    /// molded form's `pkg_*` values when the package has one, else an
    /// estimated SMA in literals.
    pub fn defaults(package: &PackageDefinition) -> Self {
        let excess = e(super::TEMPLATE_COURTYARD_EXCESS);
        if let PackageForm::Molded(f) = &package.form {
            return Self {
                pad_l: e("pkg_Lt + 0.6"),
                pad_w: e("pkg_bt + 0.2"),
                gap: e("pkg_D - 2 * pkg_Lt - 0.4"),
                courtyard_excess: excess,
                polarity: f.polarised,
            };
        }
        Self {
            pad_l: e("1.7"),
            pad_w: e("1.6"),
            gap: e("2.6"),
            courtyard_excess: excess,
            polarity: true,
        }
    }

    /// Every driving expression, for the syntax check.
    pub(super) fn exprs(&self) -> Vec<&Expr> {
        vec![&self.pad_l, &self.pad_w, &self.gap, &self.courtyard_excess]
    }
}

/// Build the two lands, centred on the origin on the x axis.
pub(super) fn build(b: &mut Builder<'_>, t: &MoldedTemplate) -> Result<Option<Pin1>> {
    b.step(vec![
        param("pad_l", t.pad_l.clone(), ParamUnit::Mm),
        param("pad_w", t.pad_w.clone(), ParamUnit::Mm),
        param("gap", t.gap.clone(), ParamUnit::Mm),
        mm("pad_x", "(gap + pad_l) / 2"),
    ])?;
    let ratio = Some(e("0.25"));
    let (one, a1) = b.pad(
        PadTemplate::RoundRect,
        [-1.0, 0.0],
        smd("pad_l", "pad_w", ratio.clone(), "1"),
    )?;
    let (_, a2) = b.pad(
        PadTemplate::RoundRect,
        [1.0, 0.0],
        smd("pad_l", "pad_w", ratio, "2"),
    )?;
    let mut edits = locate(origin(), &a1, Some("pad_x"), None);
    edits.extend(locate(origin(), &a2, Some("pad_x"), None));
    b.step(edits)?;
    if t.polarity {
        return pin1_dot(b, one, &a1, [-1.0, 0.0], Beyond::Left, "pad_l / 2").map(Some);
    }
    Ok(None)
}
