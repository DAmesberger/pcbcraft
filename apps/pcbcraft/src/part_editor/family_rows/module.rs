//! The module form's parameter rows in the 3D tab Body section: outline,
//! board and can, the antenna end, pad size and pitch, castellation and
//! the host land. Pad rows, the ground array and single pads are listed
//! read-only; they come from the datasheet import (`library.package_plan`
//! values). Range problems show as package findings in Issues.
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::package_form::{FORM_LENGTH_LIMIT_MM, FORM_LENGTH_MIN_MM, ModuleForm};

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

pub(super) fn rows(ui: &mut Ui, form: &mut ModuleForm) -> bool {
    let mut changed = false;
    changed |= length(ui, "Width", &mut form.width);
    changed |= length(ui, "Length", &mut form.length);
    changed |= length(ui, "Height", &mut form.height);
    changed |= length(ui, "Board thickness", &mut form.pcb_thickness);
    changed |= kit::number_row(
        ui,
        "Antenna depth",
        &mut form.antenna_length,
        "mm",
        0.0..=FORM_LENGTH_LIMIT_MM,
    );
    changed |= length(ui, "Can width", &mut form.can_size[0]);
    changed |= length(ui, "Can length", &mut form.can_size[1]);
    changed |= offset(ui, "Can centre x", &mut form.can_centre[0]);
    changed |= offset(ui, "Can centre y", &mut form.can_centre[1]);
    changed |= length(ui, "Pitch", &mut form.pitch);
    changed |= length(ui, "Pad length", &mut form.pad_size[0]);
    changed |= length(ui, "Pad width", &mut form.pad_size[1]);
    changed |= kit::number_row(
        ui,
        "Castellation",
        &mut form.castellation,
        "mm",
        0.0..=FORM_LENGTH_LIMIT_MM,
    );
    changed |= length(ui, "Land width", &mut form.land_width);
    changed |= offset(ui, "Land beyond pad", &mut form.land_extend[0]);
    changed |= offset(ui, "Land inside pad", &mut form.land_extend[1]);
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
