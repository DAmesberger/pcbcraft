//! Slice D3 (contract C0): the 3D model chooser sheet (search
//! `library.models`, facets, thumbnail grid, fit scores). Enter assigns
//! one `library.edit_package { SetModel }` and offers Fit leads. Owned by
//! D3.
use pcbcraft_core::{Engine, Operation};

/// The chooser's state; closed by default.
#[derive(Default)]
pub struct ModelChooser {
    /// The package draft the chooser serves.
    pub open: Option<pcbcraft_core::DefinitionId>,
}

impl ModelChooser {
    /// Contract stub: draws nothing and closes.
    pub fn show(&mut self, _ctx: &egui::Context, _engine: &Engine) -> Option<Operation> {
        self.open = None;
        None
    }
}
