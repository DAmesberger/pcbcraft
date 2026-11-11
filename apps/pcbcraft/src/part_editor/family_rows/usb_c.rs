//! The USB-C receptacle form's parameter rows in the 3D tab Body section:
//! outline, opening, shell, contact lands, stakes and pegs. Range problems
//! show as package findings in Issues.
use egui::Ui;
use pcbcraft_core::library::UsbCForm;
use pcbcraft_core::units::Nm;

fn length(ui: &mut Ui, label: &str, value: &mut Nm) -> bool {
    super::nm_row(ui, label, value, super::length_range())
}

fn offset(ui: &mut Ui, label: &str, value: &mut Nm) -> bool {
    let limit = super::limit_mm();
    super::nm_row(ui, label, value, -limit..=limit)
}

pub(super) fn rows(ui: &mut Ui, form: &mut UsbCForm) -> bool {
    let mut changed = false;
    changed |= length(ui, "Width", &mut form.width_nm);
    changed |= length(ui, "Depth", &mut form.depth_nm);
    changed |= length(ui, "Height", &mut form.height_nm);
    changed |= length(ui, "Opening width", &mut form.opening_nm[0]);
    changed |= length(ui, "Opening height", &mut form.opening_nm[1]);
    changed |= length(ui, "Opening centre height", &mut form.opening_z_nm);
    changed |= length(ui, "Shell thickness", &mut form.shell_thickness_nm);
    changed |= length(ui, "Contact pitch", &mut form.pitch_nm);
    changed |= offset(ui, "Contact lands y", &mut form.land_y_nm);
    changed |= length(ui, "Contact land length", &mut form.land_length_nm);
    changed |= length(ui, "Signal land width", &mut form.signal_land_width_nm);
    changed |= length(ui, "Power land width", &mut form.power_land_width_nm);
    changed |= length(ui, "VBUS land offset", &mut form.vbus_x_nm);
    changed |= length(ui, "GND land offset", &mut form.gnd_x_nm);
    changed |= length(ui, "Stake offset", &mut form.tab_x_nm);
    changed |= length(ui, "Stake below board", &mut form.stake_length_nm);
    for (label, tab) in ["Front stake", "Rear stake"].iter().zip(&mut form.tabs) {
        changed |= offset(ui, &format!("{label} y"), &mut tab.y_nm);
        changed |= length(ui, &format!("{label} slot width"), &mut tab.slot.x_nm);
        changed |= length(ui, &format!("{label} slot length"), &mut tab.slot.y_nm);
        changed |= length(ui, &format!("{label} land width"), &mut tab.land.x_nm);
        changed |= length(ui, &format!("{label} land length"), &mut tab.land.y_nm);
    }
    changed |= length(ui, "Peg offset", &mut form.peg_x_nm);
    changed |= offset(ui, "Peg y", &mut form.peg_y_nm);
    changed |= length(ui, "Peg diameter", &mut form.peg_diameter_nm);
    changed |= length(ui, "Peg hole", &mut form.peg_hole_nm);
    changed
}
