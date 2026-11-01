//! The 3D tab's Properties card: Body (form, size and model), Model offset
//! with its quick actions and lead fit (bodies in `model_body.rs` and
//! `model_props.rs`). Package findings (range, lead count, leads off their
//! lands) show in Issues, never here.
use super::PartEditor;
use egui::Ui;
use pcbcraft_core::{Engine, ObjectRef, Operation};

impl PartEditor {
    pub(super) fn model_card(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let operation = self.body_inspector(ui);
        let placement = self.placement_inspector(ui, engine);
        operation.or(placement)
    }

    /// `Locate` target of the 3D tab: a reference to the package draft
    /// itself (its form, envelope or model placement), or to a package
    /// object other than a terminal, shows the 3D tab. Returns whether it
    /// showed anything.
    pub(super) fn locate_model(&mut self, refs: &[ObjectRef]) -> bool {
        let Some(set) = &self.draft else {
            return false;
        };
        let package = set.ids.package;
        if !refs.iter().any(|r| match *r {
            ObjectRef::Definition { id } => id == package,
            ObjectRef::LibraryObject { definition, local } => {
                definition == package
                    && set
                        .package
                        .terminal(pcbcraft_core::PackageTerminalId(local))
                        .is_none()
            }
            _ => false,
        }) {
            return false;
        }
        self.view = super::View::Model;
        true
    }
}
