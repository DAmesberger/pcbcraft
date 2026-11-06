use super::{OffGridPin, Outcome};
use crate::diagnostics::{Error, Result};
use crate::geometry::Point;
use crate::ids::{
    ComponentRef, DefinitionId, DefinitionKind, DefinitionRef, LibraryId, PartRef, SourceLineageId,
    TerminalId,
};
use crate::instances::{self, NewInstance};
use crate::library::Bundle;
use crate::library::footprint_edit::{self, AssignRule};
use crate::library::footprint_source::FootprintEdit;
use crate::library::graphics::GraphicEdit;
use crate::library::import::{self, OtherRevision, PlaceSource};
use crate::library::model3d::ImportModel;
use crate::library::package_fit::{self, FitRotation, LeadPair};
use crate::library::package_model::PackageEdit;
use crate::library::pad_table::{self, FootprintFromPads};
use crate::library::pinout::PinoutApply;
use crate::library::templates::catalog::{NewFromTemplate, PackageFromTemplate, SavePackageRow};
use crate::library::templates::footprint_family::{self, FootprintTemplate};
use crate::library::{self, Definition, DefinitionMeta, DraftEdit, Grid, drafts, standard_fields};
use crate::model::Document;
use crate::model::PhysicalChoice;
use crate::scripting;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// A command is built once and applied once, so `NewDraft`'s inline definition
// costs one move; boxing it would only burden its many callers.
#[allow(clippy::large_enum_variant)]
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
    /// Draft a published component revision's working set (the component,
    /// its first symbol, its first binding with that binding's package and
    /// first footprint) wherever a lineage has no draft yet, as one undo
    /// step (`Outcome::DraftsCreated`, possibly empty: nothing changes).
    #[serde(rename = "library.draft_closure")]
    DraftClosure { component: ComponentRef },
    /// "Edit a copy in this project": import a folder bundle whose root is
    /// a component or a Part (revisions the project holds are reused, as
    /// `library.import` does) and draft the component's working set as
    /// `library.draft_closure` does, as one undo step. A refusal leaves the
    /// document unchanged.
    #[serde(rename = "library.edit_copy")]
    EditCopy { bundle: Box<Bundle> },
    /// Replace 1–16 draft bodies atomically, as one undo step.
    #[serde(rename = "library.edit")]
    Edit { edits: Vec<DraftEdit> },
    /// Drop a draft. A reserved new-lineage identity stays allocated.
    #[serde(rename = "library.discard_draft")]
    DiscardDraft { id: DefinitionId },
    /// Publish 1–32 drafts together, in dependency order. `close` (the
    /// default) drops the drafts it published, in the same undo step, so a
    /// library's parts do not fill the project's 64 draft slots; with
    /// `close: false` each stays open as a draft of its new revision.
    #[serde(rename = "library.publish")]
    Publish {
        drafts: Vec<DefinitionId>,
        #[serde(default = "close_by_default")]
        close: bool,
    },
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
    /// Replace the project's symbol drawing conventions (the resistor
    /// style new generated symbols use; an undoable authoring setting
    /// saved with the drafts). Existing symbols never change.
    #[serde(rename = "library.set_symbol_conventions")]
    SetSymbolConventions {
        conventions: library::symbol_style::SymbolConventions,
    },
    /// Redraw one symbol draft as another kind (a pinout import's IC box as
    /// an LED, say), as one undo step: its unit keeps its identity, and its
    /// other graphics and pins are replaced. Pins follow the component's
    /// terminal order, or `terminals` when given (every terminal once, in
    /// the kind's order: a diode's cathode first). The terminals never
    /// change; a kind drawn for another terminal count is refused.
    #[serde(rename = "library.restyle_symbol")]
    RestyleSymbol {
        draft: DefinitionId,
        kind: library::symbol_style::SymbolKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminals: Option<Vec<TerminalId>>,
    },
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
    /// A draft that already has pads or parametric content is refused
    /// (`footprint.template_not_empty`) unless `replace` is true: then its
    /// source, pads, polarity and role-solved graphics are dropped and the
    /// template built in their place, still one atomic undo step (authored
    /// graphics and mask/paste defaults stay). Needs the host's
    /// footprint evaluator.
    #[serde(rename = "library.footprint_from_template")]
    FootprintFromTemplate {
        draft: DefinitionId,
        template: Box<FootprintTemplate>,
        /// Replace the draft's current content instead of refusing it.
        #[serde(default)]
        replace: bool,
    },
    /// Build a footprint draft from a datasheet's pad table: pad rows, plated
    /// or unplated slots and holes become an explicit footprint, with the
    /// assembly outline, silkscreen broken around the pads, courtyard and
    /// designators added, as one undo step. Terminals come from each row's
    /// `terminal`, or by number with `assign_by_number`; the rest stay
    /// unassigned and are listed. A draft with content is refused unless
    /// `replace`. No evaluator is needed.
    #[serde(rename = "library.footprint_from_pads")]
    FootprintFromPads(Box<FootprintFromPads>),
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
    /// Import the built-in standard fields (`library.standard_fields`: SI
    /// quantity kinds with resistance, capacitance, inductance, voltage,
    /// current, power and frequency fields, and a relative tolerance), as
    /// `library.import` does: one undo step, and importing them again
    /// reuses the copies and changes nothing.
    #[serde(rename = "library.import_standard_fields")]
    ImportStandardFields {},
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
    /// Apply 1–256 package edits (form, model, placement, quick action) to
    /// one package draft, in order, as one undo step; any failure leaves the
    /// draft unchanged.
    #[serde(rename = "library.edit_package")]
    EditPackage {
        draft: DefinitionId,
        edits: Vec<PackageEdit>,
    },
    /// Fit the package draft's model leads onto footprint pads. The host
    /// (app or automation, through the integration crate) may supply the
    /// lead/pad pairs; empty `pairs` derives them from the typed package
    /// form and the one footprint draft landing the package. With
    /// `confirm: false` it only reports the fit.
    #[serde(rename = "library.fit_leads")]
    FitLeads {
        draft: DefinitionId,
        pairs: Vec<LeadPair>,
        rotation: FitRotation,
        confirm: bool,
    },
    /// B1: create component, symbol, package, footprint and binding drafts
    /// from a template row (or custom family values) and a skeleton, as
    /// one undo step. Needs the host's footprint evaluator.
    #[serde(rename = "library.new_from_template")]
    NewFromTemplate(Box<NewFromTemplate>),
    /// `library.new_from_template`, then `library.publish` of its five
    /// drafts (closing them), as one undo step: a component ready to place.
    /// Any refusal changes nothing. Needs the host's footprint evaluator.
    #[serde(rename = "library.publish_from_template")]
    PublishFromTemplate(Box<NewFromTemplate>),
    /// The part a text reads as (`library.generic_parts`): the identical
    /// part if the project holds it (nothing changes), else built and
    /// published as `library.publish_from_template`, one undo step. A text
    /// that reads as several parts is refused until `candidate` picks one by
    /// its index in that report. The outcome's `properties` are the text's
    /// values for `component.place`. Needs the host's footprint evaluator.
    #[serde(rename = "library.find_or_create")]
    FindOrCreate {
        query: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        candidate: Option<usize>,
    },
    /// B1: replace the package, footprint and generated model of a working
    /// set from a template row, as one undo step; a non-empty footprint is
    /// refused unless `replace`. Terminal mappings change only through
    /// `library.assign_terminals`. Needs the host's footprint evaluator.
    #[serde(rename = "library.package_from_template")]
    PackageFromTemplate(Box<PackageFromTemplate>),
    /// B1: save a package's form (or explicit values) as a new package-row
    /// draft.
    #[serde(rename = "library.save_package_row")]
    SavePackageRow(Box<SavePackageRow>),
    /// C3: apply a pinout document as previewed by `library.pinout_preview`,
    /// atomically, as one undo step; refused when the target changed since
    /// `base`. Imported rows stay unreviewed.
    #[serde(rename = "library.pinout_apply")]
    PinoutApply(Box<PinoutApply>),
    /// D1: pack a model's asset bytes and create a Model draft; with
    /// `assign`, also use it in a package draft, one undo step.
    #[serde(rename = "library.import_model")]
    ImportModel(Box<ImportModel>),
    /// D4: apply `library.edit`'s 1–16 draft edits and pack the 1–8 SVG
    /// artwork files their markings name (`image/svg+xml` text of at most
    /// 1 MiB, each named by an `Artwork` item of an edited component
    /// marking or Part top mark), as one undo step; any refusal changes
    /// nothing.
    #[serde(rename = "library.pack_artwork")]
    PackArtwork {
        edits: Vec<DraftEdit>,
        artwork: Vec<library::PackedAsset>,
    },
}

// A new command chooses its permission class here.
declare_classes!(LibraryCommand {
    NewDraft "library.new_draft" Edit,
    DraftFrom "library.draft_from" Edit,
    DraftClosure "library.draft_closure" Edit,
    EditCopy "library.edit_copy" Edit,
    Edit "library.edit" Edit,
    DiscardDraft "library.discard_draft" Edit,
    Publish "library.publish" Ask,
    GeneratePads "library.generate_pads" Edit,
    SetGrid "library.set_grid" Edit,
    SetSymbolConventions "library.set_symbol_conventions" Edit,
    RestyleSymbol "library.restyle_symbol" Edit,
    EditGraphics "library.edit_graphics" Edit,
    EditFootprint "library.edit_footprint" Edit,
    FootprintFromTemplate "library.footprint_from_template" Edit,
    FootprintFromPads "library.footprint_from_pads" Edit,
    AssignTerminals "library.assign_terminals" Edit,
    Import "library.import" Edit,
    ImportStandardFields "library.import_standard_fields" Edit,
    PlaceFrom "library.place_from" Edit,
    Promote "library.promote" Ask,
    Duplicate "library.duplicate" Edit,
    EditPackage "library.edit_package" Edit,
    FitLeads "library.fit_leads" Edit,
    NewFromTemplate "library.new_from_template" Edit,
    PublishFromTemplate "library.publish_from_template" Ask,
    FindOrCreate "library.find_or_create" Ask,
    PackageFromTemplate "library.package_from_template" Edit,
    SavePackageRow "library.save_package_row" Edit,
    PinoutApply "library.pinout_apply" Edit,
    ImportModel "library.import_model" Edit,
    PackArtwork "library.pack_artwork" Edit,
});

fn close_by_default() -> bool {
    true
}

fn package_draft(
    document: &Document,
    draft: DefinitionId,
) -> Result<(&library::Draft, &library::PackageDefinition)> {
    let current = document
        .drafts
        .get(draft)
        .ok_or_else(|| Error::NotFound("That draft no longer exists".into()))?;
    match &current.definition {
        Definition::Package(package) => Ok((current, package)),
        _ => Err(Error::Invalid(format!(
            "The draft ‘{}’ is not a package",
            current.meta.name
        ))),
    }
}

/// The name of a draft, for a message.
fn draft_name(document: &Document, draft: DefinitionId) -> String {
    document
        .drafts
        .get(draft)
        .map(|d| d.meta.name.clone())
        .unwrap_or_default()
}

/// The one footprint draft whose pads land the package draft `draft`.
fn landing_footprint(
    document: &Document,
    draft: DefinitionId,
) -> Result<&library::FootprintDefinition> {
    let mut found = document
        .drafts
        .items
        .iter()
        .filter_map(|d| match &d.definition {
            Definition::Footprint(f) if f.package.id == draft => Some(f),
            _ => None,
        });
    match (found.next(), found.next()) {
        (Some(footprint), None) => Ok(footprint),
        (None, _) => Err(Error::NotFound(format!(
            "package.no_footprint: no footprint draft lands the package ‘{}’",
            draft_name(document, draft)
        ))),
        (Some(_), Some(_)) => Err(Error::Invalid(format!(
            "package.footprint_ambiguous: several footprint drafts land the package ‘{}’; pass lead pairs",
            draft_name(document, draft)
        ))),
    }
}

fn store_package(
    document: &mut Document,
    draft: DefinitionId,
    meta: DefinitionMeta,
    package: library::PackageDefinition,
) -> Result<()> {
    let edit = DraftEdit {
        id: draft,
        meta,
        definition: Definition::Package(package),
    };
    drafts::edit(Arc::make_mut(&mut document.drafts), vec![edit])
}

/// `library.restyle_symbol`: the draft redrawn as `kind`, its pins on the
/// component's terminals in their own order or the one the request states.
fn restyle_symbol(
    document: &mut Document,
    draft: DefinitionId,
    kind: library::symbol_style::SymbolKind,
    terminals: Option<Vec<TerminalId>>,
) -> Result<Outcome> {
    let current = document
        .drafts
        .get(draft)
        .ok_or_else(|| Error::NotFound("That draft no longer exists".into()))?;
    let Definition::Symbol(symbol) = &current.definition else {
        return Err(Error::Invalid(format!(
            "The draft ‘{}’ is not a symbol",
            current.meta.name
        )));
    };
    let named: Vec<(TerminalId, String)> = match document.drafts.get(symbol.component) {
        Some(library::Draft {
            definition: Definition::Component(component),
            ..
        }) => component
            .terminals
            .iter()
            .map(|t| (t.id, t.name.clone()))
            .collect(),
        _ => document
            .project
            .library
            .latest_components()
            .into_iter()
            .find(|c| c.id == symbol.component)
            .map(|c| {
                c.definition
                    .terminals
                    .iter()
                    .map(|t| (t.id, t.name.clone()))
                    .collect()
            })
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "The component of the symbol ‘{}’ is not in the project",
                    current.meta.name
                ))
            })?,
    };
    let owned: Vec<TerminalId> = named.iter().map(|(id, _)| *id).collect();
    let order = match terminals {
        None => owned,
        Some(order) => {
            let mut given = order.clone();
            given.sort_unstable();
            let mut expected = owned;
            expected.sort_unstable();
            if given != expected {
                return Err(Error::Invalid(
                    "symbol.restyle_terminals: list each of the component's terminals once".into(),
                ));
            }
            order
        }
    };
    let conventions = document.drafts.symbols;
    let mut definition = library::symbol_style::restyle(symbol, kind, conventions, &order)?;
    if kind == library::symbol_style::SymbolKind::IcBox {
        // The box is as wide as its pin names need.
        library::samples::fit_pin_names(&mut definition, |terminal| {
            named
                .iter()
                .find(|(id, _)| *id == terminal)
                .map(|(_, name)| name.clone())
        });
    }
    let edit = DraftEdit {
        id: draft,
        meta: current.meta.clone(),
        definition: Definition::Symbol(definition),
    };
    drafts::edit(Arc::make_mut(&mut document.drafts), vec![edit])?;
    Ok(Outcome::None)
}

/// The package draft whose pending revision the footprint draft lands, with
/// its meta: the one a command may move the body of.
fn landed_package(
    document: &Document,
    footprint: DefinitionId,
) -> Option<(DefinitionId, DefinitionMeta, library::PackageDefinition)> {
    let Definition::Footprint(landing) = &document.drafts.get(footprint)?.definition else {
        return None;
    };
    let current = document.drafts.get(landing.package.id)?;
    let Definition::Package(package) = &current.definition else {
        return None;
    };
    (current.pending() == Some(landing.package.revision))
        .then(|| (current.id, current.meta.clone(), package.clone()))
}

/// Give the package draft `footprint` lands its envelope's default place
/// ([`package_fit::envelope_default`]) now that a command built its pads.
/// `library.edit` is not one: it stores the bodies it is given as they are.
/// A package already published cannot move here. The board check reports a
/// body past the outline either way.
fn follow_envelope(document: &mut Document, footprint: DefinitionId) -> Result<()> {
    let Some((draft, meta, mut package)) = landed_package(document, footprint) else {
        return Ok(());
    };
    let Some(Definition::Footprint(landing)) =
        document.drafts.get(footprint).map(|d| &d.definition)
    else {
        return Ok(());
    };
    let Some(placement) = package_fit::envelope_default(&package, landing)? else {
        return Ok(());
    };
    package.placement = placement;
    store_package(document, draft, meta, package)
}

fn footprint_draft(
    document: &Document,
    draft: DefinitionId,
) -> Result<(&library::Draft, &library::FootprintDefinition)> {
    let current = document
        .drafts
        .get(draft)
        .ok_or_else(|| Error::NotFound("That draft no longer exists".into()))?;
    match &current.definition {
        Definition::Footprint(footprint) => Ok((current, footprint)),
        _ => Err(Error::Invalid(format!(
            "The draft ‘{}’ is not a footprint",
            current.meta.name
        ))),
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
            "The footprint's {} is not in the library or the drafts",
            document
                .project
                .library
                .describe(footprint.package.erased())
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
                "library.place_from: only a component or a Part can be placed, not a {}",
                crate::library::names::noun(kind)
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
            sheet: None,
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

/// `library.import` of `bundle` into the candidate `document`. The
/// library is taken out so lineages can be allocated while it is edited; a
/// failure discards the whole candidate. Imported on a copy, so a bundle
/// whose revisions are all in the project already leaves the document
/// (revision, undo) alone.
pub(super) fn import_bundle(
    document: &mut Document,
    bundle: &Bundle,
) -> Result<import::ImportReport> {
    library::bundle::validate_bundle(bundle)?;
    let mut project = (*document.project).clone();
    let mut library = std::mem::take(&mut project.library);
    let mut allocate = || project.allocate().map(DefinitionId);
    let report = import::import(&mut library, bundle, &mut allocate);
    project.library = library;
    let report = report?;
    if report.revisions.iter().any(|r| !r.reused) {
        document.project = Arc::new(project);
    }
    Ok(report)
}

/// Draft every lineage of `component`'s working set that has no draft:
/// the component, its first symbol, its first binding, that binding's
/// package and first footprint, each from the revision the component
/// names. Returns the drafted lineages, component first.
fn draft_closure(document: &mut Document, component: ComponentRef) -> Result<Vec<DefinitionId>> {
    let library = &document.project.library;
    let body = &library
        .component(component)
        .ok_or_else(|| {
            Error::NotFound(format!(
                "component {} r{} is not in the project library",
                component.id, component.revision
            ))
        })?
        .definition;
    let mut wanted = vec![component.erased()];
    if let Some(symbol) = body.representations.first() {
        wanted.push(symbol.erased());
    }
    if let Some(binding_ref) = body.implementations.first() {
        wanted.push(binding_ref.erased());
        if let Some(binding) = library.binding(*binding_ref) {
            wanted.push(binding.definition.package.erased());
            if let Some(footprint) = binding.definition.footprints.first() {
                wanted.push(footprint.erased());
            }
        }
    }
    let mut drafted = Vec::new();
    for revision in wanted {
        if document.drafts.get(revision.id).is_some() {
            continue;
        }
        let (meta, definition) = document
            .project
            .library
            .definition(revision)
            .ok_or_else(|| Error::NotFound("that revision is not in the project library".into()))?;
        drafts::create(
            Arc::make_mut(&mut document.drafts),
            &document.project.library,
            revision.id,
            Some(revision.revision),
            meta,
            definition,
        )?;
        drafted.push(revision.id);
    }
    Ok(drafted)
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
        LibraryCommand::DraftClosure { component } => {
            let ids = draft_closure(document, component)?;
            Ok(Outcome::DraftsCreated { ids })
        }
        LibraryCommand::EditCopy { bundle } => {
            let report = import_bundle(document, &bundle)?;
            let root = report.root;
            let component = match root.kind {
                DefinitionKind::Component => ComponentRef::new(root.id, root.revision),
                DefinitionKind::Part => document
                    .project
                    .library
                    .part(PartRef::new(root.id, root.revision))
                    .map(|part| part.definition.component)
                    .ok_or_else(|| Error::NotFound("the imported Part is missing".into()))?,
                _ => {
                    return Err(Error::Invalid(
                        "library.edit_copy: only a component or a Part opens as a copy".into(),
                    ));
                }
            };
            let drafted = draft_closure(document, component)?;
            Ok(Outcome::Copied {
                component,
                import: report,
                drafted,
            })
        }
        LibraryCommand::Edit { edits } => {
            let before =
                super::assets::named_by_drafts(&document.drafts, edits.iter().map(|e| e.id));
            drafts::edit(Arc::make_mut(&mut document.drafts), edits)?;
            super::assets::drop_unnamed(document, before);
            Ok(Outcome::None)
        }
        LibraryCommand::PackArtwork { edits, artwork } => {
            super::marking::pack_artwork(document, edits, artwork)
        }
        LibraryCommand::DiscardDraft { id } => {
            let before = super::assets::named_by_drafts(&document.drafts, [id]);
            drafts::discard(Arc::make_mut(&mut document.drafts), id)?;
            super::assets::drop_unnamed(document, before);
            Ok(Outcome::None)
        }
        LibraryCommand::Publish {
            drafts: published,
            close,
        } => {
            // The pure publish rules run first, so their findings are
            // reported even without an evaluator; the candidate document
            // is discarded if the freshness check below then refuses.
            let project = Arc::make_mut(&mut document.project);
            let revisions = library::publish(
                &mut project.library,
                Arc::make_mut(&mut document.drafts),
                &published,
            )?;
            for id in &published {
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
            if close {
                drafts::close(Arc::make_mut(&mut document.drafts), &published);
            }
            Ok(Outcome::Published { revisions })
        }
        LibraryCommand::GeneratePads { draft, script } => {
            let current = document
                .drafts
                .get(draft)
                .ok_or_else(|| Error::NotFound("That draft no longer exists".into()))?;
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
            follow_envelope(document, draft)?;
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
        LibraryCommand::SetSymbolConventions { conventions } => {
            Arc::make_mut(&mut document.drafts).symbols = conventions;
            Ok(Outcome::None)
        }
        LibraryCommand::RestyleSymbol {
            draft,
            kind,
            terminals,
        } => restyle_symbol(document, draft, kind, terminals),
        LibraryCommand::EditGraphics { draft, edits } => {
            let current = document
                .drafts
                .get(draft)
                .ok_or_else(|| Error::NotFound("That draft no longer exists".into()))?;
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
        LibraryCommand::FootprintFromTemplate {
            draft,
            template,
            replace,
        } => {
            let (current, footprint) = footprint_draft(document, draft)?;
            let package = package_of(document, footprint)?;
            let build = if replace {
                footprint_family::rebuild
            } else {
                footprint_family::build
            };
            let (definition, report) = build(footprint, package, &template, need(evaluator)?)?;
            let edit = DraftEdit {
                id: draft,
                meta: current.meta.clone(),
                definition: Definition::Footprint(definition),
            };
            drafts::edit(Arc::make_mut(&mut document.drafts), vec![edit])?;
            follow_envelope(document, draft)?;
            Ok(Outcome::FootprintEdited { report })
        }
        LibraryCommand::FootprintFromPads(request) => {
            let draft = request.draft;
            let (current, footprint) = footprint_draft(document, draft)?;
            let package = package_of(document, footprint)?;
            let built =
                pad_table::build(footprint, package, &request, draft, &document.project.rules)?;
            let edit = DraftEdit {
                id: draft,
                meta: current.meta.clone(),
                definition: Definition::Footprint(built.footprint),
            };
            drafts::edit(Arc::make_mut(&mut document.drafts), vec![edit])?;
            // The courtyard was drawn around the body at this place, so it
            // is stored rather than derived again from that courtyard. A
            // published package stays where it is.
            let mut report = built.report;
            match (built.placement, landed_package(document, draft)) {
                (Some(placement), Some((id, meta, mut package))) => {
                    package.placement = placement;
                    store_package(document, id, meta, package)?;
                }
                _ => report.envelope_moved_to = None,
            }
            Ok(Outcome::FootprintFromPads { report })
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
            follow_envelope(document, draft)?;
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
            let report = import_bundle(document, &bundle)?;
            Ok(Outcome::Imported { report })
        }
        LibraryCommand::ImportStandardFields {} => {
            let report = import_bundle(document, &standard_fields::bundle()?)?;
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
        LibraryCommand::EditPackage { draft, edits } => {
            let (current, package) = package_draft(document, draft)?;
            // The pad-relative quick actions report why the footprint is
            // missing or ambiguous; other edits need none.
            let footprint = match landing_footprint(document, draft) {
                Ok(footprint) => Some(footprint),
                Err(error) if edits.iter().any(PackageEdit::needs_footprint) => {
                    return Err(error);
                }
                Err(_) => None,
            };
            let edited = package_fit::edit_package_in(package, footprint, &edits)?;
            let meta = current.meta.clone();
            store_package(document, draft, meta, edited)?;
            Ok(Outcome::None)
        }
        LibraryCommand::FitLeads {
            draft,
            pairs,
            rotation,
            confirm,
        } => {
            let (current, package) = package_draft(document, draft)?;
            let pairs = if pairs.is_empty() {
                package_fit::form_pairs(package, landing_footprint(document, draft)?)?
            } else {
                // Pads are checked against the one landing footprint when
                // there is one; host pairs may target several.
                let landing = landing_footprint(document, draft).ok();
                package_fit::check_pair_ids(package, landing, &pairs)?;
                pairs
            };
            let fit = package_fit::fit_leads(&package.placement, &pairs, rotation)?;
            if confirm {
                let mut edited = package.clone();
                edited.placement = fit.placement;
                let meta = current.meta.clone();
                store_package(document, draft, meta, edited)?;
            }
            Ok(Outcome::LeadsFitted {
                fit,
                applied: confirm,
            })
        }
        LibraryCommand::NewFromTemplate(request) => {
            super::templates::new_from_template(document, *request, evaluator)
        }
        LibraryCommand::PublishFromTemplate(request) => {
            let drafts = match super::templates::new_from_template(document, *request, evaluator)? {
                Outcome::DraftsCreated { ids } => ids,
                _ => {
                    return Err(Error::Invalid(
                        "library.new_from_template returned an unexpected outcome".into(),
                    ));
                }
            };
            apply_library(
                document,
                LibraryCommand::Publish {
                    drafts,
                    close: true,
                },
                evaluator,
            )
        }
        LibraryCommand::FindOrCreate { query, candidate } => {
            super::templates::find_or_create(document, &query, candidate, evaluator)
        }
        LibraryCommand::PackageFromTemplate(request) => {
            super::templates::package_from_template(document, *request, evaluator)
        }
        LibraryCommand::SavePackageRow(request) => {
            super::templates::save_package_row(document, *request)
        }
        LibraryCommand::PinoutApply(request) => super::pinout::apply(document, *request),
        LibraryCommand::ImportModel(request) => {
            let assign = request.assign.clone();
            let outcome = super::model3d::import_model(document, *request)?;
            if let (Some(assign), Outcome::DraftCreated { id }) = (assign, &outcome) {
                // The new model used by the package draft through
                // `library.edit_package`'s own path: one undo step.
                let revision = document
                    .drafts
                    .get(*id)
                    .and_then(|d| d.pending())
                    .ok_or_else(|| Error::NotFound("The new model draft is missing".into()))?;
                let model = crate::ids::ModelRef::new(*id, revision);
                let edits = [PackageEdit::SetModel(match assign.part {
                    Some(part) => {
                        if !assign.lead_map.is_empty() {
                            return Err(Error::Invalid(
                                "part_file.values: a part file takes no lead map".into(),
                            ));
                        }
                        library::PackageModel::PartFile(Box::new(
                            library::package_model::PartFileModel {
                                model,
                                values: part.values,
                                derived: part.derived,
                            },
                        ))
                    }
                    None => library::PackageModel::Library(library::model3d::LibraryModel {
                        model,
                        lead_map: assign.lead_map,
                    }),
                })];
                let (current, package) = package_draft(document, assign.package)?;
                let footprint = landing_footprint(document, assign.package).ok();
                let edited = package_fit::edit_package_in(package, footprint, &edits)?;
                let meta = current.meta.clone();
                store_package(document, assign.package, meta, edited)?;
            }
            Ok(outcome)
        }
    }
}
