//! Pcbcraft's electronics document and its single typed command engine.
//!
//! This crate is pure and wasm-compatible: no files, threads, windows or
//! transports. `pcbcraft-io` reads and writes documents; transports (UI, CLI,
//! MCP) translate requests into [`Operation`]s applied by [`Engine`].
pub mod catalog;
pub mod commands;
pub mod completeness;
pub mod diagnostics;
pub mod erc;
pub mod fields;
pub mod format;
pub mod functions;
pub mod geometry;
pub mod ids;
pub mod instances;
pub mod layout;
pub mod library;
pub mod materials;
pub mod model;
pub mod nets;
pub mod quantity;
pub mod schematic;
pub mod scripting;
#[cfg(test)]
mod tests;
pub mod updates;
pub mod validate;
pub use commands::{InstanceCommand, LibraryCommand, OffGridPin, Outcome};
use completeness::{CheckTarget, DefinitionReport, InstanceReport};
pub use diagnostics::{Diagnostic, Error, Finding, ObjectRef, Result, Stage};
pub use geometry::point_valid;
pub use ids::*;
use library::samples::{self, SampleRefs};
use library::{Definition, DefinitionMeta, Drafts};
pub use model::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;

/// Every document mutation. UI gestures, CLI requests and MCP tools all
/// produce one of these and apply it through [`Engine::apply`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "editor", content = "operation", deny_unknown_fields)]
pub enum Operation {
    Library(LibraryCommand),
    Instance(InstanceCommand),
    Schematic(schematic::SchematicCommand),
    Layout(layout::LayoutCommand),
    Update(Box<updates::ApplyUpdate>),
}

/// Every read-only report. Queries never change the document, its revision
/// or history.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "query", content = "params", deny_unknown_fields)]
pub enum Query {
    /// Exact pins, resolved choices, unit states and completeness.
    #[serde(rename = "component.inspect")]
    InspectInstance { instance: Id },
    /// Publish findings of exactly one draft or published revision.
    #[serde(rename = "component.check")]
    CheckDefinition { target: CheckTarget },
    /// `component.check` of every draft, in draft order, staged once.
    #[serde(rename = "component.check_drafts")]
    CheckDrafts {},
    /// Ranked component lineages at their latest published revision for a
    /// one-line query: free text, `pkg:`/`has:`/`missing:` filters and
    /// unit-aware field predicates such as `res>=10k`, `tol<=1%` or
    /// `C:1n..100n`. Field keys resolve against the project's field
    /// definitions.
    #[serde(rename = "library.search")]
    Search {
        query: String,
        /// At most 500; 50 when omitted.
        #[serde(default)]
        limit: Option<usize>,
        /// List lineages that exist only as drafts instead of counting them
        /// as hidden.
        #[serde(default)]
        include_drafts: bool,
    },
    /// The fields, package labels and completeness badges that the entries
    /// matching a query carry, with value counts.
    #[serde(rename = "library.facets")]
    Facets {
        query: String,
        #[serde(default)]
        include_drafts: bool,
    },
    /// One published component revision (the latest when `revision` is
    /// omitted) with its revisions, symbols, implementations and badges.
    #[serde(rename = "library.inspect")]
    InspectLineage {
        lineage: DefinitionId,
        #[serde(default)]
        revision: Option<Revision>,
    },
    /// The project BOM: resolved instances grouped by Part, unresolved ones
    /// by component revision, footprint and identical requirements, each
    /// line marked when it is unresolved.
    #[serde(rename = "project.bom")]
    Bom {},
    /// Groups of instances without a Part (or with an unresolved one) that
    /// share requirements and footprint, each with the `library.search`
    /// query that finds Parts for it.
    #[serde(rename = "library.resolve_groups")]
    ResolveGroups {},
    /// What `library.edit_footprint` would produce for these edits (pads,
    /// graphics, findings, including blocking ones), without changing the
    /// document. Needs the host's footprint evaluator.
    #[serde(rename = "library.footprint_preview")]
    FootprintPreview {
        draft: DefinitionId,
        edits: Vec<library::footprint_source::FootprintEdit>,
    },
    /// Project lineages copied from a library source that now holds a newer
    /// revision, or whose recorded revision changed on disk. Needs the
    /// host's source catalogs (`CatalogContext::sources`).
    #[serde(rename = "library.updates_available")]
    UpdatesAvailable {},
    /// What `library.promote` would write to the library `library` and
    /// record in the project: the export bundle of `root` and its closure.
    /// New lineages take ids from `fresh`, in order (io draws them).
    #[serde(rename = "library.promote_preview")]
    PromotePreview {
        root: DefinitionRef,
        library: LibraryId,
        /// The library's display name, carried in the bundle.
        name: String,
        fresh: Vec<SourceLineageId>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "report", content = "result", rename_all = "snake_case")]
pub enum QueryResult {
    Instance(Box<InstanceReport>),
    Definition(DefinitionReport),
    Drafts(Vec<DefinitionReport>),
    Search(catalog::SearchReport),
    Facets(catalog::FacetReport),
    Lineage(Box<catalog::ItemReport>),
    Bom(completeness::BomReport),
    ResolveGroups(completeness::ResolveReport),
    FootprintPreview(Box<library::footprint_edit::FootprintPreview>),
    UpdatesAvailable(Vec<catalog::UpdateAvailable>),
    Promote(Box<library::import::PromotePlan>),
}

/// Undo depth. Entries share unchanged data, so this bounds bookkeeping, not
/// copies of the whole project.
pub const MAX_UNDO: usize = 100;

/// The document, its revision and one shared undo history.
///
/// Every mutation is staged on a candidate and validated before it replaces
/// the document, so a failed operation leaves document, revision and history
/// unchanged.
#[derive(Default)]
pub struct Engine {
    document: Document,
    revision: u64,
    undo: VecDeque<Document>,
    redo: Vec<Document>,
}

impl Engine {
    /// An engine for an already validated file, with empty history.
    pub fn from_file(file: format::EditorFile) -> Result<Self> {
        let mut engine = Self::default();
        engine.replace_document(file)?;
        Ok(engine)
    }

    /// Two connected two-terminal components, placed on the board, with a
    /// typed resistance field shown on the symbol. The sample library is
    /// built through the public commands and its drafts stay open for
    /// editing.
    pub fn demo() -> Result<Self> {
        let mut engine = Self::default();
        let refs = engine.reserve_lineages()?;
        let kind = engine.reserve(DefinitionKind::QuantityKind)?;
        let field = engine.reserve(DefinitionKind::Field)?;
        let mut edits = samples::two_terminal("Resistor", samples::Kind::RESISTOR, refs);
        for edit in &mut edits {
            if let Definition::Component(component) = &mut edit.definition {
                component.fields.push(fields::FieldSelection {
                    field: FieldRef::new(field, Revision::FIRST),
                    default: None,
                    show_on_symbol: true,
                });
            }
        }
        edits.extend(samples::resistance_field(
            QuantityKindRef::new(kind, Revision::FIRST),
            field,
        ));
        engine.apply(
            engine.revision,
            Operation::Library(LibraryCommand::Edit { edits }),
        )?;
        engine.apply(
            engine.revision,
            Operation::Library(LibraryCommand::Publish {
                drafts: vec![
                    refs.component,
                    refs.symbol.id,
                    refs.package.id,
                    refs.footprint.id,
                    refs.binding.id,
                    kind,
                    field,
                ],
            }),
        )?;
        let mut placed = Vec::new();
        for position in [[-12.0, 0.0], [12.0, 0.0]] {
            let outcome = engine.apply(
                engine.revision,
                Operation::Instance(InstanceCommand::Place(instances::NewInstance {
                    component: ComponentRef::new(refs.component, Revision::FIRST),
                    designator: None,
                    properties: vec![fields::PropertyEdit {
                        field,
                        input: fields::PropertyInput::Text("10k".into()),
                    }],
                    representation: refs.symbol.id,
                    unit: UnitId(1),
                    position,
                    physical: Some(PhysicalChoice {
                        binding: refs.binding.id,
                        footprint: refs.footprint.id,
                    }),
                })),
            )?;
            if let Outcome::InstancePlaced { instance, .. } = outcome {
                placed.push(instance);
            }
        }
        let [first, second] = placed[..] else {
            return Err(Error::Invalid("demo placement failed".into()));
        };
        engine.apply(
            engine.revision,
            Operation::Schematic(schematic::SchematicCommand::Connect {
                from: Endpoint {
                    instance: first,
                    terminal: TerminalId(2),
                },
                to: Endpoint {
                    instance: second,
                    terminal: TerminalId(1),
                },
            }),
        )?;
        engine.apply(
            engine.revision,
            Operation::Layout(layout::LayoutCommand::Sync),
        )?;
        engine.undo.clear();
        engine.redo.clear();
        Ok(engine)
    }

    /// Reserve one empty draft lineage of `kind`.
    fn reserve(&mut self, kind: DefinitionKind) -> Result<DefinitionId> {
        match self.apply(
            self.revision,
            Operation::Library(LibraryCommand::NewDraft {
                meta: DefinitionMeta::default(),
                definition: Definition::empty(kind),
            }),
        )? {
            Outcome::DraftCreated { id } => Ok(id),
            _ => Err(Error::Invalid("draft reservation failed".into())),
        }
    }

    /// Reserve one empty draft lineage of each kind and return the pending
    /// first revisions they would pin of each other.
    fn reserve_lineages(&mut self) -> Result<SampleRefs> {
        let mut ids = Vec::new();
        for kind in [
            DefinitionKind::Component,
            DefinitionKind::Symbol,
            DefinitionKind::Package,
            DefinitionKind::Footprint,
            DefinitionKind::Binding,
        ] {
            ids.push(self.reserve(kind)?);
        }
        let [component, symbol, package, footprint, binding] = ids[..] else {
            return Err(Error::Invalid("draft reservation failed".into()));
        };
        Ok(SampleRefs {
            component,
            symbol: SymbolRef::new(symbol, Revision::FIRST),
            package: PackageRef::new(package, Revision::FIRST),
            footprint: FootprintRef::new(footprint, Revision::FIRST),
            binding: BindingRef::new(binding, Revision::FIRST),
        })
    }

    pub fn document(&self) -> &Document {
        &self.document
    }
    pub fn project(&self) -> &Project {
        &self.document.project
    }
    /// Shared handle; equal pointers mean an unchanged project.
    pub fn project_arc(&self) -> &Arc<Project> {
        &self.document.project
    }
    pub fn drafts(&self) -> &Drafts {
        &self.document.drafts
    }
    pub fn to_file(&self) -> format::EditorFile {
        format::EditorFile::from_document(&self.document)
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Replace the whole document (open, new, recover). History is cleared;
    /// an invalid file leaves everything unchanged.
    pub fn replace_document(&mut self, file: format::EditorFile) -> Result<()> {
        file.validate()?;
        let revision = self.next_revision()?;
        self.document = file.into_document();
        self.revision = revision;
        self.undo.clear();
        self.redo.clear();
        Ok(())
    }

    /// Apply one operation at an expected revision as one undo step. A
    /// failure leaves document, revision and history unchanged.
    pub fn apply(&mut self, expected_revision: u64, operation: Operation) -> Result<Outcome> {
        self.apply_inner(expected_revision, operation, None)
    }

    /// [`apply`](Self::apply) with the host's footprint evaluator, which
    /// footprint-source edits need (the core never interprets a sketch).
    /// The evaluator runs on the candidate only; undo and redo restore
    /// snapshots and never call it.
    pub fn apply_with(
        &mut self,
        expected_revision: u64,
        operation: Operation,
        evaluator: &dyn library::FootprintEvaluator,
    ) -> Result<Outcome> {
        self.apply_inner(expected_revision, operation, Some(evaluator))
    }

    fn apply_inner(
        &mut self,
        expected_revision: u64,
        operation: Operation,
        evaluator: Option<&dyn library::FootprintEvaluator>,
    ) -> Result<Outcome> {
        if expected_revision != self.revision {
            return Err(Error::Stale {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        let mut candidate = self.document.clone();
        let reviewed = matches!(operation, Operation::Update(_));
        let outcome = match operation {
            Operation::Library(command) => {
                commands::apply_library(&mut candidate, command, evaluator)?
            }
            Operation::Instance(command) => {
                commands::apply_instance(Arc::make_mut(&mut candidate.project), command)?
            }
            Operation::Schematic(command) => {
                match schematic::apply(
                    Arc::make_mut(&mut candidate.project),
                    command,
                    EditPolicy::Direct,
                )? {
                    Some(occurrence) => Outcome::UnitPlaced { occurrence },
                    None => Outcome::None,
                }
            }
            // Restating the current fabrication changes nothing, so it
            // takes no revision or undo step.
            Operation::Layout(layout::LayoutCommand::SetFabrication(fabrication))
                if fabrication == candidate.project.fabrication =>
            {
                Outcome::None
            }
            Operation::Layout(command) => {
                layout::apply_staged(Arc::make_mut(&mut candidate.project), command)?;
                Outcome::None
            }
            Operation::Update(update) => {
                candidate.project = Arc::new(updates::candidate(
                    &self.document.project,
                    self.revision,
                    *update,
                )?);
                Outcome::None
            }
        };
        if !Arc::ptr_eq(&candidate.project, &self.document.project) {
            let project = Arc::make_mut(&mut candidate.project);
            nets::reconcile(project)?;
            if !reviewed {
                schematic::protect_routed_nets(&self.document.project, project)?;
            }
            validate::edited_document(
                &self.document.project,
                &candidate.project,
                &candidate.drafts,
            )?;
        } else if candidate.drafts == self.document.drafts {
            return Ok(outcome);
        } else {
            validate::drafts_only(&candidate.project, &candidate.drafts)?;
        }
        self.commit(candidate)?;
        Ok(outcome)
    }

    /// Answer a read-only query.
    pub fn query(&self, query: Query) -> Result<QueryResult> {
        self.query_with(query, &catalog::CatalogContext::default())
    }

    /// Answer a read-only query with the host's catalog context (favourite
    /// and recently used lineages), which ranks `library.search` results.
    pub fn query_with(
        &self,
        query: Query,
        context: &catalog::CatalogContext,
    ) -> Result<QueryResult> {
        self.query_using(query, context, None)
    }

    /// [`query_with`](Self::query_with) with the host's footprint
    /// evaluator, which `library.footprint_preview` needs.
    pub fn query_using(
        &self,
        query: Query,
        context: &catalog::CatalogContext,
        evaluator: Option<&dyn library::FootprintEvaluator>,
    ) -> Result<QueryResult> {
        let index = || {
            catalog::Index::with_sources(
                &self.document.project,
                &self.document.drafts,
                &context.sources,
            )
        };
        match query {
            Query::Search {
                query,
                limit,
                include_drafts,
            } => {
                let index = index()?;
                let parsed = index.parse(&query)?;
                Ok(QueryResult::Search(index.search(
                    &parsed,
                    context,
                    catalog::SearchOptions {
                        limit: limit.unwrap_or(catalog::DEFAULT_LIMIT),
                        include_drafts,
                    },
                )))
            }
            Query::Facets {
                query,
                include_drafts,
            } => {
                let index = index()?;
                let parsed = index.parse(&query)?;
                Ok(QueryResult::Facets(index.facets(&parsed, include_drafts)))
            }
            Query::InspectLineage { lineage, revision } => {
                Ok(QueryResult::Lineage(Box::new(catalog::inspect(
                    &self.document.project,
                    &self.document.drafts,
                    lineage,
                    revision,
                )?)))
            }
            Query::Bom {} => Ok(QueryResult::Bom(completeness::bom(&self.document.project)?)),
            Query::ResolveGroups {} => Ok(QueryResult::ResolveGroups(
                completeness::resolve_groups(&self.document.project)?,
            )),
            Query::InspectInstance { instance } => Ok(QueryResult::Instance(Box::new(
                completeness::instance(&self.document.project, instance)?,
            ))),
            Query::CheckDefinition { target } => Ok(QueryResult::Definition(
                completeness::definition(&self.document.project, &self.document.drafts, target)?,
            )),
            Query::CheckDrafts {} => Ok(QueryResult::Drafts(completeness::drafts(
                &self.document.project,
                &self.document.drafts,
            )?)),
            Query::FootprintPreview { draft, edits } => {
                Ok(QueryResult::FootprintPreview(Box::new(
                    commands::preview_footprint(&self.document, draft, &edits, evaluator)?,
                )))
            }
            Query::UpdatesAvailable {} => Ok(QueryResult::UpdatesAvailable(
                catalog::updates_available(&self.document.project.library, &context.sources)?,
            )),
            Query::PromotePreview {
                root,
                library,
                name,
                fresh,
            } => Ok(QueryResult::Promote(Box::new(library::import::export(
                &self.document.project.library,
                root,
                library,
                &name,
                &fresh,
            )?))),
        }
    }

    pub fn preview_update(&self, change: updates::Change) -> updates::UpdatePlan {
        updates::preview(&self.document.project, self.revision, change).0
    }

    pub fn apply_update(
        &mut self,
        expected_revision: u64,
        change: updates::Change,
        remove_conflicting_tracks: bool,
    ) -> Result<()> {
        self.apply(
            expected_revision,
            Operation::Update(Box::new(updates::ApplyUpdate {
                expected_revision,
                change,
                remove_conflicting_tracks,
            })),
        )
        .map(|_| ())
    }

    pub fn undo(&mut self) -> Result<()> {
        let revision = self.next_revision()?;
        let previous = self
            .undo
            .pop_back()
            .ok_or_else(|| Error::History("nothing to undo".into()))?;
        self.redo
            .push(std::mem::replace(&mut self.document, previous));
        self.revision = revision;
        Ok(())
    }

    pub fn redo(&mut self) -> Result<()> {
        let revision = self.next_revision()?;
        let next = self
            .redo
            .pop()
            .ok_or_else(|| Error::History("nothing to redo".into()))?;
        self.undo
            .push_back(std::mem::replace(&mut self.document, next));
        self.revision = revision;
        Ok(())
    }

    fn next_revision(&self) -> Result<u64> {
        self.revision
            .checked_add(1)
            .ok_or_else(|| Error::Limit("revision exhausted".into()))
    }

    fn commit(&mut self, candidate: Document) -> Result<()> {
        let revision = self.next_revision()?;
        if self.undo.len() >= MAX_UNDO {
            self.undo.pop_front();
        }
        self.undo
            .push_back(std::mem::replace(&mut self.document, candidate));
        self.redo.clear();
        self.revision = revision;
        Ok(())
    }
}

/// JSON schemas of the document types every transport shares.
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "operation": schemars::schema_for!(Operation),
        "outcome": schemars::schema_for!(Outcome),
        "query": schemars::schema_for!(Query),
        "query_result": schemars::schema_for!(QueryResult),
        "project": schemars::schema_for!(Project),
        "update_plan": schemars::schema_for!(updates::UpdatePlan),
        "editor_file": schemars::schema_for!(format::EditorFile),
        "finding": schemars::schema_for!(Finding),
        "diagnostic": schemars::schema_for!(Diagnostic),
    })
}
