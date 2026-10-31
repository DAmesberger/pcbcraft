//! The Instance card of the selected instance: its properties, Part and
//! pin functions.
use super::{FieldBuffer, SchematicEditor};
use crate::kit::{self, Tone};
use crate::resolve_parts;
use pcbcraft_core::fields::{PropertyEdit, PropertyInput};
use pcbcraft_core::functions::{FunctionEdit, Resolution, TerminalFunctions};
use pcbcraft_core::instances::{self, UnitState};
use pcbcraft_core::library::{Rotation, Terminal};
use pcbcraft_core::{
    ComponentRef, Engine, InstanceCommand, Operation, Project,
    schematic::SchematicCommand,
    updates::{Change, InstanceChange},
};
use solidcraft_ui_egui::ui_kit::{self, Button};

/// The occurrence rotations the instance properties offer.
const ROTATIONS: [(Rotation, &str); 4] = [
    (Rotation::R0, "0°"),
    (Rotation::R90, "90°"),
    (Rotation::R180, "180°"),
    (Rotation::R270, "270°"),
];

impl SchematicEditor {
    pub(super) fn instance_sections(
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
        let mut update_clicked = None;
        kit::section(ui, "instance", "Instance", true, None, |ui| {
            kit::text_row(ui, "Designator", &mut self.draft_reference);
            let component = project.library.component(instance.component).map_or_else(
                || format!("missing · r{}", instance.component.revision.0),
                |c| format!("{} · r{}", c.meta.name, instance.component.revision.0),
            );
            kit::value_row(ui, "Component", &component);
            let newer = project
                .library
                .latest(instance.component.id)
                .map(|(_, r)| r)
                .filter(|r| *r > instance.component.revision);
            let offered = self
                .updates
                .iter()
                .find(|u| u.local.id == instance.component.id && u.latest > u.origin.revision)
                .cloned();
            let text = match (newer, &offered) {
                (Some(r), _) => Some(format!("{r} available in the project")),
                (None, Some(u)) => Some(format!("{} available in {}", u.latest, u.source)),
                (None, None) => None,
            };
            if let Some(text) = text {
                kit::status(ui, &text, Tone::Note);
                kit::actions(ui, |ui| {
                    if ui_kit::button(ui, "Update…", Button::Normal)
                        .on_hover_text("Review moving this instance to the newer revision")
                        .clicked()
                    {
                        update_clicked = Some((newer, offered.clone()));
                    }
                });
            }
            if position.is_some() {
                kit::pair_row(
                    ui,
                    "Position",
                    &mut self.draft_position,
                    "mm",
                    -10_000.0..=10_000.0,
                );
                kit::choice_row(
                    ui,
                    "Rotation",
                    &mut self.draft_rotation,
                    &ROTATIONS.map(|(r, l)| (r, l.to_owned())),
                );
                kit::check_row(ui, "Mirror", &mut self.draft_mirror);
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
        match update_clicked {
            Some((Some(revision), _)) => {
                let change = Change::ComponentRevision {
                    instances: vec![instance.id],
                    target: ComponentRef::new(instance.component.id, revision),
                    resolutions: Default::default(),
                };
                self.review_update(engine, change, None);
                return None;
            }
            Some((None, Some(update))) => {
                let name = project
                    .library
                    .component(instance.component)
                    .map_or_else(String::new, |c| c.meta.name.clone());
                self.library_update = Some((
                    instance.id,
                    crate::library_sources::FolderPick {
                        library: update.origin.library,
                        lineage: update.origin.lineage,
                        revision: update.latest,
                        name,
                    },
                ));
            }
            _ => {}
        }
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
                } else if let Some(hint) = part_hint(project, instance, buffer) {
                    kit::status(ui, &hint, Tone::Warning);
                }
            }
        });
        let moved = position.is_some_and(|o| o.position != self.draft_position);
        let oriented = position
            .is_some_and(|o| (o.rotation, o.mirror) != (self.draft_rotation, self.draft_mirror));
        let dirty = self.draft_reference != instance.designator
            || moved
            || oriented
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
                if let Some(o) = position.filter(|_| oriented) {
                    operations.push(Operation::Schematic(SchematicCommand::OrientUnit {
                        occurrence: o.id,
                        rotation: self.draft_rotation,
                        mirror: self.draft_mirror,
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
        output = output.or(self.part_section(ui, engine, instance));
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

    /// The instance's manufacturer part: Choose… opens the palette in
    /// assign mode, Clear sends `instance.assign_part` without a Part.
    fn part_section(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
        instance: &pcbcraft_core::ComponentInstance,
    ) -> Option<Operation> {
        let purchasing = self.report(engine).map(|r| r.purchasing.clone());
        let mut output = None;
        let mut choose = false;
        kit::section(ui, "part", "Part", true, None, |ui| {
            let shown = match (&purchasing, instance.part) {
                (Some(p), Some(r)) => match (&p.manufacturer, &p.mpn) {
                    (Some(m), Some(mpn)) => format!("{m} {mpn} · {}", r.revision),
                    _ => r.to_string(),
                },
                (None, Some(r)) => r.to_string(),
                (_, None) => "None · generic".to_owned(),
            };
            kit::value_row(ui, "Part", &shown);
            if let Some(p) = &purchasing {
                kit::value_row(
                    ui,
                    "Purchasing",
                    match p.state {
                        pcbcraft_core::completeness::PurchasingState::NoPart => "no part chosen",
                        pcbcraft_core::completeness::PurchasingState::Unresolved => "unresolved",
                        pcbcraft_core::completeness::PurchasingState::Resolved => "resolved",
                    },
                );
                for finding in p.findings.iter().take(4) {
                    kit::status(ui, &finding.message, Tone::Warning);
                }
            }
            kit::actions(ui, |ui| {
                choose = ui_kit::button(ui, "Choose…", Button::Normal)
                    .on_hover_text("Find a manufacturer part that meets this instance's values")
                    .clicked();
                if instance.part.is_some()
                    && ui_kit::button(ui, "Clear", Button::Normal)
                        .on_hover_text("Return the instance to a generic component")
                        .clicked()
                {
                    output = Some(resolve_parts::assign(vec![instance.id], None));
                }
            });
        });
        if choose {
            self.palette.open_assign(
                resolve_parts::query_for(engine, instance.id),
                vec![instance.id],
            );
        }
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
}

/// Why the typed text would be rejected, from the same core parser the
/// engine applies; the engine still decides when the edit is sent.
/// Before Apply: whether an edited value contradicts the instance's pinned
/// Part, through the same `fields::satisfies` comparison purchasing uses.
fn part_hint(
    project: &Project,
    instance: &pcbcraft_core::ComponentInstance,
    buffer: &FieldBuffer,
) -> Option<String> {
    let text = buffer.text.trim();
    if text.is_empty() || text == buffer.loaded {
        return None;
    }
    let library = &project.library;
    let part = &library.part(instance.part?)?.definition;
    let view = library.field_view(buffer.revision)?;
    let value_type = view.field.definition.value_type.clone();
    let value = pcbcraft_core::fields::parse_value(text, &value_type, view.kind_body()).ok()?;
    let mpn = &part.mpn;
    let Some(spec) = part.specs.iter().find(|s| s.field.id == buffer.field) else {
        return Some(format!(
            "{mpn} does not rate {}; applying keeps the Part and leaves purchasing unresolved.",
            buffer.label
        ));
    };
    let spec_view = library.field_view(spec.field)?;
    let requirement = pcbcraft_core::fields::Requirement {
        field: buffer.revision,
        label: buffer.label.clone(),
        value,
        value_type,
        nominal: None,
    };
    let findings = requirement.satisfied_by(spec, &spec_view.field.definition.value_type);
    let finding = findings.first()?;
    let rated = pcbcraft_core::fields::display(
        &spec.value,
        &spec_view.field.definition.value_type,
        spec_view.kind_body(),
    );
    Some(match finding.code.as_str() {
        "part.spec_mismatch" => format!(
            "{text} does not match {mpn}'s rated {rated}; applying keeps the Part and leaves purchasing unresolved."
        ),
        _ => format!("{text} cannot be checked against {mpn}'s rated {rated}."),
    })
}

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
