//! The Footprint tab's Properties card: the armed tool's options, the
//! template section and the footprint and pad sections.
use super::PartEditor;
use egui::Ui;
use pcbcraft_core::Operation;

impl PartEditor {
    pub(super) fn footprint_card(&mut self, ui: &mut Ui) -> Option<Operation> {
        self.footprint_tool_options(ui);
        let operation = self.template_section(ui);
        self.footprint_inspector(ui);
        operation
    }
}
