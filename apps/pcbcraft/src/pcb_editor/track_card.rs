//! The Track card of the selected track.
use super::layer_name;
use crate::kit;
use egui::Ui;
use pcbcraft_core::{Operation, Project, Track, layout::LayoutCommand};
use solidcraft_ui_egui::ui_kit::{self, Button};

/// Read-only track properties with lock and delete.
pub(super) fn track_section(ui: &mut Ui, project: &Project, track: &Track) -> Option<Operation> {
    let mut action = None;
    kit::section(ui, "pcb-track", "Track", true, None, |ui| {
        kit::value_row(ui, "Net", project.net_name(track.net));
        kit::value_row(
            ui,
            "Width",
            &format!("{} mm", kit::number_text(track.width)),
        );
        kit::value_row(ui, "Layer", layer_name(track.layer));
        let mut locked = track.locked;
        if kit::check_row(ui, "Locked", &mut locked) {
            action = Some(LayoutCommand::SetTrackLock {
                id: track.id,
                locked,
            });
        }
        kit::actions(ui, |ui| {
            if ui_kit::button(ui, "Delete track", Button::Enabled(!track.locked)).clicked()
                && !track.locked
            {
                action = Some(LayoutCommand::DeleteTrack { id: track.id });
            }
        });
    });
    action.map(Operation::Layout)
}
