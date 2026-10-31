//! The Mapping tab's Properties card: each package terminal's designation
//! and the electrical terminal it carries.
use super::PartEditor;
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::TerminalMap;
use solidcraft_ui_egui::theme::Tokens;

impl PartEditor {
    pub(super) fn mapping_card(&mut self, ui: &mut Ui) {
        let Some(set) = &mut self.draft else {
            return;
        };
        let options: Vec<_> = std::iter::once((None, "Unmapped".to_owned()))
            .chain(
                set.component
                    .terminals
                    .iter()
                    .map(|t| (Some(t.id), t.name.clone())),
            )
            .collect();
        let count = set.package.terminals.len();
        let binding = &mut set.binding;
        let terminals = &mut set.package.terminals;
        kit::section(
            ui,
            "mapping",
            "Package terminals",
            true,
            Some(count),
            |ui| {
                kit::note(
                    ui,
                    "Designation, then the electrical terminal it carries. Matching names do not establish compatibility.",
                );
                for physical in terminals.iter_mut() {
                    let mut mapped = binding.terminal_of(physical.id);
                    let before = mapped;
                    ui.push_id(("package-terminal", physical.id.0), |ui| {
                        kit::row(ui, &format!("Package {}", physical.id), |ui| {
                            let t = Tokens::get();
                            let half = ((ui.available_width() - t.space_4) / 2.0).max(0.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(half, t.control_height),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().text_edit_width = half;
                                    kit::text_field(
                                        ui,
                                        "designation",
                                        &mut physical.designation,
                                        "",
                                        false,
                                    )
                                    .on_hover_text("Package terminal designation");
                                },
                            );
                            ui.allocate_ui_with_layout(
                                egui::vec2(half, t.control_height),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| kit::dropdown(ui, "electrical", &mut mapped, &options),
                            );
                        });
                    });
                    if mapped != before {
                        binding
                            .mappings
                            .retain(|m| m.package_terminal != physical.id);
                        if let Some(terminal) = mapped {
                            binding.mappings.push(TerminalMap {
                                package_terminal: physical.id,
                                terminal,
                            });
                        }
                    }
                }
            },
        );
    }
}
