//! Slice A1 (contract C0): the Component workspace start page (no subject:
//! catalog search, Recent, New from template…, Blank component, Import
//! pinout…). Owned by A1.
use super::PartEditor;
use egui::Ui;
use pcbcraft_core::Engine;

impl PartEditor {
    /// Draw the start page when the workspace has no subject; `true` when
    /// it drew. Contract stub: never draws, so the existing empty state
    /// stays until A1.
    pub(super) fn start_page(&mut self, _ui: &mut Ui, _engine: &Engine) -> bool {
        false
    }
}
