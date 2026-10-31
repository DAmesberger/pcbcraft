use super::{OffGridPin, Outcome};
use crate::diagnostics::{Error, Result};
use crate::geometry::Point;
use crate::ids::{
    ComponentRef, DefinitionId, DefinitionKind, DefinitionRef, LibraryId, PartRef, SourceLineageId,
};
use crate::instances::{self, NewInstance};
use crate::library::Bundle;
use crate::library::footprint_edit::{self, AssignRule};
use crate::library::footprint_source::FootprintEdit;
use crate::library::graphics::GraphicEdit;
use crate::library::templates::footprint_family::{self, FootprintTemplate};
use crate::library::import::{self, OtherRevision, PlaceSource};
use crate::library::{self, Definition, DefinitionMeta, DraftEdit, Grid, drafts};
use crate::model::Document;
use crate::model::PhysicalChoice;
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
    /// Import a library bundle (a root revision and its closure) into the
    /// project snapshot as one undo step: dedupe by origin, fresh project
    /// lineages, every reference rewritten, origins recorded, assets
    /// packed. Places nothing.
    #[serde(rename = "library.import")]
    Import { bundle: Box<Bundle> },
    /// Import if needed, then place the component, as one transaction and
    /// one undo step. A bundle whose root is a Part places its component
    /// with the Part's binding and assigns the Part. The placement takes
    /// the first symbol and unit and the first implementation and
    /// footprint, as the Place palette does. With `other_revision: refuse`
    /// (the default), a project that holds another revision of the root's
    /// lineage is a `conflict` whose message starts with
    /// `library.other_revision`.
    #[serde(rename = "library.place_from")]
    PlaceFrom {
        source: PlaceSource,
        position: Point,
        #[serde(default)]
        designator: Option<String>,
        #[serde(default)]
        other_revision: OtherRevision,
    },
    /// Record the origins of `library.promote_preview` with the same
    /// arguments after io wrote its bundle to the folder. The core
    /// recomputes the plan, so the recorded origins equal what was written.
    #[serde(rename = "library.promote")]
    Promote {
        root: DefinitionRef,
        library: LibraryId,
        name: String,
        fresh: Vec<SourceLineageId>,
    },
    /// D6: copy a component revision with its symbols and bindings into new
    /// draft lineages named `name`.
    #[serde(rename = "library.duplicate")]
    Duplicate {
        component: ComponentRef,
        name: String,
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

/// The placement `library.place_from` makes of a component or Part
/// revision: its first symbol and unit and, when it has one, its first
/// implementation (a Part's own binding) with that binding's first
/// footprint, as the Place palette chooses them. A Part is assigned after
/// placing.
fn place_request(
    library: &library::Library,
    root: DefinitionRef,
    position: Point,
    designator: Option<String>,
) -> Result<(NewInstance, Option<PartRef>)> {
    let (component, binding, part) = match root.kind {
        DefinitionKind::Component => (ComponentRef::new(root.id, root.revision), None, None),
        DefinitionKind::Part => {
            let part = PartRef::new(root.id, root.revision);
            let body = library
                .part(part)
                .ok_or_else(|| Error::NotFound(format!("Part {part} is not in the project")))?;
            (
                body.definition.component,
                Some(body.definition.binding.id),
                Some(part),
            )
        }
        kind => {
            return Err(Error::Invalid(format!(
                "library.place_from places a component or a Part, not a {kind:?}"
            )));
        }
    };
    let published = library
        .component(component)
        .ok_or_else(|| Error::NotFound(format!("component {component} is not in the project")))?;
    let symbol = published
        .definition
        .representations
        .first()
        .and_then(|r| library.symbol(*r))
        .ok_or_else(|| {
            Error::Invalid(format!(
                "component ‘{}’ has no symbol to place",
                published.meta.name
            ))
        })?;
    let unit = symbol.definition.units.first().ok_or_else(|| {
        Error::Invalid(format!(
            "symbol ‘{}’ has no unit to place",
            symbol.meta.name
        ))
    })?;
    let implementation = match binding {
        Some(lineage) => published.definition.implementation(lineage),
        None => published.definition.implementations.first().copied(),
    };
    let physical = implementation.and_then(|b| {
        let footprint = library.binding(b)?.definition.footprints.first()?;
        Some(PhysicalChoice {
            binding: b.id,
            footprint: footprint.id,
        })
    });
    Ok((
        NewInstance {
            component,
            designator,
            properties: Vec::new(),
            representation: symbol.id,
            unit: unit.id,
            position,
            physical,
        },
        part,
    ))
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
        LibraryCommand::Import { bundle } => {
            library::bundle::validate_bundle(&bundle)?;
            // The library is taken out so lineages can be allocated while it
            // is edited; a failure discards the whole candidate.
            // Imported on a copy, so a bundle whose revisions are all in the
            // project already leaves the document (revision, undo) alone.
            let mut project = (*document.project).clone();
            let mut library = std::mem::take(&mut project.library);
            let mut allocate = || project.allocate().map(DefinitionId);
            let report = import::import(&mut library, &bundle, &mut allocate);
            project.library = library;
            let report = report?;
            if report.revisions.iter().any(|r| !r.reused) {
                document.project = Arc::new(project);
            }
            Ok(Outcome::Imported { report })
        }
        LibraryCommand::PlaceFrom {
            source,
            position,
            designator,
            other_revision,
        } => {
            let project = Arc::make_mut(&mut document.project);
            let (report, root) = match source {
                PlaceSource::Project { component } => (None, component.erased()),
                PlaceSource::Bundle { bundle } => {
                    library::bundle::validate_bundle(&bundle)?;
                    if other_revision == OtherRevision::Refuse {
                        import::check_other_revision(&project.library, &bundle)?;
                    }
                    let mut library = std::mem::take(&mut project.library);
                    let mut allocate = || project.allocate().map(DefinitionId);
                    let report = import::import(&mut library, &bundle, &mut allocate);
                    project.library = library;
                    let report = report?;
                    let root = report.root;
                    (Some(report), root)
                }
            };
            let (request, part) = place_request(&project.library, root, position, designator)?;
            let (instance, occurrence) = instances::place(project, request)?;
            if let Some(part) = part {
                instances::assign_part(project, &[instance], Some(part))?;
            }
            let designator = project
                .instance(instance)
                .map(|i| i.designator.clone())
                .unwrap_or_default();
            Ok(Outcome::PlacedFrom {
                import: report,
                instance,
                occurrence,
                designator,
            })
        }
        LibraryCommand::Promote {
            root,
            library,
            name,
            fresh,
        } => {
            let plan = import::export(&document.project.library, root, library, &name, &fresh)?;
            // Promoting what the library already holds records nothing and
            // takes no undo step.
            if !plan.origins.is_empty() {
                let project = Arc::make_mut(&mut document.project);
                project.library.origins.extend(plan.origins.iter().copied());
            }
            Ok(Outcome::Promoted {
                origins: plan.origins,
            })
        }
        LibraryCommand::Duplicate { component, name } => {
            let project = Arc::make_mut(&mut document.project);
            let drafts = Arc::make_mut(&mut document.drafts);
            let library = std::mem::take(&mut project.library);
            let mut allocate = || project.allocate().map(DefinitionId);
            let lineages = import::duplicate(&library, drafts, component, &name, &mut allocate);
            project.library = library;
            Ok(Outcome::Duplicated {
                lineages: lineages?,
            })
        }
    }
}
