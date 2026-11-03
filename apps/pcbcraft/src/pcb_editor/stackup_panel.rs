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
    sent: Option<u64>,
    refusal: Option<String>,
    opening: bool,
}

impl StackupSetup {
    pub(super) fn new(stackup: &Stackup) -> Self {
        Self {
            draft: StackupDraft::of(stackup),
            dropped: BTreeMap::new(),
            sent: None,
            refusal: None,
            opening: true,
        }
    }

    fn id() -> egui::Id {
        egui::Id::new("pcb-stackup")
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
        dialog.body_rows = 16.0;
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
    let stackup = &setup.draft.stackup;
    kit::value_row(
        ui,
        "Total",
        &format!(
            "{} mm over copper · {} copper layers",
            kit::number_text((stackup.thickness_mm() * 1000.0).round() / 1000.0),
            stackup.copper.len()
        ),
    );
    let edit = table(ui, &mut setup.draft);
    if let Some(edit) = edit {
        let done = match edit {
            RowEdit::Move(row, up) => setup.draft.move_row(row, up),
            RowEdit::Remove(index) => setup.draft.remove_layer(index),
        };
        if done.is_ok() {
            setup.refusal = None;
        }
    }
    kit::actions(ui, |ui| {
        let full = setup.draft.stackup.copper.len() >= pcbcraft_core::MAX_COPPER_LAYERS;
        let pair = ui_kit::button(ui, "Add layer pair", Button::Enabled(!full))
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

/// The layer table: a row per foil and per dielectric, top to bottom.
/// Returns the add, remove or move a row's buttons asked for.
fn table(ui: &mut Ui, draft: &mut StackupDraft) -> Option<RowEdit> {
    let columns = [
        kit::TableColumn {
            title: "Layer",
            weight: 1.5,
        },
        kit::TableColumn {
            title: "Material",
            weight: 1.3,
        },
        kit::TableColumn {
            title: "Thickness",
            weight: 1.3,
        },
        kit::TableColumn {
            title: "Glass · use",
            weight: 1.4,
        },
        kit::TableColumn {
            title: "Dk",
            weight: 0.8,
        },
        kit::TableColumn {
            title: "",
            weight: 1.2,
        },
    ];
    let foils = draft.stackup.copper.len();
    let rows = foils + draft.stackup.dielectrics.len();
    let layers = draft.stackup.layers();
    let origins: Vec<Option<CopperLayer>> = (0..foils).map(|i| draft.origin(i)).collect();
    let mut edit = None;
    let kinds = [
        (DielectricKind::Core, "Core".to_owned()),
        (DielectricKind::Prepreg, "Prepreg".to_owned()),
    ];
    let glasses = [
        (GlassStyle::Style106, "106".to_owned()),
        (GlassStyle::Style1080, "1080".to_owned()),
        (GlassStyle::Style2116, "2116".to_owned()),
        (GlassStyle::Style7628, "7628".to_owned()),
    ];
    let roles = [(false, "Signal".to_owned()), (true, "Plane".to_owned())];
    let stackup = &mut draft.stackup;
    kit::table(
        ui,
        &columns,
        rows,
        |ui, row, column| {
            // Even rows are foils, odd rows the dielectric below.
            let index = row / 2;
            if row % 2 == 0 {
                let (Some(layer), Some(foil)) = (layers.get(index), stackup.copper.get_mut(index))
                else {
                    return;
                };
                let inner = index > 0 && index + 1 < foils;
                match column {
                    0 => {
                        let label = ui.label(layer.name());
                        let origin = origins.get(index).copied().flatten();
                        let was = match origin {
                            None => "New layer".to_owned(),
                            Some(o) if o != *layer => {
                                format!("Was {}; its copper follows", o.name())
                            }
                            Some(_) => "Copper stays on this layer".to_owned(),
                        };
                        label.on_hover_text(was);
                    }
                    1 => {
                        ui.label(format!(
                            "Copper · {} oz",
                            kit::number_text((foil.thickness_um / OUNCE_UM * 10.0).round() / 10.0)
                        ));
                    }
                    2 => {
                        kit::number_field(
                            ui,
                            &format!("cu-{index}"),
                            &mut foil.thickness_um,
                            "µm",
                            COPPER_UM_RANGE,
                        );
                    }
                    3 => {
                        kit::dropdown(ui, ("role", index), &mut foil.plane, &roles);
                    }
                    5 if inner => {
                        if let Some(e) = row_buttons(ui, DraftRow::Foil(index), foils, true) {
                            edit = Some(e);
                        }
                    }
                    _ => {}
                }
            } else {
                let Some(d) = stackup.dielectrics.get_mut(index) else {
                    return;
                };
                match column {
                    1 => {
                        kit::dropdown(ui, ("kind", index), &mut d.kind, &kinds);
                    }
                    2 => {
                        kit::number_field(
                            ui,
                            &format!("d-{index}"),
                            &mut d.thickness_mm,
                            "mm",
                            DIELECTRIC_MM_RANGE,
                        );
                    }
                    3 => {
                        kit::dropdown(ui, ("glass", index), &mut d.glass, &glasses);
                    }
                    4 => {
                        let mut dk = d.permittivity();
                        let input =
                            kit::number_input(ui, &format!("dk-{index}"), &mut dk, "", DK_RANGE);
                        if input.changed {
                            d.dk = Some(dk);
                        }
                        if d.dk.is_none() {
                            input.response.on_hover_text(
                                "Estimated from the glass; enter the fabricator's Dk",
                            );
                        }
                    }
                    5 => {
                        if let Some(e) = row_buttons(ui, DraftRow::Dielectric(index), foils, false)
                        {
                            edit = Some(e);
                        }
                    }
                    _ => {}
                }
            }
        },
        None,
    );
    edit
}

/// A row's move buttons, and Remove on an inner foil.
fn row_buttons(ui: &mut Ui, row: DraftRow, foils: usize, foil: bool) -> Option<RowEdit> {
    let (index, first, last) = match row {
        DraftRow::Foil(i) => (i, 1, foils.saturating_sub(2)),
        DraftRow::Dielectric(i) => (i, 0, foils.saturating_sub(2)),
    };
    let mut edit = None;
    let up = ui_kit::button(ui, "↑", Button::Enabled(index > first)).on_hover_text("Move up");
    if up.clicked() {
        edit = Some(RowEdit::Move(row, true));
    }
    let down = ui_kit::button(ui, "↓", Button::Enabled(index < last)).on_hover_text("Move down");
    if down.clicked() {
        edit = Some(RowEdit::Move(row, false));
    }
    if foil {
        let remove = ui_kit::button(ui, "×", Button::Enabled(true))
            .on_hover_text("Remove this layer and the dielectric below it");
        if remove.clicked() {
            edit = Some(RowEdit::Remove(index));
        }
    }
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
