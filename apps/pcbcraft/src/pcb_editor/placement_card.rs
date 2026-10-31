//! The Placement card of the selected instance, or the not-placed state
//! with Update PCB.
use super::{PcbEditor, Tool, layer_options, opposite};
use crate::kit;
use egui::Ui;
use pcbcraft_core::{
    Engine, Layer, Operation, Placement, Point, layout::LayoutCommand, updates::Change,
};
use solidcraft_ui_egui::ui_kit::{self, Button};

/// Bound on typed placement coordinates (mm); the board is far smaller.
const COORDINATE_LIMIT: f64 = 10_000.0;

impl PcbEditor {
    /// A selected instance without a placement: the header reads "Not
    /// placed", and the body offers the update that places it.
    pub(super) fn unplaced_section(&mut self, ui: &mut Ui, engine: &Engine, designator: &str) {
        let mut update = false;
        kit::section(ui, "pcb-unplaced", "Placement", true, None, |ui| {
            kit::note(
                ui,
                &format!("{designator} is not on the board yet. Run Update PCB to place it."),
            );
            kit::actions(ui, |ui| {
                update = ui_kit::button(ui, "Update PCB…", Button::Normal)
                    .on_hover_text("Review the schematic changes the board does not have yet")
                    .clicked();
            });
        });
        if update {
            self.update.begin(engine, Change::SyncPcb {});
            self.tool = Tool::Select;
            self.route_start = None;
            self.cancel_drag();
        }
    }

    pub(super) fn placement_section(
        &mut self,
        ui: &mut Ui,
        placement: &Placement,
    ) -> Option<Operation> {
        let locked = placement.locked;
        let mut action = None;
        kit::section(ui, "pcb-placement", "Placement", true, None, |ui| {
            ui.add_enabled_ui(!locked, |ui| {
                kit::pair_row(
                    ui,
                    "Position",
                    &mut self.position,
                    "mm",
                    -COORDINATE_LIMIT..=COORDINATE_LIMIT,
                );
                kit::number_row(ui, "Rotation", &mut self.rotation, "°", 0.0..=360.0);
                kit::choice_row(ui, "Side", &mut self.placement_layer, &layer_options());
            });
            let mut lock = locked;
            if kit::check_row(ui, "Locked", &mut lock) {
                action = Some(LayoutCommand::SetPlacementLock {
                    instance: placement.instance,
                    locked: lock,
                });
            }
            if locked {
                kit::note(ui, "Unlock the placement to move, rotate or flip it.");
            }
            let edit = placement_edit(
                placement,
                self.position,
                self.rotation,
                self.placement_layer,
            );
            kit::actions(ui, |ui| {
                let apply = kit::primary(ui, "Apply", edit.is_some());
                if apply.clicked() && edit.is_some() {
                    action = edit;
                }
                if ui_kit::button(ui, "Flip", Button::Enabled(!locked)).clicked() && !locked {
                    action = Some(LayoutCommand::Move {
                        instance: placement.instance,
                        position: placement.position,
                        rotation_deg: placement.rotation_deg,
                        layer: opposite(placement.layer),
                    });
                }
                if ui_kit::button(ui, "Rotate 90°", Button::Enabled(!locked)).clicked() && !locked
                {
                    action = Some(LayoutCommand::Move {
                        instance: placement.instance,
                        position: placement.position,
                        rotation_deg: (placement.rotation_deg + 90.0).rem_euclid(360.0),
                        layer: placement.layer,
                    });
                }
            });
        });
        action.map(Operation::Layout)
    }
}

/// The Move that the placement form's Apply button issues: `None` while the
/// placement is locked or the form still matches it.
pub(super) fn placement_edit(
    placement: &Placement,
    position: Point,
    rotation_deg: f64,
    layer: Layer,
) -> Option<LayoutCommand> {
    let unchanged = placement.position == position
        && placement.rotation_deg == rotation_deg
        && placement.layer == layer;
    (!placement.locked && !unchanged).then_some(LayoutCommand::Move {
        instance: placement.instance,
        position,
        rotation_deg,
        layer,
    })
}
