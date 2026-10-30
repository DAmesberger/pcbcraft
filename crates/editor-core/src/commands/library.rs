use super::{OffGridPin, Outcome};
use crate::diagnostics::{Error, Result};
use crate::ids::{DefinitionId, DefinitionRef};
use crate::library::graphics::GraphicEdit;
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
}

pub(crate) fn apply_library(document: &mut Document, command: LibraryCommand) -> Result<Outcome> {
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
            let project = Arc::make_mut(&mut document.project);
            let revisions = library::publish(
                &mut project.library,
                Arc::make_mut(&mut document.drafts),
                &drafts,
            )?;
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
    }
}
