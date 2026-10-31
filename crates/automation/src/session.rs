//! The automation request contract. Each request is a document query, a
//! document mutation through [`Engine::apply`], or an external file effect.
//! File effects never enter undo history.
use crate::{Error, Result};
use pcbcraft_core::{
    DefinitionId, Engine, LibraryCommand, Operation, Project, Query, QueryResult, erc,
    format::EditorFile,
    geometry::Point,
    layout,
    library::{
        Drafts,
        graphics::{GraphicTarget, SILK_STROKE_MM, SYMBOL_STROKE_MM},
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
                "Answer one read-only query: component.inspect reports an instance's exact component pin, resolved symbol and implementation, per-unit states, effective field values with their origin, active and unresolved terminal functions, occurrence and placement counts and completeness findings; component.check reports the publish findings, with the stage each blocks, of one draft or published revision; component.check_drafts reports every draft at once; library.search ranks component lineages at their latest published revision for a one-line query (free text, pkg:/has:/missing: filters and unit-aware field predicates such as res>=10k, tol<=1% or C:1n..100n, whose keys resolve against the project's field definitions; parse errors name the term, e.g. V>=10k: 10 k has no unit of voltage); library.facets lists the fields, package labels and completeness badges the matching entries carry; library.inspect reports one revision of a lineage with its revisions, symbols, implementations and badges; project.bom lists the bill of materials, resolved instances grouped by their manufacturer Part and unresolved ones mar; library.footprint_preview reports what library.edit_footprint would produce for a footprint draft (pads, graphics and findings, blocked; library.resolve_groups lists instances without an agreeing Part that share component revision, footprint and requirements, each group with the library.search query that finds Parts for it; ing ones included). Nothing is stored.",
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

/// A headless editing session: one engine and the current file path.
#[derive(Default)]
pub struct Session {
    engine: Engine,
    path: Option<PathBuf>,
}

impl Session {
    pub fn new(engine: Engine) -> Self {
        Self { engine, path: None }
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
                    &Default::default(),
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
            Request::ImportSvg(params) => return self.import_svg(params),
        }
        Ok(self.snapshot())
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

/// Request, document and diagnostic schemas for discovery.
pub fn schemas() -> Value {
    json!({
        "request": schemars::schema_for!(Request),
        "commands": Request::catalog(),
        "document": pcbcraft_core::schemas(),
        "solidcraft_revision": pcbcraft_solidcraft::SOLIDCRAFT_REVISION,
    })
}

#[cfg(test)]
mod package_tests;
#[cfg(test)]
mod parts_tests;
#[cfg(test)]
mod svg_tests;
#[cfg(test)]
mod tests;
