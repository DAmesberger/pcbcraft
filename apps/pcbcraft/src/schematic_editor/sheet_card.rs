//! The sheet-level Properties card: the Sheet summary, the Place tool's
//! component and the Checks section.
use super::{Locate, SchematicEditor, Tool, locate};
use crate::kit::{self, Tone};
use pcbcraft_core::{Id, Project, instances};
use solidcraft_ui_egui::ui_kit::{self, Button};

impl SchematicEditor {
    /// The component the Place tool puts down, picked in the palette.
    pub(super) fn place_section(&mut self, ui: &mut egui::Ui, project: &Project) {
        kit::section(ui, "place", "Place", true, None, |ui| {
            let armed = self
                .component
                .and_then(|c| project.library.component(c).map(|p| (c, p)));
            match armed {
                Some((component, published)) => {
                    kit::value_row(ui, "Component", &published.meta.name);
                    kit::value_row(ui, "Revision", &component.revision.to_string());
                }
                None => kit::value_row(ui, "Component", "None"),
            }
            kit::note(
                ui,
                if project.library.components.is_empty() {
                    "Publish a component in the Component workspace, then place it here."
                } else {
                    "Click the sheet to place the component. Esc returns to Select."
                },
            );
            kit::actions(ui, |ui| {
                if ui_kit::button(
                    ui,
                    "Find part…",
                    Button::Enabled(!project.library.components.is_empty()),
                )
                .on_hover_text("Search the project's components (P)")
                .clicked()
                {
                    self.palette.open();
                }
            });
        });
    }

    pub(super) fn sheet_section(&mut self, ui: &mut egui::Ui, project: &Project) {
        kit::section(ui, "sheet", "Sheet", true, None, |ui| {
            kit::value_row(ui, "Instances", &project.instances.len().to_string());
            kit::value_row(ui, "Wires", &project.wires.len().to_string());
            let connected = self.nets.iter().filter(|n| n.terminals.len() >= 2).count();
            ui.scope(|ui| kit::value_row(ui, "Nets", &connected.to_string()))
                .response
                .on_hover_text("Nets that join two or more terminals");
            kit::note(ui, self.tool_hint());
            kit::note(
                ui,
                "Crossing wires are not joined. Connect terminals explicitly.",
            );
        });
    }

    fn tool_hint(&self) -> &'static str {
        match self.tool {
            Tool::Select => "Click to select. Drag a symbol to move.",
            Tool::Place => "Click the sheet to place the selected component.",
            Tool::Wire => {
                if self.wire_from.is_some() {
                    "Click a second terminal. Esc cancels."
                } else {
                    "Click a terminal to start a wire."
                }
            }
        }
    }

    /// The Checks section: electrical checks and purchasing findings,
    /// each row selecting what it names.
    pub(super) fn checks_section(&mut self, ui: &mut egui::Ui, project: &Project) {
        if self.show_checks {
            // Purchasing findings (advisory) join the electrical checks.
            let purchasing: Vec<(Id, String)> = project
                .instances
                .iter()
                .flat_map(|i| {
                    instances::part_findings(&project.library, i)
                        .into_iter()
                        .map(|f| (i.id, f.message))
                })
                .collect();
            let mut target = None;
            kit::section(
                ui,
                "checks",
                "Checks",
                true,
                Some(self.issues.len() + purchasing.len()),
                |ui| {
                    if self.issues.is_empty() && purchasing.is_empty() {
                        kit::note(ui, "No issues in the supported check subset.");
                    }
                    for issue in &self.issues {
                        let locates = locate(&issue.objects);
                        let hint = locates.map(|l| match l {
                            Locate::Instance(_) => "Select the instance on the sheet",
                            Locate::Net(_) => "Select the net on the sheet",
                        });
                        if kit::finding_row(ui, &issue.message, Tone::Error, hint).clicked() {
                            target = target.or(locates);
                        }
                    }
                    for (instance, message) in &purchasing {
                        let hint = Some("Select the instance on the sheet");
                        if kit::finding_row(ui, message, Tone::Warning, hint).clicked() {
                            target = target.or(Some(Locate::Instance(*instance)));
                        }
                    }
                    kit::note(
                        ui,
                        "Checks: undriven power inputs and multiple outputs. Unconnected \
                     terminals are completeness findings of each instance.",
                    );
                },
            );
            match target {
                Some(Locate::Instance(id)) => {
                    self.select(Some(id), None, project);
                    self.tool = Tool::Select;
                }
                Some(Locate::Net(net)) => self.select_net(net, project),
                None => {}
            }
        }
    }
}
