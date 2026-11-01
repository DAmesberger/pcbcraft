//! Slice C6 (contract C0): the schematic pin-mux panel (peripherals,
//! pins, package mini-view; Suggest, cycle, Apply as one
//! `instance.assign_peripheral` / `select_functions` batch; inline rule
//! explanations). Owned by C6, with `schematic_editor/instance_card.rs`,
//! which calls [`entry`] in the instance's Properties. State lives in egui
//! memory, so no shared editor struct changes.
use pcbcraft_core::{ComponentInstance, Engine, Operation};

/// The "Pin assignment…" entry and, when open, the docked panel.
/// Contract stub: draws nothing.
pub fn entry(
    _ui: &mut egui::Ui,
    _engine: &Engine,
    _instance: &ComponentInstance,
) -> Option<Operation> {
    None
}
