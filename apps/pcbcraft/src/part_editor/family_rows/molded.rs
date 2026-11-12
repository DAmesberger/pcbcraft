//! Slice B2b: the molded two-terminal form's parameter rows in the 3D tab
//! Body section (SOD, SMA/SMB/SMC, tantalum, MELF): lead kind, body,
//! terminals and the polarity band. A MELF body is round, so its width row
//! sets the height too. Range problems show as package findings in Issues.
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::package_form::MoldedForm;
use pcbcraft_core::library::package_form::molded::MoldedLead;
use pcbcraft_core::units::Nm;

fn length(ui: &mut Ui, label: &str, value: &mut Nm) -> bool {
    super::nm_row(ui, label, value, super::length_range())
}

const LEADS: [(MoldedLead, &str); 4] = [
    (MoldedLead::Flat, "Flat (SOD-523, SOD-123F)"),
    (MoldedLead::Bent, "Bent (SOD-123, SOD-323)"),
    (MoldedLead::JLead, "J-lead (SMA, tantalum)"),
    (MoldedLead::Melf, "End caps (MELF)"),
];

/// Keep a MELF body round after an edit.
fn rounded(form: &mut MoldedForm) {
    if form.lead == MoldedLead::Melf {
        form.body_height_nm = form.body_width_nm;
        form.terminal_width_nm = form.terminal_width_nm.max(form.body_width_nm);
    }
}

/// The rows of a molded form; returns whether a value changed.
pub(super) fn rows(ui: &mut Ui, form: &mut MoldedForm) -> bool {
    let mut changed = false;
    let options: Vec<(MoldedLead, String)> =
        LEADS.iter().map(|(l, s)| (*l, (*s).to_owned())).collect();
    changed |= kit::choice_row(ui, "Terminals", &mut form.lead, &options);
    let melf = form.lead == MoldedLead::Melf;
    changed |= length(ui, "Body length", &mut form.body_length_nm);
    changed |= length(
        ui,
        if melf { "Body diameter" } else { "Body width" },
        &mut form.body_width_nm,
    );
    if !melf {
        changed |= length(ui, "Body height", &mut form.body_height_nm);
    }
    changed |= length(ui, "Overall length", &mut form.span_nm);
    changed |= length(
        ui,
        if melf {
            "Cap length"
        } else {
            "Terminal length"
        },
        &mut form.terminal_length_nm,
    );
    changed |= length(
        ui,
        if melf {
            "Cap diameter"
        } else {
            "Terminal width"
        },
        &mut form.terminal_width_nm,
    );
    changed |= kit::check_row(ui, "Polarity band at terminal 1", &mut form.polarised);
    if changed {
        rounded(form);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_melf_stays_round() {
        let mut f = MoldedForm {
            lead: MoldedLead::Melf,
            body_length_nm: Nm::from_mm_f64_rounded(3.0).unwrap_or_default(),
            body_width_nm: Nm::from_mm_f64_rounded(1.6).unwrap_or_default(),
            body_height_nm: Nm::from_mm_f64_rounded(1.4).unwrap_or_default(),
            span_nm: Nm::from_mm_f64_rounded(3.5).unwrap_or_default(),
            terminal_length_nm: Nm::from_mm_f64_rounded(0.4).unwrap_or_default(),
            terminal_width_nm: Nm::from_mm_f64_rounded(1.5).unwrap_or_default(),
            polarised: true,
        };
        rounded(&mut f);
        assert_eq!((f.body_height(), f.terminal_width()), (1.6, 1.6));
        assert!(
            pcbcraft_core::library::PackageForm::Molded(Box::new(f))
                .range_problems()
                .is_empty()
        );
    }
}
