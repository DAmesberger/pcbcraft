//! The USB-C receptacle form's parameter rows in the 3D tab Body section:
//! outline, opening, shell, contact lands, stakes and pegs. Range problems
//! show as package findings in Issues.
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::UsbCForm;
use pcbcraft_core::library::package_form::{FORM_LENGTH_LIMIT_MM, FORM_LENGTH_MIN_MM};

fn length(ui: &mut Ui, label: &str, value: &mut f64) -> bool {
    kit::number_row(
        ui,
        label,
        value,
        "mm",
        FORM_LENGTH_MIN_MM..=FORM_LENGTH_LIMIT_MM,
    )
}

fn offset(ui: &mut Ui, label: &str, value: &mut f64) -> bool {
    kit::number_row(
        ui,
        label,
        value,
        "mm",
        -FORM_LENGTH_LIMIT_MM..=FORM_LENGTH_LIMIT_MM,
    )
}

pub(super) fn rows(ui: &mut Ui, form: &mut UsbCForm) -> bool {
    let mut changed = false;
    changed |= length(ui, "Width", &mut form.width);
    changed |= length(ui, "Depth", &mut form.depth);
    changed |= length(ui, "Height", &mut form.height);
    changed |= length(ui, "Opening width", &mut form.opening[0]);
    changed |= length(ui, "Opening height", &mut form.opening[1]);
    changed |= length(ui, "Opening centre height", &mut form.opening_z);
    changed |= length(ui, "Shell thickness", &mut form.shell_thickness);
    changed |= length(ui, "Contact pitch", &mut form.pitch);
    changed |= offset(ui, "Contact lands y", &mut form.land_y);
    changed |= length(ui, "Contact land length", &mut form.land_length);
    changed |= length(ui, "Signal land width", &mut form.signal_land_width);
    changed |= length(ui, "Power land width", &mut form.power_land_width);
    changed |= length(ui, "VBUS land offset", &mut form.vbus_x);
    changed |= length(ui, "GND land offset", &mut form.gnd_x);
    changed |= length(ui, "Stake offset", &mut form.tab_x);
    changed |= length(ui, "Stake below board", &mut form.stake_length);
    for (label, tab) in ["Front stake", "Rear stake"].iter().zip(&mut form.tabs) {
        changed |= offset(ui, &format!("{label} y"), &mut tab.y);
        changed |= length(ui, &format!("{label} slot width"), &mut tab.slot[0]);
        changed |= length(ui, &format!("{label} slot length"), &mut tab.slot[1]);
        changed |= length(ui, &format!("{label} land width"), &mut tab.land[0]);
        changed |= length(ui, &format!("{label} land length"), &mut tab.land[1]);
    }
    changed |= length(ui, "Peg offset", &mut form.peg_x);
    changed |= offset(ui, "Peg y", &mut form.peg_y);
    changed |= length(ui, "Peg diameter", &mut form.peg_diameter);
    changed |= length(ui, "Peg hole", &mut form.peg_hole);
    changed
}
