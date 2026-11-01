//! Slice B4 (contract C0): the "New from template" gallery sheet (search,
//! family chips, results grid, preview column with pads and 3D thumbnail,
//! Edit values, Save as row…), opened from New component ▾, the catalog's
//! Templates folder, the Footprint tab's From template (package scope) and
//! the 3D tab's Change package… (package scope). Owned by B4.
use pcbcraft_core::{Engine, Operation};

/// The sheet's state; closed by default. B4 adds the scope it was opened
/// for: a new component (`library.new_from_template`) or the open package
/// draft (`library.package_from_template`).
#[derive(Default)]
pub struct TemplateGallery {}

impl TemplateGallery {
    /// Show the sheet when open; the operation to apply when the user
    /// creates. Contract stub: draws nothing.
    pub fn show(&mut self, _ctx: &egui::Context, _engine: &Engine) -> Option<Operation> {
        None
    }
}
