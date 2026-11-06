//! The Instance card of the selected instance: INSTANCE (designator,
//! component, position, orientation, units), PROPERTIES, PART and PIN
//! FUNCTIONS. Every field commits live as one typed command and one undo
//! step (docs/sidebar-ia.md, principle 13); there is no Apply. Remove… is
//! the card's footer, and findings are counted in its issue slot.
use super::{FieldBuffer, Live, SchematicEditor, focus_left, live};
use crate::kit::{self, Tone};
use crate::refusal::{self, Origin};
use crate::resolve_parts;
use pcbcraft_core::completeness::{PurchasingState, purchasing};
use pcbcraft_core::fields::{PropertyEdit, PropertyInput};
use pcbcraft_core::functions::{FunctionEdit, TerminalFunctions};
use pcbcraft_core::instances::{self, UnitState};
use pcbcraft_core::library::{Rotation, Terminal};
use pcbcraft_core::{
    ComponentRef, Engine, InstanceCommand, Operation, Project, schematic::SchematicCommand,
    updates::Change,
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
        let units = self
            .report(engine)
            .map(|r| {
                r.units
                    .iter()
                    .map(|u| (u.label.clone(), u.state))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut output = None;
        let mut update_clicked = None;
        kit::section(ui, "instance", "Instance", true, None, |ui| {
            output = self.designator_row(ui, instance);
            let component = project.library.component(instance.component).map_or_else(
                || format!("missing · {}", instance.component.revision),
                |c| format!("{} · {}", c.meta.name, instance.component.revision),
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
                (None, Some(u)) => Some(format!("{} available from {}", u.latest, u.source)),
                (None, None) => None,
            };
            if let Some(text) = text
                && kit::summary_row(ui, &text, Some("Update…"))
            {
                update_clicked = Some((newer, offered.clone()));
            }
            output = output.take().or(self.placement_rows(ui, project));
            // Unit rows only when the component has several.
            if units.len() > 1 {
                for (label, state) in &units {
                    kit::value_row(
                        ui,
                        &format!("Unit {label}"),
                        match state {
                            UnitState::Placed { .. } => "placed",
                            UnitState::Unplaced => "not placed",
                            UnitState::Unused => "unused",
                        },
                    );
                }
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
        output = output.or(self.property_rows(ui, project, instance));
        output = output.or(self.part_section(ui, engine, instance));
        output = output.or(self.function_section(ui, engine, instance));
        // C6: the docked pin-mux panel, opened by Pin assignment… above;
        // D6: the Marking row.
        output = output.or(crate::pin_mux::entry(ui, engine, instance));
        output.or(super::marking_row::row(ui, engine, instance))
    }

    /// The designator field: `instance.set_designator` on Enter or focus
    /// loss; a refusal (a designator in use) shows under it.
    fn designator_row(
        &mut self,
        ui: &mut egui::Ui,
        instance: &pcbcraft_core::ComponentInstance,
    ) -> Option<Operation> {
        let response = kit::text_row(ui, "Designator", &mut self.draft_reference);
        let changed = self.draft_reference.trim() != instance.designator;
        let invalid = self
            .draft_reference
            .trim()
            .is_empty()
            .then_some("Enter a designator, such as R1");
        match live(ui, &response, &mut self.refused, changed, invalid) {
            Live::Commit => {
                refusal::mark(ui.ctx(), Origin::Field(response.id));
                Some(Operation::Instance(InstanceCommand::SetDesignator {
                    instance: instance.id,
                    designator: self.draft_reference.trim().to_owned(),
                }))
            }
            Live::Revert => {
                self.draft_reference = instance.designator.clone();
                None
            }
            Live::Idle => None,
        }
    }

    /// Position, rotation and mirror of the picked occurrence. The X/Y pair
    /// commits together as one `symbol.move_unit` when focus leaves the
    /// pair; rotation and mirror commit at once as `symbol.orient_unit`.
    fn placement_rows(&mut self, ui: &mut egui::Ui, project: &Project) -> Option<Operation> {
        let occurrence = self
            .selected_occurrence
            .and_then(|o| project.occurrence(o))?;
        let mut output = None;
        let row = ui
            .scope(|ui| {
                kit::pair_row(
                    ui,
                    "Position",
                    &mut self.draft_position,
                    "mm",
                    -10_000.0..=10_000.0,
                );
            })
            .response;
        let position_id = ui.scope_id().with("position");
        if let Some(message) = refusal::claim(ui.ctx(), Origin::Field(position_id)) {
            self.refused = Some((position_id, message));
        }
        match focus_left(ui, position_id.with("focus"), row.rect) {
            Some(true) => self.draft_position = occurrence.position,
            Some(false) if self.draft_position != occurrence.position => {
                refusal::mark(ui.ctx(), Origin::Field(position_id));
                output = Some(Operation::Schematic(SchematicCommand::MoveUnit {
                    occurrence: occurrence.id,
                    position: self.draft_position,
                }));
            }
            _ => {}
        }
        if let Some((id, message)) = &self.refused
            && *id == position_id
        {
            kit::field_error(ui, &row, message, false);
        }
        if kit::choice_row(
            ui,
            "Rotation",
            &mut self.draft_rotation,
            &ROTATIONS.map(|(r, l)| (r, l.to_owned())),
        ) {
            output = output.or(Some(Operation::Schematic(SchematicCommand::OrientUnit {
                occurrence: occurrence.id,
                rotation: self.draft_rotation,
                mirror: occurrence.mirror,
            })));
        }
        if kit::check_row(ui, "Mirror", &mut self.draft_mirror) {
            output = output.or(Some(Operation::Schematic(SchematicCommand::OrientUnit {
                occurrence: occurrence.id,
                rotation: occurrence.rotation,
                mirror: self.draft_mirror,
            })));
        }
        output
    }

    /// One field per shown property. A field commits its own
    /// `instance.set_properties` edit on Enter or focus loss; an empty box
    /// returns it to the component's default. Text the core parser rejects
    /// is marked under the field and never sent.
    fn property_rows(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        instance: &pcbcraft_core::ComponentInstance,
    ) -> Option<Operation> {
        let mut output = None;
        let count = self.draft_fields.len();
        kit::section(ui, "properties", "Properties", true, Some(count), |ui| {
            if self.draft_fields.is_empty() {
                kit::hint(ui, "This component shows no fields on its symbol", "");
            }
            for index in 0..self.draft_fields.len() {
                let Some(buffer) = self.draft_fields.get_mut(index) else {
                    continue;
                };
                let marked = typed_error(project, buffer).is_some();
                let response =
                    kit::checked_text_row(ui, &buffer.label, &mut buffer.text, "default", marked);
                // Checked against this frame's text, so Enter never sends
                // what the parser rejects.
                let error = typed_error(project, buffer);
                let changed = buffer.text.trim() != buffer.loaded;
                let buffer = buffer.clone();
                match live(ui, &response, &mut self.refused, changed, error.as_deref()) {
                    Live::Commit => {
                        refusal::mark(ui.ctx(), Origin::Field(response.id));
                        self.purchasing_watch =
                            Some((instance.id, purchasing(project, instance).state));
                        output = Some(Operation::Instance(InstanceCommand::SetProperties {
                            instance: instance.id,
                            edits: vec![PropertyEdit {
                                field: buffer.field,
                                input: if buffer.text.trim().is_empty() {
                                    PropertyInput::Inherit
                                } else {
                                    PropertyInput::Text(buffer.text.clone())
                                },
                            }],
                        }));
                    }
                    Live::Revert => {
                        if let Some(b) = self.draft_fields.get_mut(index) {
                            b.text = b.loaded.clone();
                        }
                    }
                    Live::Idle => {
                        // Before it commits: whether the value contradicts
                        // the pinned Part (it then becomes an Issue).
                        if error.is_none()
                            && let Some(hint) = part_hint(project, instance, &buffer)
                        {
                            kit::status(ui, &hint, Tone::Warning);
                        }
                    }
                }
            }
        });
        output
    }

    /// The instance's manufacturer part as one row with Choose… (the
    /// palette in assign mode), and Clear (`instance.assign_part` without
    /// a Part) when one is chosen. Why purchasing is unresolved is in
    /// Issues.
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
            let named = match (&purchasing, instance.part) {
                (Some(p), Some(r)) => match (&p.manufacturer, &p.mpn) {
                    (Some(m), Some(mpn)) => format!("{m} {mpn} · {}", r.revision),
                    _ => r.to_string(),
                },
                (None, Some(r)) => r.to_string(),
                (_, None) => "None".to_owned(),
            };
            let shown = match purchasing.as_ref().map(|p| p.state) {
                Some(PurchasingState::Resolved) => named,
                _ => format!("{named} · unresolved"),
            };
            choose = kit::summary_row(ui, &shown, Some("Choose…"));
            if instance.part.is_some() {
                kit::actions(ui, |ui| {
                    if ui_kit::button(ui, "Clear", Button::Normal)
                        .on_hover_text("Return the instance to a generic component")
                        .clicked()
                    {
                        output = Some(resolve_parts::assign(vec![instance.id], None));
                    }
                });
            }
        });
        if choose {
            self.palette.open_assign(
                resolve_parts::query_for(engine, instance.id),
                vec![instance.id],
            );
        }
        output
    }

    /// PIN FUNCTIONS, folded by default unless the component declares
    /// peripherals: Pin assignment… (C6) when it does, a live choice per
    /// multi-function terminal, then the fixed terminals read-only. A terminal with no
    /// active function is an Issue, not a line here.
    fn function_section(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
        instance: &pcbcraft_core::ComponentInstance,
    ) -> Option<Operation> {
        let project = engine.project();
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
        // C6: a component with peripherals opens on Pin assignment….
        let peripherals = !resolved.component.definition.resources.is_empty();
        kit::section(
            ui,
            "functions",
            "Pin functions",
            peripherals,
            Some(count),
            |ui| {
                crate::pin_mux::row(ui, engine, instance);
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
                for (terminal, functions) in &fixed {
                    let label = functions
                        .functions
                        .first()
                        .map_or("—", |f| f.label.as_str());
                    kit::value_row(ui, &terminal.name, &format!("{label} · fixed"));
                }
            },
        );
        output
    }
}

/// While a value is typed and before it commits: whether it contradicts
/// the instance's pinned Part, through the same `fields::satisfies`
/// comparison purchasing uses ("EX-R0603-10K0-F is rated 10 kΩ").
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
        return Some(format!("{mpn} does not rate {}", buffer.label));
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
        "part.spec_mismatch" => format!("{mpn} is rated {rated}"),
        _ => format!("{text} cannot be checked against {mpn}"),
    })
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
