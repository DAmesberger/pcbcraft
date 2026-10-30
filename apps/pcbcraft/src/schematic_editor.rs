//! Single-sheet content for the shared shell. Control metrics and input ownership
//! follow Solidcraft ui_kit/{layout,fields}.rs and tools.rs; public kit controls,
//! fonts and palette are used directly. Electrical connectivity lives in core.
use egui::{Align2, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use pcbcraft_core::completeness::InstanceReport;
use pcbcraft_core::fields::{PropertyEdit, PropertyInput};
use pcbcraft_core::instances::{self, NewInstance, UnitState};
use pcbcraft_core::library::Unit;
use pcbcraft_core::{
    ComponentRef, DefinitionId, Endpoint, Engine, Id, InstanceCommand, Net, Operation,
    PhysicalChoice, Point, Project, Query, QueryResult, erc,
    schematic::{self, SchematicCommand},
    updates::{Change, InstanceChange},
};
use solidcraft_ui_egui::{
    fonts,
    theme::Tokens,
    ui_kit::{self, Button},
};
use std::collections::BTreeMap;

/// An edit buffer for one field shown on the symbol: its lineage, label,
/// the effective text when loaded and the text being typed.
#[derive(Clone)]
struct FieldBuffer {
    field: DefinitionId,
    revision: pcbcraft_core::FieldRef,
    label: String,
    loaded: String,
    text: String,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Tool {
    #[default]
    Select,
    Place,
    Wire,
}

pub struct SchematicEditor {
    tool: Tool,
    component: Option<ComponentRef>,
    /// The selected instance and the occurrence that was picked.
    selected: Option<Id>,
    selected_occurrence: Option<Id>,
    selected_wire: Option<Id>,
    /// Net-name edit buffer for the selected wire's net.
    net_label: Option<(Id, String)>,
    wire_from: Option<Endpoint>,
    draft_reference: String,
    draft_fields: Vec<FieldBuffer>,
    draft_position: Point,
    draft_id: Option<Id>,
    pan: Vec2,
    scale: f32,
    /// Dragged occurrence and the pointer's offset from its position.
    drag: Option<(Id, Point)>,
    /// Occurrence being dragged and its previewed position.
    preview: Option<(Id, Point)>,
    /// A second property edit sent after the first one applies.
    queued: Option<Operation>,
    report: Option<(u64, Id, InstanceReport)>,
    show_checks: bool,
    fit_requested: bool,
    observed_revision: Option<u64>,
    nets_revision: Option<u64>,
    nets: Vec<Net>,
    issues: Vec<pcbcraft_core::Finding>,
    /// Effective texts of fields shown on each instance's symbol, per
    /// revision.
    symbol_texts: BTreeMap<Id, String>,
    update: crate::update_preview::UpdatePreview,
}

impl Default for SchematicEditor {
    fn default() -> Self {
        Self {
            tool: Tool::Select,
            component: None,
            selected: None,
            selected_occurrence: None,
            selected_wire: None,
            net_label: None,
            wire_from: None,
            draft_reference: String::new(),
            draft_fields: Vec::new(),
            draft_position: [0.0; 2],
            draft_id: None,
            pan: Vec2::ZERO,
            scale: 12.0,
            drag: None,
            preview: None,
            queued: None,
            report: None,
            show_checks: false,
            fit_requested: false,
            observed_revision: None,
            nets_revision: None,
            nets: Vec::new(),
            issues: Vec::new(),
            symbol_texts: BTreeMap::new(),
            update: Default::default(),
        }
    }
}

fn operation(command: SchematicCommand) -> Option<Operation> {
    Some(Operation::Schematic(command))
}

/// The drawn unit of an occurrence.
fn unit_of<'a>(
    project: &'a Project,
    occurrence: &pcbcraft_core::SymbolOccurrence,
) -> Option<&'a Unit> {
    let instance = project.instance(occurrence.instance)?;
    instances::resolve(&project.library, instance)
        .ok()?
        .symbol
        .definition
        .units
        .iter()
        .find(|u| u.id == occurrence.unit)
}

/// What placing `component` creates: its first representation and unit and
/// its first offered implementation, if any.
fn first_choices(project: &Project, component: ComponentRef) -> Option<(NewInstance, &Unit)> {
    let library = &project.library;
    let published = library.component(component)?;
    let symbol_ref = *published.definition.representations.first()?;
    let symbol = library.symbol(symbol_ref)?;
    let unit = symbol.definition.units.first()?;
    let physical = published.definition.implementations.first().and_then(|b| {
        let binding = library.binding(*b)?;
        let footprint = binding.definition.footprints.first()?;
        Some(PhysicalChoice {
            binding: b.id,
            footprint: footprint.id,
        })
    });
    Some((
        NewInstance {
            component,
            designator: String::new(),
            properties: Vec::new(),
            representation: symbol_ref.id,
            unit: unit.id,
            position: [0.0, 0.0],
            physical,
        },
        unit,
    ))
}

impl SchematicEditor {
    fn stage_update(&mut self, engine: &Engine, change: Change) -> Option<Operation> {
        self.update.begin(engine, change);
        self.tool = Tool::Select;
        self.wire_from = None;
        self.drag = None;
        self.preview = None;
        None
    }

    fn refresh_connectivity(&mut self, engine: &Engine) {
        if self.nets_revision != Some(engine.revision()) {
            self.nets = schematic::nets(engine.project()).to_vec();
            self.issues = erc::checks(engine.project());
            self.symbol_texts = engine
                .project()
                .instances
                .iter()
                .map(|i| (i.id, symbol_text(engine.project(), i)))
                .collect();
            self.nets_revision = Some(engine.revision());
        }
    }

    pub fn ribbon(&mut self, engine: &Engine) -> crate::shell::Ribbon {
        use crate::shell::{Group, Ribbon, Tab, Tool as Button};
        let reviewing = self.update.is_open();
        let has_parts = !engine.project().library.components.is_empty();
        Ribbon {
            tabs: vec![Tab {
                id: "sheet",
                label: "Sheet",
                active: true,
            }],
            groups: vec![
                Group {
                    label: "Select",
                    tools: vec![
                        Button::new("select", "select", "Select")
                            .selected(self.tool == Tool::Select)
                            .tip("Select and move symbols and wires"),
                    ],
                },
                Group {
                    label: "Create",
                    tools: vec![
                        Button::new("place", "component", "Place")
                            .enabled(has_parts && !reviewing)
                            .selected(self.tool == Tool::Place)
                            .tip("Place the chosen component revision"),
                        Button::new("wire", "line", "Wire")
                            .enabled(!reviewing)
                            .selected(self.tool == Tool::Wire)
                            .tip("Connect two terminals"),
                    ],
                },
                Group {
                    label: "Inspect",
                    tools: vec![
                        Button::new("check", "inspect", "Check")
                            .selected(self.show_checks)
                            .tip("Show the electrical rule checks"),
                    ],
                },
            ],
            notice: None,
        }
    }

    pub fn on_tool(&mut self, tool: &str, _engine: &Engine) -> Option<Operation> {
        let next = match tool {
            "select" => Tool::Select,
            "place" => Tool::Place,
            "wire" => Tool::Wire,
            "check" => {
                self.show_checks = !self.show_checks;
                return None;
            }
            _ => return None,
        };
        self.tool = next;
        self.wire_from = None;
        None
    }

    pub fn header(&self, engine: &Engine) -> crate::shell::Header {
        use crate::shell::Header;
        let project = engine.project();
        if self.update.is_open() {
            return Header::new("component", "Review change", "Review");
        }
        if let Some(instance) = self.selected.and_then(|id| project.instance(id)) {
            return Header::new("component", instance.designator.clone(), "Instance");
        }
        if let Some(net) = self.selected_net(engine).and_then(|id| project.net(id)) {
            return Header::new("line", net.name.clone(), "Net");
        }
        match self.tool {
            Tool::Place => Header::new("component", "Place component", "Tool"),
            Tool::Wire => Header::new("line", "Wire", "Tool"),
            Tool::Select => Header::new("sketch", "Sheet", "Schematic"),
        }
    }

    pub fn fit(&mut self) {
        self.fit_requested = true;
    }

    pub fn selected_instance(&self) -> Option<Id> {
        self.selected
    }

    /// The net of the selected wire.
    pub fn selected_net(&self, engine: &Engine) -> Option<Id> {
        let project = engine.project();
        let wire = self
            .selected_wire
            .and_then(|id| project.wires.iter().find(|w| w.id == id))?;
        project.net_of(wire.from).map(|n| n.id)
    }

    /// Select an instance from outside the canvas (the Browser).
    pub fn select_instance(&mut self, id: Option<Id>, project: &Project) {
        self.select(id, None, project);
        self.tool = Tool::Select;
    }

    /// Select a net through its first wire.
    pub fn select_net(&mut self, net: Id, project: &Project) {
        let wire = project
            .wires
            .iter()
            .find(|w| project.net_of(w.from).is_some_and(|n| n.id == net));
        self.select(None, None, project);
        self.selected_wire = wire.map(|w| w.id);
        self.tool = Tool::Select;
    }

    /// Select an instance, through one of its occurrences when given.
    fn select(&mut self, id: Option<Id>, occurrence: Option<Id>, project: &Project) {
        self.selected = id;
        self.selected_occurrence = occurrence
            .or_else(|| id.and_then(|id| project.occurrences_of(id).next().map(|o| o.id)));
        self.selected_wire = None;
        if self.draft_id != id {
            if let Some(instance) = id.and_then(|id| project.instance(id)) {
                self.draft_reference = instance.designator.clone();
                self.draft_fields = shown_fields(project, instance);
            }
            self.draft_id = id;
        }
        if let Some(o) = self.selected_occurrence.and_then(|o| project.occurrence(o)) {
            self.draft_position = o.position;
        }
    }

    /// Unit states of the selected instance, read from `component.inspect`.
    fn report(&mut self, engine: &Engine) -> Option<&InstanceReport> {
        let instance = self.selected?;
        let fresh = self
            .report
            .as_ref()
            .is_some_and(|(revision, id, _)| *revision == engine.revision() && *id == instance);
        if !fresh {
            self.report = match engine.query(Query::InspectInstance { instance }) {
                Ok(QueryResult::Instance(report)) => Some((engine.revision(), instance, *report)),
                _ => None,
            };
        }
        self.report.as_ref().map(|(_, _, report)| report)
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
        if let Some(queued) = self.queued.take() {
            return Some(queued);
        }
        if self.observed_revision != Some(engine.revision()) {
            self.draft_id = None;
            if self.selected.is_some() {
                let occurrence = self
                    .selected_occurrence
                    .filter(|o| project.occurrence(*o).is_some());
                self.select(self.selected, occurrence, project);
            }
            self.observed_revision = Some(engine.revision());
        }
        ui.spacing_mut().item_spacing.y = t.form_gap_y;
        if self
            .component
            .is_none_or(|c| project.library.component(c).is_none())
        {
            self.component = project
                .library
                .components
                .last()
                .map(|c| ComponentRef::new(c.id, c.revision));
        }
        ui.label(egui::RichText::new("Library snapshot").font(fonts::semibold(t.font_label)));
        if project.library.components.is_empty() {
            ui.label("Publish a component in the Component workspace, then place it here.");
        }
        for component in &project.library.components {
            let reference = ComponentRef::new(component.id, component.revision);
            if ui_kit::button(
                ui,
                format!("{} · rev {}", component.meta.name, component.revision.0),
                if self.component == Some(reference) {
                    Button::SmallSelected
                } else {
                    Button::Ghost
                },
            )
            .clicked()
            {
                self.component = Some(reference);
                self.tool = Tool::Place;
                self.wire_from = None;
            }
        }
        ui.separator();
        ui.label(egui::RichText::new("Sheet contents").font(fonts::semibold(t.font_label)));
        for instance in &project.instances {
            if ui_kit::button(
                ui,
                format!(
                    "{}  {}",
                    instance.designator,
                    self.symbol_texts
                        .get(&instance.id)
                        .map_or("", String::as_str)
                ),
                if self.selected == Some(instance.id) {
                    Button::SmallSelected
                } else {
                    Button::Ghost
                },
            )
            .clicked()
            {
                self.select(Some(instance.id), None, project);
                self.tool = Tool::Select;
            }
        }
        if self
            .selected
            .is_some_and(|id| project.instance(id).is_none())
        {
            self.select(None, None, project);
        }
        let units: Vec<(String, UnitState)> = self
            .report(engine)
            .map(|r| {
                r.units
                    .iter()
                    .map(|u| (u.display.clone(), u.state))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(instance) = self.selected.and_then(|id| project.instance(id)) {
            ui.separator();
            ui.label(egui::RichText::new("Instance").font(fonts::semibold(t.font_label)));
            ui.label(format!(
                "Component revision {}",
                instance.component.revision.0
            ));
            for (label, state) in &units {
                ui.weak(format!(
                    "{label} · {}",
                    match state {
                        UnitState::Placed { .. } => "placed",
                        UnitState::Unplaced => "unplaced",
                        UnitState::Unused => "unused",
                    }
                ));
            }
            field(
                ui,
                "Designator",
                "schematic-reference",
                &mut self.draft_reference,
            );
            for (i, buffer) in self.draft_fields.iter_mut().enumerate() {
                field(
                    ui,
                    &buffer.label,
                    &format!("schematic-field-{i}"),
                    &mut buffer.text,
                );
                if let Some(error) = typed_error(project, buffer) {
                    ui.colored_label(t.form_error, error);
                }
            }
            if ui_kit::button(ui, "Apply properties", Button::Normal).clicked() {
                let designator = (self.draft_reference != instance.designator).then(|| {
                    Operation::Instance(InstanceCommand::SetDesignator {
                        instance: instance.id,
                        designator: self.draft_reference.clone(),
                    })
                });
                // Text is parsed and checked in core; an empty box returns the
                // field to its default. Invalid text fails the whole batch.
                let edits: Vec<PropertyEdit> = self
                    .draft_fields
                    .iter()
                    .filter(|b| b.text.trim() != b.loaded)
                    .map(|b| PropertyEdit {
                        field: b.field,
                        input: if b.text.trim().is_empty() {
                            PropertyInput::Inherit
                        } else {
                            PropertyInput::Text(b.text.clone())
                        },
                    })
                    .collect();
                let value = (!edits.is_empty()).then_some(Operation::Instance(
                    InstanceCommand::SetProperties {
                        instance: instance.id,
                        edits,
                    },
                ));
                match (designator, value) {
                    (Some(first), second) => {
                        output = Some(first);
                        self.queued = second;
                    }
                    (None, value) => output = value,
                }
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
            if let Some(occurrence) = self.selected_occurrence
                && ui_kit::button(ui, "Move symbol", Button::Normal).clicked()
            {
                output = operation(SchematicCommand::MoveUnit {
                    occurrence,
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
                    Change::Instance {
                        edit: InstanceChange::Remove {
                            instance: instance.id,
                        },
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
            let label = |e: Endpoint| {
                format!(
                    "{}.{}",
                    project
                        .instance(e.instance)
                        .map_or("?", |i| i.designator.as_str()),
                    e.terminal
                )
            };
            ui.label(format!("{} → {}", label(wire.from), label(wire.to)));
            if let Some(net) = project.net_of(wire.from) {
                if self.net_label.as_ref().is_none_or(|(id, _)| *id != net.id) {
                    self.net_label = Some((net.id, net.name.clone()));
                }
                let mut renamed = None;
                if let Some((id, name)) = &mut self.net_label {
                    ui.horizontal(|ui| {
                        ui.label("Net name");
                        let response = ui.text_edit_singleline(name);
                        if response.lost_focus() && *name != net.name {
                            renamed = Some((*id, name.trim().to_string()));
                        }
                    });
                }
                if let Some((net, name)) = renamed {
                    output = Some(Operation::Schematic(SchematicCommand::RenameNet {
                        net,
                        name,
                    }));
                    self.net_label = None;
                }
                ui.weak(format!(
                    "Net ID {} · {} terminals",
                    net.id,
                    net.terminals.len()
                ));
            }
            if ui_kit::button(ui, "Disconnect wire", Button::Normal).clicked() {
                output = self.stage_update(
                    engine,
                    Change::Schematic {
                        command: SchematicCommand::Disconnect { wire: wire.id },
                    },
                );
            }
        }
        ui.separator();
        ui.label(match self.tool {
            Tool::Select => "Click to select. Drag a symbol to move.",
            Tool::Place => "Click the sheet to place the selected component.",
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
                ui.colored_label(t.form_error, &issue.message);
            }
            ui.small("Checks: undriven power inputs and multiple outputs. Unconnected terminals are completeness findings of each instance.");
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
            for occurrence in &project.occurrences {
                if let Some(unit) = unit_of(project, occurrence) {
                    let center = pos2(
                        occurrence.position[0] as f32,
                        -occurrence.position[1] as f32,
                    );
                    bounds = bounds.union(Rect::from_center_size(
                        center,
                        vec2(unit.body_size[0] as f32, unit.body_size[1] as f32),
                    ));
                    for anchor in &unit.anchors {
                        bounds.extend_with(
                            center + vec2(anchor.position[0] as f32, -anchor.position[1] as f32),
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
        for occurrence in &project.occurrences {
            let (Some(instance), Some(unit)) = (
                project.instance(occurrence.instance),
                unit_of(project, occurrence),
            ) else {
                continue;
            };
            let component = project.library.component(instance.component);
            let position = self
                .preview
                .filter(|(id, _)| *id == occurrence.id)
                .map(|(_, p)| p)
                .unwrap_or(occurrence.position);
            let center = screen(position);
            let size = vec2(
                unit.body_size[0] as f32 * scale,
                unit.body_size[1] as f32 * scale,
            );
            let body = Rect::from_center_size(center, size);
            body_hits.push((instance.id, occurrence.id, body.expand(t.space_6)));
            let selected = self.selected == Some(instance.id);
            painter.rect_filled(body, t.control_radius, t.panel);
            painter.rect_stroke(
                body,
                t.control_radius,
                Stroke::new(
                    if selected { 2.5 } else { 1.5 },
                    if selected { t.accent } else { t.text },
                ),
                egui::StrokeKind::Inside,
            );
            painter.text(
                body.center_top() - vec2(0.0, t.space_6),
                Align2::CENTER_BOTTOM,
                instances::display_label(project, instance, unit.id),
                fonts::semibold(t.font_label),
                t.text,
            );
            painter.text(
                body.center(),
                Align2::CENTER_CENTER,
                self.symbol_texts
                    .get(&instance.id)
                    .map_or("", String::as_str),
                fonts::medium(t.font_small),
                t.text_dim,
            );
            for anchor in &unit.anchors {
                let endpoint = Endpoint {
                    instance: instance.id,
                    terminal: anchor.terminal,
                };
                let p = screen([
                    position[0] + anchor.position[0],
                    position[1] + anchor.position[1],
                ]);
                let edge = pos2(
                    p.x.clamp(body.left(), body.right()),
                    p.y.clamp(body.top(), body.bottom()),
                );
                let active = self.wire_from == Some(endpoint);
                let color = if active { t.accent } else { t.form_success };
                painter.line_segment([edge, p], Stroke::new(1.5, color));
                painter.circle_stroke(p, if active { 5.0 } else { 3.5 }, Stroke::new(1.5, color));
                pin_hits.push((endpoint, p));
                if let Some(terminal) =
                    component.and_then(|c| c.definition.terminal(anchor.terminal))
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
                && let Some((_, unit)) = self.component.and_then(|c| first_choices(project, c))
            {
                let p = screen(world(pointer));
                let size = vec2(
                    unit.body_size[0] as f32 * scale,
                    unit.body_size[1] as f32 * scale,
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
                    if let Some((request, _)) =
                        self.component.and_then(|c| first_choices(project, c))
                    {
                        output = Some(Operation::Instance(InstanceCommand::Place(NewInstance {
                            designator: next_reference(project),
                            position: world(pointer),
                            ..request
                        })));
                    }
                }
                Tool::Wire => {
                    if let Some(to) = pin {
                        if let Some(from) = self.wire_from.take() {
                            if from != to {
                                output = self.stage_update(
                                    engine,
                                    Change::Schematic {
                                        command: SchematicCommand::Connect { from, to },
                                    },
                                );
                            }
                        } else {
                            self.wire_from = Some(to);
                        }
                    }
                }
                Tool::Select => {
                    let body = body_hits
                        .iter()
                        .rev()
                        .find(|(_, _, r)| r.contains(pointer))
                        .map(|(id, occurrence, _)| (*id, *occurrence));
                    let id = pin.map(|p| p.instance).or(body.map(|b| b.0));
                    if id.is_some() {
                        self.select(id, body.map(|b| b.1), project);
                    } else {
                        self.select(None, None, project);
                        self.selected_wire = hit_wire;
                    }
                }
            }
        }
        if self.tool == Tool::Select
            && !self.update.is_open()
            && response.drag_started_by(egui::PointerButton::Primary)
            && let Some(pointer) = ui.input(|i| i.pointer.press_origin())
            && let Some((id, occurrence_id, _)) =
                body_hits.iter().rev().find(|(_, _, r)| r.contains(pointer))
            && let Some(occurrence) = project.occurrence(*occurrence_id)
        {
            let p = world(pointer);
            self.drag = Some((
                *occurrence_id,
                [p[0] - occurrence.position[0], p[1] - occurrence.position[1]],
            ));
            self.select(Some(*id), Some(*occurrence_id), project);
        }
        if let Some((id, offset)) = self.drag
            && let Some(pointer) = ui.input(|i| i.pointer.latest_pos())
        {
            let p = world(pointer);
            self.preview = Some((id, [p[0] - offset[0], p[1] - offset[1]]));
        }
        if response.drag_stopped_by(egui::PointerButton::Primary)
            && let Some((occurrence, position)) = self.preview.take()
        {
            self.drag = None;
            self.draft_position = position;
            output = operation(SchematicCommand::MoveUnit {
                occurrence,
                position,
            });
        }
        output
    }

    /// Where a wire end is drawn while its unit's occurrence is dragged.
    fn preview_endpoint(&self, project: &Project, endpoint: Endpoint, original: Point) -> Point {
        if let Some((id, position)) = self.preview
            && let Some(occurrence) = project.occurrence(id)
            && occurrence.instance == endpoint.instance
            && let Some(instance) = project.instance(endpoint.instance)
            && let Ok(resolved) = instances::resolve(&project.library, instance)
            && instances::owning_unit(&resolved.symbol.definition, endpoint.terminal)
                == Some(occurrence.unit)
        {
            return [
                original[0] + position[0] - occurrence.position[0],
                original[1] + position[1] - occurrence.position[1],
            ];
        }
        original
    }
}

/// Fields the pinned component shows on the symbol, with their effective
/// text as formatted in core.
fn shown_fields(
    project: &Project,
    instance: &pcbcraft_core::ComponentInstance,
) -> Vec<FieldBuffer> {
    instances::effective_fields(&project.library, instance)
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.show_on_symbol)
        .map(|f| {
            let text = f.text.unwrap_or_default();
            FieldBuffer {
                field: f.field.id,
                revision: f.field,
                label: f.label,
                loaded: text.clone(),
                text,
            }
        })
        .collect()
}

/// Why the typed text would be rejected, from the same core parser the
/// engine applies; the engine still decides when the edit is sent.
fn typed_error(project: &Project, buffer: &FieldBuffer) -> Option<String> {
    let text = buffer.text.trim();
    if text.is_empty() || text == buffer.loaded {
        return None;
    }
    let view = project.library.field_view(buffer.revision)?;
    pcbcraft_core::fields::parse_value(
        &buffer.text,
        &view.field.definition.value_type,
        view.kind_body(),
    )
    .err()
    .map(|e| e.to_string())
}

fn symbol_text(project: &Project, instance: &pcbcraft_core::ComponentInstance) -> String {
    shown_fields(project, instance)
        .into_iter()
        .map(|f| f.loaded)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
                .any(|i| i.designator.eq_ignore_ascii_case(reference))
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
        let r1 = engine.project().instances[0].id;
        let occurrence = engine.project().occurrences[0].id;
        let mut editor = SchematicEditor {
            wire_from: Some(Endpoint {
                instance: r1,
                terminal: pcbcraft_core::TerminalId(2),
            }),
            drag: Some((occurrence, [0.0, 0.0])),
            preview: Some((occurrence, [10.0, 10.0])),
            ..Default::default()
        };
        assert!(
            editor
                .stage_update(
                    &engine,
                    Change::Instance {
                        edit: InstanceChange::Remove { instance: r1 }
                    }
                )
                .is_none()
        );
        assert!(editor.update.is_open());
        assert!(editor.wire_from.is_none());
        assert!(editor.drag.is_none());
        assert!(editor.preview.is_none());
        assert_eq!(engine.project(), &before);
        Ok(())
    }

    fn frame(
        editor: &mut SchematicEditor,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<Operation>) {
        let mut command = None;
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(700.0, 1400.0))),
                events,
                ..Default::default()
            },
            |ui| command = editor.inspector(ui, engine),
        );
        (output, command)
    }

    fn texts(shape: &egui::Shape, out: &mut Vec<(String, Pos2)>) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.job.text.clone(),
                text.pos + text.galley.size() * 0.5,
            )),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
            _ => {}
        }
    }

    fn shown(output: &egui::FullOutput) -> Vec<(String, Pos2)> {
        let mut out = Vec::new();
        for shape in &output.shapes {
            texts(&shape.shape, &mut out);
        }
        out
    }

    /// Click a button and return the operation the inspector emits.
    fn click(
        editor: &mut SchematicEditor,
        engine: &Engine,
        ctx: &egui::Context,
        label: &str,
    ) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
        let (output, _) = frame(editor, engine, ctx, vec![]);
        let point = shown(&output)
            .into_iter()
            .find(|(text, _)| text == label)
            .map(|(_, p)| p)
            .ok_or_else(|| format!("missing button {label}"))?;
        let press = |pressed| egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            editor,
            engine,
            ctx,
            vec![egui::Event::PointerMoved(point), press(true)],
        );
        Ok(frame(editor, engine, ctx, vec![press(false)]).1)
    }

    #[test]
    fn typed_fields_are_edited_through_set_properties_with_core_diagnostics()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let r1 = engine.project().instances[0].id;
        let mut editor = SchematicEditor::default();
        editor.select(Some(r1), None, engine.project());
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        let labels: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        assert!(labels.iter().any(|t| t == "Resistance"), "{labels:?}");
        assert!(labels.iter().any(|t| t == "R1  10 kΩ"), "{labels:?}");
        assert_eq!(editor.draft_fields[0].text, "10 kΩ");

        // Text of another kind shows the core diagnostic before sending,
        // and the engine rejects it without changing the document.
        editor.draft_fields[0].text = "10 µF".into();
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(
            shown(&output)
                .iter()
                .any(|(t, _)| t.contains("µF") && t.contains("not a unit")),
            "no inline diagnostic"
        );
        let rejected = click(&mut editor, &engine, &ctx, "Apply properties")?
            .ok_or("no operation for invalid text")?;
        let before = engine.document().clone();
        let revision = engine.revision();
        assert!(engine.apply(revision, rejected).is_err());
        assert_eq!(engine.document(), &before);
        assert_eq!(engine.revision(), revision);

        editor.draft_fields[0].text = "4.7k".into();
        let operation =
            click(&mut editor, &engine, &ctx, "Apply properties")?.ok_or("no operation")?;
        let Operation::Instance(InstanceCommand::SetProperties { instance, edits }) = &operation
        else {
            return Err("properties bypassed instance.set_properties".into());
        };
        assert_eq!(*instance, r1);
        assert_eq!(
            edits[0].input,
            PropertyInput::Text("4.7k".into()),
            "text is parsed in core"
        );
        engine.apply(engine.revision(), operation)?;
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(shown(&output).iter().any(|(t, _)| t == "R1  4.7 kΩ"));
        assert_eq!(editor.draft_fields[0].text, "4.7 kΩ");
        // Clearing the box returns the field to its default.
        editor.draft_fields[0].text.clear();
        let operation =
            click(&mut editor, &engine, &ctx, "Apply properties")?.ok_or("no operation")?;
        engine.apply(engine.revision(), operation)?;
        assert!(
            engine
                .project()
                .instance(r1)
                .is_some_and(|i| i.properties.is_empty())
        );
        Ok(())
    }

    #[test]
    fn placing_uses_the_components_first_representation_and_implementation()
    -> pcbcraft_core::Result<()> {
        let mut engine = Engine::demo()?;
        let component = engine.project().instances[0].component;
        let (request, unit) = first_choices(engine.project(), component)
            .ok_or_else(|| pcbcraft_core::Error::NotFound("choices".into()))?;
        assert_eq!(unit.id, pcbcraft_core::UnitId(1));
        assert!(request.physical.is_some());
        engine.apply(
            engine.revision(),
            Operation::Instance(InstanceCommand::Place(NewInstance {
                designator: next_reference(engine.project()),
                position: [30.0, 0.0],
                ..request
            })),
        )?;
        let placed = engine
            .project()
            .instance_by_designator("U1")
            .ok_or_else(|| pcbcraft_core::Error::NotFound("U1".into()))?;
        assert_eq!(engine.project().occurrences_of(placed.id).count(), 1);
        Ok(())
    }
}
