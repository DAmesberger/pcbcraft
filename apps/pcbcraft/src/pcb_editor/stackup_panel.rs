//! The Stackup dialog (Board card › Copper › Stackup…): the board's layer
//! sequence top to bottom as one table of copper foils and the dielectrics
//! between them, with a preset picker, add and remove, reordering and the
//! total thickness. It edits a [`StackupDraft`] and OK sends the one
//! `layout.edit_stackup` that applies it, one undo step; the core's rules
//! (`Stackup::check`) say why a draft cannot be applied, and the engine's
//! refusal (copper on a removed layer) shows as the dialog's last row.
use super::PcbEditor;
use crate::kit::{
    self,
    dialog::{DialogAction, DialogPanel},
};
use crate::refusal::{self, Origin};
use egui::Ui;
use pcbcraft_core::{
    COPPER_UM_RANGE, CopperLayer, DIELECTRIC_MM_RANGE, DK_RANGE, DielectricKind, Engine,
    GlassStyle, OUNCE_UM, Operation, Project, Stackup,
    layout::{
        LayoutCommand,
        restack::{DraftRow, StackupDraft},
    },
    model::fill::KeepoutLayers,
};
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::collections::BTreeMap;

/// The Stackup dialog's state: the draft, where removed layers' copper
/// goes, and the revision its OK was sent at.
pub(super) struct StackupSetup {
    draft: StackupDraft,
    /// A removed layer's copper → the layer of the new stackup it moves to.
    dropped: BTreeMap<CopperLayer, CopperLayer>,
    /// The table row the editor below it edits.
    selected: Option<DraftRow>,
    sent: Option<u64>,
    refusal: Option<String>,
    opening: bool,
}

impl StackupSetup {
    pub(super) fn new(stackup: &Stackup) -> Self {
        Self {
            draft: StackupDraft::of(stackup),
            dropped: BTreeMap::new(),
            selected: None,
            sent: None,
            refusal: None,
            opening: true,
        }
    }

    fn id() -> egui::Id {
        egui::Id::unique("pcb-stackup")
    }

    /// Take a refusal of the dialog's OK, handed back this frame.
    pub(super) fn claim(&mut self, ctx: &egui::Context) {
        if let Some(text) = refusal::claim(ctx, Origin::Dialog(Self::id())) {
            self.refusal = Some(text);
            self.sent = None;
        }
    }

    /// The operation OK sends; `None` when the draft changes nothing.
    pub(super) fn operation(&self) -> Option<Operation> {
        self.draft
            .changed()
            .then(|| Operation::Layout(LayoutCommand::EditStackup(self.draft.edit(&self.dropped))))
    }
}

/// One edit a table row asks for, applied after the table is drawn.
enum RowEdit {
    Move(DraftRow, bool),
    Remove(usize),
}

/// "4 layers · 1.67 mm": the stackup in one line.
pub(super) fn stackup_summary(stackup: &Stackup) -> String {
    format!(
        "{} layers · {} mm",
        stackup.copper.len(),
        kit::number_text((stackup.thickness_mm() * 1000.0).round() / 1000.0)
    )
}

impl PcbEditor {
    pub(super) fn open_stackup(&mut self, project: &Project) {
        self.net_classes = None;
        self.signals = None;
        self.rules = None;
        self.setup = None;
        self.stackup = Some(StackupSetup::new(&project.stackup));
    }

    /// The docked Stackup dialog.
    pub(super) fn stackup_setup(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let project = engine.project();
        let setup = self.stackup.as_mut()?;
        if setup.sent.is_some_and(|sent| sent != engine.revision()) {
            self.stackup = None;
            return None;
        }
        let rule = setup
            .draft
            .stackup
            .check()
            .err()
            .map(|e| refusal::shown_error(&e));
        let mut dialog = DialogPanel::new(StackupSetup::id(), "plane", "Stackup", "OK");
        dialog.kind = "Board";
        dialog.body_rows = 28.0;
        dialog.reset_scroll = setup.opening;
        let refused = setup.refusal.clone();
        dialog.refusal = refused.as_deref();
        dialog.primary_disabled = rule.as_deref();
        dialog.status = rule.as_deref();
        setup.opening = false;
        let carried = carried_layers(project);
        let action = dialog.show(ui, |ui| body(ui, setup, &carried));
        match action {
            Some(DialogAction::Cancel) => {
                self.stackup = None;
                None
            }
            Some(DialogAction::Primary) if rule.is_none() => match setup.operation() {
                Some(operation) => {
                    setup.refusal = None;
                    setup.sent = Some(engine.revision());
                    refusal::mark(ui.ctx(), Origin::Dialog(StackupSetup::id()));
                    Some(operation)
                }
                None => {
                    self.stackup = None;
                    None
                }
            },
            _ => None,
        }
    }
}

/// The dialog's body: preset, total, the layer table, the add buttons and
/// a row per removed layer that carries copper.
fn body(ui: &mut Ui, setup: &mut StackupSetup, carried: &[CopperLayer]) {
    let presets: Vec<(Option<usize>, String)> = std::iter::once((None, "Custom".to_owned()))
        .chain(Stackup::preset_counts().map(|n| (Some(n), format!("{n} layers"))))
        .collect();
    let mut preset = Stackup::preset_counts().find(|n| {
        Stackup::preset(*n).is_ok_and(|p| {
            p.copper == setup.draft.stackup.copper
                && p.dielectrics == setup.draft.stackup.dielectrics
        })
    });
    if kit::choice_row(ui, "Preset", &mut preset, &presets)
        && let Some(count) = preset
        && setup.draft.use_preset(count).is_ok()
    {
        setup.refusal = None;
    }
    kit::value_row(
        ui,
        "Total",
        &format!(
            "{} mm over copper",
            kit::number_text((setup.draft.stackup.thickness_mm() * 1000.0).round() / 1000.0)
        ),
    );
    if let Some(row) = table(ui, &setup.draft, setup.selected) {
        setup.selected = Some(row);
    }
    let edit = match setup.selected {
        Some(row) => {
            ui.push_id(("stackup-row", row_key(row)), |ui| {
                row_editor(ui, &mut setup.draft, row)
            })
            .inner
        }
        None => {
            kit::hint(ui, "Select a row to edit it", "");
            None
        }
    };
    if let Some(edit) = edit {
        let done = match edit {
            RowEdit::Move(row, up) => setup.draft.move_row(row, up).map(|()| {
                let step = |i: usize| if up { i.saturating_sub(1) } else { i + 1 };
                Some(match row {
                    DraftRow::Foil(i) => DraftRow::Foil(step(i)),
                    DraftRow::Dielectric(i) => DraftRow::Dielectric(step(i)),
                })
            }),
            RowEdit::Remove(index) => setup.draft.remove_layer(index).map(|()| None),
        };
        if let Ok(selected) = done {
            setup.selected = selected;
            setup.refusal = None;
        }
    }
    kit::actions(ui, |ui| {
        let full = setup.draft.stackup.copper.len() >= pcbcraft_core::MAX_COPPER_LAYERS;
        let pair = ui_kit::button(ui, "Add pair", Button::Enabled(!full))
            .on_hover_text("A prepreg and a core with two inner foils, above the bottom");
        if pair.clicked() && setup.draft.add_pair().is_ok() {
            setup.refusal = None;
        }
        for (kind, label) in [
            (DielectricKind::Core, "Add core"),
            (DielectricKind::Prepreg, "Add prepreg"),
        ] {
            let add = ui_kit::button(ui, label, Button::Enabled(!full))
                .on_hover_text("One inner foil on it, above the bottom");
            if add.clicked() && setup.draft.add_layer(kind).is_ok() {
                setup.refusal = None;
            }
        }
    });
    // The selection may name a row the draft no longer has.
    let foils = setup.draft.stackup.copper.len();
    if setup.selected.is_some_and(|row| match row {
        DraftRow::Foil(i) => i >= foils,
        DraftRow::Dielectric(i) => i + 1 >= foils,
    }) {
        setup.selected = None;
    }
    // A removed layer that carries copper: where its copper goes. Left
    // unmoved, the engine refuses the edit and names what it carries.
    let layers = setup.draft.stackup.layers();
    for removed in setup.draft.removed() {
        if !carried.contains(&removed) {
            setup.dropped.remove(&removed);
            continue;
        }
        let mut to = setup.dropped.get(&removed).copied();
        let options: Vec<(Option<CopperLayer>, String)> =
            std::iter::once((None, "Not moved".to_owned()))
                .chain(
                    layers
                        .iter()
                        .map(|l| (Some(*l), format!("Move to {}", l.name()))),
                )
                .collect();
        if kit::choice_row(ui, &format!("{} copper", removed.name()), &mut to, &options) {
            match to {
                Some(to) => setup.dropped.insert(removed, to),
                None => setup.dropped.remove(&removed),
            };
            setup.refusal = None;
        }
    }
    setup.dropped.retain(|_, to| layers.contains(to));
}

/// A stable key of a draft row, for ids.
fn row_key(row: DraftRow) -> (bool, usize) {
    match row {
        DraftRow::Foil(i) => (true, i),
        DraftRow::Dielectric(i) => (false, i),
    }
}

/// The layer table, top to bottom: each foil, then the dielectric below
/// it, read-only (layer, thickness, use or glass and Dk). Returns the row
/// clicked, which the editor below the table then edits.
fn table(ui: &mut Ui, draft: &StackupDraft, selected: Option<DraftRow>) -> Option<DraftRow> {
    let columns = [
        // Room for the indented "Prepreg" and for "THICKNESS", what a
        // stackup is read for; uses are short words. A cut caption shows
        // whole on hover.
        kit::TableColumn {
            title: "Layer",
            weight: 1.1,
        },
        kit::TableColumn {
            title: "Thickness",
            weight: 1.05,
        },
        kit::TableColumn {
            title: "Use · glass",
            weight: 0.85,
        },
    ];
    let stackup = &draft.stackup;
    let foils = stackup.copper.len();
    let rows = foils + stackup.dielectrics.len();
    let layers = stackup.layers();
    let mut clicked = None;
    kit::table(
        ui,
        &columns,
        rows,
        |ui, row, column| {
            // Even rows are foils, odd rows the dielectric below.
            let index = row / 2;
            let this = if row % 2 == 0 {
                DraftRow::Foil(index)
            } else {
                DraftRow::Dielectric(index)
            };
            let text = match this {
                DraftRow::Foil(i) => {
                    let (Some(layer), Some(foil)) = (layers.get(i), stackup.copper.get(i)) else {
                        return;
                    };
                    match column {
                        0 => layer.name(),
                        1 => format!("{} µm", kit::number_text(foil.thickness_um)),
                        _ => if foil.plane { "Plane" } else { "Signal" }.to_owned(),
                    }
                }
                DraftRow::Dielectric(i) => {
                    let Some(d) = stackup.dielectrics.get(i) else {
                        return;
                    };
                    match column {
                        0 => format!("  {}", kind_label(d.kind)),
                        1 => format!("{} mm", kit::number_text(d.thickness_mm)),
                        _ => glass_label(d.glass).to_owned(),
                    }
                }
            };
            if ui.selectable_label(selected == Some(this), text).clicked() {
                clicked = Some(this);
            }
        },
        None,
    );
    clicked
}

fn kind_label(kind: DielectricKind) -> &'static str {
    match kind {
        DielectricKind::Core => "Core",
        DielectricKind::Prepreg => "Prepreg",
    }
}

fn glass_label(glass: GlassStyle) -> &'static str {
    match glass {
        GlassStyle::Style106 => "106",
        GlassStyle::Style1080 => "1080",
        GlassStyle::Style2116 => "2116",
        GlassStyle::Style7628 => "7628",
    }
}

/// The selected row's fields, edited in the draft, and its move and
/// remove buttons.
fn row_editor(ui: &mut Ui, draft: &mut StackupDraft, row: DraftRow) -> Option<RowEdit> {
    let foils = draft.stackup.copper.len();
    let layers = draft.stackup.layers();
    match row {
        DraftRow::Foil(i) => {
            let layer = layers.get(i).copied()?;
            let origin = draft.origin(i);
            let foil = draft.stackup.copper.get_mut(i)?;
            let ounces = kit::number_text((foil.thickness_um / OUNCE_UM * 10.0).round() / 10.0);
            let title = match origin {
                None => format!("{} · new · {ounces} oz", layer.name()),
                Some(o) if o != layer => {
                    format!("{} · was {} · {ounces} oz", layer.name(), o.name())
                }
                Some(_) => format!("{} · {ounces} oz", layer.name()),
            };
            kit::value_row(ui, "Foil", &title);
            kit::number_row(ui, "Copper", &mut foil.thickness_um, "µm", COPPER_UM_RANGE);
            let roles = [(false, "Signal".to_owned()), (true, "Plane".to_owned())];
            kit::choice_row(ui, "Use", &mut foil.plane, &roles);
            let inner = i > 0 && i + 1 < foils;
            row_buttons(ui, row, inner && i > 1, inner && i + 2 < foils, inner)
        }
        DraftRow::Dielectric(i) => {
            let count = draft.stackup.dielectrics.len();
            let d = draft.stackup.dielectrics.get_mut(i)?;
            let kinds = [DielectricKind::Core, DielectricKind::Prepreg]
                .map(|k| (k, kind_label(k).to_owned()));
            kit::choice_row(ui, "Material", &mut d.kind, &kinds);
            kit::number_row(
                ui,
                "Thickness",
                &mut d.thickness_mm,
                "mm",
                DIELECTRIC_MM_RANGE,
            );
            let glasses = [
                GlassStyle::Style106,
                GlassStyle::Style1080,
                GlassStyle::Style2116,
                GlassStyle::Style7628,
            ]
            .map(|g| (g, glass_label(g).to_owned()));
            kit::choice_row(ui, "Glass", &mut d.glass, &glasses);
            let mut dk = d.permittivity();
            if kit::number_row(ui, "Dk", &mut dk, "", DK_RANGE) {
                d.dk = Some(dk);
            }
            if d.dk.is_none() {
                kit::hint(
                    ui,
                    "Dk estimated from the glass; enter the fabricator's",
                    "",
                );
            }
            row_buttons(ui, row, i > 0, i + 1 < count, false)
        }
    }
}

/// Move up, Move down and, for an inner foil, Remove.
fn row_buttons(
    ui: &mut Ui,
    row: DraftRow,
    up: bool,
    down: bool,
    removable: bool,
) -> Option<RowEdit> {
    let mut edit = None;
    kit::actions(ui, |ui| {
        if removable
            && ui_kit::button(ui, "Remove", Button::Enabled(true))
                .on_hover_text("Remove this layer and the dielectric below it")
                .clicked()
            && let DraftRow::Foil(i) = row
        {
            edit = Some(RowEdit::Remove(i));
        }
        if ui_kit::button(ui, "Move down", Button::Enabled(down)).clicked() {
            edit = Some(RowEdit::Move(row, false));
        }
        if ui_kit::button(ui, "Move up", Button::Enabled(up)).clicked() {
            edit = Some(RowEdit::Move(row, true));
        }
    });
    edit
}

/// The copper layers that carry tracks, fills, keepouts or blind vias.
fn carried_layers(project: &Project) -> Vec<CopperLayer> {
    let mut out: Vec<CopperLayer> = project
        .tracks
        .iter()
        .map(|t| t.layer)
        .chain(project.fills.iter().map(|f| f.layer))
        .collect();
    for keepout in &project.keepouts {
        if let KeepoutLayers::Only(layers) = &keepout.layers {
            out.extend(layers.iter().copied());
        }
    }
    for via in &project.vias {
        if let pcbcraft_core::ViaSpan::Between { from, to } = via.span {
            out.extend([from, to]);
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcraft_ui_egui::fonts;

    /// At the Properties column's width (about 230 points, as measured in
    /// the app), in the dialog's form, the table shows every value and
    /// the Thickness caption whole: the 4-layer preset's "0.2 mm" is not
    /// cut to "0.2 m…", nor "Prepreg" to "Prep…". Only the last caption
    /// may be cut (it shows whole on hover).
    #[test]
    fn the_layer_table_fits_the_properties_column() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        solidcraft_theme::apply(&ctx);
        let draft = StackupDraft::of(&Stackup::preset(4)?);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(230.0, 600.0),
            )),
            ..Default::default()
        };
        let mut cut = Vec::new();
        // The second frame lays out with the fonts the first loaded.
        for _ in 0..2 {
            let mut output = ctx.run_ui(input(), |ui| {
                // In the dialog's form, with its label font.
                kit::form(ui, |ui| table(ui, &draft, None));
            });
            output.textures_delta.clear();
            cut = output
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) if t.galley.elided => Some(t.galley.job.text.clone()),
                    _ => None,
                })
                .collect();
        }
        cut.retain(|text| text != "USE · GLASS");
        assert_eq!(cut, Vec::<String>::new());
        Ok(())
    }
}
