//! Slice D6 (contract C0): the instance Properties Marking row (resolved
//! text, Edit: variables such as date code, hide, replace through
//! `instance.set_marking`). Owned by D6; the instance card calls [`row`].
use pcbcraft_core::{ComponentInstance, Engine, Operation};

/// Contract stub: draws nothing.
pub fn row(
    _ui: &mut egui::Ui,
    _engine: &Engine,
    _instance: &ComponentInstance,
) -> Option<Operation> {
    None
}
