//! The sheet-level Properties cards: SHEET with nothing selected, and the
//! COMPONENT preview of the Place tool while its palette is closed.
//! Findings are not shown here; they live in Issues.
use super::SchematicEditor;
use crate::kit;
use sparksmith_core::completeness::{PurchasingState, purchasing};
use sparksmith_core::{Engine, Project};

impl SchematicEditor {
    /// The Place tool with its palette closed: the COMPONENT the next click
    /// places, and Find component… to choose another.
    pub(super) fn place_section(&mut self, ui: &mut egui::Ui, engine: &Engine) {
        let project = engine.project();
        let armed = self
            .component
            .filter(|c| project.library.component(*c).is_some());
        match armed {
            Some(component) => {
                // The Library row when a source holds a newer revision.
                let update = self
                    .updates
                    .iter()
                    .find(|u| {
                        u.local.id == component.id
                            && (u.latest > u.origin.revision || !u.modified.is_empty())
                    })
                    .cloned()
                    .map(|u| {
                        let name = project
                            .library
                            .component(component)
                            .map_or_else(String::new, |c| c.meta.name.clone());
                        (u, name)
                    });
                self.palette
                    .component_preview(ui, engine, component, update);
            }
            None => {
                kit::section(ui, "palette-component", "Component", true, None, |ui| {
                    kit::value_row(ui, "Component", "None");
                });
            }
        }
        if kit::summary_row(ui, "", Some("Find component…")) {
            self.palette.open();
        }
    }
}

/// Instances whose purchasing is not resolved: no Part chosen yet, or a
/// Part that disagrees with them.
pub(super) fn unresolved_parts(project: &Project) -> usize {
    project
        .instances
        .iter()
        .filter(|i| purchasing(project, i).state != PurchasingState::Resolved)
        .count()
}
