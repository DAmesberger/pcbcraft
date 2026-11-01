//! Slice A2 (contract C0): the Component workspace's Browser as the
//! catalog (project and every source with shelves, `library.search`,
//! Templates, Favourites and Recent, open / place actions). Owned by A2,
//! with `browser.rs`, which calls it in the Component workspace.
use pcbcraft_core::Engine;

/// The catalog tree's state.
#[derive(Default)]
pub struct Catalog {}

impl Catalog {
    /// Contract stub: draws nothing; `false` keeps the existing tree.
    pub fn show(&mut self, _ui: &mut egui::Ui, _engine: &Engine) -> bool {
        false
    }
}
