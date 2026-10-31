//! The 3D tab's Properties card: the Body section and the model placement.
use super::PartEditor;
use egui::Ui;
use pcbcraft_core::{Engine, Operation};

impl PartEditor {
    pub(super) fn model_card(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let operation = self.body_inspector(ui);
        let placement = self.placement_inspector(ui, engine);
        operation.or(placement)
    }
}
