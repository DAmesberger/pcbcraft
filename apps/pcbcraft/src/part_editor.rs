//! Component authoring content for the shared shell. Form spacing, controls and
//! typography follow Solidcraft ui_kit/layout.rs and fields.rs at the pinned revision.
use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use pcbcraft_core::{
    Engine, Id, Operation, PackageTerminal, Pad, Part, PinKind, Point, SymbolPin, Terminal,
    TerminalMap,
    parts::{self, PartCommand},
};
use solidcraft_ui_egui::{
    fonts,
    theme::Tokens,
    ui_kit::{self, Button},
};
use std::sync::Arc;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum View {
    #[default]
    Symbol,
    Footprint,
    Mapping,
    Model,
}

/// Component authoring. The authoring draft lives in the engine; this editor
/// holds a working copy that widgets edit during a gesture and commits as one
/// `part.draft_set` operation once no pointer button or text field is active.
/// Every finished edit is therefore one shared undo step.
pub struct PartEditor {
    /// Working copy edited by widgets.
    draft: Part,
    /// What the working copy was loaded from; a difference is uncommitted work.
    baseline: Part,
    /// The engine draft last loaded, compared by identity to notice undo,
    /// redo, open and publish.
    source: Option<Arc<Part>>,
    loaded: bool,
    /// A commit was sent; resync from the engine whatever its outcome (the
    /// draft may even return to `source` through an undo).
    awaiting: bool,
    publishing: bool,
    view: View,
    selected: Option<Id>,
    notice: String,
    drag_scale: Option<f32>,
    update: crate::update_preview::UpdatePreview,
    update_instances: std::collections::BTreeSet<Id>,
}

impl Default for PartEditor {
    fn default() -> Self {
        let starter = parts::passive_template("Resistor");
        Self {
            draft: starter.clone(),
            baseline: starter,
            source: None,
            loaded: false,
            awaiting: false,
            publishing: false,
            view: View::Symbol,
            selected: Some(1),
            notice: String::new(),
            drag_scale: None,
            update: Default::default(),
            update_instances: Default::default(),
        }
    }
}

impl PartEditor {
    pub fn preview_part(&self) -> Option<&Part> {
        Some(&self.draft)
    }
    pub fn is_model_view(&self) -> bool {
        self.view == View::Model
    }

    /// Whether the working copy differs from its published snapshot.
    fn unpublished(&self, engine: &Engine) -> bool {
        unpublished(engine, &self.draft)
    }

    /// Reload the working copy when the engine's draft changed elsewhere.
    fn sync(&mut self, engine: &Engine) {
        let current = engine.document().draft.clone();
        let changed = match (&current, &self.source) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if !changed && self.loaded && !self.awaiting {
            return;
        }
        self.awaiting = false;
        let part = current
            .as_deref()
            .cloned()
            .unwrap_or_else(|| parts::passive_template("Resistor"));
        if self
            .selected
            .is_none_or(|id| !part.component.terminals.iter().any(|t| t.id == id))
        {
            self.selected = part.component.terminals.first().map(|t| t.id);
        }
        if self.publishing && !unpublished(engine, &part) {
            self.notice = format!(
                "Revision {} published. Existing instances retain their snapshots.",
                part.revision
            );
        }
        self.publishing = false;
        self.baseline = part.clone();
        self.draft = part;
        self.source = current;
        self.loaded = true;
        self.drag_scale = None;
    }

    /// The operation committing a finished edit, if any. Call once per frame
    /// after the editor's widgets ran.
    pub fn commit(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        let interacting =
            ctx.memory(|m| m.focused().is_some()) || ctx.input(|i| i.pointer.any_down());
        if interacting || self.draft == self.baseline {
            return None;
        }
        // Sent once; the next sync shows what the engine accepted.
        self.baseline = self.draft.clone();
        self.awaiting = true;
        Some(Operation::Part(PartCommand::SetDraft {
            draft: self.draft.clone(),
        }))
    }

    pub fn tools(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        let unpublished = self.unpublished(engine);
        if self.update.is_open() {
            ui.label("Review the update in properties");
            return None;
        }
        let mut command = None;
        ui.horizontal(|ui| {
            for (icon, label, view) in [
                ("sketch", "Symbol", View::Symbol),
                ("component", "Footprint", View::Footprint),
                ("params", "Mapping", View::Mapping),
                ("box", "3D", View::Model),
            ] {
                if crate::shell::tool(ui, icon, label, true, self.view == view).clicked() {
                    self.view = view;
                }
            }
            ui.separator();
            if crate::shell::tool(ui, "component", "Passive", true, false).clicked() {
                self.load(parts::passive_template("Resistor"));
            }
            if crate::shell::tool(ui, "component", "Connector", true, false).clicked() {
                self.load(parts::connector_template("Connector"));
            }
            if crate::shell::tool(ui, "point", "Terminal", true, false).clicked() {
                self.add_terminal(engine);
            }
            if crate::shell::tool(ui, "save", "Publish", unpublished, false).clicked() {
                match parts::validate_part(&self.draft) {
                    Ok(()) => {
                        self.publishing = true;
                        self.baseline = self.draft.clone();
                        self.awaiting = true;
                        command = Some(Operation::Part(PartCommand::Publish {
                            draft: Some(self.draft.clone()),
                        }));
                    }
                    Err(error) => self.notice = error.to_string(),
                }
            }
        });
        command
    }

    /// Replace the working copy; the next commit makes it one undoable step.
    fn load(&mut self, part: Part) {
        self.selected = part.component.terminals.first().map(|t| t.id);
        self.draft = part;
        self.notice.clear();
        self.drag_scale = None;
    }

    fn add_terminal(&mut self, engine: &Engine) {
        let electrical = self
            .draft
            .component
            .terminals
            .iter()
            .map(|t| t.id)
            .chain(
                engine
                    .project()
                    .parts
                    .iter()
                    .filter(|p| p.id == self.draft.id)
                    .flat_map(|p| p.component.terminals.iter().map(|t| t.id)),
            )
            .max()
            .unwrap_or(0)
            .checked_add(1);
        let physical = self
            .draft
            .package
            .terminals
            .iter()
            .map(|t| t.id)
            .chain(
                engine
                    .project()
                    .parts
                    .iter()
                    .filter(|p| p.id == self.draft.id)
                    .flat_map(|p| p.package.terminals.iter().map(|t| t.id)),
            )
            .max()
            .unwrap_or(0)
            .checked_add(1);
        let pad = self
            .draft
            .footprint
            .pads
            .iter()
            .map(|p| p.id)
            .chain(
                engine
                    .project()
                    .parts
                    .iter()
                    .filter(|p| p.id == self.draft.id)
                    .flat_map(|p| p.footprint.pads.iter().map(|p| p.id)),
            )
            .max()
            .unwrap_or(0)
            .checked_add(1);
        let (Some(electrical), Some(physical), Some(pad)) = (electrical, physical, pad) else {
            self.notice = "Terminal identity exhausted".into();
            return;
        };
        if self.draft.component.terminals.len() >= 256 {
            self.notice = "Maximum 256 terminals in this edition".into();
            return;
        }
        let row = self.draft.component.terminals.len() as f64;
        self.draft.component.terminals.push(Terminal {
            id: electrical,
            name: electrical.to_string(),
            kind: PinKind::Passive,
        });
        self.draft.symbol.pins.push(SymbolPin {
            terminal: electrical,
            position: [-5.0, -row * 2.5],
        });
        self.draft.package.terminals.push(PackageTerminal {
            id: physical,
            number: physical.to_string(),
        });
        self.draft.footprint.pads.push(Pad {
            id: pad,
            terminal: physical,
            position: [0.0, row * 2.0],
            size: [1.0, 1.4],
        });
        self.draft.binding.mappings.push(TerminalMap {
            electrical,
            physical,
        });
        self.selected = Some(electrical);
    }

    fn remove_selected(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        if self.draft.component.terminals.len() <= 1 {
            self.notice = "A component needs at least one terminal".into();
            return;
        }
        let physical = self
            .draft
            .binding
            .mappings
            .iter()
            .filter(|m| m.electrical == selected)
            .map(|m| m.physical)
            .collect::<Vec<_>>();
        self.draft.component.terminals.retain(|t| t.id != selected);
        self.draft.symbol.pins.retain(|p| p.terminal != selected);
        self.draft
            .binding
            .mappings
            .retain(|m| m.electrical != selected);
        self.draft
            .package
            .terminals
            .retain(|t| !physical.contains(&t.id));
        self.draft
            .footprint
            .pads
            .retain(|p| !physical.contains(&p.terminal));
        self.selected = self.draft.component.terminals.first().map(|t| t.id);
    }

    fn add_pad(&mut self, engine: &Engine) {
        let Some(terminal) = self
            .draft
            .binding
            .mappings
            .iter()
            .find(|mapping| Some(mapping.electrical) == self.selected)
            .map(|mapping| mapping.physical)
        else {
            return;
        };
        if self.draft.footprint.pads.len() >= 1024 {
            self.notice = "Maximum 1024 pads in this edition".into();
            return;
        }
        let Some(id) = self
            .draft
            .footprint
            .pads
            .iter()
            .map(|pad| pad.id)
            .chain(
                engine
                    .project()
                    .parts
                    .iter()
                    .filter(|p| p.id == self.draft.id)
                    .flat_map(|p| p.footprint.pads.iter().map(|p| p.id)),
            )
            .max()
            .unwrap_or(0)
            .checked_add(1)
        else {
            self.notice = "Pad identity exhausted".into();
            return;
        };
        self.draft.footprint.pads.push(Pad {
            id,
            terminal,
            position: [0.0, 0.0],
            size: [1.0, 1.4],
        });
    }

    pub fn inspector(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        if self.update.is_open() {
            return self.update.show(ui, engine);
        }
        let t = Tokens::get();
        ui.spacing_mut().item_spacing = egui::vec2(t.space_4, t.form_gap_y);
        ui.spacing_mut().interact_size.y = t.control_height;
        ui.label(egui::RichText::new("Component definition").font(fonts::semibold(t.font_panel)));
        text_row(ui, "Name", &mut self.draft.name);
        ui.weak(if self.draft.id == 0 {
            "New project-local draft".into()
        } else {
            format!(
                "Definition {} · revision {}{}",
                self.draft.id,
                self.draft.revision,
                if self.unpublished(engine) {
                    " · unpublished edits"
                } else {
                    ""
                }
            )
        });
        ui.label(
            "Publishing creates an immutable snapshot. Placed parts keep their existing revision.",
        );
        if !self.notice.is_empty() {
            ui.colored_label(t.form_primary, &self.notice);
        }
        if let Err(error) = parts::validate_part(&self.draft) {
            ui.colored_label(t.form_error, error.to_string());
        }
        ui.separator();
        match self.view {
            View::Symbol => self.symbol_inspector(ui),
            View::Footprint => self.footprint_inspector(ui, engine),
            View::Mapping => self.mapping_inspector(ui),
            View::Model => {
                ui.label("Parametric package envelope");
                ui.weak("Approximate body; not a verified supplier model.");
                for (index, label) in ["Length", "Width", "Height"].into_iter().enumerate() {
                    number_row(
                        ui,
                        label,
                        &mut self.draft.package.body_size[index],
                        0.01..=10000.0,
                    );
                }
                for (index, label) in ["Offset X", "Offset Y", "Offset Z"].into_iter().enumerate() {
                    number_row(
                        ui,
                        label,
                        &mut self.draft.package.model_offset[index],
                        -10000.0..=10000.0,
                    );
                }
                ui.weak("The live model and native export use Solidcraft.");
            }
        }
        ui.separator();
        ui.collapsing("Project component snapshots", |ui| {
            for part in &engine.project().parts {
                let label = format!("{} · r{}", part.name, part.revision);
                if ui_kit::button(ui, label, Button::Ghost)
                    .on_hover_text("Load this snapshot into the authoring draft")
                    .clicked()
                {
                    self.load(part.clone());
                }
            }
            if engine.project().parts.is_empty() {
                ui.weak("Publish a draft to make it available in the schematic.");
            }
        });
        if !self.unpublished(engine) {
            let target = pcbcraft_core::PartRef {
                id: self.draft.id,
                revision: self.draft.revision,
            };
            let candidates: Vec<_> = engine
                .project()
                .instances
                .iter()
                .filter(|i| i.part.id == target.id && i.part.revision != target.revision)
                .collect();
            self.update_instances
                .retain(|id| candidates.iter().any(|i| i.id == *id));
            if !candidates.is_empty() {
                ui.separator();
                ui.strong("Update placed instances");
                ui.weak(format!(
                    "Use published revision {} for the selected instances.",
                    target.revision
                ));
                for instance in candidates {
                    let mut checked = self.update_instances.contains(&instance.id);
                    if ui
                        .checkbox(
                            &mut checked,
                            format!(
                                "{} · revision {}",
                                instance.reference, instance.part.revision
                            ),
                        )
                        .changed()
                    {
                        if checked {
                            self.update_instances.insert(instance.id);
                        } else {
                            self.update_instances.remove(&instance.id);
                        }
                    }
                }
                if ui_kit::button(
                    ui,
                    "Preview instance update",
                    Button::Enabled(!self.update_instances.is_empty()),
                )
                .clicked()
                {
                    self.update.begin(
                        engine,
                        pcbcraft_core::updates::Change::PartRevision {
                            instances: self.update_instances.iter().copied().collect(),
                            target,
                        },
                    );
                }
            }
        }
        ui.collapsing("Provenance", |ui| {
            text_row(ui, "Source", &mut self.draft.provenance);
            ui.weak("Structural validation does not replace a datasheet review.");
        });
        None
    }

    fn terminal_choices(&mut self, ui: &mut Ui) {
        for terminal in &self.draft.component.terminals {
            if ui_kit::button(
                ui,
                format!("{} · {:?}", terminal.name, terminal.kind),
                if self.selected == Some(terminal.id) {
                    Button::SmallSelected
                } else {
                    Button::Ghost
                },
            )
            .clicked()
            {
                self.selected = Some(terminal.id);
            }
        }
    }

    fn symbol_inspector(&mut self, ui: &mut Ui) {
        pair_row(
            ui,
            "Body size",
            &mut self.draft.symbol.body_size,
            0.01..=10000.0,
        );
        self.terminal_choices(ui);
        if let Some(id) = self.selected {
            if let Some(terminal) = self
                .draft
                .component
                .terminals
                .iter_mut()
                .find(|t| t.id == id)
            {
                ui.separator();
                text_row(ui, "Pin name", &mut terminal.name);
                egui::ComboBox::from_id_salt(("pin-kind", id))
                    .selected_text(format!("{:?}", terminal.kind))
                    .show_ui(ui, |ui| {
                        for kind in [
                            PinKind::Passive,
                            PinKind::Input,
                            PinKind::Output,
                            PinKind::PowerInput,
                            PinKind::PowerOutput,
                            PinKind::NotConnected,
                        ] {
                            ui.selectable_value(&mut terminal.kind, kind, format!("{kind:?}"))
                                .changed();
                        }
                    });
            }
            if let Some(pin) = self.draft.symbol.pins.iter_mut().find(|p| p.terminal == id) {
                pair_row(ui, "Pin anchor", &mut pin.position, -10000.0..=10000.0);
            }
            if ui_kit::button(ui, "Remove terminal", Button::Small).clicked() {
                self.remove_selected();
            }
        }
        ui.weak("Drag a pin anchor on the canvas; publish when the draft is ready.");
    }

    fn footprint_inspector(&mut self, ui: &mut Ui, engine: &Engine) {
        pair_row(
            ui,
            "Package datum",
            &mut self.draft.footprint.datum_offset,
            -10000.0..=10000.0,
        );
        self.terminal_choices(ui);
        let physical = self
            .draft
            .binding
            .mappings
            .iter()
            .filter(|m| Some(m.electrical) == self.selected)
            .map(|m| m.physical)
            .collect::<Vec<_>>();
        for pad in self
            .draft
            .footprint
            .pads
            .iter_mut()
            .filter(|p| physical.contains(&p.terminal))
        {
            ui.separator();
            ui.label(format!(
                "Pad {} · package terminal {}",
                pad.id, pad.terminal
            ));
            egui::ComboBox::from_id_salt(("pad-terminal", pad.id))
                .selected_text(format!("Package terminal {}", pad.terminal))
                .show_ui(ui, |ui| {
                    for terminal in &self.draft.package.terminals {
                        ui.selectable_value(&mut pad.terminal, terminal.id, &terminal.number)
                            .changed();
                    }
                });
            pair_row(ui, "Position", &mut pad.position, -10000.0..=10000.0);
            pair_row(ui, "Land size", &mut pad.size, 0.01..=10000.0);
        }
        if ui_kit::button(ui, "Add pad to selected terminal", Button::Small).clicked() {
            self.add_pad(engine);
        }
        ui.weak("Pad size is independent of the package body. Pads remain individual routing obligations.");
    }

    fn mapping_inspector(&mut self, ui: &mut Ui) {
        ui.label("Electrical → package terminal");
        ui.weak("Mappings are explicit; matching names do not establish compatibility.");
        for physical in &mut self.draft.package.terminals {
            ui.separator();
            text_row(
                ui,
                &format!("Terminal {}", physical.id),
                &mut physical.number,
            );
            if let Some(mapping) = self
                .draft
                .binding
                .mappings
                .iter_mut()
                .find(|m| m.physical == physical.id)
            {
                let selected = self
                    .draft
                    .component
                    .terminals
                    .iter()
                    .find(|t| t.id == mapping.electrical)
                    .map(|t| t.name.as_str())
                    .unwrap_or("Unmapped");
                egui::ComboBox::from_id_salt(("mapping", physical.id))
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for terminal in &self.draft.component.terminals {
                            ui.selectable_value(
                                &mut mapping.electrical,
                                terminal.id,
                                &terminal.name,
                            )
                            .changed();
                        }
                    });
            }
        }
    }

    pub fn canvas(&mut self, ui: &mut Ui, _engine: &Engine) -> Option<Operation> {
        if self.update.is_open() {
            ui.disable();
        }
        if self.view == View::Model {
            ui.label("Native Solidcraft package preview");
            return None;
        }
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return None;
        }
        let t = Tokens::get();
        let painter = ui.painter().with_clip_rect(rect);
        if self.view == View::Mapping {
            self.mapping_canvas(ui, rect);
            return None;
        }
        let (extent_x, extent_y) = if self.view == View::Symbol {
            self.draft.symbol.pins.iter().fold(
                (
                    self.draft.symbol.body_size[0] / 2.0,
                    self.draft.symbol.body_size[1] / 2.0,
                ),
                |(x, y), p| (x.max(p.position[0].abs()), y.max(p.position[1].abs())),
            )
        } else {
            self.draft.footprint.pads.iter().fold(
                (
                    self.draft.package.body_size[0] / 2.0,
                    self.draft.package.body_size[1] / 2.0,
                ),
                |(x, y), p| {
                    (
                        x.max(p.position[0].abs() + p.size[0] / 2.0),
                        y.max(p.position[1].abs() + p.size[1] / 2.0),
                    )
                },
            )
        };
        if !ui.input(|input| input.pointer.any_down()) {
            self.drag_scale = None;
        }
        let fitted_scale = (rect.width() as f64 / (2.0 * extent_x + 6.0))
            .min(rect.height() as f64 / (2.0 * extent_y + 6.0))
            .clamp(0.001, 80.0) as f32;
        let scale = self.drag_scale.unwrap_or(fitted_scale);
        let transform =
            |p: Point| rect.center() + egui::vec2(p[0] as f32 * scale, -p[1] as f32 * scale);
        let origin = rect.center();
        painter.line_segment(
            [
                origin - egui::vec2(10.0, 0.0),
                origin + egui::vec2(10.0, 0.0),
            ],
            Stroke::new(1.0, t.text_dim),
        );
        painter.line_segment(
            [
                origin - egui::vec2(0.0, 10.0),
                origin + egui::vec2(0.0, 10.0),
            ],
            Stroke::new(1.0, t.text_dim),
        );
        let title = if self.view == View::Symbol {
            "Symbol · electrical terminals"
        } else {
            "Footprint · copper lands"
        };
        painter.text(
            rect.left_top() + egui::vec2(t.space_16, t.space_16),
            Align2::LEFT_TOP,
            title,
            fonts::semibold(t.font_panel),
            t.text,
        );
        if self.view == View::Symbol {
            let body = Rect::from_center_size(
                origin,
                egui::vec2(
                    self.draft.symbol.body_size[0] as f32,
                    self.draft.symbol.body_size[1] as f32,
                ) * scale,
            );
            painter.rect(
                body,
                t.radius_sm,
                t.panel,
                Stroke::new(1.5, t.accent),
                StrokeKind::Inside,
            );
            painter.text(
                body.center(),
                Align2::CENTER_CENTER,
                &self.draft.name,
                FontId::proportional(t.font_label),
                t.text,
            );
            for pin in &mut self.draft.symbol.pins {
                let at = transform(pin.position);
                let edge = body.clamp(at);
                let selected = self.selected == Some(pin.terminal);
                let ink = if selected { t.body_selected } else { t.accent };
                painter.line_segment([at, edge], Stroke::new(2.0, ink));
                painter.circle_filled(at, 4.0, ink);
                let label = self
                    .draft
                    .component
                    .terminals
                    .iter()
                    .find(|t| t.id == pin.terminal)
                    .map(|t| t.name.as_str())
                    .unwrap_or("?");
                painter.text(
                    at + egui::vec2(0.0, -t.space_10),
                    Align2::CENTER_BOTTOM,
                    label,
                    FontId::proportional(t.font_label),
                    t.text,
                );
                let response = ui.interact(
                    Rect::from_center_size(at, Vec2::splat(18.0)),
                    ui.id().with(("symbol-pin", pin.terminal)),
                    Sense::click_and_drag(),
                );
                if response.clicked() || response.drag_started() {
                    self.selected = Some(pin.terminal);
                }
                if response.drag_started() {
                    self.drag_scale = Some(scale);
                }
                if response.dragged()
                    && let Some(p) = response.interact_pointer_pos()
                {
                    pin.position = [
                        f64::from((p.x - origin.x) / scale),
                        f64::from((origin.y - p.y) / scale),
                    ];
                }
            }
        } else {
            let body_origin = transform(self.draft.footprint.datum_offset);
            let body = Rect::from_center_size(
                body_origin,
                egui::vec2(
                    self.draft.package.body_size[0] as f32,
                    self.draft.package.body_size[1] as f32,
                ) * scale,
            );
            painter.rect_stroke(
                body,
                t.radius_sm,
                Stroke::new(1.0, t.text_dim),
                StrokeKind::Inside,
            );
            for pad in &mut self.draft.footprint.pads {
                let electrical = self
                    .draft
                    .binding
                    .mappings
                    .iter()
                    .find(|m| m.physical == pad.terminal)
                    .map(|m| m.electrical);
                let at = transform(pad.position);
                let size = egui::vec2(pad.size[0] as f32, pad.size[1] as f32) * scale;
                let land = Rect::from_center_size(at, size);
                let ink = if electrical == self.selected {
                    t.body_selected
                } else {
                    t.accent
                };
                painter.rect_filled(land, 0, ink);
                let number = self
                    .draft
                    .package
                    .terminals
                    .iter()
                    .find(|p| p.id == pad.terminal)
                    .map(|p| p.number.as_str())
                    .unwrap_or("?");
                painter.text(
                    land.center(),
                    Align2::CENTER_CENTER,
                    number,
                    fonts::semibold(t.font_label),
                    t.on_accent,
                );
                let response = ui.interact(
                    land.expand(3.0),
                    ui.id().with(("footprint-pad", pad.id)),
                    Sense::click_and_drag(),
                );
                if response.clicked() || response.drag_started() {
                    self.selected = electrical;
                }
                if response.drag_started() {
                    self.drag_scale = Some(scale);
                }
                if response.dragged()
                    && let Some(p) = response.interact_pointer_pos()
                {
                    pad.position = [
                        f64::from((p.x - origin.x) / scale),
                        f64::from((origin.y - p.y) / scale),
                    ];
                }
            }
        }
        None
    }

    fn mapping_canvas(&mut self, ui: &mut Ui, rect: Rect) {
        let t = Tokens::get();
        let painter = ui.painter().with_clip_rect(rect);
        let left = rect.left() + t.space_24;
        let right = rect.right() - t.space_24;
        painter.text(
            Pos2::new(left, rect.top() + t.space_16),
            Align2::LEFT_TOP,
            "Electrical terminal",
            fonts::semibold(t.font_panel),
            t.text,
        );
        painter.text(
            Pos2::new(right, rect.top() + t.space_16),
            Align2::RIGHT_TOP,
            "Package terminal / copper pads",
            fonts::semibold(t.font_panel),
            t.text,
        );
        let top = rect.top() + t.form_header_height + t.space_16;
        for (index, physical) in self.draft.package.terminals.iter().enumerate() {
            let y = top + index as f32 * (t.control_height + t.space_12);
            let electrical = self
                .draft
                .binding
                .mappings
                .iter()
                .find(|m| m.physical == physical.id)
                .and_then(|m| {
                    self.draft
                        .component
                        .terminals
                        .iter()
                        .find(|e| e.id == m.electrical)
                });
            let ink = if electrical.is_some_and(|e| Some(e.id) == self.selected) {
                t.body_selected
            } else {
                t.accent
            };
            painter.line_segment(
                [Pos2::new(left + 120.0, y), Pos2::new(right - 150.0, y)],
                Stroke::new(2.0, ink),
            );
            let pad_ids = self
                .draft
                .footprint
                .pads
                .iter()
                .filter(|p| p.terminal == physical.id)
                .map(|p| p.id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            painter.text(
                Pos2::new(left, y),
                Align2::LEFT_CENTER,
                electrical.map(|e| e.name.as_str()).unwrap_or("Unmapped"),
                FontId::proportional(t.font_label),
                t.text,
            );
            painter.text(
                Pos2::new(right, y),
                Align2::RIGHT_CENTER,
                format!("{}  ·  pads {}", physical.number, pad_ids),
                FontId::proportional(t.font_label),
                t.text,
            );
            if ui
                .interact(
                    Rect::from_min_max(
                        Pos2::new(left, y - t.control_height / 2.0),
                        Pos2::new(right, y + t.control_height / 2.0),
                    ),
                    ui.id().with(("mapping-row", physical.id)),
                    Sense::click(),
                )
                .clicked()
            {
                self.selected = electrical.map(|e| e.id);
            }
        }
    }
}

fn unpublished(engine: &Engine, part: &Part) -> bool {
    part.id == 0
        || engine.project().part(pcbcraft_core::PartRef {
            id: part.id,
            revision: part.revision,
        }) != Some(part)
}

fn text_row(ui: &mut Ui, label: &str, value: &mut String) -> bool {
    let t = Tokens::get();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized(
            [t.form_label_width, t.control_height],
            egui::Label::new(label),
        );
        changed = ui_kit::cell_editor(ui, ui.id().with(label), value, "", false).changed();
    });
    changed
}
fn number_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let t = Tokens::get();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized(
            [t.form_label_width, t.control_height],
            egui::Label::new(label),
        );
        changed = ui
            .add(
                egui::DragValue::new(value)
                    .range(range)
                    .speed(0.05)
                    .suffix(" mm"),
            )
            .changed();
    });
    changed
}
fn pair_row(
    ui: &mut Ui,
    label: &str,
    value: &mut Point,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let t = Tokens::get();
    let mut changed = false;
    ui.label(label);
    ui.horizontal(|ui| {
        for (index, axis) in ["X", "Y"].iter().enumerate() {
            ui.label(*axis);
            changed |= ui
                .add_sized(
                    [t.form_label_width, t.control_height],
                    egui::DragValue::new(&mut value[index])
                        .range(range.clone())
                        .speed(0.05),
                )
                .changed();
        }
        ui.weak("mm");
    });
    changed
}

#[cfg(test)]
mod draft_tests {
    use super::*;

    #[test]
    fn finished_edits_commit_once_and_engine_changes_reload_the_working_copy()
    -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        assert!(
            editor.commit(&ctx, &engine).is_none(),
            "loading is not an edit"
        );
        let original = engine.draft().cloned();
        editor.draft.name = "Edited".into();
        let operation = editor.commit(&ctx, &engine);
        assert!(editor.commit(&ctx, &engine).is_none(), "sent once");
        if let Some(operation) = operation {
            engine.apply(engine.revision(), operation)?;
        }
        assert_eq!(engine.draft().map(|d| d.name.as_str()), Some("Edited"));
        engine.undo()?;
        assert!(editor.commit(&ctx, &engine).is_none());
        assert_eq!(Some(&editor.draft), original.as_ref());
        Ok(())
    }

    #[test]
    fn loading_a_template_is_an_undoable_draft_change() -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        editor.load(parts::connector_template("Replacement"));
        if let Some(operation) = editor.commit(&ctx, &engine) {
            engine.apply(engine.revision(), operation)?;
        }
        assert_eq!(engine.draft().map(|d| d.name.as_str()), Some("Replacement"));
        engine.undo()?;
        assert_eq!(engine.draft().map(|d| d.name.as_str()), Some("Resistor"));
        Ok(())
    }
}
