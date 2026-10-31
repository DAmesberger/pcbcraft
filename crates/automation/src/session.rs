//! The automation request contract. Each request is a document query, a
//! document mutation through [`Engine::apply`], or an external file effect.
//! File effects never enter undo history.
use crate::{Error, Result};
use pcbcraft_core::{
    DefinitionId, Engine, LibraryCommand, Operation, Project, Query, QueryResult,
    catalog::EntryKey,
    erc,
    format::EditorFile,
    geometry::Point,
    ids::{DefinitionRef, LibraryId, Revision, SourceLineageId},
    layout,
    library::{
        Drafts,
        graphics::{GraphicTarget, SILK_STROKE_MM, SYMBOL_STROKE_MM},
        import::{OtherRevision, PlaceSource},
    },
    updates,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyParams {
    /// The revision the operation was prepared against.
    pub expected_revision: u64,
    pub operation: Operation,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathParams {
    pub path: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveParams {
    /// Destination `.pcbcraft` file; defaults to the path last opened or saved.
    #[serde(default)]
    pub path: Option<String>,
}

/// Import an SVG file's drawing into a symbol or footprint draft.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportSvgParams {
    /// The revision the import was prepared against.
    pub expected_revision: u64,
    /// A symbol or footprint draft.
    pub draft: DefinitionId,
    /// A symbol unit, or a footprint's silkscreen or assembly layer and side.
    pub target: GraphicTarget,
    /// The `.svg` file (at most 1 MiB).
    pub path: String,
    /// Where the artwork's bounding-box centre goes, in millimetres.
    #[serde(default)]
    pub at: Point,
    /// Multiplies the size the SVG declares (its width and height over the
    /// viewBox; 96 user units per inch without units), 0.001–1000.
    #[serde(default = "unit_scale")]
    pub scale: f64,
    /// Chord tolerance for flattened Béziers and ellipses, 0.001–1 mm.
    #[serde(default = "default_tolerance")]
    pub tolerance_mm: f64,
}

fn unit_scale() -> f64 {
    1.0
}
fn default_tolerance() -> f64 {
    pcbcraft_solidcraft::svg::DEFAULT_TOLERANCE_MM
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceAddParams {
    /// A library folder holding `pcbcraft-library.json`.
    pub path: String,
    /// Whether `library.publish_to_folder` may write to it.
    #[serde(default)]
    pub writable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceCreateParams {
    /// A new folder, or an empty existing one.
    pub path: String,
    /// The library's display name, stored in its manifest.
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceEnableParams {
    pub path: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FavouriteParams {
    pub key: EntryKey,
    pub favourite: bool,
}

/// One revision of a folder lineage (`source_lineage` of a search entry).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BundleParams {
    pub library: LibraryId,
    pub lineage: SourceLineageId,
    /// Defaults to the newest revision the folder holds.
    #[serde(default)]
    pub revision: Option<Revision>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportFromParams {
    pub expected_revision: u64,
    pub library: LibraryId,
    pub lineage: SourceLineageId,
    #[serde(default)]
    pub revision: Option<Revision>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaceFromParams {
    pub expected_revision: u64,
    pub library: LibraryId,
    /// A component lineage of the folder.
    pub lineage: SourceLineageId,
    #[serde(default)]
    pub revision: Option<Revision>,
    pub position: Point,
    #[serde(default)]
    pub designator: Option<String>,
    #[serde(default)]
    pub other_revision: OtherRevision,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishParams {
    pub expected_revision: u64,
    /// The published project revision to promote, with its closure.
    pub root: DefinitionRef,
    /// A writable library source.
    pub library: LibraryId,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewParams {
    pub change: updates::Change,
}

/// One request. `command` names it; `params` holds its typed parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum Request {
    /// Apply one document operation at an expected revision.
    #[serde(rename = "project.apply")]
    Apply(ApplyParams),
    /// Return the document, drafts with their findings and design checks.
    #[serde(rename = "project.inspect")]
    Inspect(NoParams),
    /// Answer one read-only core query (`component.inspect`,
    /// `component.check`, `component.check_drafts`, `library.search`,
    /// `library.facets`, `library.inspect`, `project.bom`,
    /// `library.resolve_groups`), passed unchanged to `Engine::query`.
    #[serde(rename = "project.query")]
    Query(Query),
    #[serde(rename = "project.undo")]
    Undo(NoParams),
    #[serde(rename = "project.redo")]
    Redo(NoParams),
    /// Start a new, empty project. History is cleared.
    #[serde(rename = "project.new")]
    New(NoParams),
    /// Open a `.pcbcraft` file; a failure leaves the session unchanged.
    #[serde(rename = "project.open")]
    Open(PathParams),
    /// Atomically write the document to a `.pcbcraft` file.
    #[serde(rename = "project.save")]
    Save(SaveParams),
    /// Preview a revision-bound update without changing the document.
    #[serde(rename = "project.preview_update")]
    PreviewUpdate(PreviewParams),
    /// Apply a previewed update at its revision.
    #[serde(rename = "project.apply_update")]
    ApplyUpdate(updates::ApplyUpdate),
    /// Write the board as a native, parametric `.solidcraft` model.
    /// Refuses existing files.
    #[serde(rename = "project.export_native")]
    ExportNative(PathParams),
    /// List the configured library sources with their state.
    #[serde(rename = "library.sources")]
    Sources(NoParams),
    /// Add a library folder as a source (per-user settings).
    #[serde(rename = "library.source.add")]
    SourceAdd(SourceAddParams),
    /// Create an empty library folder and add it as a writable source.
    #[serde(rename = "library.source.create")]
    SourceCreate(SourceCreateParams),
    /// Remove a source from the settings; the folder is untouched.
    #[serde(rename = "library.source.remove")]
    SourceRemove(PathParams),
    /// Turn a source on or off.
    #[serde(rename = "library.source.enable")]
    SourceEnable(SourceEnableParams),
    /// Re-read every source's manifest and catalog.
    #[serde(rename = "library.rescan")]
    Rescan(NoParams),
    /// Mark or unmark a favourite (per-user settings).
    #[serde(rename = "library.favourite")]
    Favourite(FavouriteParams),
    /// Read one folder revision and its closure as a bundle.
    #[serde(rename = "library.bundle")]
    Bundle(BundleParams),
    /// Read a folder bundle and apply `library.import` with it.
    #[serde(rename = "library.import_from_folder")]
    ImportFrom(ImportFromParams),
    /// Read a folder bundle and apply `library.place_from` with it.
    #[serde(rename = "library.place_from_folder")]
    PlaceFrom(PlaceFromParams),
    /// Promote a project revision to a writable folder: preview, write the
    /// bundle, then record the origins with `library.promote`.
    #[serde(rename = "library.publish_to_folder")]
    Publish(PublishParams),
    /// Read an SVG file and add its drawing to a draft through
    /// `library.edit_graphics`: one atomic undo step.
    #[serde(rename = "library.import_svg")]
    ImportSvg(ImportSvgParams),
}

impl Request {
    /// Every request ID with its description and parameter schema, generated
    /// from the Rust types the dispatcher deserializes.
    pub fn catalog() -> Vec<CommandInfo> {
        fn info<T: JsonSchema>(id: &'static str, description: &'static str) -> CommandInfo {
            CommandInfo {
                id,
                description,
                params: serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null),
            }
        }
        vec![
            info::<ApplyParams>(
                "project.apply",
                "Apply one typed document operation (library, instance including instance.assign_part, schematic, layout including layout.set_fabrication, or update) at an expected revision and return its outcome. Fails without changes on stale revisions or invalid input.",
            ),
            info::<NoParams>(
                "project.inspect",
                "Return the revision, project, drafts with their publish findings, ERC and board-check findings.",
            ),
            info::<Query>(
                "project.query",
                "Answer one read-only query: component.inspect reports an instance's exact component pin, resolved symbol and implementation, per-unit states, effective field values with their origin, active and unresolved terminal functions, occurrence and placement counts and completeness findings; component.check reports the publish findings, with the stage each blocks, of one draft or published revision; component.check_drafts reports every draft at once; library.search ranks component lineages at their latest published revision for a one-line query (free text, pkg:/has:/missing: filters and unit-aware field predicates such as res>=10k, tol<=1% or C:1n..100n, whose keys resolve against the project's field definitions; parse errors name the term, e.g. V>=10k: 10 k has no unit of voltage); library.facets lists the fields, package labels and completeness badges the matching entries carry; library.inspect reports one revision of a lineage with its revisions, symbols, implementations and badges; project.bom lists the bill of materials, resolved instances grouped by their manufacturer Part and unresolved ones mar; library.footprint_preview reports what library.edit_footprint would produce for a footprint draft (pads, graphics and findings, blocked; library.resolve_groups lists instances without an agreeing Part that share component revision, footprint and requirements, each group with the library.search query that finds Parts for it; ing ones included); library.updates_available lists project lineages copied from a library source that holds a newer revision or a record whose hash changed (modified); library.promote_preview reports the bundle and origins library.promote would write and record (library.publish_to_folder runs it). library.search and library.facets include the entries of every ready library source. Nothing is stored.",
            ),
            info::<NoParams>("project.undo", "Undo the last document change."),
            info::<NoParams>("project.redo", "Redo the last undone document change."),
            info::<NoParams>("project.new", "Start a new empty project; clears history."),
            info::<PathParams>(
                "project.open",
                "Open a .pcbcraft project file of the current format version; other versions are refused. Failure leaves the session unchanged.",
            ),
            info::<SaveParams>(
                "project.save",
                "Atomically save to a .pcbcraft file (defaults to the current path). Not undoable.",
            ),
            info::<PreviewParams>(
                "project.preview_update",
                "Preview a revision-bound component-revision, instance, schematic or PCB-sync change and its routed-copper impact.",
            ),
            info::<updates::ApplyUpdate>(
                "project.apply_update",
                "Apply a previewed update at its revision, optionally removing the listed conflicting tracks.",
            ),
            info::<PathParams>(
                "project.export_native",
                "Export the board as a native parametric .solidcraft model to a new file.",
            ),
            info::<NoParams>(
                "library.sources",
                "List the configured library folders in settings order: path, enabled, writable, state (ready with library id, name, entry and revision counts and how the index was obtained; disabled with the reason, e.g. path missing or unsupported format v2; failed with a diagnostic). Not a document change.",
            ),
            info::<SourceAddParams>(
                "library.source.add",
                "Add a library folder (pcbcraft-library.json manifest) as a source in the per-user settings. A missing folder, a malformed manifest, another format version, an unknown required feature, a repeated path or a repeated library id is refused and nothing changes. library.search and library.facets then include its entries (source library, with source_lineage).",
            ),
            info::<SourceCreateParams>(
                "library.source.create",
                "Create an empty library folder with a new random library id and add it as a writable source.",
            ),
            info::<PathParams>(
                "library.source.remove",
                "Remove a library source from the settings. The folder and every project copy are untouched.",
            ),
            info::<SourceEnableParams>(
                "library.source.enable",
                "Turn a library source on or off; an off source stays listed as disabled (turned off).",
            ),
            info::<NoParams>(
                "library.rescan",
                "Re-read every source's manifest and catalog index (after a git pull or another writer), then list the sources.",
            ),
            info::<FavouriteParams>(
                "library.favourite",
                "Mark or unmark a lineage, keyed by project lineage or by library id and source lineage, as a favourite. Favourites rank library.search results and persist per user, never in a project.",
            ),
            info::<BundleParams>(
                "library.bundle",
                "Read one revision of a folder lineage (default the newest) with its complete dependency closure and packed assets, as the bundle library.import and library.place_from carry.",
            ),
            info::<ImportFromParams>(
                "library.import_from_folder",
                "Read a folder bundle and apply library.import with it at the expected revision: the closure is copied into the project under fresh project lineages, or appended as the next project revision of a lineage already imported from the same source; an equal origin is reused. One undo step; places nothing. Follow with project.preview_update to move instances to the new revision.",
            ),
            info::<PlaceFromParams>(
                "library.place_from_folder",
                "Read a folder bundle of a component lineage and apply library.place_from with it: import if needed, then place, as one transaction and one undo step. When the project holds another revision of that lineage, other_revision refuse (default) fails with code library.other_revision and keep_both imports the folder revision beside it.",
            ),
            info::<PublishParams>(
                "library.publish_to_folder",
                "Promote a published project revision and its closure to a writable library source: compute library.promote_preview with fresh random lineage ids, write the records and assets to the folder (an existing rNNNN.json with other content refuses the write before anything is written), then apply library.promote, which records the origins as one undo step. Returns the written and unchanged record paths.",
            ),
            info::<ImportSvgParams>(
                "library.import_svg",
                "Read a .svg file (at most 1 MiB) and add its drawing to a symbol unit or a footprint's silkscreen or assembly layer as editable graphics, centred at `at` and scaled from the SVG's declared millimetre size: circles, circular arcs and axis-aligned rectangles stay exact, tangent arcs become corner radii, Béziers and ellipses flatten within tolerance_mm, filled closed shapes become solid fills and strokes keep their width. Filled regions with holes are imported as outlines; text, images and use references are skipped and listed in import.notes. Applied as one library.edit_graphics step at expected_revision: atomic and one undo step; the outcome lists the new graphic IDs.",
            ),
        ]
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CommandInfo {
    pub id: &'static str,
    pub description: &'static str,
    pub params: Value,
}

/// A headless editing session: one engine, the current file path and the
/// library sources.
#[derive(Default)]
pub struct Session {
    engine: Engine,
    path: Option<PathBuf>,
    sources: sources::Sources,
}

impl Session {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            path: None,
            sources: sources::Sources::default(),
        }
    }
    /// A session with the given library sources, e.g.
    /// [`sources::Sources::from_env`].
    pub fn with_sources(engine: Engine, sources: sources::Sources) -> Self {
        Self {
            engine,
            path: None,
            sources,
        }
    }
    pub fn sources(&self) -> &sources::Sources {
        &self.sources
    }
    pub fn engine(&self) -> &Engine {
        &self.engine
    }
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Execute one typed request.
    pub fn execute(&mut self, request: Request) -> Result<Value> {
        match request {
            Request::Apply(params) => {
                let outcome = self.engine.apply_with(
                    params.expected_revision,
                    params.operation,
                    &pcbcraft_solidcraft::footprint::SketchEvaluator,
                )?;
                let mut snapshot = self.snapshot();
                snapshot["outcome"] = serde_json::to_value(outcome)?;
                return Ok(snapshot);
            }
            Request::Inspect(_) => {}
            Request::Query(query) => {
                return Ok(serde_json::to_value(self.engine.query_using(
                    query,
                    &self.sources.context(),
                    Some(&pcbcraft_solidcraft::footprint::SketchEvaluator),
                )?)?);
            }
            Request::Undo(_) => self.engine.undo()?,
            Request::Redo(_) => self.engine.redo()?,
            Request::New(_) => {
                self.engine.replace_document(EditorFile {
                    version: pcbcraft_core::format::FILE_VERSION,
                    project: Project::default(),
                    drafts: Drafts::default(),
                })?;
                self.path = None;
            }
            Request::Open(params) => {
                let path = PathBuf::from(params.path);
                let file = pcbcraft_io::read_project(&path)?;
                self.engine.replace_document(file)?;
                self.path = Some(path);
            }
            Request::Save(params) => {
                let path = match params.path {
                    Some(path) => PathBuf::from(path),
                    None => self.path.clone().ok_or_else(|| {
                        pcbcraft_io::Error::Invalid(
                            "this project has no file yet; give a path".into(),
                        )
                    })?,
                };
                pcbcraft_io::write_project(&path, &self.engine.to_file())?;
                let saved = path.display().to_string();
                self.path = Some(path);
                return Ok(json!({"revision": self.engine.revision(), "saved_path": saved}));
            }
            Request::PreviewUpdate(params) => {
                return Ok(serde_json::to_value(
                    self.engine.preview_update(params.change),
                )?);
            }
            Request::ApplyUpdate(update) => self.engine.apply_update(
                update.expected_revision,
                update.change,
                update.remove_conflicting_tracks,
            )?,
            Request::ExportNative(params) => {
                pcbcraft_solidcraft::project::build(self.engine.project())?.save(&params.path)?;
                return Ok(json!({
                    "revision": self.engine.revision(),
                    "exported_path": params.path
                }));
            }
            Request::Sources(_) => return self.list_sources(),
            Request::SourceAdd(params) => {
                let added = self.sources.add(Path::new(&params.path), params.writable)?;
                return Ok(json!({"added": added, "sources": self.sources.list()}));
            }
            Request::SourceCreate(params) => {
                let created = self.sources.create(Path::new(&params.path), &params.name)?;
                return Ok(json!({"created": created, "sources": self.sources.list()}));
            }
            Request::SourceRemove(params) => {
                self.sources.remove(Path::new(&params.path))?;
                return self.list_sources();
            }
            Request::SourceEnable(params) => {
                self.sources
                    .enable(Path::new(&params.path), params.enabled)?;
                return self.list_sources();
            }
            Request::Rescan(_) => {
                self.sources.rescan();
                return self.list_sources();
            }
            Request::Favourite(params) => {
                self.sources.favourite(params.key, params.favourite)?;
                return Ok(json!({"favourites": self.sources.settings().favourites}));
            }
            Request::Bundle(params) => {
                let bundle =
                    self.sources
                        .bundle(params.library, params.lineage, params.revision)?;
                return Ok(serde_json::to_value(bundle)?);
            }
            Request::ImportFrom(params) => {
                let bundle =
                    self.sources
                        .bundle(params.library, params.lineage, params.revision)?;
                let operation = Operation::Library(LibraryCommand::Import {
                    bundle: Box::new(bundle),
                });
                return self.apply(params.expected_revision, operation);
            }
            Request::PlaceFrom(params) => {
                let bundle =
                    self.sources
                        .bundle(params.library, params.lineage, params.revision)?;
                let operation = Operation::Library(LibraryCommand::PlaceFrom {
                    source: PlaceSource::Bundle {
                        bundle: Box::new(bundle),
                    },
                    position: params.position,
                    designator: params.designator,
                    other_revision: params.other_revision,
                });
                let reply = self.apply(params.expected_revision, operation)?;
                self.sources.used(EntryKey::Library {
                    library: params.library,
                    lineage: params.lineage,
                })?;
                return Ok(reply);
            }
            Request::Publish(params) => return self.publish(params),
            Request::ImportSvg(params) => return self.import_svg(params),
        }
        Ok(self.snapshot())
    }

    /// Apply one operation and return the snapshot with its outcome.
    fn apply(&mut self, expected_revision: u64, operation: Operation) -> Result<Value> {
        let outcome = self.engine.apply_with(
            expected_revision,
            operation,
            &pcbcraft_solidcraft::footprint::SketchEvaluator,
        )?;
        let mut snapshot = self.snapshot();
        snapshot["outcome"] = serde_json::to_value(outcome)?;
        Ok(snapshot)
    }

    fn list_sources(&self) -> Result<Value> {
        Ok(json!({"sources": self.sources.list()}))
    }

    /// `library.publish_to_folder`: the three promote steps. The revision is
    /// checked before the folder is written, and the plan is recomputed by
    /// `library.promote` with the same arguments, so the recorded origins
    /// equal the records written.
    fn publish(&mut self, params: PublishParams) -> Result<Value> {
        let actual = self.engine.revision();
        if params.expected_revision != actual {
            return Err(pcbcraft_core::Error::Stale {
                expected: params.expected_revision,
                actual,
            }
            .into());
        }
        let folder = self.sources.writable(params.library)?.clone();
        let members =
            pcbcraft_core::library::bundle::closure(&self.engine.project().library, params.root)?;
        let fresh = (0..members.len())
            .map(|_| pcbcraft_io::library_folder::new_lineage_id())
            .collect::<pcbcraft_io::Result<Vec<_>>>()?;
        let name = folder.manifest.name.clone();
        let plan = match self.engine.query(Query::PromotePreview {
            root: params.root,
            library: params.library,
            name: name.clone(),
            fresh: fresh.clone(),
        })? {
            QueryResult::Promote(plan) => plan,
            _ => {
                return Err(pcbcraft_core::Error::Invalid(
                    "library.promote_preview gave another report".into(),
                )
                .into());
            }
        };
        // Only the ids the plan used travel in the undoable operation.
        let used = plan.used.clone();
        let written = pcbcraft_io::library_folder::write_bundle(&folder, &plan.bundle)?;
        let operation = Operation::Library(LibraryCommand::Promote {
            root: params.root,
            library: params.library,
            name,
            fresh: used,
        });
        let mut reply = self.apply(params.expected_revision, operation)?;
        self.sources.rescan();
        reply["written"] = json!({
            "records": written.written,
            "unchanged": written.unchanged,
            "assets": written.assets,
        });
        reply["root"] = serde_json::to_value(plan.bundle.root)?;
        Ok(reply)
    }

    /// Read, convert and add an SVG drawing as one graphic edit.
    fn import_svg(&mut self, params: ImportSvgParams) -> Result<Value> {
        use pcbcraft_solidcraft::svg;
        let text = pcbcraft_io::artwork::read_svg(Path::new(&params.path))?;
        let options = svg::Options {
            scale: params.scale,
            tolerance_mm: params.tolerance_mm,
            fallback_stroke_mm: match params.target {
                GraphicTarget::Unit(_) => SYMBOL_STROKE_MM,
                GraphicTarget::Footprint { .. } => SILK_STROKE_MM,
            },
        };
        let artwork = svg::convert(&text, &options)?;
        let edits = artwork.edits(params.target, params.at)?;
        let outcome = self.engine.apply_with(
            params.expected_revision,
            Operation::Library(LibraryCommand::EditGraphics {
                draft: params.draft,
                edits,
            }),
            &pcbcraft_solidcraft::footprint::SketchEvaluator,
        )?;
        let mut snapshot = self.snapshot();
        snapshot["outcome"] = serde_json::to_value(outcome)?;
        snapshot["import"] = json!({"size": artwork.size, "notes": artwork.notes});
        Ok(snapshot)
    }

    /// The document state every mutation and inspection returns. Each draft
    /// carries the core's `component.check` report, from one staged
    /// `component.check_drafts` query.
    pub fn snapshot(&self) -> Value {
        let project = self.engine.project();
        let items = &self.engine.drafts().items;
        let drafts: Vec<Value> = match self.engine.query(Query::CheckDrafts {}) {
            Ok(QueryResult::Drafts(reports)) => items
                .iter()
                .zip(reports)
                .map(|(draft, report)| {
                    let check = serde_json::to_value(QueryResult::Definition(report))
                        .unwrap_or(Value::Null);
                    json!({"draft": draft, "check": check})
                })
                .collect(),
            other => {
                let error = match other {
                    Err(error) => json!({"error": error.diagnostic()}),
                    Ok(_) => json!({"error": "unexpected report"}),
                };
                items
                    .iter()
                    .map(|draft| json!({"draft": draft, "check": error}))
                    .collect()
            }
        };
        json!({
            "revision": self.engine.revision(),
            "project": project,
            "drafts": drafts,
            "can_undo": self.engine.can_undo(),
            "can_redo": self.engine.can_redo(),
            "erc": erc::checks(project),
            "board_checks": layout::checks(project),
        })
    }

    /// Parse and execute a JSON request, returning the shared envelope:
    /// `{"ok":true,"result":…}` or `{"ok":false,"error":{code,message,…}}`.
    pub fn dispatch(&mut self, request: Value) -> Value {
        let result = serde_json::from_value::<Request>(request)
            .map_err(Error::from)
            .and_then(|request| self.execute(request));
        envelope(result)
    }
}

pub fn envelope(result: Result<Value>) -> Value {
    match result {
        Ok(result) => json!({"ok": true, "result": result}),
        Err(error) => json!({"ok": false, "error": error.diagnostic()}),
    }
}

/// Create a library folder at `path` as a writable source of `session` and
/// publish every latest Part and component of its project to it, Parts
/// first (a Part's closure brings its component, which the later component
/// publish then finds unchanged). Every step is a request of the JSON
/// contract. Returns the library id, the records written and the sources.
pub fn publish_library(session: &mut Session, path: &str, name: &str) -> Result<Value> {
    let library = &session.engine().project().library;
    let roots: Vec<DefinitionRef> = library
        .latest_parts()
        .iter()
        .map(|p| DefinitionRef {
            kind: pcbcraft_core::DefinitionKind::Part,
            id: p.id,
            revision: p.revision,
        })
        .chain(library.latest_components().iter().map(|c| DefinitionRef {
            kind: pcbcraft_core::DefinitionKind::Component,
            id: c.id,
            revision: c.revision,
        }))
        .collect();
    let created = session.execute(Request::SourceCreate(SourceCreateParams {
        path: path.into(),
        name: name.into(),
    }))?;
    let id: LibraryId = serde_json::from_value(created["created"]["library"].clone())?;
    let mut written = Vec::new();
    for root in roots {
        let reply = session.execute(Request::Publish(PublishParams {
            expected_revision: session.engine().revision(),
            root,
            library: id,
        }))?;
        if let Some(records) = reply["written"]["records"].as_array() {
            written.extend(records.iter().cloned());
        }
    }
    Ok(json!({
        "library": id,
        "records_written": written,
        "sources": session.sources.list(),
    }))
}

/// Request, document and diagnostic schemas for discovery.
pub fn schemas() -> Value {
    json!({
        "request": schemars::schema_for!(Request),
        "commands": Request::catalog(),
        "document": pcbcraft_core::schemas(),
        "solidcraft_revision": pcbcraft_solidcraft::SOLIDCRAFT_REVISION,
    })
}

pub mod sources;

#[cfg(test)]
mod folder_tests;
#[cfg(test)]
mod parts_tests;
#[cfg(test)]
mod svg_tests;
#[cfg(test)]
mod tests;
