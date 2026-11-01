//! Slice D6 (contract C0): the 3D tab's Marking section (component or Part
//! marking spec: items, finish, live preview, "Use instance value").
//! Owned by D6; the 3D tab card calls [`PartEditor::marking_section`].
use super::PartEditor;
use egui::Ui;
use pcbcraft_core::{Engine, Operation};

/// The section's edit buffers.
#[derive(Default)]
pub struct MarkingProps {}

impl PartEditor {
    /// The Marking section. Contract stub: draws nothing.
    pub(super) fn marking_section(&mut self, _ui: &mut Ui, _engine: &Engine) -> Option<Operation> {
        let MarkingProps {} = &self.marking;
        None
    }
}
