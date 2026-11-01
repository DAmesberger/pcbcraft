//! The Mapping tab's Properties card: one Package terminals table, each
//! row a package terminal's designation and the electrical terminal it
//! carries. Both cells edit the working copy, which commits once the
//! field is left (one `library.edit`, one undo step). A terminal is chosen
//! explicitly, never inferred from a matching name.
use super::PartEditor;
use crate::kit;
use egui::Ui;
use pcbcraft_core::TerminalId;
use pcbcraft_core::library::TerminalMap;

/// The table's columns.
const COLUMNS: [kit::TableColumn<'static>; 2] = [
    kit::TableColumn {
        title: "Designation",
        weight: 1.0,
    },
    kit::TableColumn {
        title: "Terminal",
        weight: 1.2,
    },
];

impl PartEditor {
    pub(super) fn mapping_card(&mut self, ui: &mut Ui) {
        let Some(set) = &mut self.draft else {
            return;
        };
        let options: Vec<(Option<TerminalId>, String)> =
            std::iter::once((None, "Unmapped".to_owned()))
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
        let mut remapped = Vec::new();
        kit::section(
            ui,
            "mapping",
            "Package terminals",
            true,
            Some(count),
            |ui| {
                kit::table(
                    ui,
                    &COLUMNS,
                    terminals.len(),
                    |ui, row, column| {
                        let Some(physical) = terminals.get_mut(row) else {
                            return;
                        };
                        ui.push_id(("package-terminal", physical.id.0), |ui| match column {
                            0 => {
                                let blank = physical.designation.trim().is_empty();
                                kit::text_field(
                                    ui,
                                    "designation",
                                    &mut physical.designation,
                                    "",
                                    blank,
                                )
                                .on_hover_text("Package terminal designation");
                            }
                            _ => {
                                let before = binding.terminal_of(physical.id);
                                let mut mapped = before;
                                kit::dropdown(ui, "electrical", &mut mapped, &options);
                                if mapped != before {
                                    remapped.push((physical.id, mapped));
                                }
                            }
                        });
                    },
                    None,
                );
                kit::hint(
                    ui,
                    "Mapped explicitly, never by name",
                    "Matching names do not establish electrical compatibility, so every package terminal is mapped explicitly",
                );
            },
        );
        for (physical, mapped) in remapped {
            binding.mappings.retain(|m| m.package_terminal != physical);
            if let Some(terminal) = mapped {
                binding.mappings.push(TerminalMap {
                    package_terminal: physical,
                    terminal,
                });
            }
        }
    }
}

impl PartEditor {
    /// `Locate` target of the Mapping tab: a reference to the binding, or
    /// to a package terminal without a footprint object beside it, shows
    /// the Mapping tab with that terminal's electrical terminal selected.
    /// Selection only. Returns whether it showed anything.
    pub(super) fn locate_mapping(&mut self, refs: &[pcbcraft_core::ObjectRef]) -> bool {
        use pcbcraft_core::ObjectRef;
        let Some(set) = &self.draft else {
            return false;
        };
        let ids = &set.ids;
        let mut shown = false;
        let mut electrical = None;
        for object in refs {
            match object {
                ObjectRef::Definition { id } if *id == ids.binding => shown = true,
                ObjectRef::LibraryObject { definition, local } if *definition == ids.binding => {
                    shown = true;
                    if set.component.terminal(TerminalId(*local)).is_some() {
                        electrical = electrical.or(Some(TerminalId(*local)));
                    }
                }
                ObjectRef::LibraryObject { definition, local } if *definition == ids.package => {
                    let physical = pcbcraft_core::PackageTerminalId(*local);
                    if set.package.terminal(physical).is_some() {
                        shown = true;
                        electrical = electrical.or(set.binding.terminal_of(physical));
                    }
                }
                _ => {}
            }
        }
        if !shown {
            return false;
        }
        if electrical.is_some() {
            self.selected = electrical;
        }
        self.view = super::View::Mapping;
        true
    }
}
