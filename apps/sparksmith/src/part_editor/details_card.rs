//! The Details tab's Properties card: the component-level data that used
//! to repeat under every geometry tab (docs/sidebar-ia.md, "Properties card
//! by subject"). DEFINITION, FIELDS, PARTS, LIBRARY and PROVENANCE, in that
//! order; the footer's Update placed instances… opens the instance review.
use super::{FieldText, PartEditor};
use crate::kit;
use egui::Ui;
use sparksmith_core::fields::{self, FieldDefinition, FieldSelection, FieldValue, MAX_FIELDS};
use sparksmith_core::library::{
    ComponentDefinition, FieldView, Library, Published, designator_prefix_valid, standard_fields,
};
use sparksmith_core::{DefinitionId, Engine, FieldRef, Id, LibraryCommand, Operation};
use sparksmith_ui::ui_kit::{self, Button};

impl PartEditor {
    /// The Details card body. Every edit commits live through the working
    /// copy; the Parts section issues its own typed operations.
    pub(super) fn details_card(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let unpublished = self.unpublished(engine);
        self.definition_section(ui, engine, unpublished);
        let standard = self.fields_section(ui, engine);
        let operation = self.part_section(ui, engine, None).or(standard);
        self.library_section(ui, engine);
        self.provenance_section(ui);
        operation
    }

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
                // The editor works on a draft of r{r}, unchanged: r{r} as
                // published, as the bar says.
                Some(r) => format!("r{r} · published"),
            };
            kit::value_row(ui, "Revision", &revision);
        });
    }

    /// Where the component's data comes from.
    pub(super) fn provenance_section(&mut self, ui: &mut Ui) {
        if let Some(set) = &mut self.draft {
            kit::section(ui, "provenance", "Provenance", false, None, |ui| {
                kit::text_row(ui, "Source", &mut set.metas.component.provenance)
                    .on_hover_text("Where the component's data comes from: a datasheet, a URL");
                kit::hint(
                    ui,
                    "Checks do not replace a datasheet review",
                    "Structural checks confirm the drafts are consistent, not that they \
                     match the real part",
                );
            });
        }
    }

    /// Library folders (S3): Promote to library… and Duplicate of the
    /// published revision; the app runs them.
    pub(super) fn library_section(&mut self, ui: &mut Ui, engine: &Engine) {
        let Some(set) = &self.draft else {
            return;
        };
        match set.published(engine) {
            Some(component) => crate::library_sources::component_section(
                ui,
                engine,
                component,
                &set.metas.component.name,
            ),
            // Promote and Duplicate act on a published revision.
            None => {
                kit::section(ui, "library-source", "Library", true, None, |ui| {
                    kit::value_row(ui, "Source", "This project · not published yet");
                });
            }
        }
    }

    /// Placed instances of an older revision that Update placed instances…
    /// would move to the published one; none while edits are unpublished.
    pub(super) fn update_candidates(&self, engine: &Engine) -> Vec<Id> {
        if self.unpublished(engine) {
            return Vec::new();
        }
        let Some(target) = self.draft.as_ref().and_then(|s| s.published(engine)) else {
            return Vec::new();
        };
        engine
            .project()
            .instances
            .iter()
            .filter(|i| i.component.id == target.id && i.component.revision != target.revision)
            .map(|i| i.id)
            .collect()
    }

    /// Open the instance review for `instances`, each one chosen there.
    pub(super) fn begin_instance_update(&mut self, engine: &Engine, instances: Vec<Id>) {
        let Some(target) = self.draft.as_ref().and_then(|s| s.published(engine)) else {
            return;
        };
        if instances.is_empty() {
            return;
        }
        self.update.begin_choosing(
            engine,
            sparksmith_core::updates::Change::ComponentRevision {
                instances,
                target,
                resolutions: Default::default(),
            },
            None,
        );
    }

    /// The component's selected fields with their component defaults.
    /// Returns the import of the built-in fields when it is asked for.
    pub(super) fn fields_section(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let library = &engine.project().library;
        let set = self.draft.as_mut()?;
        let mut import = None;
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
                    // Issues reports the missing revision; the row only
                    // says what it is.
                    let Some(view) = library.field_view(selection.field) else {
                        kit::value_row(ui, "Field", "Missing from the project library");
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
                        kit::field_error(ui, &response, error, true);
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
                kit::hint(ui, "Every published field is selected", "");
            } else if component.fields.len() >= MAX_FIELDS {
                kit::hint(
                    ui,
                    &format!("A component selects at most {MAX_FIELDS} fields"),
                    "",
                );
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
            if !has_standard_fields(library)
                && ui_kit::button(ui, "Add standard fields", Button::Small)
                    .on_hover_text("Resistance, capacitance, voltage and other SI fields")
                    .clicked()
            {
                import = Some(Operation::Library(LibraryCommand::ImportStandardFields {}));
            }
        });
        import
    }
}

/// Whether the project holds the built-in fields already.
fn has_standard_fields(library: &Library) -> bool {
    let source = standard_fields::library_id();
    library
        .origins
        .iter()
        .any(|o| Some(o.origin.library) == source)
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
