//! Slice D3 (contract C0): the 3D model import wizard (Frame, Materials,
//! Leads, Marking area, Licence and provenance, Save to), docked steps
//! ending in one `library.import_model` and one `library.edit_package`.
//! Owned by D3, with `model_body.rs` and `crate::model_library`.
use super::PartEditor;
use egui::Ui;
use pcbcraft_core::{Engine, Operation};

/// The wizard's step and staged file.
#[derive(Default)]
pub struct ModelImport {}

impl PartEditor {
    /// The docked wizard steps while an import runs. Contract stub: draws
    /// nothing.
    pub(super) fn import_wizard(&mut self, _ui: &mut Ui, _engine: &Engine) -> Option<Operation> {
        let ModelImport {} = &self.model_import;
        None
    }
}
