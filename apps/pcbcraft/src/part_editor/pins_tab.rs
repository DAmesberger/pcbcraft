//! Slice C5 (contract C0): the Pins tab (between Symbol and Footprint) with
//! its Table, Peripherals and Rules sub-views. Owned by C5, with
//! `pins_table.rs` and `rules_editor.rs`. The ribbon lists the tab only
//! when [`AVAILABLE`] is true.
use super::PartEditor;
use egui::{Rect, Ui};
use pcbcraft_core::{Engine, Operation};

/// Whether the ribbon offers the Pins tab. `false` until C5.
pub const AVAILABLE: bool = false;

/// The tab's state (C5 adds the Table / Peripherals / Rules sub-view).
#[derive(Default)]
pub struct PinsTab {
    pub table: super::pins_table::PinsTable,
    pub rules: super::rules_editor::RulesEditor,
}

impl PartEditor {
    /// The Pins tab canvas. Contract stub: draws nothing.
    pub(super) fn pins_canvas(
        &mut self,
        _ui: &mut Ui,
        _rect: Rect,
        _engine: &Engine,
    ) -> Option<Operation> {
        let _ = (&self.pins.table, &self.pins.rules);
        None
    }

    /// The Pins tab's Properties card (the selected function). Contract
    /// stub: draws nothing.
    pub(super) fn pins_card(&mut self, _ui: &mut Ui, _engine: &Engine) -> Option<Operation> {
        None
    }
}
