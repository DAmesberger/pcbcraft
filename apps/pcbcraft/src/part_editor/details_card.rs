//! The component-level sections of the Component workspace's Properties
//! card: Definition, Fields, Provenance, placed-instance updates and the
//! library folder actions. They show on every tab.
use super::{FieldText, PartEditor};
use crate::kit::{self, Tone};
use egui::Ui;
use pcbcraft_core::fields::{self, FieldDefinition, FieldSelection, FieldValue, MAX_FIELDS};
use pcbcraft_core::library::{
    ComponentDefinition, FieldView, Library, Published, designator_prefix_valid,
};
use pcbcraft_core::{DefinitionId, Engine, FieldRef};
use solidcraft_ui_egui::ui_kit::{self, Button};

impl PartEditor {
    /// Name, designator prefix and revision state of the component.
    pub(super) fn definition_section(&mut self, ui: &mut Ui, engine: &Engine, unpublished: bool) {
        let Some(set) = &mut self.draft else {
            return;
        };
        let published = set.published(engine).map(|r| r.revision.0);
        kit::section(ui, "definition", "Definition", true, None, |ui| {
            kit::text_row(ui, "Name", &mut set.metas.component.name);
            let prefix = &mut set.component.designator_prefix;
            let invalid = !designator_prefix_valid(prefix);
            let response = kit::checked_text_row(ui, "Designator", prefix, "R", invalid)
                .on_hover_text(
                    "Prefix new instances are numbered under: 1–8 letters, e.g. R, C, U",
                );
            if response.changed() {
                prefix.make_ascii_uppercase();
            }
            let revision = match published {
                None => "Draft · new".to_owned(),
                Some(r) if unpublished => format!("r{r} · unpublished edits"),
                Some(r) => format!("r{r} · published"),
            };
            kit::value_row(ui, "Revision", &revision);
        });
    }

    /// Where the component's data comes from.
    pub(super) fn provenance_section(&mut self, ui: &mut Ui) {
        if let Some(set) = &mut self.draft {
            kit::section(ui, "provenance", "Provenance", false, None, |ui| {
                kit::text_row(ui, "Source", &mut set.metas.component.provenance);
                kit::note(
                    ui,
                    "Structural validation does not replace a datasheet review.",
                );
            });
        }
    }

    /// Library folders (S3): Promote to library… and Duplicate of the
    /// published revision; the app runs them.
    pub(super) fn library_section(&mut self, ui: &mut Ui, engine: &Engine) {
        if let Some(set) = &self.draft
            && let Some(component) = set.published(engine)
        {
            crate::library_sources::component_section(
                ui,
                engine,
                component,
                &set.metas.component.name,
            );
        }
    }

    /// Offer to move placed instances to the published revision.
    pub(super) fn update_section(&mut self, ui: &mut Ui, engine: &Engine) {
        let Some(target) = self.draft.as_ref().and_then(|s| s.published(engine)) else {
            return;
        };
        let candidates: Vec<_> = engine
            .project()
            .instances
            .iter()
            .filter(|i| i.component.id == target.id && i.component.revision != target.revision)
            .collect();
        self.update_instances
            .retain(|id| candidates.iter().any(|i| i.id == *id));
        if candidates.is_empty() {
            return;
        }
        let selected = &mut self.update_instances;
        let begin = kit::section(
            ui,
            "update-instances",
            "Update placed instances",
            true,
            Some(candidates.len()),
            |ui| {
                kit::note(
                    ui,
                    &format!(
                        "Use published revision {} for the selected instances.",
                        target.revision.0
                    ),
                );
                for instance in &candidates {
                    let mut checked = selected.contains(&instance.id);
                    let label = format!(
                        "{} · revision {}",
                        instance.designator, instance.component.revision.0
                    );
                    if kit::check_row(ui, &label, &mut checked) {
                        if checked {
                            selected.insert(instance.id);
                        } else {
                            selected.remove(&instance.id);
                        }
                    }
                }
                let mut begin = false;
                kit::actions(ui, |ui| {
                    let style = if selected.is_empty() {
                        Button::Enabled(false)
                    } else {
                        Button::Primary
                    };
                    begin = ui_kit::button(ui, "Preview update", style).clicked()
                        && !selected.is_empty();
                });
                begin
            },
        );
        if begin == Some(true) {
            self.update.begin(
                engine,
                pcbcraft_core::updates::Change::ComponentRevision {
                    instances: self.update_instances.iter().copied().collect(),
                    target,
                    resolutions: Default::default(),
                },
            );
        }
    }

    /// The component's selected fields with their component defaults.
    pub(super) fn fields_section(&mut self, ui: &mut Ui, engine: &Engine) {
        let library = &engine.project().library;
        let Some(set) = &mut self.draft else {
            return;
        };
        let component = &mut set.component;
        let buffers = &mut self.field_buffers;
        buffers.retain(|lineage, _| component.field(*lineage).is_some());
        let count = component.fields.len();
        kit::section(ui, "fields", "Fields", true, Some(count), |ui| {
            let mut remove = None;
            for index in 0..component.fields.len() {
                let Some(selection) = component.fields.get(index).cloned() else {
                    continue;
                };
                let lineage = selection.field.id;
                if index > 0 {
                    kit::rule(ui);
                }
                ui.push_id(("field", lineage), |ui| {
                    let Some(view) = library.field_view(selection.field) else {
                        kit::value_row(ui, "Field", &selection.field.to_string());
                        kit::status(
                            ui,
                            "This field revision is not in the project library.",
                            Tone::Error,
                        );
                        return;
                    };
                    let shown = default_text(selection.default.as_ref(), &view);
                    let buffer = buffers
                        .entry(lineage)
                        .or_insert_with(|| FieldText::new(shown.clone()));
                    if buffer.loaded != shown {
                        *buffer = FieldText::new(shown);
                    }
                    let error = parse_default(&buffer.text, &view).err();
                    let hint = view
                        .field
                        .definition
                        .default
                        .as_ref()
                        .map(|d| default_text(Some(d), &view))
                        .unwrap_or_else(|| "No default".into());
                    let invalid = error.is_some();
                    let response = kit::row(ui, &view.field.meta.name, |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui_kit::button(ui, "Remove", Button::Small)
                                .on_hover_text("Stop selecting this field")
                                .clicked()
                            {
                                remove = Some(index);
                            }
                            ui.spacing_mut().text_edit_width = ui.available_width().max(0.0);
                            kit::text_field(ui, "default", &mut buffer.text, &hint, invalid)
                                .on_hover_text("Component default; empty uses the field default")
                        })
                        .inner
                    });
                    if response.changed()
                        && let Ok(stored) =
                            set_field_default(component, library, lineage, &buffer.text)
                    {
                        buffer.loaded = stored;
                    }
                    let error = parse_default(&buffer.text, &view).err();
                    if !response.has_focus() && error.is_none() && buffer.text != buffer.loaded {
                        buffer.text = buffer.loaded.clone();
                    }
                    if let Some(error) = &error {
                        kit::status(ui, error, Tone::Error);
                    }
                    if let Some(selection) = component.fields.get_mut(index) {
                        kit::check_row(ui, "Show on symbol", &mut selection.show_on_symbol);
                    }
                });
            }
            if let Some(index) = remove
                && index < component.fields.len()
            {
                component.fields.remove(index);
            }
            if count > 0 {
                kit::rule(ui);
            }
            let addable = addable_fields(component, library);
            if addable.is_empty() {
                kit::note(ui, "Every published field is selected.");
            } else if component.fields.len() >= MAX_FIELDS {
                kit::note(ui, "Maximum 64 fields per component.");
            } else {
                let mut chosen = None;
                kit::row(ui, "Add field", |ui| {
                    kit::dropdown_menu(ui, "add-field", "Choose a field", |ui| {
                        for field in &addable {
                            let label = format!("{} · r{}", field.meta.name, field.revision.0);
                            if ui.selectable_label(false, label).clicked() {
                                chosen = Some(FieldRef::new(field.id, field.revision));
                            }
                        }
                    });
                });
                if let Some(field) = chosen {
                    component.fields.push(FieldSelection {
                        field,
                        default: None,
                        show_on_symbol: true,
                    });
                }
            }
        });
    }
}

/// Parse a component default for a field: empty text is no default.
fn parse_default(text: &str, view: &FieldView<'_>) -> Result<Option<FieldValue>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    fields::parse_value(text, &view.field.definition.value_type, view.kind_body())
        .map(Some)
        .map_err(|e| e.to_string())
}

/// A default as the field displays it; empty for none.
fn default_text(value: Option<&FieldValue>, view: &FieldView<'_>) -> String {
    value
        .map(|v| fields::display(v, &view.field.definition.value_type, view.kind_body()))
        .unwrap_or_default()
}

/// Set the component default of a selected field from text. Invalid text
/// leaves the component unchanged. Returns the stored default as text.
pub(super) fn set_field_default(
    component: &mut ComponentDefinition,
    library: &Library,
    lineage: DefinitionId,
    text: &str,
) -> Result<String, String> {
    let selection = component
        .fields
        .iter_mut()
        .find(|f| f.field.id == lineage)
        .ok_or("The component does not select this field")?;
    let view = library
        .field_view(selection.field)
        .ok_or("The field is not in the project library")?;
    let value = parse_default(text, &view)?;
    let shown = default_text(value.as_ref(), &view);
    selection.default = value;
    Ok(shown)
}

/// Published field lineages the component does not select yet, at their
/// latest revision.
fn addable_fields<'a>(
    component: &ComponentDefinition,
    library: &'a Library,
) -> Vec<&'a Published<FieldDefinition>> {
    let mut latest: Vec<&Published<FieldDefinition>> = Vec::new();
    for field in &library.fields {
        if component.field(field.id).is_some() {
            continue;
        }
        match latest.iter_mut().find(|f| f.id == field.id) {
            Some(slot) if slot.revision < field.revision => *slot = field,
            Some(_) => {}
            None => latest.push(field),
        }
    }
    latest.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
    latest
}
