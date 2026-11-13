//! Slices B2a–d: typed parameter rows of the later package
//! families in the 3D tab Body section, one file per family under
//! `family_rows/`. `model_body::form_rows` dispatches here.
use crate::kit;
use egui::Ui;
use sparksmith_core::library::PackageForm;
use sparksmith_core::library::package_form::{FORM_LENGTH_LIMIT_NM, FORM_LENGTH_MIN_NM};
use sparksmith_core::units::Nm;
use std::ops::RangeInclusive;

mod grid;
mod module;
mod molded;
mod tabbed;
mod tht;
mod usb_c;

/// A row editing a stored length in millimetres, within `range` (mm);
/// returns whether it changed.
pub(super) fn nm_row(ui: &mut Ui, label: &str, value: &mut Nm, range: RangeInclusive<f64>) -> bool {
    let mut mm = value.to_mm_f64();
    if !kit::number_row(ui, label, &mut mm, "mm", range) {
        return false;
    }
    match Nm::from_mm_f64_rounded(mm) {
        Ok(nm) => {
            *value = nm;
            true
        }
        Err(_) => false,
    }
}

/// The typed-form length range in mm.
pub(super) fn length_range() -> RangeInclusive<f64> {
    FORM_LENGTH_MIN_NM.to_mm_f64()..=limit_mm()
}

/// The typed-form length limit in mm.
pub(super) fn limit_mm() -> f64 {
    FORM_LENGTH_LIMIT_NM.to_mm_f64()
}

/// The rows of a later family's form; returns whether a value changed.
pub fn rows(ui: &mut Ui, form: &mut PackageForm) -> bool {
    match form {
        PackageForm::Tabbed(f) => tabbed::rows(ui, f),
        PackageForm::Molded(f) => molded::rows(ui, f),
        PackageForm::Grid(f) => grid::rows(ui, f),
        PackageForm::Tht(f) => tht::rows(ui, f),
        PackageForm::Module(f) => module::rows(ui, f),
        PackageForm::UsbC(f) => usb_c::rows(ui, f),
        PackageForm::Generic
        | PackageForm::Chip(_)
        | PackageForm::GullWing(_)
        | PackageForm::NoLead(_)
        | PackageForm::Header(_) => false,
    }
}
