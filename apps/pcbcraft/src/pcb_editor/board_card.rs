//! The Board card (size, copper layers and the finish summary), its Board
//! setup dialog, and the view popover's layer and display toggles
//! (docs/sidebar-ia.md, "Properties card by subject").
use super::PcbEditor;
use crate::kit::{
    self,
    dialog::{DialogAction, DialogPanel},
};
use crate::refusal::{self, Origin};
use egui::Ui;
use pcbcraft_core::{
    Engine, Fabrication, MaskColour, MaskFinish, Operation, PadFinish, Project, SilkscreenColour,
    layout::LayoutCommand,
};
use solidcraft_ui_egui::theme::Tokens;

/// The Board setup dialog's state: the fabrication choices being edited,
/// and the revision its OK was sent at.
pub(super) struct BoardSetup {
    fabrication: Fabrication,
    /// The revision OK sent its operation at; the dialog closes once the
    /// document has moved on, and stays open with the refusal otherwise.
    sent: Option<u64>,
    refusal: Option<String>,
    /// True until the dialog has drawn once (its body starts at the top).
    opening: bool,
}

impl BoardSetup {
    fn new(fabrication: Fabrication) -> Self {
        Self {
            fabrication,
            sent: None,
            refusal: None,
            opening: true,
        }
    }

    fn id() -> egui::Id {
        egui::Id::new("pcb-board-setup")
    }

    /// Take a refusal of the dialog's OK, handed back this frame.
    pub(super) fn claim(&mut self, ctx: &egui::Context) {
        if let Some(text) = refusal::claim(ctx, Origin::Dialog(Self::id())) {
            self.refusal = Some(text);
            self.sent = None;
        }
    }
}

impl PcbEditor {
    /// BOARD: size and copper layers, then the finish in one line with
    /// Setup…, which opens the Board setup dialog.
    pub(super) fn board_section(&mut self, ui: &mut Ui, project: &Project) {
        let mut setup = false;
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
            kit::row(ui, "Finish", |ui| {
                setup = kit::summary_row(ui, &finish_summary(&project.fabrication), Some("Setup…"));
            });
        });
        if setup {
            self.open_board_setup(project);
        }
    }

    pub(super) fn open_board_setup(&mut self, project: &Project) {
        self.setup = Some(BoardSetup::new(project.fabrication));
    }

    /// The docked Board setup dialog: soldermask, mask finish, pad finish
    /// and silkscreen, committed together by OK as one
    /// `layout.set_fabrication` step.
    pub(super) fn board_setup(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let project = engine.project();
        let setup = self.setup.as_mut()?;
        if setup.sent.is_some_and(|sent| sent != engine.revision()) {
            self.setup = None;
            return None;
        }
        let mut dialog = DialogPanel::new(BoardSetup::id(), "plane", "Board setup", "OK");
        dialog.kind = "Board";
        dialog.body_rows = 6.0;
        dialog.reset_scroll = setup.opening;
        dialog.refusal = setup.refusal.as_deref();
        setup.opening = false;
        let mut fabrication = setup.fabrication;
        let action = dialog.show(ui, |ui| {
            let f = &mut fabrication;
            kit::choice_row(ui, "Soldermask", &mut f.mask_colour, &mask_colour_options());
            kit::choice_row(
                ui,
                "Mask finish",
                &mut f.mask_finish,
                &mask_finish_options(),
            );
            kit::choice_row(ui, "Pad finish", &mut f.pad_finish, &pad_finish_options());
            kit::choice_row(
                ui,
                "Silkscreen",
                &mut f.silkscreen_colour,
                &silkscreen_options(),
            );
            kit::hint(
                ui,
                "Silkscreen colour shows in Layout only",
                "Saved with the design for fabrication. The 3D view shows the mask and pad \
                 finish; silkscreen colour is drawn in 2D Layout only for now.",
            );
        });
        setup.fabrication = fabrication;
        match action {
            Some(DialogAction::Cancel) => {
                self.setup = None;
                None
            }
            Some(DialogAction::Primary) => match fabrication_edit(project.fabrication, fabrication)
            {
                Some(operation) => {
                    setup.refusal = None;
                    setup.sent = Some(engine.revision());
                    refusal::mark(ui.ctx(), Origin::Dialog(BoardSetup::id()));
                    Some(operation)
                }
                None => {
                    self.setup = None;
                    None
                }
            },
            None => None,
        }
    }

    /// Whether the navigation bar offers the view popover: Layout only,
    /// since these layers and overlays do not drive the 3D scene.
    pub fn has_view_menu(&self) -> bool {
        !self.view_3d
    }

    /// The view popover: layer visibility, the side the board is seen
    /// from, and the net highlight and airwire overlays. Display state
    /// only; nothing here changes the document or rebuilds the board.
    pub fn view_menu(&mut self, ui: &mut Ui) {
        kit::form(ui, |ui| {
            ui.set_width(Tokens::get().form_label_width * 3.2);
            kit::section(ui, "pcb-view-layers", "Layers", true, None, |ui| {
                let l = &mut self.layers;
                kit::check_row(ui, "Top copper", &mut l.copper_top);
                kit::check_row(ui, "Bottom copper", &mut l.copper_bottom);
                kit::check_row(ui, "Top silkscreen", &mut l.silk_top);
                kit::check_row(ui, "Bottom silkscreen", &mut l.silk_bottom);
                kit::check_row(ui, "Courtyard", &mut l.courtyard);
                kit::check_row(ui, "Assembly", &mut l.assembly);
                kit::check_row(ui, "Holes", &mut l.holes);
                kit::hint(ui, "Hidden layers are not picked", "");
            });
            kit::section(ui, "pcb-view-display", "View", true, None, |ui| {
                kit::check_row(ui, "View from bottom", &mut self.from_bottom);
                let mut highlight = self.highlighted.is_some();
                let net = self.net;
                ui.add_enabled_ui(net.is_some() || highlight, |ui| {
                    if kit::check_row(ui, "Highlight complete net", &mut highlight) {
                        self.highlighted = if highlight { net } else { None };
                    }
                })
                .response
                .on_disabled_hover_text("Select a pad or track to choose its net");
                kit::check_row(ui, "Show airwires", &mut self.show_airwires);
            });
        });
    }
}

/// "Green · HASL · White silk": the finish in one line.
pub(super) fn finish_summary(f: &Fabrication) -> String {
    let label = |options: &[(MaskColour, String)], v: MaskColour| {
        options
            .iter()
            .find(|(o, _)| *o == v)
            .map_or_else(String::new, |(_, l)| l.clone())
    };
    let pad = pad_finish_options()
        .into_iter()
        .find(|(o, _)| *o == f.pad_finish)
        .map_or_else(String::new, |(_, l)| l);
    let silk = silkscreen_options()
        .into_iter()
        .find(|(o, _)| *o == f.silkscreen_colour)
        .map_or_else(String::new, |(_, l)| l);
    format!(
        "{} · {pad} · {silk} silk",
        label(&mask_colour_options(), f.mask_colour)
    )
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
