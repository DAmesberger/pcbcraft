use super::{OffGridPin, Outcome};
use crate::diagnostics::{Error, Result};
use crate::ids::{DefinitionId, DefinitionRef};
use crate::library::footprint_edit::{self, AssignRule};
use crate::library::footprint_source::FootprintEdit;
use crate::library::graphics::GraphicEdit;
use crate::library::templates::footprint_family::{self, FootprintTemplate};
use crate::library::{self, Definition, DefinitionMeta, DraftEdit, Grid, drafts};
use crate::model::Document;
use crate::scripting;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum LibraryCommand {
    /// Reserve a new lineage of any kind and draft it. The body may be empty.
    #[serde(rename = "library.new_draft")]
    NewDraft {
        meta: DefinitionMeta,
        definition: Definition,
    },
    /// Draft an existing lineage starting from one of its revisions.
    #[serde(rename = "library.draft_from")]
    DraftFrom { revision: DefinitionRef },
    /// Replace 1–16 draft bodies atomically, as one undo step.
    #[serde(rename = "library.edit")]
    Edit { edits: Vec<DraftEdit> },
    /// Drop a draft. A reserved new-lineage identity stays allocated.
    #[serde(rename = "library.discard_draft")]
    DiscardDraft { id: DefinitionId },
    /// Publish 1–32 drafts together, in dependency order.
    #[serde(rename = "library.publish")]
    Publish { drafts: Vec<DefinitionId> },
    /// Regenerate a footprint draft's existing pads with a bounded script.
    /// Pad IDs and package-terminal associations are preserved.
    #[serde(rename = "library.generate_pads")]
    GeneratePads {
        draft: DefinitionId,
        script: scripting::Script,
    },
    /// Replace the authoring grid (an undoable editor setting saved with the
    /// drafts). Geometry never moves; the outcome lists draft pins that are
    /// now off the symbol grid.
    #[serde(rename = "library.set_grid")]
    SetGrid { grid: Grid },
    /// Apply 1–256 graphic and pin edits to one symbol draft, in order, as
    /// one undo step; any failure leaves the draft unchanged. The outcome
    /// lists the IDs `add` allocated.
    #[serde(rename = "library.edit_graphics")]
    EditGraphics {
        draft: DefinitionId,
        edits: Vec<GraphicEdit>,
    },
    /// Apply 1–256 parametric edits to one footprint draft, in order, then
    /// evaluate once and store source and solved pads together, as one
    /// undo step; any failure (including a blocking evaluation finding)
    /// leaves the draft unchanged. A draft without pads gains a source on
    /// its first edit. Needs the host's footprint evaluator.
    #[serde(rename = "library.edit_footprint")]
    EditFootprint {
        draft: DefinitionId,
        edits: Vec<FootprintEdit>,
    },
    /// Build a family template (chip, gull-wing, QFN/DFN, pin header) into
    /// an empty footprint draft: the template's edits (named parameters,
    /// template pads, arrays, the pin-1 polarity and explicit derived
    /// courtyard and assembly roles) go through the same path as
    /// `library.edit_footprint`, evaluated once, as one undo step; any
    /// failure leaves the draft unchanged. Terminals stay unassigned.
    /// Needs the host's footprint evaluator.
    #[serde(rename = "library.footprint_from_template")]
    FootprintFromTemplate {
        draft: DefinitionId,
        template: Box<FootprintTemplate>,
    },
    /// Land package terminals on footprint pads by an explicit rule. With
    /// `confirm: false` it only reports the changes; nothing is ever
    /// inferred without this command.
    #[serde(rename = "library.assign_terminals")]
    AssignTerminals {
        draft: DefinitionId,
        rule: AssignRule,
        confirm: bool,
    },
}

fn footprint_draft(
    document: &Document,
    draft: DefinitionId,
) -> Result<(&library::Draft, &library::FootprintDefinition)> {
    let current = document
        .drafts
        .get(draft)
        .ok_or_else(|| Error::NotFound(format!("there is no draft of {draft}")))?;
    match &current.definition {
        Definition::Footprint(footprint) => Ok((current, footprint)),
        _ => Err(Error::Invalid(format!("draft {draft} is not a footprint"))),
    }
}

fn package_of<'a>(
    document: &'a Document,
    footprint: &library::FootprintDefinition,
) -> Result<&'a library::PackageDefinition> {
    footprint_edit::footprint_package(
        &document.project.library,
        &document.drafts,
        footprint.package,
    )
    .ok_or_else(|| {
        Error::NotFound(format!(
            "package {} {} is not in the library or the drafts",
            footprint.package.id, footprint.package.revision
        ))
    })
}

fn need(
    evaluator: Option<&dyn library::FootprintEvaluator>,
) -> Result<&dyn library::FootprintEvaluator> {
    evaluator.ok_or_else(|| {
        Error::Unsupported(
            "footprint.evaluator_required: parametric footprints need the host's footprint evaluator".into(),
        )
    })
}

/// `library.footprint_preview`: what `library.edit_footprint` would do.
pub(crate) fn preview_footprint(
    document: &Document,
    draft: DefinitionId,
    edits: &[FootprintEdit],
    evaluator: Option<&dyn library::FootprintEvaluator>,
) -> Result<footprint_edit::FootprintPreview> {
    let (_, footprint) = footprint_draft(document, draft)?;
    let package = package_of(document, footprint)?;
    footprint_edit::preview(footprint, package, edits, need(evaluator)?)
}

/// `evaluator` is the host's footprint evaluator (`Engine::apply_with`);
/// footprint-source edits refuse without one.
pub(crate) fn apply_library(
    document: &mut Document,
    command: LibraryCommand,
    evaluator: Option<&dyn library::FootprintEvaluator>,
) -> Result<Outcome> {
    match command {
        LibraryCommand::NewDraft { meta, definition } => {
            library::check_bounds(&meta, &definition)?;
            let project = Arc::make_mut(&mut document.project);
            let id = DefinitionId(project.allocate()?);
            let drafts = Arc::make_mut(&mut document.drafts);
            drafts::create(drafts, &project.library, id, None, meta, definition)?;
            Ok(Outcome::DraftCreated { id })
        }
        LibraryCommand::DraftFrom { revision } => {
            let (meta, definition) =
                document
                    .project
                    .library
                    .definition(revision)
                    .ok_or_else(|| {
                        Error::NotFound("that revision is not in the project library".into())
                    })?;
            let drafts = Arc::make_mut(&mut document.drafts);
            drafts::create(
                drafts,
                &document.project.library,
                revision.id,
                Some(revision.revision),
                meta,
                definition,
            )?;
            Ok(Outcome::DraftCreated { id: revision.id })
        }
        LibraryCommand::Edit { edits } => {
            drafts::edit(Arc::make_mut(&mut document.drafts), edits)?;
            Ok(Outcome::None)
        }
        LibraryCommand::DiscardDraft { id } => {
            drafts::discard(Arc::make_mut(&mut document.drafts), id)?;
            Ok(Outcome::None)
        }
        LibraryCommand::Publish { drafts } => {
            // The pure publish rules run first, so their findings are
            // reported even without an evaluator; the candidate document
            // is discarded if the freshness check below then refuses.
            let project = Arc::make_mut(&mut document.project);
            let revisions = library::publish(
                &mut project.library,
                Arc::make_mut(&mut document.drafts),
                &drafts,
            )?;
            for id in &drafts {
                if let Some(library::Draft {
                    definition: Definition::Footprint(footprint),
                    ..
                }) = document.drafts.get(*id)
                    && footprint.source.is_some()
                {
                    let package = package_of(document, footprint)?;
                    footprint_edit::check_fresh(footprint, package, need(evaluator)?)?;
                }
            }
            Ok(Outcome::Published { revisions })
        }
        LibraryCommand::GeneratePads { draft, script } => {
            let current = document
                .drafts
                .get(draft)
                .ok_or_else(|| Error::NotFound(format!("there is no draft of {draft}")))?;
            let Definition::Footprint(footprint) = &current.definition else {
                return Err(Error::Invalid(
                    "pad generation requires a footprint draft".into(),
                ));
            };
            if footprint.source.is_some() {
                return Err(Error::Conflict(
                    "footprint.derived: this footprint's pads are derived from its sketch; detach it (library.edit_footprint Detach) before scripting pads".into(),
                ));
            }
            let generated = scripting::generate_pads(&script, footprint)?;
            let edit = DraftEdit {
                id: draft,
                meta: current.meta.clone(),
                definition: Definition::Footprint(generated),
            };
            drafts::edit(Arc::make_mut(&mut document.drafts), vec![edit])?;
            Ok(Outcome::None)
        }
        LibraryCommand::SetGrid { grid } => {
            let drafts = Arc::make_mut(&mut document.drafts);
            drafts::set_grid(drafts, grid)?;
            let off_grid = drafts
                .items
                .iter()
                .filter_map(|d| match &d.definition {
                    Definition::Symbol(symbol) => Some((d.id, symbol)),
                    _ => None,
                })
                .flat_map(|(draft, symbol)| {
                    symbol
                        .units
                        .iter()
                        .flat_map(|u| u.pins.iter())
                        .filter(|p| !library::graphics::on_grid(p.position, grid.symbol_mm))
                        .map(move |p| OffGridPin {
                            draft,
                            terminal: p.terminal,
                        })
                })
                .collect();
            Ok(Outcome::GridSet { off_grid })
        }
        LibraryCommand::EditGraphics { draft, edits } => {
            let current = document
                .drafts
                .get(draft)
                .ok_or_else(|| Error::NotFound(format!("there is no draft of {draft}")))?;
            let terminals = match &current.definition {
                Definition::Symbol(symbol) => library::graphics::owner_terminals(
                    &document.drafts,
                    &document.project.library,
                    symbol.component,
                ),
                _ => Default::default(),
            };
            let (definition, added) =
                library::graphics::apply(&current.definition, &terminals, &edits)?;
            let edit = DraftEdit {
                id: draft,
                meta: current.meta.clone(),
                definition,
            };
            drafts::edit(Arc::make_mut(&mut document.drafts), vec![edit])?;
            Ok(Outcome::GraphicsEdited { added })
        }
        LibraryCommand::FootprintFromTemplate { draft, template } => {
            let (current, footprint) = footprint_draft(document, draft)?;
            let package = package_of(document, footprint)?;
            let (definition, report) =
                footprint_family::build(footprint, package, &template, need(evaluator)?)?;
            let edit = DraftEdit {
                id: draft,
                meta: current.meta.clone(),
                definition: Definition::Footprint(definition),
            };
            drafts::edit(Arc::make_mut(&mut document.drafts), vec![edit])?;
            Ok(Outcome::FootprintEdited { report })
        }
        LibraryCommand::EditFootprint { draft, edits } => {
            let (current, footprint) = footprint_draft(document, draft)?;
            let package = package_of(document, footprint)?;
            let (definition, report) =
                footprint_edit::edit(footprint, package, &edits, need(evaluator)?)?;
            let edit = DraftEdit {
                id: draft,
                meta: current.meta.clone(),
                definition: Definition::Footprint(definition),
            };
            drafts::edit(Arc::make_mut(&mut document.drafts), vec![edit])?;
            Ok(Outcome::FootprintEdited { report })
        }
        LibraryCommand::AssignTerminals {
            draft,
            rule,
            confirm,
        } => {
            let (current, footprint) = footprint_draft(document, draft)?;
            let package = package_of(document, footprint)?;
            let (definition, changes) =
                footprint_edit::assign_terminals(footprint, package, &rule)?;
            if confirm && !changes.is_empty() {
                let edit = DraftEdit {
                    id: draft,
                    meta: current.meta.clone(),
                    definition: Definition::Footprint(definition),
                };
                drafts::edit(Arc::make_mut(&mut document.drafts), vec![edit])?;
            }
            Ok(Outcome::TerminalsAssigned {
                changes,
                applied: confirm,
            })
        }
    }
}
