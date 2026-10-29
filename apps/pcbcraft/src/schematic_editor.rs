//! Single-sheet content for the shared shell. Control metrics and input ownership
//! follow Solidcraft ui_kit/{layout,fields}.rs and tools.rs; public kit controls,
//! fonts and palette are used directly. Electrical connectivity lives in core.
use egui::{Align2, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use pcbcraft_core::{
    Endpoint, Engine, Id, Net, Operation, PartRef, Point, Project,
    schematic::{self, SchematicCommand},
    updates::Change,
};
use solidcraft_ui_egui::{
    fonts,
    theme::Tokens,
    ui_kit::{self, Button},
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Tool {
    #[default]
    Select,
    Place,
    Wire,
}

pub struct SchematicEditor {
    tool: Tool,
    part: Option<PartRef>,
    selected: Option<Id>,
    selected_wire: Option<Id>,
    wire_from: Option<Endpoint>,
    draft_reference: String,
    draft_value: String,
    draft_position: Point,
    draft_id: Option<Id>,
    pan: Vec2,
    scale: f32,
    drag: Option<(Id, Point)>,
    preview: Option<(Id, Point)>,
    show_checks: bool,
    fit_requested: bool,
    observed_revision: Option<u64>,
    nets_revision: Option<u64>,
    nets: Vec<Net>,
    issues: Vec<String>,
    update: crate::update_preview::UpdatePreview,
}

impl Default for SchematicEditor {
    fn default() -> Self {
        Self {
            tool: Tool::Select,
            part: None,
            selected: None,
            selected_wire: None,
            wire_from: None,
            draft_reference: String::new(),
            draft_value: String::new(),
            draft_position: [0.0; 2],
            draft_id: None,
            pan: Vec2::ZERO,
            scale: 12.0,
            drag: None,
            preview: None,
            show_checks: false,
            fit_requested: false,
            observed_revision: None,
            nets_revision: None,
            nets: Vec::new(),
            issues: Vec::new(),
            update: Default::default(),
        }
    }
}

fn operation(command: SchematicCommand) -> Option<Operation> {
    Some(Operation::Schematic(command))
}

impl SchematicEditor {
    fn stage_update(&mut self, engine: &Engine, command: SchematicCommand) -> Option<Operation> {
        self.update.begin(engine, Change::Schematic { command });
        self.tool = Tool::Select;
        self.wire_from = None;
        self.drag = None;
        self.preview = None;
        None
    }

    fn refresh_connectivity(&mut self, engine: &Engine) {
        if self.nets_revision != Some(engine.revision()) {
            self.nets = schematic::nets(engine.project());
            self.issues = schematic::checks(engine.project());
            self.nets_revision = Some(engine.revision());
        }
    }

    pub fn tools(&mut self, ui: &mut egui::Ui, engine: &Engine) -> Option<Operation> {
        for (label, icon, tool) in [
            ("Select", "select", Tool::Select),
            ("Place part", "component", Tool::Place),
            ("Wire", "line", Tool::Wire),
        ] {
            if crate::shell::tool(
                ui,
                icon,
                label,
                (tool != Tool::Place || !engine.project().parts.is_empty())
                    && (tool == Tool::Select || !self.update.is_open()),
                self.tool == tool,
            )
            .clicked()
            {
                self.tool = tool;
                self.wire_from = None;
            }
        }
        if crate::shell::tool(ui, "fit", "Fit", true, false).clicked() {
            self.fit_requested = true;
        }
        if crate::shell::tool(ui, "inspect", "Check", true, self.show_checks).clicked() {
            self.show_checks = !self.show_checks;
        }
        None
    }

    fn select(&mut self, id: Option<Id>, project: &Project) {
        self.selected = id;
        self.selected_wire = None;
        if self.draft_id != id {
            if let Some(instance) = id.and_then(|id| project.instance(id)) {
                self.draft_reference = instance.reference.clone();
                self.draft_value = instance.value.clone();
                self.draft_position = instance.position;
            }
            self.draft_id = id;
        }
    }

    pub fn inspector(&mut self, ui: &mut egui::Ui, engine: &Engine) -> Option<Operation> {
        self.refresh_connectivity(engine);
        if let Some(operation) = self.update.show(ui, engine) {
            return Some(operation);
        }
        if self.update.is_open() {
            return None;
        }
        let project = engine.project();
        let t = Tokens::get();
        let mut output = None;
        if self.observed_revision != Some(engine.revision()) {
            self.draft_id = None;
            if self.selected.is_some() {
                self.select(self.selected, project);
            }
            self.observed_revision = Some(engine.revision());
        }
        ui.spacing_mut().item_spacing.y = t.form_gap_y;
        if self.part.is_none_or(|p| project.part(p).is_none()) {
            self.part = project.parts.last().map(|p| PartRef {
                id: p.id,
                revision: p.revision,
            });
        }
        ui.label(egui::RichText::new("Library snapshot").font(fonts::semibold(t.font_label)));
        if project.parts.is_empty() {
            ui.label("Publish a part in the Part workspace, then place it here.");
        }
        for part in &project.parts {
            let reference = PartRef {
                id: part.id,
                revision: part.revision,
            };
            if ui_kit::button(
                ui,
                format!("{} · rev {}", part.name, part.revision),
                if self.part == Some(reference) {
                    Button::SmallSelected
                } else {
                    Button::Ghost
                },
            )
            .clicked()
            {
                self.part = Some(reference);
                self.tool = Tool::Place;
                self.wire_from = None;
            }
        }
        ui.separator();
        ui.label(egui::RichText::new("Sheet contents").font(fonts::semibold(t.font_label)));
        for instance in &project.instances {
            if ui_kit::button(
                ui,
                format!("{}  {}", instance.reference, instance.value),
                if self.selected == Some(instance.id) {
                    Button::SmallSelected
                } else {
                    Button::Ghost
                },
            )
            .clicked()
            {
                self.select(Some(instance.id), project);
                self.tool = Tool::Select;
            }
        }
        if self
            .selected
            .is_some_and(|id| project.instance(id).is_none())
        {
            self.select(None, project);
        }
        if let Some(instance) = self.selected.and_then(|id| project.instance(id)) {
            ui.separator();
            ui.label(egui::RichText::new("Instance").font(fonts::semibold(t.font_label)));
            ui.label(format!("Part revision {}", instance.part.revision));
            field(
                ui,
                "Reference",
                "schematic-reference",
                &mut self.draft_reference,
            );
            field(ui, "Value", "schematic-value", &mut self.draft_value);
            if ui_kit::button(ui, "Apply properties", Button::Normal).clicked() {
                output = operation(SchematicCommand::Edit {
                    instance: instance.id,
                    reference: self.draft_reference.clone(),
                    value: self.draft_value.clone(),
                });
            }
            ui.horizontal(|ui| {
                ui.label("X");
                ui.add(
                    egui::DragValue::new(&mut self.draft_position[0])
                        .speed(0.5)
                        .suffix(" mm"),
                );
                ui.label("Y");
                ui.add(
                    egui::DragValue::new(&mut self.draft_position[1])
                        .speed(0.5)
                        .suffix(" mm"),
                );
            });
            if ui_kit::button(ui, "Move instance", Button::Normal).clicked() {
                output = operation(SchematicCommand::Move {
                    instance: instance.id,
                    position: self.draft_position,
                });
            }
            if ui_kit::button(ui, "Remove instance…", Button::Normal)
                .on_hover_text(
                    "Preview changes to the schematic, PCB placement and affected tracks.",
                )
                .clicked()
            {
                output = self.stage_update(
                    engine,
                    SchematicCommand::Remove {
                        instance: instance.id,
                    },
                );
            }
        }
        if let Some(wire) = self
            .selected_wire
            .and_then(|id| project.wires.iter().find(|w| w.id == id))
        {
            ui.separator();
            ui.label("Selected wire");
            ui.label(format!(
                "{}:{} → {}:{}",
                wire.from.instance, wire.from.terminal, wire.to.instance, wire.to.terminal
            ));
            if ui_kit::button(ui, "Disconnect wire", Button::Normal).clicked() {
                output = self.stage_update(engine, SchematicCommand::Disconnect { wire: wire.id });
            }
        }
        ui.separator();
        ui.label(match self.tool {
            Tool::Select => "Click to select. Drag a symbol to move.",
            Tool::Place => "Click the sheet to place the selected part.",
            Tool::Wire => {
                if self.wire_from.is_some() {
                    "Click a second terminal. Esc cancels."
                } else {
                    "Click a terminal to start a wire."
                }
            }
        });
        ui.small("Crossing wires are not joined. Connect terminals explicitly.");
        if self.show_checks {
            ui.separator();
            ui.label(egui::RichText::new("Electrical checks").font(fonts::semibold(t.font_label)));
            if self.issues.is_empty() {
                ui.label("No issues in the supported check subset.");
            }
            for issue in &self.issues {
                ui.colored_label(t.form_error, issue);
            }
            ui.small("Checks: unconnected inputs/power and multiple outputs.");
        }
        output
    }

    pub fn canvas(&mut self, ui: &mut egui::Ui, engine: &Engine) -> Option<Operation> {
        self.refresh_connectivity(engine);
        let project = engine.project();
        let t = Tokens::get();
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return None;
        }
        if self.fit_requested {
            let mut bounds = Rect::NOTHING;
            for instance in &project.instances {
                if let Some(part) = project.part(instance.part) {
                    let center = pos2(instance.position[0] as f32, -instance.position[1] as f32);
                    bounds = bounds.union(Rect::from_center_size(
                        center,
                        vec2(
                            part.symbol.body_size[0] as f32,
                            part.symbol.body_size[1] as f32,
                        ),
                    ));
                    for pin in &part.symbol.pins {
                        bounds.extend_with(
                            center + vec2(pin.position[0] as f32, -pin.position[1] as f32),
                        );
                    }
                }
            }
            if bounds.is_finite() {
                bounds = bounds.expand(5.0);
                self.scale = (rect.width() / bounds.width())
                    .min(rect.height() / bounds.height())
                    .clamp(2.0, 60.0);
                self.pan = -bounds.center().to_vec2() * self.scale;
            } else {
                self.pan = Vec2::ZERO;
                self.scale = 12.0;
            }
            self.fit_requested = false;
        }
        let mut output = None;
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let old_scale = self.scale;
                self.scale = (self.scale * (scroll * 0.002).exp()).clamp(2.0, 60.0);
                if let Some(pointer) = response.hover_pos() {
                    let relative = pointer - rect.center();
                    self.pan = relative - (relative - self.pan) * (self.scale / old_scale);
                }
            }
        }
        if response.dragged_by(egui::PointerButton::Middle) {
            self.pan += ui.input(|i| i.pointer.delta());
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.wire_from = None;
            self.tool = Tool::Select;
            self.drag = None;
            self.preview = None;
        }
        let origin = rect.center() + self.pan;
        let scale = self.scale;
        let screen = |p: Point| {
            pos2(
                origin.x + p[0] as f32 * scale,
                origin.y - p[1] as f32 * scale,
            )
        };
        let world = |p: Pos2| {
            [
                (f64::from((p.x - origin.x) / scale) * 2.0).round() / 2.0,
                (f64::from((origin.y - p.y) / scale) * 2.0).round() / 2.0,
            ]
        };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, t.viewport_bottom);
        let step = 5.0 * scale;
        let start = pos2(
            rect.left() + (origin.x - rect.left()).rem_euclid(step),
            rect.top() + (origin.y - rect.top()).rem_euclid(step),
        );
        let mut x = start.x;
        while x < rect.right() {
            let mut y = start.y;
            while y < rect.bottom() {
                painter.circle_filled(pos2(x, y), 0.8, t.grid);
                y += step;
            }
            x += step;
        }

        let mut pin_hits = Vec::new();
        let mut body_hits = Vec::new();
        let mut hit_wire = None;
        let selected_net = self
            .wire_from
            .or_else(|| {
                self.selected_wire
                    .and_then(|id| project.wires.iter().find(|w| w.id == id))
                    .map(|w| w.from)
            })
            .and_then(|endpoint| self.nets.iter().find(|n| n.terminals.contains(&endpoint)));
        for wire in &project.wires {
            if let Some((a, b)) = schematic::pin_position(project, wire.from)
                .zip(schematic::pin_position(project, wire.to))
            {
                let a = screen(self.preview_endpoint(project, wire.from, a));
                let b = screen(self.preview_endpoint(project, wire.to, b));
                let mid = (a.x + b.x) * 0.5;
                let points = vec![a, pos2(mid, a.y), pos2(mid, b.y), b];
                let active = self.selected_wire == Some(wire.id)
                    || selected_net.is_some_and(|n| n.terminals.contains(&wire.from));
                painter.add(egui::Shape::line(
                    points.clone(),
                    Stroke::new(
                        if active { 2.5 } else { 1.5 },
                        if active { t.accent } else { t.form_success },
                    ),
                ));
                if response.clicked()
                    && self.tool == Tool::Select
                    && let Some(p) = response.interact_pointer_pos()
                    && points
                        .windows(2)
                        .any(|s| segment_distance(p, s[0], s[1]) < 6.0)
                {
                    hit_wire = Some(wire.id);
                }
            }
        }
        for instance in &project.instances {
            let Some(part) = project.part(instance.part) else {
                continue;
            };
            let position = self
                .preview
                .filter(|(id, _)| *id == instance.id)
                .map(|(_, p)| p)
                .unwrap_or(instance.position);
            let center = screen(position);
            let size = vec2(
                part.symbol.body_size[0] as f32 * scale,
                part.symbol.body_size[1] as f32 * scale,
            );
            let body = Rect::from_center_size(center, size);
            body_hits.push((instance.id, body.expand(t.space_6)));
            painter.rect_filled(body, t.control_radius, t.panel);
            painter.rect_stroke(
                body,
                t.control_radius,
                Stroke::new(
                    if self.selected == Some(instance.id) {
                        2.5
                    } else {
                        1.5
                    },
                    if self.selected == Some(instance.id) {
                        t.accent
                    } else {
                        t.text
                    },
                ),
                egui::StrokeKind::Inside,
            );
            painter.text(
                body.center_top() - vec2(0.0, t.space_6),
                Align2::CENTER_BOTTOM,
                &instance.reference,
                fonts::semibold(t.font_label),
                t.text,
            );
            painter.text(
                body.center(),
                Align2::CENTER_CENTER,
                &instance.value,
                fonts::medium(t.font_small),
                t.text_dim,
            );
            for pin in &part.symbol.pins {
                let endpoint = Endpoint {
                    instance: instance.id,
                    terminal: pin.terminal,
                };
                let p = screen([position[0] + pin.position[0], position[1] + pin.position[1]]);
                let edge = pos2(
                    p.x.clamp(body.left(), body.right()),
                    p.y.clamp(body.top(), body.bottom()),
                );
                let active = self.wire_from == Some(endpoint);
                let color = if active { t.accent } else { t.form_success };
                painter.line_segment([edge, p], Stroke::new(1.5, color));
                painter.circle_stroke(p, if active { 5.0 } else { 3.5 }, Stroke::new(1.5, color));
                pin_hits.push((endpoint, p));
                if let Some(terminal) = part
                    .component
                    .terminals
                    .iter()
                    .find(|terminal| terminal.id == pin.terminal)
                {
                    painter.text(
                        p + vec2(0.0, -t.space_6),
                        Align2::CENTER_BOTTOM,
                        &terminal.name,
                        fonts::medium(t.font_micro),
                        t.text_dim,
                    );
                }
            }
        }
        if let Some(pointer) = response.hover_pos() {
            let hit = pin_hits
                .iter()
                .filter(|(_, p)| p.distance(pointer) < 9.0)
                .min_by(|a, b| a.1.distance(pointer).total_cmp(&b.1.distance(pointer)));
            if let Some((_, p)) = hit {
                painter.circle_stroke(*p, 7.0, Stroke::new(1.0, t.accent));
            }
            if self.tool == Tool::Place
                && let Some(part) = self.part.and_then(|p| project.part(p))
            {
                let p = screen(world(pointer));
                let size = vec2(
                    part.symbol.body_size[0] as f32 * scale,
                    part.symbol.body_size[1] as f32 * scale,
                );
                painter.rect_stroke(
                    Rect::from_center_size(p, size),
                    t.control_radius,
                    Stroke::new(1.5, t.accent),
                    egui::StrokeKind::Inside,
                );
            }
            if let Some(from) = self
                .wire_from
                .and_then(|e| schematic::pin_position(project, e))
            {
                let a = screen(from);
                let b = hit.map(|(_, p)| *p).unwrap_or(pointer);
                painter.add(egui::Shape::line(
                    vec![a, pos2(b.x, a.y), b],
                    Stroke::new(1.5, t.accent),
                ));
            }
        }
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let pin = pin_hits
                .iter()
                .filter(|(_, p)| p.distance(pointer) < 9.0)
                .min_by(|a, b| a.1.distance(pointer).total_cmp(&b.1.distance(pointer)))
                .map(|(e, _)| *e);
            match self.tool {
                Tool::Place => {
                    if let Some(part) = self.part {
                        let reference = next_reference(project);
                        output = operation(SchematicCommand::Place {
                            part,
                            reference,
                            value: String::new(),
                            position: world(pointer),
                        });
                    }
                }
                Tool::Wire => {
                    if let Some(to) = pin {
                        if let Some(from) = self.wire_from.take() {
                            if from != to {
                                output = self
                                    .stage_update(engine, SchematicCommand::Connect { from, to });
                            }
                        } else {
                            self.wire_from = Some(to);
                        }
                    }
                }
                Tool::Select => {
                    let id = pin.map(|p| p.instance).or_else(|| {
                        body_hits
                            .iter()
                            .rev()
                            .find(|(_, r)| r.contains(pointer))
                            .map(|(id, _)| *id)
                    });
                    if id.is_some() {
                        self.select(id, project);
                    } else {
                        self.select(None, project);
                        self.selected_wire = hit_wire;
                    }
                }
            }
        }
        if self.tool == Tool::Select
            && !self.update.is_open()
            && response.drag_started_by(egui::PointerButton::Primary)
            && let Some(pointer) = ui.input(|i| i.pointer.press_origin())
            && let Some((id, _)) = body_hits.iter().rev().find(|(_, r)| r.contains(pointer))
            && let Some(instance) = project.instance(*id)
        {
            let p = world(pointer);
            self.drag = Some((
                *id,
                [p[0] - instance.position[0], p[1] - instance.position[1]],
            ));
            self.select(Some(*id), project);
        }
        if let Some((id, offset)) = self.drag
            && let Some(pointer) = ui.input(|i| i.pointer.latest_pos())
        {
            let p = world(pointer);
            self.preview = Some((id, [p[0] - offset[0], p[1] - offset[1]]));
        }
        if response.drag_stopped_by(egui::PointerButton::Primary)
            && let Some((instance, position)) = self.preview.take()
        {
            self.drag = None;
            self.draft_position = position;
            output = operation(SchematicCommand::Move { instance, position });
        }
        output
    }

    fn preview_endpoint(&self, project: &Project, endpoint: Endpoint, original: Point) -> Point {
        if let Some((id, position)) = self.preview
            && id == endpoint.instance
            && let Some(instance) = project.instance(id)
        {
            return [
                original[0] + position[0] - instance.position[0],
                original[1] + position[1] - instance.position[1],
            ];
        }
        original
    }
}

fn field(ui: &mut egui::Ui, label: &str, id: &str, value: &mut String) {
    let t = Tokens::get();
    ui.horizontal(|ui| {
        ui.add_sized(
            [t.form_label_width, t.control_height],
            egui::Label::new(label),
        );
        ui_kit::cell_editor(ui, egui::Id::new(id), value, "", false);
    });
}

fn next_reference(project: &Project) -> String {
    (1..=project.instances.len() + 1)
        .map(|i| format!("U{i}"))
        .find(|reference| {
            !project
                .instances
                .iter()
                .any(|i| i.reference.eq_ignore_ascii_case(reference))
        })
        .unwrap_or_else(|| "U_new".into())
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let delta = b - a;
    let length = delta.length_sq();
    if length <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p - a).dot(delta) / length).clamp(0.0, 1.0);
    p.distance(a + delta * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placed_instance_removal_opens_review_without_mutating_the_board() -> pcbcraft_core::Result<()>
    {
        let engine = Engine::demo()?;
        let before = engine.project().clone();
        let mut editor = SchematicEditor {
            wire_from: Some(Endpoint {
                instance: 2,
                terminal: 2,
            }),
            drag: Some((2, [0.0, 0.0])),
            preview: Some((2, [10.0, 10.0])),
            ..Default::default()
        };
        assert!(
            editor
                .stage_update(&engine, SchematicCommand::Remove { instance: 2 })
                .is_none()
        );
        assert!(editor.update.is_open());
        assert!(editor.wire_from.is_none());
        assert!(editor.drag.is_none());
        assert!(editor.preview.is_none());
        assert_eq!(engine.project(), &before);
        Ok(())
    }
}
