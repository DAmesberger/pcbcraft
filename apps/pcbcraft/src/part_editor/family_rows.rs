//! Slices B2a–d: typed parameter rows of the later package
//! families in the 3D tab Body section, one file per family under
//! `family_rows/`. `model_body::form_rows` dispatches here.
use egui::Ui;
use pcbcraft_core::library::PackageForm;

mod grid;
mod module;
mod molded;
mod tabbed;
mod tht;
mod usb_c;

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
