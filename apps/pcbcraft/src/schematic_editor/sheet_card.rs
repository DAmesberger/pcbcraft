//! The sheet-level Properties cards: SHEET with nothing selected, and the
//! COMPONENT preview of the Place tool while its palette is closed.
//! Findings are not shown here; they live in Issues.
use super::SchematicEditor;
use crate::kit;
use pcbcraft_core::completeness::{PurchasingState, purchasing};
use pcbcraft_core::{Engine, Project};

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

    /// Nothing selected: what the sheet holds and how many instances still
    /// need a manufacturer part, with Resolve parts….
    pub(super) fn sheet_section(&mut self, ui: &mut egui::Ui, project: &Project) {
        let mut resolve = false;
        kit::section(ui, "sheet", "Sheet", true, None, |ui| {
            kit::value_row(ui, "Instances", &project.instances.len().to_string());
            kit::value_row(ui, "Wires", &project.wires.len().to_string());
            let connected = self.nets.iter().filter(|n| n.terminals.len() >= 2).count();
            ui.scope(|ui| kit::value_row(ui, "Nets", &connected.to_string()))
                .response
                .on_hover_text("Nets that join two or more terminals");
            let unresolved = unresolved_parts(project);
            if unresolved == 0 {
                kit::value_row(ui, "Parts", "All resolved");
            } else {
                kit::value_row(ui, "Parts", &format!("{unresolved} unresolved"));
                resolve = kit::summary_row(ui, "", Some("Resolve parts…"));
            }
        });
        if resolve {
            self.palette.close();
            self.resolve.open();
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
