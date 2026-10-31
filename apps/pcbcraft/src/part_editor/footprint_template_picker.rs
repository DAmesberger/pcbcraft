//! "New from template" in the Footprint tab (slice F5): pick a family,
//! review its prefilled form, and send one
//! `library.footprint_from_template`, so the result is an ordinary
//! editable sketch with named parameters.
//!
//! Contract (docs/f45-contract.md): the picker state and [`request`] are
//! final; the templates task fills in the form UI.
#![expect(
    dead_code,
    reason = "F4/F5 contract: the templates task wires the picker"
)]
use pcbcraft_core::library::PackageDefinition;
use pcbcraft_core::library::templates::footprint_family::{FootprintTemplate, TemplateKind};
use pcbcraft_core::{DefinitionId, LibraryCommand, Operation};

/// The open picker and the form being filled.
#[derive(Clone, Debug, Default)]
pub struct TemplatePicker {
    pub form: Option<FootprintTemplate>,
}

impl TemplatePicker {
    /// Open the form for `kind`, prefilled from the package.
    pub fn open(&mut self, kind: TemplateKind, package: &PackageDefinition) {
        self.form = Some(FootprintTemplate::defaults(kind, package));
    }

    pub fn close(&mut self) {
        self.form = None;
    }
}

/// The one operation that builds `template` into footprint `draft`.
pub fn request(draft: DefinitionId, template: FootprintTemplate) -> Operation {
    Operation::Library(LibraryCommand::FootprintFromTemplate {
        draft,
        template: Box::new(template),
    })
}
