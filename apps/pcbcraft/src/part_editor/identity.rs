//! Slice A1 (contract C0): the Component workspace identity bar
//! (source › shelf › name · revision · state chip, origin chip, Switch ▾
//! with recents, Open… and New…, back/forward). Owned by A1; the canvas
//! calls [`PartEditor::identity_bar`] every frame. Draws nothing until A1.
use super::PartEditor;
use egui::Ui;
use pcbcraft_core::Engine;

/// The bar's per-session state: open history and recents.
#[derive(Default)]
pub struct Identity {}

impl PartEditor {
    /// The identity bar above the canvas. Contract stub: draws nothing.
    pub(super) fn identity_bar(&mut self, _ui: &mut Ui, _engine: &Engine) {
        let Identity {} = &self.identity;
    }
}
