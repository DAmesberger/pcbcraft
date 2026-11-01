//! Slice B2d: the through-hole form's parameter rows in the 3D tab Body
//! section: the construction (axial, radial, DIP, TO) with its body, the
//! leads, the standoff and bend radius (shown with their estimates until
//! set) and, for radial and TO parts, formed leads.
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::package_form::tht::ThtKind;
use pcbcraft_core::library::package_form::{MAX_FORM_LEADS, ThtForm};
use std::ops::RangeInclusive;

const LENGTH: RangeInclusive<f64> = 0.01..=1000.0;

/// The constructions offered, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Construction {
    Axial,
    Radial,
    Dip,
    To,
}

pub(super) fn construction(kind: &ThtKind) -> Construction {
    match kind {
        ThtKind::Axial { .. } => Construction::Axial,
        ThtKind::Radial { .. } => Construction::Radial,
        ThtKind::Dip { .. } => Construction::Dip,
        ThtKind::To { .. } => Construction::To,
    }
}

/// A starting form of each construction: estimated round values to edit,
/// from no table.
pub(super) fn starting(c: Construction) -> ThtForm {
    match c {
        Construction::Axial => ThtForm::new(
            ThtKind::Axial {
                body_diameter: 2.5,
                body_length: 6.5,
            },
            10.16,
            0.6,
            3.0,
        ),
        Construction::Radial => ThtForm::new(
            ThtKind::Radial {
                body_diameter: 5.0,
                body_height: 11.0,
            },
            2.0,
            0.5,
            3.0,
        ),
        Construction::Dip => ThtForm::new(
            ThtKind::Dip {
                pins: 8,
                row_pitch: 7.62,
                body_length: 9.8,
                body_width: 6.4,
                body_height: 3.3,
            },
            2.54,
            0.46,
            3.0,
        ),
        Construction::To => ThtForm::new(
            ThtKind::To {
                pins: 3,
                body_length: 4.6,
                body_width: 3.6,
                body_height: 4.6,
            },
            1.27,
            0.45,
            3.0,
        ),
    }
}

fn length(ui: &mut Ui, label: &str, value: &mut f64) -> bool {
    kit::number_row(ui, label, value, "mm", LENGTH)
}

fn pins(ui: &mut Ui, label: &str, value: &mut u32, step: u32) -> bool {
    let mut v = f64::from(*value);
    let range = f64::from(step)..=f64::from(MAX_FORM_LEADS);
    if kit::number_row(ui, label, &mut v, "", range) && v.fract() == 0.0 {
        // Bounded and whole by the range and the check above.
        let n = v as u32;
        if n.is_multiple_of(step) {
            *value = n;
            return true;
        }
    }
    false
}

/// A length shown with its estimate until set; returns whether it changed.
fn optional(ui: &mut Ui, label: &str, value: &mut Option<f64>, estimate: f64, min: f64) -> bool {
    let mut v = value.unwrap_or(estimate);
    if kit::number_row(ui, label, &mut v, "mm", min..=1000.0) {
        *value = Some(v);
        return true;
    }
    false
}

/// The rows; returns whether a value changed.
pub(super) fn rows(ui: &mut Ui, form: &mut ThtForm) -> bool {
    let mut changed = false;
    let mut chosen = construction(&form.kind);
    let options = [
        (Construction::Axial, "Axial (lying)".to_owned()),
        (Construction::Radial, "Radial (standing)".to_owned()),
        (Construction::Dip, "DIP".to_owned()),
        (Construction::To, "TO (single row)".to_owned()),
    ];
    if kit::choice_row(ui, "Construction", &mut chosen, &options)
        && chosen != construction(&form.kind)
    {
        *form = starting(chosen);
        changed = true;
    }
    let flat = matches!(form.kind, ThtKind::Dip { .. });
    match &mut form.kind {
        ThtKind::Axial {
            body_diameter,
            body_length,
        } => {
            changed |= length(ui, "Body diameter", body_diameter);
            changed |= length(ui, "Body length", body_length);
        }
        ThtKind::Radial {
            body_diameter,
            body_height,
        } => {
            changed |= length(ui, "Body diameter", body_diameter);
            changed |= length(ui, "Body height", body_height);
        }
        ThtKind::Dip {
            pins: n,
            row_pitch,
            body_length,
            body_width,
            body_height,
        } => {
            changed |= pins(ui, "Pins", n, 2);
            changed |= length(ui, "Row pitch", row_pitch);
            changed |= length(ui, "Body length", body_length);
            changed |= length(ui, "Body width", body_width);
            changed |= length(ui, "Body height", body_height);
        }
        ThtKind::To {
            pins: n,
            body_length,
            body_width,
            body_height,
        } => {
            changed |= pins(ui, "Pins", n, 1);
            changed |= length(ui, "Body length", body_length);
            changed |= length(ui, "Body width", body_width);
            changed |= length(ui, "Body height", body_height);
        }
    }
    changed |= length(ui, "Lead pitch", &mut form.lead_pitch);
    changed |= length(
        ui,
        if flat { "Lead width" } else { "Lead diameter" },
        &mut form.lead_diameter,
    );
    changed |= length(ui, "Lead below board", &mut form.lead_below);
    let standoff = form.standoff();
    changed |= optional(ui, "Standoff", &mut form.standoff, standoff, 0.0);
    let bend = form.bend_radius();
    changed |= optional(ui, "Bend radius", &mut form.bend_radius, bend, 0.01);
    if matches!(form.kind, ThtKind::Radial { .. } | ThtKind::To { .. }) {
        let mut formed = form.body_pitch.is_some();
        if kit::check_row(ui, "Formed leads", &mut formed) {
            form.body_pitch = formed.then_some(form.lead_pitch / 2.0);
            if formed && form.standoff.is_none() {
                // Forming needs room under the body.
                form.standoff = Some(form.standoff().max(2.0 * form.lead_diameter));
            }
            changed = true;
        }
        if let Some(pitch) = &mut form.body_pitch {
            changed |= length(ui, "Pitch at body", pitch);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::library::package_form::FamilyForm;

    #[test]
    fn every_starting_form_is_consistent() {
        for c in [
            Construction::Axial,
            Construction::Radial,
            Construction::Dip,
            Construction::To,
        ] {
            let form = starting(c);
            assert_eq!(construction(&form.kind), c);
            assert_eq!(form.range_problems(), Vec::<String>::new(), "{c:?}");
        }
    }
}
