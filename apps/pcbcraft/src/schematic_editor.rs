//! Single-sheet content for the shared shell. Control metrics and input ownership
//! follow Solidcraft ui_kit/{layout,fields}.rs and tools.rs; public kit controls,
//! fonts and palette are used directly. Electrical connectivity lives in core.
use crate::kit::{self, Tone};
use crate::place_palette::{PaletteAction, PlacePalette};
use egui::{Align2, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use pcbcraft_core::completeness::InstanceReport;
use pcbcraft_core::fields::{PropertyEdit, PropertyInput};
use pcbcraft_core::functions::{FunctionEdit, Resolution, TerminalFunctions};
use pcbcraft_core::instances::{self, NewInstance, UnitState};
use pcbcraft_core::library::{Terminal, Unit};
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
use std::collections::{BTreeMap, VecDeque};

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    /// Further edits of one Apply, sent one per frame after the first.
    queued: VecDeque<Operation>,
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
    /// Finds the component the Place tool puts down.
    palette: PlacePalette,
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
            queued: VecDeque::new(),
            report: None,
            show_checks: false,
            fit_requested: false,
            observed_revision: None,
            nets_revision: None,
            nets: Vec::new(),
            issues: Vec::new(),
            symbol_texts: BTreeMap::new(),
            update: Default::default(),
            palette: PlacePalette::default(),
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
            designator: None,
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
                            .tip("Find a component and place it (P)"),
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

    pub fn on_tool(&mut self, tool: &str, engine: &Engine) -> Option<Operation> {
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
        // A creation tool takes over the panel, so the selection is released.
        if next != Tool::Select {
            self.select(None, None, engine.project());
        }
        if next == Tool::Place {
            self.palette.open();
        } else {
            self.palette.close();
        }
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
        if self.palette.is_open() {
            return Header::new("component", "Place part", "Library");
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
        if let Some(queued) = self.queued.pop_front() {
            return Some(queued);
        }
        let project = engine.project();
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
        if self
            .selected
            .is_some_and(|id| project.instance(id).is_none())
        {
            self.select(None, None, project);
        }
        let mut output = None;
        if self.tool == Tool::Place {
            if self.palette.is_open() {
                self.palette.preview(ui, engine);
            } else {
                self.place_section(ui, project);
            }
        }
        if let Some(instance) = self.selected.and_then(|id| project.instance(id)) {
            output = output.or(self.instance_sections(ui, engine, instance));
        } else if let Some(wire) = self
            .selected_wire
            .and_then(|id| project.wires.iter().find(|w| w.id == id))
        {
            output = output.or(self.net_section(ui, engine, wire));
        } else if self.tool != Tool::Place {
            self.sheet_section(ui, project);
        }
        if self.show_checks {
            kit::section(
                ui,
                "checks",
                "Checks",
                true,
                Some(self.issues.len()),
                |ui| {
                    if self.issues.is_empty() {
                        kit::note(ui, "No issues in the supported check subset.");
                    }
                    for issue in &self.issues {
                        kit::status(ui, &issue.message, Tone::Error);
                    }
                    kit::note(
                        ui,
                        "Checks: undriven power inputs and multiple outputs. Unconnected \
                     terminals are completeness findings of each instance.",
                    );
                },
            );
        }
        output
    }

    /// The component the Place tool puts down, picked in the palette.
    fn place_section(&mut self, ui: &mut egui::Ui, project: &Project) {
        kit::section(ui, "place", "Place", true, None, |ui| {
            let armed = self
                .component
                .and_then(|c| project.library.component(c).map(|p| (c, p)));
            match armed {
                Some((component, published)) => {
                    kit::value_row(ui, "Component", &published.meta.name);
                    kit::value_row(ui, "Revision", &component.revision.to_string());
                }
                None => kit::value_row(ui, "Component", "None"),
            }
            kit::note(
                ui,
                if project.library.components.is_empty() {
                    "Publish a component in the Component workspace, then place it here."
                } else {
                    "Click the sheet to place the component. Esc returns to Select."
                },
            );
            kit::actions(ui, |ui| {
                if ui_kit::button(
                    ui,
                    "Find part…",
                    Button::Enabled(!project.library.components.is_empty()),
                )
                .on_hover_text("Search the project's components (P)")
                .clicked()
                {
                    self.palette.open();
                }
            });
        });
    }

    fn sheet_section(&mut self, ui: &mut egui::Ui, project: &Project) {
        kit::section(ui, "sheet", "Sheet", true, None, |ui| {
            kit::value_row(ui, "Instances", &project.instances.len().to_string());
            kit::value_row(ui, "Wires", &project.wires.len().to_string());
            kit::value_row(ui, "Nets", &self.nets.len().to_string());
            kit::note(ui, self.tool_hint());
            kit::note(
                ui,
                "Crossing wires are not joined. Connect terminals explicitly.",
            );
        });
    }

    fn tool_hint(&self) -> &'static str {
        match self.tool {
            Tool::Select => "Click to select. Drag a symbol to move.",
            Tool::Place => "Click the sheet to place the selected component.",
            Tool::Wire => {
                if self.wire_from.is_some() {
                    "Click a second terminal. Esc cancels."
                } else {
                    "Click a terminal to start a wire."
                }
            }
        }
    }

    fn net_section(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
        wire: &pcbcraft_core::Wire,
    ) -> Option<Operation> {
        let project = engine.project();
        let mut output = None;
        kit::section(ui, "net", "Net", true, None, |ui| {
            if let Some(net) = project.net_of(wire.from) {
                if self.net_label.as_ref().is_none_or(|(id, _)| *id != net.id) {
                    self.net_label = Some((net.id, net.name.clone()));
                }
                let mut renamed = None;
                if let Some((id, name)) = &mut self.net_label {
                    let response = kit::text_row(ui, "Net name", name);
                    if response.lost_focus() && *name != net.name {
                        renamed = Some((*id, name.trim().to_string()));
                    }
                }
                if let Some((net, name)) = renamed {
                    output = Some(Operation::Schematic(SchematicCommand::RenameNet {
                        net,
                        name,
                    }));
                    self.net_label = None;
                }
            }
            let label = |e: Endpoint| {
                format!(
                    "{}.{}",
                    project
                        .instance(e.instance)
                        .map_or("?", |i| i.designator.as_str()),
                    e.terminal
                )
            };
            kit::value_row(
                ui,
                "Wire",
                &format!("{} → {}", label(wire.from), label(wire.to)),
            );
            if let Some(net) = project.net_of(wire.from) {
                kit::value_row(ui, "Members", &format!("{} terminals", net.terminals.len()));
            }
            kit::actions(ui, |ui| {
                if ui_kit::button(ui, "Disconnect wire", Button::Normal)
                    .on_hover_text("Preview the change before it applies.")
                    .clicked()
                {
                    output = self.stage_update(
                        engine,
                        Change::Schematic {
                            command: SchematicCommand::Disconnect { wire: wire.id },
                        },
                    );
                }
            });
        });
        output
    }

    fn instance_sections(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
        instance: &pcbcraft_core::ComponentInstance,
    ) -> Option<Operation> {
        let project = engine.project();
        let mut output = None;
        let (units, functions) = self
            .report(engine)
            .map(|r| {
                (
                    r.units
                        .iter()
                        .map(|u| (u.label.clone(), u.state))
                        .collect::<Vec<_>>(),
                    r.functions.clone(),
                )
            })
            .unwrap_or_default();
        let position = self.selected_occurrence.and_then(|o| project.occurrence(o));
        kit::section(ui, "instance", "Instance", true, None, |ui| {
            kit::text_row(ui, "Designator", &mut self.draft_reference);
            let component = project.library.component(instance.component).map_or_else(
                || format!("missing · r{}", instance.component.revision.0),
                |c| format!("{} · r{}", c.meta.name, instance.component.revision.0),
            );
            kit::value_row(ui, "Component", &component);
            if position.is_some() {
                kit::pair_row(
                    ui,
                    "Position",
                    &mut self.draft_position,
                    "mm",
                    -10_000.0..=10_000.0,
                );
            }
            for (label, state) in &units {
                let label = if label.is_empty() {
                    "Unit".to_owned()
                } else {
                    format!("Unit {label}")
                };
                kit::value_row(
                    ui,
                    &label,
                    match state {
                        UnitState::Placed { .. } => "placed",
                        UnitState::Unplaced => "unplaced",
                        UnitState::Unused => "unused",
                    },
                );
            }
        });
        let count = self.draft_fields.len();
        kit::section(ui, "properties", "Properties", true, Some(count), |ui| {
            if self.draft_fields.is_empty() {
                kit::note(ui, "This component shows no fields on its symbol.");
            }
            for buffer in &mut self.draft_fields {
                let error = typed_error(project, buffer);
                kit::checked_text_row(
                    ui,
                    &buffer.label,
                    &mut buffer.text,
                    "default",
                    error.is_some(),
                );
                if let Some(error) = error {
                    kit::status(ui, &error, Tone::Error);
                }
            }
        });
        let moved = position.is_some_and(|o| o.position != self.draft_position);
        let dirty = self.draft_reference != instance.designator
            || moved
            || self.draft_fields.iter().any(|b| b.text.trim() != b.loaded);
        kit::actions(ui, |ui| {
            if kit::primary(ui, "Apply", dirty).clicked() && dirty {
                let mut operations = self.apply_operations(instance);
                if let Some(o) = position.filter(|_| moved) {
                    operations.push(Operation::Schematic(SchematicCommand::MoveUnit {
                        occurrence: o.id,
                        position: self.draft_position,
                    }));
                }
                self.queued = operations.into();
                output = self.queued.pop_front();
            }
            if ui_kit::button(ui, "Revert", Button::Enabled(dirty)).clicked() {
                self.draft_id = None;
                self.select(self.selected, self.selected_occurrence, project);
            }
        });
        // Function choices apply at once; Apply covers the rows above.
        output = output.or(self.function_section(ui, project, instance, &functions));
        kit::rule(ui);
        kit::actions(ui, |ui| {
            if ui_kit::button(ui, "Remove…", Button::Normal)
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
        });
        output
    }

    /// Designator and property edits of the buffers, in order. Text is
    /// parsed and checked in core; an empty box returns the field to its
    /// default. Invalid text fails the whole batch.
    fn apply_operations(&self, instance: &pcbcraft_core::ComponentInstance) -> Vec<Operation> {
        let mut operations = Vec::new();
        if self.draft_reference != instance.designator {
            operations.push(Operation::Instance(InstanceCommand::SetDesignator {
                instance: instance.id,
                designator: self.draft_reference.clone(),
            }));
        }
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
        if !edits.is_empty() {
            operations.push(Operation::Instance(InstanceCommand::SetProperties {
                instance: instance.id,
                edits,
            }));
        }
        operations
    }

    /// The pin-configuration table: a choice per multi-function terminal,
    /// fixed terminals as read-only rows after them.
    fn function_section(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        instance: &pcbcraft_core::ComponentInstance,
        resolution: &Resolution,
    ) -> Option<Operation> {
        let Ok(resolved) = instances::resolve(&project.library, instance) else {
            return None;
        };
        let terminals: Vec<(&Terminal, &TerminalFunctions)> = resolved
            .component
            .definition
            .terminals
            .iter()
            .filter_map(|t| t.functions().map(|f| (t, f)))
            .collect();
        if terminals.is_empty() {
            return None;
        }
        let (choices, fixed): (Vec<_>, Vec<_>) = terminals
            .into_iter()
            .partition(|(_, f)| f.functions.len() > 1);
        let mut output = None;
        let count = choices.len() + fixed.len();
        kit::section(
            ui,
            "functions",
            "Pin functions",
            !choices.is_empty(),
            Some(count),
            |ui| {
                for (terminal, functions) in &choices {
                    let default = functions
                        .design_default
                        .and_then(|d| functions.functions.iter().find(|f| f.id == d))
                        .map_or_else(
                            || "Unselected".to_owned(),
                            |f| format!("Default ({})", f.label),
                        );
                    let mut options = vec![(None, default)];
                    options.extend(
                        functions
                            .functions
                            .iter()
                            .map(|f| (Some(f.id), f.label.clone())),
                    );
                    let mut value = instance.functions.get(&terminal.id).copied();
                    if kit::choice_row(ui, &terminal.name, &mut value, &options) {
                        output = Some(Operation::Instance(InstanceCommand::SelectFunctions {
                            instance: instance.id,
                            edits: vec![FunctionEdit {
                                terminal: terminal.id,
                                function: value,
                            }],
                        }));
                    }
                }
                for id in &resolution.unresolved {
                    let name = resolved
                        .component
                        .definition
                        .terminal(*id)
                        .map_or_else(|| id.to_string(), |t| t.name.clone());
                    kit::status(
                        ui,
                        &format!("Terminal {name} has no active function. Choose one."),
                        Tone::Warning,
                    );
                }
                for requirement in &resolution.incomplete {
                    kit::status(ui, requirement.message(), Tone::Warning);
                }
                if !fixed.is_empty() && !choices.is_empty() {
                    kit::note(ui, "Fixed functions");
                }
                for (terminal, functions) in &fixed {
                    let label = functions
                        .functions
                        .first()
                        .map_or("—", |f| f.label.as_str());
                    kit::value_row(ui, &terminal.name, label);
                }
            },
        );
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
        if !self.palette.is_open()
            && !self.update.is_open()
            && !project.library.components.is_empty()
            && !ui.ctx().egui_wants_keyboard_input()
            && !ui.ctx().any_popup_open()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::P))
        {
            self.select(None, None, project);
            self.tool = Tool::Place;
            self.wire_from = None;
            self.palette.open();
        }
        // The palette owns Esc, arrows and Enter while it is open.
        let placeable = |c: ComponentRef| first_choices(project, c).is_some();
        match self.palette.show(ui, rect, engine, &placeable) {
            Some(PaletteAction::Place {
                component,
                keep_open,
            }) => {
                if let Some((request, unit)) = first_choices(project, component) {
                    output = Some(Operation::Instance(InstanceCommand::Place(NewInstance {
                        position: self.drop_point(project, unit, rect),
                        ..request
                    })));
                }
                self.component = Some(component);
                if !keep_open {
                    self.tool = Tool::Select;
                }
            }
            Some(PaletteAction::Pick(component)) => {
                self.component = Some(component);
                self.tool = Tool::Place;
            }
            Some(PaletteAction::Close) => self.tool = Tool::Select,
            None => {}
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
                && !self.palette.is_open()
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
                // A click beside the open palette dismisses it.
                Tool::Place if self.palette.is_open() => {
                    self.palette.close();
                    self.tool = Tool::Select;
                }
                Tool::Place => {
                    if let Some((request, _)) =
                        self.component.and_then(|c| first_choices(project, c))
                    {
                        output = Some(Operation::Instance(InstanceCommand::Place(NewInstance {
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

    /// Where the palette's Enter places `unit`: the free 5 mm grid point
    /// nearest the view centre whose symbol bounds (body and pin ends, plus
    /// a margin) clear every drawn symbol and lie inside the visible canvas
    /// `rect`, which already excludes the Browser and properties cards.
    /// Falls back to the view centre when the view has no free spot.
    fn drop_point(&self, project: &Project, unit: &Unit, rect: Rect) -> Point {
        const GRID: f64 = 5.0;
        const MARGIN: f32 = 2.5;
        let snap = |v: f64| (v / GRID).round() * GRID;
        let scale = f64::from(self.scale);
        let centre = [
            snap(f64::from(-self.pan.x) / scale),
            snap(f64::from(self.pan.y) / scale),
        ];
        // The visible world, y up.
        let half = [
            f64::from(rect.width()) / scale / 2.0,
            f64::from(rect.height()) / scale / 2.0,
        ];
        let view_centre = [
            f64::from(-self.pan.x) / scale,
            f64::from(self.pan.y) / scale,
        ];
        let visible = Rect::from_min_max(
            pos2(
                (view_centre[0] - half[0]) as f32,
                (view_centre[1] - half[1]) as f32,
            ),
            pos2(
                (view_centre[0] + half[0]) as f32,
                (view_centre[1] + half[1]) as f32,
            ),
        );
        let taken: Vec<Rect> = project
            .occurrences
            .iter()
            .filter_map(|o| Some(symbol_bounds(unit_of(project, o)?, o.position)))
            .collect();
        let steps = |extent: f64| ((extent / GRID).ceil() as i32).clamp(0, 64);
        let (nx, ny) = (steps(half[0]), steps(half[1]));
        let mut candidates: Vec<(f64, i32, i32)> = (-ny..=ny)
            .flat_map(|j| (-nx..=nx).map(move |i| (i, j)))
            .map(|(i, j)| {
                let d = f64::from(i * i + j * j);
                (d, j, i)
            })
            .collect();
        // Nearest first; ties go down the sheet, then to the right.
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        candidates
            .into_iter()
            .map(|(_, j, i)| {
                [
                    centre[0] + f64::from(i) * GRID,
                    centre[1] + f64::from(j) * GRID,
                ]
            })
            .find(|p| {
                let bounds = symbol_bounds(unit, *p).expand(MARGIN);
                visible.contains_rect(bounds) && taken.iter().all(|r| !r.intersects(bounds))
            })
            .unwrap_or(centre)
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

/// A unit's extent in sheet millimetres (y up) when placed at `at`: its body
/// and every pin end.
fn symbol_bounds(unit: &Unit, at: Point) -> Rect {
    let centre = pos2(at[0] as f32, at[1] as f32);
    let mut bounds = Rect::from_center_size(
        centre,
        vec2(unit.body_size[0] as f32, unit.body_size[1] as f32),
    );
    for anchor in &unit.anchors {
        bounds.extend_with(centre + vec2(anchor.position[0] as f32, anchor.position[1] as f32));
    }
    bounds
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
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(700.0, 1400.0))),
                events,
                ..Default::default()
            },
            |ui| command = kit::form(ui, |ui| editor.inspector(ui, engine)),
        );
        // Tests have no renderer to apply texture deltas.
        output.textures_delta.clear();
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
        assert!(labels.iter().any(|t| t == "10 kΩ"), "{labels:?}");
        assert_eq!(editor.draft_fields[0].text, "10 kΩ");
        assert_eq!(
            editor.symbol_texts.get(&r1).map(String::as_str),
            Some("10 kΩ")
        );
        // Nothing changed yet: Apply is disabled and sends nothing.
        assert!(click(&mut editor, &engine, &ctx, "Apply")?.is_none());

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
        let rejected =
            click(&mut editor, &engine, &ctx, "Apply")?.ok_or("no operation for invalid text")?;
        let before = engine.document().clone();
        let revision = engine.revision();
        assert!(engine.apply(revision, rejected).is_err());
        assert_eq!(engine.document(), &before);
        assert_eq!(engine.revision(), revision);

        editor.draft_fields[0].text = "4.7k".into();
        let operation = click(&mut editor, &engine, &ctx, "Apply")?.ok_or("no operation")?;
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
        assert!(shown(&output).iter().any(|(t, _)| t == "4.7 kΩ"));
        assert_eq!(
            editor.symbol_texts.get(&r1).map(String::as_str),
            Some("4.7 kΩ")
        );
        assert_eq!(editor.draft_fields[0].text, "4.7 kΩ");
        // Clearing the box returns the field to its default.
        editor.draft_fields[0].text.clear();
        let operation = click(&mut editor, &engine, &ctx, "Apply")?.ok_or("no operation")?;
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
                position: [30.0, 0.0],
                ..request
            })),
        )?;
        // The demo holds R1 and R2; the core numbers the next resistor.
        let placed = engine
            .project()
            .instance_by_designator("R3")
            .ok_or_else(|| pcbcraft_core::Error::NotFound("R3".into()))?;
        assert_eq!(engine.project().occurrences_of(placed.id).count(), 1);
        Ok(())
    }

    /// Publish r2 of the demo resistor whose terminal 1 can be `passive`
    /// (the design default) or `sense-in`, through public library commands,
    /// and place it as U1. Returns the instance and the `sense-in` ID.
    fn multi_function_instance(
        engine: &mut Engine,
    ) -> Result<(Id, pcbcraft_core::FunctionId), Box<dyn std::error::Error>> {
        use pcbcraft_core::functions::{Direction, Drive, Function, SignalKind};
        use pcbcraft_core::library::{Definition, DraftEdit, TerminalClass};
        use pcbcraft_core::{FunctionId, LibraryCommand, Outcome};
        let r1 = engine.project().instances[0].component;
        let id = r1.id;
        if engine.drafts().get(id).is_some() {
            engine.apply(
                engine.revision(),
                Operation::Library(LibraryCommand::DiscardDraft { id }),
            )?;
        }
        engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::DraftFrom {
                revision: r1.erased(),
            }),
        )?;
        let draft = engine.drafts().get(id).cloned().ok_or("no draft")?;
        let mut definition = draft.definition;
        let Definition::Component(component) = &mut definition else {
            return Err("not a component draft".into());
        };
        // New function IDs come from the revision's local ID counter.
        let first = component.next_local;
        component.next_local = first + 2;
        let terminal = component
            .terminals
            .iter_mut()
            .find(|t| t.id == pcbcraft_core::TerminalId(1))
            .ok_or("no terminal 1")?;
        terminal.class = TerminalClass::Electrical(TerminalFunctions {
            functions: vec![
                Function {
                    id: FunctionId(first),
                    label: "passive".into(),
                    direction: Direction::Passive,
                    signal: SignalKind::Unspecified,
                    drive: Drive::None,
                    resource_role: None,
                },
                Function {
                    id: FunctionId(first + 1),
                    label: "sense-in".into(),
                    direction: Direction::Input,
                    signal: SignalKind::Analog,
                    drive: Drive::None,
                    resource_role: None,
                },
            ],
            design_default: Some(FunctionId(first)),
        });
        engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::Edit {
                edits: vec![DraftEdit {
                    id,
                    meta: draft.meta,
                    definition,
                }],
            }),
        )?;
        engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::Publish { drafts: vec![id] }),
        )?;
        let r2 = ComponentRef::new(id, pcbcraft_core::Revision(2));
        let (request, _) = first_choices(engine.project(), r2).ok_or("no r2 choices")?;
        match engine.apply(
            engine.revision(),
            Operation::Instance(InstanceCommand::Place(NewInstance {
                designator: Some("U1".into()),
                position: [0.0, 20.0],
                ..request
            })),
        )? {
            Outcome::InstancePlaced { instance, .. } => Ok((instance, FunctionId(first + 1))),
            other => Err(format!("unexpected outcome {other:?}").into()),
        }
    }

    #[test]
    fn choosing_a_pin_function_sends_one_select_functions_that_undo_restores()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let (u1, sense_in) = multi_function_instance(&mut engine)?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        editor.select(Some(u1), None, engine.project());
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        let labels: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        assert!(labels.iter().any(|t| t == "PIN FUNCTIONS"), "{labels:?}");
        // Terminal 2 stays fixed and is listed read-only.
        assert!(labels.iter().any(|t| t == "Fixed functions"), "{labels:?}");

        // Open the dropdown, then pick the alternative function.
        assert!(click(&mut editor, &engine, &ctx, "Default (passive)")?.is_none());
        let mut operations = Vec::new();
        operations.extend(click(&mut editor, &engine, &ctx, "sense-in")?);
        operations.extend(frame(&mut editor, &engine, &ctx, vec![]).1);
        assert_eq!(operations.len(), 1, "{operations:?}");
        let operation = operations.remove(0);
        let Operation::Instance(InstanceCommand::SelectFunctions { instance, edits }) = &operation
        else {
            return Err(format!("expected instance.select_functions, got {operation:?}").into());
        };
        assert_eq!(*instance, u1);
        assert_eq!(
            edits,
            &[FunctionEdit {
                terminal: pcbcraft_core::TerminalId(1),
                function: Some(sense_in),
            }]
        );

        let before = engine.project().clone();
        engine.apply(engine.revision(), operation)?;
        let selected = |engine: &Engine| {
            engine
                .project()
                .instance(u1)
                .and_then(|i| i.functions.get(&pcbcraft_core::TerminalId(1)).copied())
        };
        assert_eq!(selected(&engine), Some(sense_in));
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(shown(&output).iter().any(|(t, _)| t == "sense-in"));
        engine.undo()?;
        assert_eq!(selected(&engine), None);
        assert_eq!(engine.project(), &before);
        Ok(())
    }

    /// One frame of the whole Schematic workspace: canvas (with the palette
    /// over it) and the properties panel, as the app lays them out.
    fn workspace_frame(
        editor: &mut SchematicEditor,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Vec<Operation>) {
        let mut operations = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))),
                events,
                ..Default::default()
            },
            |ui| {
                let canvas = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 800.0));
                let panel = Rect::from_min_size(pos2(820.0, 0.0), vec2(360.0, 800.0));
                ui.scope_builder(egui::UiBuilder::new().max_rect(canvas), |ui| {
                    operations.extend(editor.canvas(ui, engine));
                });
                ui.scope_builder(egui::UiBuilder::new().max_rect(panel), |ui| {
                    operations.extend(kit::form(ui, |ui| editor.inspector(ui, engine)));
                });
            },
        );
        output.textures_delta.clear();
        (output, operations)
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn parts_engine() -> Result<Engine, Box<dyn std::error::Error>> {
        Ok(Engine::from_file(pcbcraft_core::format::EditorFile {
            version: pcbcraft_core::format::FILE_VERSION,
            project: pcbcraft_core::catalog::synthetic::project(24)?,
            drafts: Default::default(),
        })?)
    }

    /// Type a query, move with the arrows, Enter places the highlighted
    /// revision through `component.place`; undo removes it again. The
    /// properties panel previews the highlighted row.
    #[test]
    fn the_place_palette_finds_moves_and_places_through_component_place()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = parts_engine()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        assert!(editor.on_tool("place", &engine).is_none());
        assert!(editor.palette.is_open());
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        let (output, _) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::Text("res>=10k pkg:0603".into())],
        );
        let labels: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        // Ranked by name: 10 kΩ, 1 MΩ, 22 kΩ, each with its key values,
        // revision and four separate badges.
        for name in ["EX-R0603-10K0-F", "EX-R0603-1M00-J", "EX-R0603-22K0-D"] {
            assert!(labels.iter().any(|t| t == name), "{name} in {labels:?}");
        }
        assert!(
            labels
                .iter()
                .any(|t| t == "10 kΩ · ±1% · 0603 · project r1")
        );
        assert!(labels.iter().any(|t| t == "—MPN"), "{labels:?}");
        assert!(
            labels.iter().any(|t| t == "Resistance ≥ 10 kΩ"),
            "{labels:?}"
        );
        assert!(
            labels.iter().any(|t| t == "IN THIS PROJECT  3"),
            "{labels:?}"
        );
        // The preview follows the highlighted (first) row.
        assert_eq!(editor.header(&engine).title, "Place part");
        assert!(labels.iter().any(|t| t == "r1 (latest)"), "{labels:?}");

        let (output, _) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
        );
        let second = editor.palette.highlighted().ok_or("no highlight")?;
        let name = engine
            .project()
            .library
            .component(second)
            .map(|c| c.meta.name.clone());
        assert_eq!(name.as_deref(), Some("EX-R0603-1M00-J"));
        let labels: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        assert!(labels.iter().any(|t| t == "1 MΩ"), "preview {labels:?}");

        let (_, operations) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        let [operation] = &operations[..] else {
            return Err(format!("expected one operation, got {operations:?}").into());
        };
        let Operation::Instance(InstanceCommand::Place(request)) = operation else {
            return Err(format!("expected component.place, got {operation:?}").into());
        };
        assert_eq!(request.component, second);
        assert!(request.physical.is_some());
        assert_eq!(request.designator, None, "the core numbers it");
        assert!(!editor.palette.is_open());
        assert_eq!(editor.tool, Tool::Select);

        let before = engine.project().clone();
        engine.apply(engine.revision(), operation.clone())?;
        let placed = engine.project().instances.last().ok_or("not placed")?;
        assert_eq!(placed.component, second);
        assert_eq!(placed.designator, "R1", "a resistor takes its prefix");
        engine.undo()?;
        assert_eq!(engine.project(), &before);
        Ok(())
    }

    #[test]
    fn shift_enter_keeps_the_palette_and_esc_closes_it_without_placing()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = parts_engine()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        editor.on_tool("place", &engine);
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::Text("hdr".into())],
        );
        for _ in 0..2 {
            let (_, operations) = workspace_frame(
                &mut editor,
                &engine,
                &ctx,
                vec![key(egui::Key::Enter, egui::Modifiers::SHIFT)],
            );
            assert_eq!(operations.len(), 1, "{operations:?}");
            assert!(editor.palette.is_open());
            for operation in operations {
                engine.apply(engine.revision(), operation)?;
            }
        }
        // Both placed the best "hdr" match, each at its own free spot.
        for instance in &engine.project().instances {
            let name = engine
                .project()
                .library
                .component(instance.component)
                .map(|c| c.meta.name.as_str());
            assert_eq!(name, Some("EX-HDR-1X2-001"));
        }
        let [a, b] = &engine.project().occurrences[..] else {
            return Err("expected two occurrences".into());
        };
        assert_ne!(a.position, b.position);
        // Their symbols, pin ends included, keep a gap: no pin lands on
        // the other part's pin.
        let bounds = |o: &pcbcraft_core::SymbolOccurrence| {
            unit_of(engine.project(), o).map(|unit| symbol_bounds(unit, o.position))
        };
        let (Some(first), Some(second)) = (bounds(a), bounds(b)) else {
            return Err("no symbol bounds".into());
        };
        assert!(
            !first.expand(1.0).intersects(second),
            "{first:?} touches {second:?}"
        );
        // A typing error shows inline and Enter does nothing.
        workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::Text(" V>=10k".into())],
        );
        let (output, operations) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert!(operations.is_empty());
        assert!(
            shown(&output)
                .iter()
                .any(|(t, _)| t == "V>=10k: 10 k has no unit of voltage"),
            "no inline parse error"
        );
        let revision = engine.revision();
        let (_, operations) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        assert!(operations.is_empty());
        assert!(!editor.palette.is_open());
        assert_eq!(editor.tool, Tool::Select);
        assert_eq!(engine.revision(), revision);
        Ok(())
    }

    /// Hover, press and release at `at` over three frames; returns the
    /// operations of all three.
    fn click_at(
        editor: &mut SchematicEditor,
        engine: &Engine,
        ctx: &egui::Context,
        at: Pos2,
    ) -> (Vec<String>, Vec<Operation>) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut operations = Vec::new();
        let mut labels = Vec::new();
        for events in [
            vec![egui::Event::PointerMoved(at)],
            vec![button(true)],
            vec![button(false)],
        ] {
            let (output, ops) = workspace_frame(editor, engine, ctx, events);
            operations.extend(ops);
            labels = shown(&output).into_iter().map(|(t, _)| t).collect();
        }
        (labels, operations)
    }

    /// Clicking a row picks it: the palette closes, the Place tool is armed
    /// with that revision, and one click on the sheet places it once.
    #[test]
    fn clicking_a_row_arms_place_and_the_sheet_click_places_it_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = parts_engine()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        editor.on_tool("place", &engine);
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::Text("EX-C0402".into())],
        );
        let (output, _) = workspace_frame(&mut editor, &engine, &ctx, vec![]);
        // The last row label on the canvas side (the preview's copy of the
        // name is in the panel, right of x = 800).
        let (_, at) = shown(&output)
            .into_iter()
            .rfind(|(t, p)| t.starts_with("EX-C0402") && p.x < 800.0)
            .ok_or("no row")?;
        let row = at + vec2(4.0, 4.0);
        let (_, operations) = click_at(&mut editor, &engine, &ctx, row);
        assert!(
            operations.is_empty(),
            "a pick places nothing: {operations:?}"
        );
        assert!(!editor.palette.is_open());
        assert_eq!(editor.tool, Tool::Place);
        let picked = editor.component.ok_or("nothing armed")?;
        let name = engine
            .project()
            .library
            .component(picked)
            .map(|c| c.meta.name.clone())
            .ok_or("unknown component")?;
        assert!(name.starts_with("EX-C0402"), "{name}");
        // Release the pointer elsewhere, then click an empty spot.
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        let (_, operations) = click_at(&mut editor, &engine, &ctx, pos2(120.0, 680.0));
        let [Operation::Instance(InstanceCommand::Place(request))] = &operations[..] else {
            return Err(format!("expected one component.place, got {operations:?}").into());
        };
        assert_eq!(request.component, picked);
        assert_eq!(editor.tool, Tool::Place);
        let [operation] = &operations[..] else {
            return Err("one operation".into());
        };
        let mut engine = engine;
        match engine.apply(engine.revision(), operation.clone())? {
            pcbcraft_core::Outcome::InstancePlaced { designator, .. } => {
                assert_eq!(designator, "C1")
            }
            other => return Err(format!("unexpected outcome {other:?}").into()),
        }
        Ok(())
    }

    /// A row with no symbol to place keeps the palette open with a notice;
    /// Enter and a click choose nothing.
    #[test]
    fn a_row_without_a_symbol_keeps_the_palette_open_with_a_notice()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = parts_engine()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut palette = PlacePalette::default();
        palette.open();
        let run = |palette: &mut PlacePalette, events: Vec<egui::Event>| {
            let mut action = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let area = ui.max_rect();
                    action = palette.show(ui, area, &engine, &|_| false);
                },
            );
            output.textures_delta.clear();
            (output, action)
        };
        run(&mut palette, vec![]);
        run(&mut palette, vec![egui::Event::Text("hdr".into())]);
        let (output, action) = run(
            &mut palette,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert_eq!(action, None);
        assert!(palette.is_open());
        let (output, _) = if shown(&output)
            .iter()
            .any(|(t, _)| t.ends_with("has no symbol to place."))
        {
            (output, None)
        } else {
            run(&mut palette, vec![])
        };
        let labels: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        assert!(
            labels
                .iter()
                .any(|t| t == "EX-HDR-1X2-001 has no symbol to place."),
            "{labels:?}"
        );
        // Nothing can be placed, so the footer drops the Enter hints.
        assert!(
            labels.iter().any(|t| t == "↑↓ move   Esc close"),
            "{labels:?}"
        );
        Ok(())
    }
}
