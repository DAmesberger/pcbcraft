//! Slice B2d: the through-hole form's parameter rows in the 3D tab Body
//! section: the construction (axial, radial, DIP, TO) with its body, the
//! leads, the standoff and bend radius (shown with their estimates until
//! set) and, for radial and TO parts, formed leads.
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::package_form::tht::ThtKind;
use pcbcraft_core::library::package_form::{MAX_FORM_LEADS, ThtForm};
use pcbcraft_core::units::Nm;

/// A built-in starting value in mm, as stored (each lies far inside the
/// stored range; a miss reads 0, which the range rules report).
fn mm(v: f64) -> Nm {
    Nm::from_mm_f64_rounded(v).unwrap_or(Nm::ZERO)
}

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
                body_diameter_nm: mm(2.5),
                body_length_nm: mm(6.5),
            },
            mm(10.16),
            mm(0.6),
            mm(3.0),
        ),
        Construction::Radial => ThtForm::new(
            ThtKind::Radial {
                body_diameter_nm: mm(5.0),
                body_height_nm: mm(11.0),
            },
            mm(2.0),
            mm(0.5),
            mm(3.0),
        ),
        Construction::Dip => ThtForm::new(
            ThtKind::Dip {
                pins: 8,
                row_pitch_nm: mm(7.62),
                body_length_nm: mm(9.8),
                body_width_nm: mm(6.4),
                body_height_nm: mm(3.3),
            },
            mm(2.54),
            mm(0.46),
            mm(3.0),
        ),
        Construction::To => ThtForm::new(
            ThtKind::To {
                pins: 3,
                body_length_nm: mm(4.6),
                body_width_nm: mm(3.6),
                body_height_nm: mm(4.6),
            },
            mm(1.27),
            mm(0.45),
            mm(3.0),
        ),
    }
}

fn length(ui: &mut Ui, label: &str, value: &mut Nm) -> bool {
    super::nm_row(ui, label, value, super::length_range())
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
fn optional(ui: &mut Ui, label: &str, value: &mut Option<Nm>, estimate: f64, min: f64) -> bool {
    let mut v = value.map_or(estimate, Nm::to_mm_f64);
    if kit::number_row(ui, label, &mut v, "mm", min..=super::limit_mm())
        && let Ok(nm) = Nm::from_mm_f64_rounded(v)
    {
        *value = Some(nm);
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
            body_diameter_nm: body_diameter,
            body_length_nm: body_length,
        } => {
            changed |= length(ui, "Body diameter", body_diameter);
            changed |= length(ui, "Body length", body_length);
        }
        ThtKind::Radial {
            body_diameter_nm: body_diameter,
            body_height_nm: body_height,
        } => {
            changed |= length(ui, "Body diameter", body_diameter);
            changed |= length(ui, "Body height", body_height);
        }
        ThtKind::Dip {
            pins: n,
            row_pitch_nm: row_pitch,
            body_length_nm: body_length,
            body_width_nm: body_width,
            body_height_nm: body_height,
        } => {
            changed |= pins(ui, "Pins", n, 2);
            changed |= length(ui, "Row pitch", row_pitch);
            changed |= length(ui, "Body length", body_length);
            changed |= length(ui, "Body width", body_width);
            changed |= length(ui, "Body height", body_height);
        }
        ThtKind::To {
            pins: n,
            body_length_nm: body_length,
            body_width_nm: body_width,
            body_height_nm: body_height,
        } => {
            changed |= pins(ui, "Pins", n, 1);
            changed |= length(ui, "Body length", body_length);
            changed |= length(ui, "Body width", body_width);
            changed |= length(ui, "Body height", body_height);
        }
    }
    changed |= length(ui, "Lead pitch", &mut form.lead_pitch_nm);
    changed |= length(
        ui,
        if flat { "Lead width" } else { "Lead diameter" },
        &mut form.lead_diameter_nm,
    );
    changed |= length(ui, "Lead below board", &mut form.lead_below_nm);
    let standoff = form.standoff();
    changed |= optional(ui, "Standoff", &mut form.standoff_nm, standoff, 0.0);
    let bend = form.bend_radius();
    changed |= optional(ui, "Bend radius", &mut form.bend_radius_nm, bend, 0.01);
    if matches!(form.kind, ThtKind::Radial { .. } | ThtKind::To { .. }) {
        let mut formed = form.body_pitch().is_some();
        if kit::check_row(ui, "Formed leads", &mut formed) {
            form.body_pitch_nm = formed
                .then(|| Nm::from_mm_f64_rounded(form.lead_pitch() / 2.0).ok())
                .flatten();
            if formed && form.standoff_nm.is_none() {
                // Forming needs room under the body.
                form.standoff_nm =
                    Nm::from_mm_f64_rounded(form.standoff().max(2.0 * form.lead_diameter())).ok();
            }
            changed = true;
        }
        if let Some(pitch) = &mut form.body_pitch_nm {
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
