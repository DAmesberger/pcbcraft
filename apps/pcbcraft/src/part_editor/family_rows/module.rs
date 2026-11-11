//! The module form's parameter rows in the 3D tab Body section: outline,
//! board and can, the antenna end, pad size and pitch, castellation and
//! the host land. Pad rows, the ground array and single pads are listed
//! read-only; they come from the datasheet import (`library.package_plan`
//! values). Range problems show as package findings in Issues.
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::package_form::ModuleForm;
use pcbcraft_core::units::Nm;

fn length(ui: &mut Ui, label: &str, value: &mut Nm) -> bool {
    super::nm_row(ui, label, value, super::length_range())
}

fn offset(ui: &mut Ui, label: &str, value: &mut Nm) -> bool {
    let limit = super::limit_mm();
    super::nm_row(ui, label, value, -limit..=limit)
}

pub(super) fn rows(ui: &mut Ui, form: &mut ModuleForm) -> bool {
    let mut changed = false;
    changed |= length(ui, "Width", &mut form.width_nm);
    changed |= length(ui, "Length", &mut form.length_nm);
    changed |= length(ui, "Height", &mut form.height_nm);
    changed |= length(ui, "Board thickness", &mut form.pcb_thickness_nm);
    changed |= super::nm_row(
        ui,
        "Antenna depth",
        &mut form.antenna_length_nm,
        0.0..=super::limit_mm(),
    );
    changed |= length(ui, "Can width", &mut form.can_size.x_nm);
    changed |= length(ui, "Can length", &mut form.can_size.y_nm);
    changed |= offset(ui, "Can centre x", &mut form.can_centre.x_nm);
    changed |= offset(ui, "Can centre y", &mut form.can_centre.y_nm);
    changed |= length(ui, "Pitch", &mut form.pitch_nm);
    changed |= length(ui, "Pad length", &mut form.pad_size_nm[0]);
    changed |= length(ui, "Pad width", &mut form.pad_size_nm[1]);
    changed |= super::nm_row(
        ui,
        "Castellation",
        &mut form.castellation_nm,
        0.0..=super::limit_mm(),
    );
    changed |= length(ui, "Land width", &mut form.land_width_nm);
    changed |= offset(ui, "Land beyond pad", &mut form.land_extend_nm[0]);
    changed |= offset(ui, "Land inside pad", &mut form.land_extend_nm[1]);
    let rows: Vec<String> = form
        .rows
        .iter()
        .map(|r| format!("{:?} {}", r.side, r.count).to_lowercase())
        .collect();
    kit::value_row(ui, "Pad rows", &rows.join(", "));
    if let Some(g) = &form.ground {
        kit::value_row(
            ui,
            "Ground pads",
            &format!("{} × {} as pin {}", g.rows, g.cols, g.number),
        );
    }
    if !form.singles.is_empty() {
        let numbers: Vec<&str> = form.singles.iter().map(|p| p.number.as_str()).collect();
        kit::value_row(ui, "Single pads", &numbers.join(", "));
    }
    changed
}
