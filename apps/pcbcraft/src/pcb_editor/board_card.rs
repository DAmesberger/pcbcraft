//! The board-level Properties card: Board size and fabrication choices,
//! layer visibility and the Board checks list.
use super::{PcbEditor, Selection, locate};
use crate::kit::{self, Tone};
use egui::Ui;
use pcbcraft_core::{
    Fabrication, MaskColour, MaskFinish, Operation, PadFinish, Project, SilkscreenColour,
    layout::LayoutCommand,
};

/// Board-check findings listed in the card; the rest are counted.
const MAX_LISTED_ISSUES: usize = 40;

impl PcbEditor {
    /// Layer visibility and the board side Layout is seen from. Display
    /// state only: nothing here changes the document.
    pub(super) fn layers_section(&mut self, ui: &mut Ui) {
        kit::section(ui, "pcb-layers", "Layers", true, None, |ui| {
            let l = &mut self.layers;
            kit::check_row(ui, "Top copper", &mut l.copper_top);
            kit::check_row(ui, "Bottom copper", &mut l.copper_bottom);
            kit::check_row(ui, "Top silkscreen", &mut l.silk_top);
            kit::check_row(ui, "Bottom silkscreen", &mut l.silk_bottom);
            kit::check_row(ui, "Courtyard", &mut l.courtyard);
            kit::check_row(ui, "Assembly", &mut l.assembly);
            kit::check_row(ui, "Holes", &mut l.holes);
            kit::check_row(ui, "View from bottom", &mut self.from_bottom);
            kit::note(ui, "Hidden layers are not picked.");
        });
    }

    pub(super) fn checks_section(&mut self, ui: &mut Ui) {
        let mut select = None;
        self.checks_list(ui, &mut select);
        if let Some((instance, track, net)) = select {
            self.set_selection(instance, track, net);
        }
    }

    /// The Board checks list. Each row naming a part, pad, track or net is
    /// a button that selects the first such object.
    fn checks_list(&self, ui: &mut Ui, select: &mut Option<Selection>) {
        let count = self.issues.len();
        kit::section(
            ui,
            "pcb-checks",
            "Board checks",
            count > 0,
            Some(count),
            |ui| {
                if self.issues.is_empty() {
                    kit::status(ui, "No issues found by these checks.", Tone::Dim);
                }
                for issue in self.issues.iter().take(MAX_LISTED_ISSUES) {
                    let target = locate(&issue.objects);
                    let hint = target.map(|(instance, track, _)| match (instance, track) {
                        (Some(_), _) => "Select the part on the board",
                        (None, Some(_)) => "Select the track on the board",
                        (None, None) => "Select the net on the board",
                    });
                    let response = kit::finding_row(ui, &issue.message, Tone::Error, hint);
                    if target.is_some() && response.clicked() {
                        *select = target;
                    }
                }
                if count > MAX_LISTED_ISSUES {
                    kit::note(ui, &format!("{} more issues", count - MAX_LISTED_ISSUES));
                }
                kit::note(
                    ui,
                    "Connectivity, outline and 0.2 mm copper clearance only; not a full manufacturing DRC.",
                );
            },
        );
    }
}

/// Board size and layers, and the fabrication choices. Choosing another
/// option issues `layout.set_fabrication` with the other attributes kept.
pub(super) fn board_section(ui: &mut Ui, project: &Project) -> Option<Operation> {
    let mut fabrication = project.fabrication;
    kit::section(ui, "pcb-board", "Board", true, None, |ui| {
        kit::value_row(
            ui,
            "Size",
            &format!(
                "{} × {} mm",
                kit::number_text(project.board_size[0]),
                kit::number_text(project.board_size[1])
            ),
        );
        kit::value_row(ui, "Copper", "2 layers");
        kit::choice_row(
            ui,
            "Soldermask",
            &mut fabrication.mask_colour,
            &mask_colour_options(),
        );
        kit::choice_row(
            ui,
            "Mask finish",
            &mut fabrication.mask_finish,
            &mask_finish_options(),
        );
        kit::choice_row(
            ui,
            "Pad finish",
            &mut fabrication.pad_finish,
            &pad_finish_options(),
        );
        kit::choice_row(
            ui,
            "Silkscreen",
            &mut fabrication.silkscreen_colour,
            &silkscreen_options(),
        );
        kit::note(
            ui,
            "Saved with the design for fabrication. The 3D view shows mask and pad finish; silkscreen is drawn in 2D Layout only for now.",
        );
    });
    fabrication_edit(project.fabrication, fabrication)
}
/// The operation a changed fabrication form issues; `None` when unchanged.
pub(super) fn fabrication_edit(current: Fabrication, edited: Fabrication) -> Option<Operation> {
    (current != edited).then_some(Operation::Layout(LayoutCommand::SetFabrication(edited)))
}
fn mask_colour_options() -> [(MaskColour, String); 8] {
    [
        (MaskColour::Green, "Green"),
        (MaskColour::Blue, "Blue"),
        (MaskColour::Red, "Red"),
        (MaskColour::Purple, "Purple"),
        (MaskColour::Yellow, "Yellow"),
        (MaskColour::Black, "Black"),
        (MaskColour::White, "White"),
        (MaskColour::Clear, "Clear"),
    ]
    .map(|(value, label)| (value, label.to_owned()))
}
fn mask_finish_options() -> [(MaskFinish, String); 3] {
    [
        (MaskFinish::Gloss, "Gloss"),
        (MaskFinish::Satin, "Satin"),
        (MaskFinish::Matte, "Matte"),
    ]
    .map(|(value, label)| (value, label.to_owned()))
}
fn pad_finish_options() -> [(PadFinish, String); 5] {
    [
        (PadFinish::Enig, "ENIG"),
        (PadFinish::Hasl, "HASL (tin-lead)"),
        (PadFinish::LeadFreeHasl, "Lead-free HASL"),
        (PadFinish::Osp, "OSP"),
        (PadFinish::ImmersionSilver, "Immersion silver"),
    ]
    .map(|(value, label)| (value, label.to_owned()))
}
fn silkscreen_options() -> [(SilkscreenColour, String); 3] {
    [
        (SilkscreenColour::White, "White"),
        (SilkscreenColour::Black, "Black"),
        (SilkscreenColour::Yellow, "Yellow"),
    ]
    .map(|(value, label)| (value, label.to_owned()))
}
