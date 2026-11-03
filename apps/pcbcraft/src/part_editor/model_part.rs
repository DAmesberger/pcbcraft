//! The 3D tab's part-file model (docs/mechanical-parts.md M5): Part file…
//! opens the part-file chooser (`crate::part_chooser`) and Use applies the
//! one operation `library.use_part_file` applies
//! (`part_files::use_part_file_operation`). Once the package uses a part
//! file, the Model section shows its file, licence and size and its
//! parameters as a form; each committed field (Enter or focus loss) is
//! evaluated off the UI thread and then sent as the one operation of
//! `library.set_part_values` (`part_files::set_part_values_operation`):
//! one undo step. Use generated model returns to the package's own model:
//! the generated model of a typed form, the envelope of a generic one.
use super::PartEditor;
use super::model_place;
use crate::kit;
use crate::kit::severity::Level;
use crate::part_chooser::{self, ChooserMode, Evaluator, PartSource};
use egui::Ui;
use pcbcraft_automation::part_files::{self, UsePartFile};
use pcbcraft_core::library::package_model::PartFileModel;
use pcbcraft_core::library::{
    GeneratedMaterials, PackageDefinition, PackageEdit, PackageForm, PackageModel,
};
use pcbcraft_core::materials::MaterialId;
use pcbcraft_core::{DefinitionId, Engine, ModelRef, Operation};
use pcbcraft_solidcraft::models::part_file::PartValues;

/// The Model section's part-file state: the parameters' evaluation, the
/// edited values and a commit waiting for its check.
#[derive(Default)]
pub struct PartFileCard {
    pub(super) evaluator: Evaluator,
    /// The package, model and stored values the form was filled from.
    shown_for: Option<(DefinitionId, ModelRef, PartValues)>,
    editing: PartValues,
    /// Committed values whose check is running; sent once it passes.
    pub(super) pending: Option<PartValues>,
}

/// The model Use generated model sets: the generated model for a typed
/// form, the envelope for a generic one.
pub fn own_model(package: &PackageDefinition) -> PackageModel {
    if package.form == PackageForm::Generic {
        PackageModel::Envelope {
            material: MaterialId::EpoxyMould,
        }
    } else {
        PackageModel::Generated {
            family_rev: crate::model_library::GENERATED_FAMILY_REV,
            materials: GeneratedMaterials::default(),
        }
    }
}

impl PartEditor {
    /// The part-file chooser (for the Component workspace).
    pub fn part_chooser_mut(&mut self) -> &mut part_chooser::PartChooser {
        &mut self.part_chooser
    }

    /// Open the chooser at the package's part file, or the first built-in.
    pub(super) fn open_part_chooser(&mut self) {
        let current = self
            .draft
            .as_ref()
            .and_then(|set| match &set.package.model {
                PackageModel::PartFile(part) => {
                    Some((PartSource::Model(part.model), part.values.clone()))
                }
                _ => None,
            });
        let (source, values) = current.map_or((None, PartValues::new()), |(s, v)| (Some(s), v));
        self.part_chooser.open(ChooserMode::Use, source, values);
    }

    /// The chooser sheet; Use builds `library.use_part_file`'s operation
    /// for the open package draft.
    pub(super) fn part_chooser_sheet(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
    ) -> Option<Operation> {
        let pick = self.part_chooser.show(ctx, engine)?;
        let Some(set) = self.draft.as_ref() else {
            self.part_chooser
                .refused("Open a component to use a part file".into());
            return None;
        };
        let request = UsePartFile {
            package: set.ids.package,
            file: pick.file.as_ref().clone(),
            values: pick.values.clone(),
            name: None,
            licence: pick.licence,
            redistribution: None,
        };
        match part_files::use_part_file_operation(engine, &request) {
            Ok((operation, _)) => {
                self.part_chooser.close();
                kit::toast::post(
                    ctx,
                    Level::Info,
                    format!(
                        "Uses {}: {} bodies",
                        request.file.file_name,
                        pick.report.bodies.len()
                    ),
                    Vec::new(),
                );
                Some(operation)
            }
            Err(error) => {
                self.part_chooser
                    .refused(part_chooser::shown_message(&error.diagnostic().message));
                None
            }
        }
    }

    /// The Model section's rows for a package using a part file: file,
    /// licence and size, then the parameters form. A committed edit is
    /// returned as one operation once its check passes.
    pub(super) fn part_file_rows(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        draft: DefinitionId,
        part: &PartFileModel,
    ) -> Option<Operation> {
        let ctx = ui.ctx().clone();
        let card = &mut self.part_file;
        let key = (draft, part.model, part.values.clone());
        if card.shown_for.as_ref() != Some(&key) {
            card.editing = part.values.clone();
            card.pending = None;
            card.shown_for = Some(key);
        }
        card.evaluator.poll();
        let values = card.pending.clone().unwrap_or_else(|| part.values.clone());
        card.evaluator
            .request(&ctx, engine, &PartSource::Model(part.model), &values);
        let mut operation = None;
        if let Some(pending) = card.pending.clone()
            && let Some(done) = card.evaluator.current()
        {
            let blocked = match &done.result {
                Ok(checked) => checked
                    .report
                    .findings
                    .iter()
                    .find(|f| f.blocking)
                    .map(|f| f.message.clone()),
                Err(error) => Some(error.clone()),
            };
            card.pending = None;
            match blocked {
                Some(why) => {
                    card.editing = part.values.clone();
                    kit::toast::post(&ctx, Level::Error, why, Vec::new());
                }
                None => match part_files::set_part_values_operation(engine, draft, &pending) {
                    Ok((op, _)) => operation = Some(op),
                    Err(error) => {
                        card.editing = part.values.clone();
                        kit::toast::post(
                            &ctx,
                            Level::Error,
                            part_chooser::shown_message(&error.diagnostic().message),
                            Vec::new(),
                        );
                    }
                },
            }
        }
        if let Some((_, definition)) = crate::model_library::model_definition(engine, part.model) {
            kit::value_row(ui, "File", &definition.asset.file_name);
            kit::value_row(
                ui,
                "File size",
                &part_chooser::size_label(definition.asset.bytes),
            );
        }
        // The last report stays up while a newer one is evaluated.
        let report = card
            .evaluator
            .last()
            .filter(|e| e.source == PartSource::Model(part.model))
            .and_then(|e| e.result.as_ref().ok())
            .map(|c| c.report.clone());
        match report {
            Some(report) => {
                let editing = &mut card.editing;
                let pending = &mut card.pending;
                let busy = card.evaluator.busy();
                kit::section(
                    ui,
                    "model-part-values",
                    "Part parameters",
                    true,
                    Some(report.parameters.len()),
                    |ui| {
                        if let Some(values) = part_chooser::values_form(
                            ui,
                            "model-card",
                            &report.parameters,
                            editing,
                            &part.values,
                        ) {
                            *pending = Some(values);
                        }
                        if busy {
                            kit::note(ui, "Checking…");
                        }
                    },
                );
            }
            None => {
                let error = card
                    .evaluator
                    .last()
                    .and_then(|e| e.result.as_ref().err())
                    .cloned();
                match error {
                    Some(error) => kit::note(ui, &error),
                    None => kit::note(ui, "Reading the part file's parameters…"),
                };
            }
        }
        operation
    }
}

/// `library.edit_package { SetModel }` back to the package's own model.
pub fn use_own_model(draft: DefinitionId, package: &PackageDefinition) -> Operation {
    model_place::edit(draft, vec![PackageEdit::SetModel(own_model(package))])
}
