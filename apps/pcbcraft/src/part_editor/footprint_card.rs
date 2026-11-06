//! The Footprint tab's subject (docs/sidebar-ia.md, "Properties card by
//! subject"): exactly one of the template or Assign dialog, the armed
//! tool's card, the selected pad's card, or, with nothing selected, the
//! footprint card (Footprint, Mask and paste, Parameters, Terminals).
use super::PartEditor;
use super::footprint_props::{current_pad, first_pad_of};
use crate::kit::{self, card::Subject};
use egui::Ui;
use pcbcraft_core::Operation;

impl PartEditor {
    /// What the Footprint tab's subject slot holds now.
    pub(super) fn footprint_subject(&self) -> Subject {
        if self.template_picker.is_open() || self.template_picker.is_assigning() {
            Subject::Dialog
        } else if self.footprint_tool_armed().is_some() {
            Subject::Tool
        } else {
            Subject::Properties
        }
    }

    /// The header of the Footprint tab's Properties card: the selected pad,
    /// or the component's footprint. Dialogs and tool cards draw their own.
    pub(super) fn footprint_header(&self) -> Option<crate::shell::Header> {
        use crate::shell::Header;
        let set = self.draft.as_ref()?;
        let pad = self
            .inspected_pad()
            .and_then(|id| set.footprint.pad(id))
            .map(|pad| {
                let number = if pad.number.is_empty() {
                    "—"
                } else {
                    pad.number.as_str()
                };
                Header::new("rect", format!("Pad {number}"), "Pad")
            });
        Some(pad.unwrap_or_else(|| {
            Header::new("component", set.metas.component.name.clone(), "Footprint")
        }))
    }

    /// The pad the card inspects: the selected one while it exists.
    pub(super) fn inspected_pad(&self) -> Option<pcbcraft_core::PadId> {
        let set = self.draft.as_ref()?;
        current_pad(
            &set.footprint,
            &set.binding,
            self.selected_pad,
            self.selected,
        )
    }

    /// The Footprint tab's Properties card body; its dialogs and tool card
    /// fill the slot instead (`PartEditor::slot_subject`).
    pub(super) fn footprint_card(&mut self, ui: &mut Ui) -> Option<Operation> {
        self.post_footprint_refusals(ui.ctx());
        match self.inspected_pad() {
            Some(_) => self.pad_sections(ui),
            None => self.footprint_overview(ui),
        }
        None
    }

    /// Nothing selected: the footprint's own sections.
    fn footprint_overview(&mut self, ui: &mut Ui) {
        self.footprint_section(ui);
        self.footprint_parameters(ui);
        self.footprint_terminals(ui);
    }

    /// The Terminals list: each terminal with the pad that lands it. A
    /// row selects its pad (or only the terminal when no pad lands it);
    /// pads without a package terminal offer Assign by number.
    fn footprint_terminals(&mut self, ui: &mut Ui) {
        let Some(set) = &self.draft else {
            return;
        };
        let rows: Vec<(pcbcraft_core::TerminalId, String, String)> = set
            .component
            .terminals
            .iter()
            .map(|t| {
                let pads: Vec<&str> = set
                    .footprint
                    .pads
                    .iter()
                    .filter(|p| {
                        p.terminal
                            .and_then(|pt| set.binding.terminal_of(pt))
                            .is_some_and(|e| e == t.id)
                    })
                    .map(|p| p.number.as_str())
                    .collect();
                let secondary = match pads.as_slice() {
                    [] => "no pad".to_owned(),
                    [one] => format!("pad {one}"),
                    many => format!("pads {}", many.join(", ")),
                };
                (t.id, t.name.clone(), secondary)
            })
            .collect();
        let open = super::footprint_template_picker::unassigned(&set.footprint);
        let lit = self.selected;
        let mut clicked = None;
        let mut assign = false;
        let show_all_id = ui.scope_id().with("footprint-terminals-all");
        let show_all = ui
            .data(|d| d.get_temp::<bool>(show_all_id))
            .unwrap_or(false);
        kit::section(ui, "terminals", "Terminals", true, Some(rows.len()), |ui| {
            let max = if show_all {
                rows.len()
            } else {
                kit::LIST_MAX_ROWS
            };
            let list = kit::list_frame(ui, rows.len(), max, |ui, index| {
                let Some((id, name, secondary)) = rows.get(index) else {
                    return;
                };
                let row = kit::Row {
                    icon: "point",
                    label: name,
                    secondary,
                    selected: lit == Some(*id),
                    tip: "Select this terminal's pad",
                    ..Default::default()
                };
                if kit::tree_row(ui, ui.scope_id().with(("terminal", id.0)), &row).clicked {
                    clicked = Some(*id);
                }
            });
            if list.show_all {
                ui.data_mut(|d| d.insert_temp(show_all_id, true));
            }
            if open > 0 {
                let summary = if open == 1 {
                    "1 pad unassigned".to_owned()
                } else {
                    format!("{open} pads unassigned")
                };
                assign = kit::summary_row(ui, &summary, Some("Assign by number…"));
            }
        });
        if let Some(terminal) = clicked
            && let Some(set) = &self.draft
        {
            self.selected = Some(terminal);
            self.selected_pad = first_pad_of(&set.footprint, &set.binding, self.selected);
        }
        if assign {
            self.open_assign_dialog();
        }
    }
}

impl PartEditor {
    /// `Locate` target of the Footprint tab: select the first pad, or
    /// outline the graphic, of the footprint draft that `refs` names, and
    /// frame the footprint; a reference to the footprint itself shows the
    /// footprint card. Selection and camera only. Returns whether it showed
    /// anything.
    pub(super) fn locate_footprint(&mut self, refs: &[pcbcraft_core::ObjectRef]) -> bool {
        use pcbcraft_core::ObjectRef;
        let Some(set) = &self.draft else {
            return false;
        };
        let footprint = set.ids.footprint;
        let mut pad = None;
        let mut graphic = None;
        let mut itself = false;
        for object in refs {
            match object {
                ObjectRef::LibraryObject { definition, local } if *definition == footprint => {
                    if let Some(found) = set.footprint.pads.iter().find(|p| p.id.0 == *local) {
                        pad = pad.or(Some(found));
                    } else if set.footprint.graphics.iter().any(|g| g.id.0 == *local) {
                        graphic = graphic.or(Some(pcbcraft_core::GraphicId(*local)));
                    }
                }
                ObjectRef::Definition { id } if *id == footprint => itself = true,
                _ => {}
            }
        }
        if pad.is_none() && graphic.is_none() && !itself {
            return false;
        }
        let electrical = pad
            .and_then(|p| p.terminal)
            .and_then(|t| set.binding.terminal_of(t));
        self.selected_pad = pad.map(|p| p.id);
        if let Some(electrical) = electrical {
            self.selected = Some(electrical);
        }
        self.finding_graphic = graphic;
        self.view = super::View::Footprint;
        // Framed: the whole footprint, so the located pad is in view.
        self.footprint_view = None;
        true
    }
}
