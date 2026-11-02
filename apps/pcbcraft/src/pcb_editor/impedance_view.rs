//! Read-only impedance rows: each copper layer's line structure and the
//! widths and gaps a net class's impedance targets need there, computed by
//! the core (`layout::impedance`) from the stackup. Shown in the Design
//! rules panel, the Net classes card and the Route card; a layer that
//! cannot reach a target within the rules says why, in the row itself.
use crate::kit;
use egui::Ui;
use pcbcraft_core::layout::impedance::{self, Derived, Structure};
use pcbcraft_core::rules::NetClassId;
use pcbcraft_core::{CopperLayer, Project};

pub(super) fn mm(value: f64) -> String {
    format!("{} mm", kit::number_text((value * 1000.0).round() / 1000.0))
}

pub(super) fn ohms(value: f64) -> String {
    format!("{value:.1} Ω")
}

/// "Microstrip · h 0.2 mm · Dk 4.3".
pub(super) fn structure_text(structure: &Structure) -> String {
    match *structure {
        Structure::Microstrip { h, er } => {
            format!("{} · h {} · Dk {er:.2}", structure.label(), mm(h))
        }
        Structure::EmbeddedMicrostrip { h, cover, er } => format!(
            "{} · h {} · cover {} · Dk {er:.2}",
            structure.label(),
            mm(h),
            mm(cover)
        ),
        Structure::Stripline { h1, h2, er } => format!(
            "{} · {} / {} · Dk {er:.2}",
            structure.label(),
            mm(h1),
            mm(h2)
        ),
    }
}

/// "0.18 mm → 50.0 Ω · pair 0.15 / 0.2 mm → 90.0 Ω".
pub(super) fn derived_text(derived: &Derived) -> String {
    let mut parts = Vec::new();
    if let (Some(w), Some(z)) = (derived.width, derived.z0) {
        parts.push(format!("{} → {}", mm(w), ohms(z)));
    }
    if let (Some(w), Some(g), Some(z)) = (derived.pair_width, derived.pair_gap, derived.zdiff) {
        parts.push(format!("pair {} / gap {} → {}", mm(w), mm(g), ohms(z)));
    }
    if parts.is_empty() {
        "No target".to_owned()
    } else {
        parts.join(" · ")
    }
}

/// Why the targets cannot be met on a layer: one Limit row each. They
/// are the derivation's results, shown with the values they limit; a
/// routed track off its target is an Issues finding
/// (`impedance.mismatch`).
fn limit_rows(ui: &mut Ui, derived: &Derived) {
    for problem in &derived.problems {
        kit::value_row(ui, "Limit", problem);
    }
}

/// One class's derived values on every layer, with each layer's limits.
pub(super) fn class_layers(ui: &mut Ui, project: &Project, class: NetClassId) {
    for layer in project.stackup.layers() {
        let derived = impedance::derive(project, Some(class), layer);
        kit::value_row(ui, &layer.name(), &derived_text(&derived));
        limit_rows(ui, &derived);
    }
}

/// The stackup's line structures, one row per layer.
pub(super) fn layer_rows(ui: &mut Ui, project: &Project) {
    let mut assumed = false;
    let mut estimated = false;
    for layer in project.stackup.layers() {
        match impedance::structure(&project.stackup, layer) {
            Ok(line) => {
                assumed |= line.assumed_references;
                estimated |= line.dk_estimated;
                kit::value_row(ui, &layer.name(), &structure_text(&line.structure));
            }
            Err(e) => kit::value_row(ui, &layer.name(), &crate::refusal::shown_error(&e)),
        }
    }
    if assumed {
        kit::note(
            ui,
            "No layer is marked as a plane: each layer's neighbours are its references",
        );
    }
    if estimated {
        kit::note(
            ui,
            "Dk is estimated from the glass style; enter the fabricator's values in the stackup",
        );
    }
}

/// The Design rules panel's Impedance section: the layers, then every
/// class with a target and its derived values.
pub(super) fn impedance_section(ui: &mut Ui, project: &Project) {
    let classes: Vec<_> = project
        .rules
        .net_classes
        .iter()
        .filter(|c| c.signal.impedance.is_some() || c.signal.diff_impedance.is_some())
        .collect();
    let summary = match classes.len() {
        0 => "No targets".to_owned(),
        1 => "1 class with a target".to_owned(),
        n => format!("{n} classes with targets"),
    };
    kit::summary_section(
        ui,
        "pcb-rules-impedance",
        "Impedance",
        false,
        &summary,
        |ui| {
            layer_rows(ui, project);
            for class in classes {
                ui.push_id(("impedance-class", class.id), |ui| {
                    kit::section(ui, ("imp", class.id), &class.name, true, None, |ui| {
                        class_layers(ui, project, class.id);
                    });
                });
            }
            kit::note(
                ui,
                "Closed-form estimates (Hammerstad–Jensen, Wheeler, IPC-2141): no mask or etch",
            );
        },
    );
}

/// The Route card's impedance row for a track of `width` on `layer`.
pub(super) fn route_rows(
    ui: &mut Ui,
    project: &Project,
    class: Option<NetClassId>,
    layer: CopperLayer,
    width: f64,
    pair: Option<(f64, f64)>,
) {
    let Ok(line) = impedance::structure(&project.stackup, layer) else {
        return;
    };
    let mut text = format!(
        "{} · {}",
        line.structure.label(),
        ohms(line.structure.z0(width, line.t))
    );
    if let Some((w, gap)) = pair {
        text = format!(
            "{} · pair {}",
            line.structure.label(),
            ohms(line.structure.zdiff(w, gap, line.t))
        );
    }
    kit::value_row(ui, "Impedance", &text);
    let derived = impedance::derive(project, class, layer);
    if derived.width.is_some() || derived.pair_width.is_some() {
        kit::value_row(ui, "Target", &derived_text(&derived));
    }
    limit_rows(ui, &derived);
}
