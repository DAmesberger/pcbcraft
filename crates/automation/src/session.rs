//! The automation request contract. Each request is a document query, a
//! document mutation through [`Engine::apply`], or an external file effect.
//! File effects never enter undo history.
use crate::{Error, Result};
use pcbcraft_core::{
    Class, DefinitionId, Engine, LibraryCommand, Operation, Project, Query, QueryResult,
    catalog::EntryKey,
    erc,
    fabrication::ProfileChoice,
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

/// Document operations applied in order as one undo step.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyAllParams {
    /// The revision the operations were prepared against.
    pub expected_revision: u64,
    /// One to [`pcbcraft_core::MAX_BATCH`] operations; each sees the
    /// document the ones before it made.
    pub operations: Vec<Operation>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

/// A document change that names nothing but the revision it expects
/// (`project.undo`, `project.redo`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevisionParams {
    /// The revision of the last reply; refused as stale when the document
    /// has moved since.
    pub expected_revision: u64,
}

/// Start a new project (`project.new`) with its board's fab profile.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewParams {
    /// The revision of the last reply; refused as stale when the document
    /// has moved since.
    pub expected_revision: u64,
    /// The board's fab profile: a profile library id or a whole profile
    /// file's `profile`. Omitted: the user's default fab profile
    /// (Settings); with neither, `rules.no_fab_profile` refuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<pcbcraft_core::fabrication::ProfileChoice>,
}

/// Open a project file in place of the current document.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenParams {
    /// The revision of the document being replaced; refused as stale when
    /// it has moved since.
    pub expected_revision: u64,
    pub path: String,
}

/// What `project.inspect` returns.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectParams {
    /// A JSON pointer into the snapshot, such as `/project/vias` or
    /// `/project/nets`: just what it names is returned.
    #[serde(default)]
    pub at: Option<String>,
}

/// Which schema `project.schema` returns.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaParams {
    /// An operation command (`layout.add_via`), a query (`library.search`)
    /// or a command or tool name. Without it, the index of all of them.
    #[serde(default)]
    pub name: Option<String>,
    /// One definition `name`'s reply lists under `more_defs`.
    #[serde(default)]
    pub def: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathParams {
    pub path: String,
}

/// Where `schematic.export_pdf` writes and which sheets it prints.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchematicPdfParams {
    pub path: String,
    /// Sheets to print, in page order; every sheet when empty.
    #[serde(default)]
    pub sheets: Vec<pcbcraft_core::SheetId>,
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
    /// The library id; drawn at random when omitted. A recorded session
    /// passes the one it recorded, so its replay reproduces the folder.
    #[serde(default)]
    pub library: Option<LibraryId>,
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
    #[serde(with = "pcbcraft_core::units::point_mm")]
    #[schemars(with = "[pcbcraft_core::units::Length; 2]")]
    pub position: pcbcraft_core::units::PointNm,
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
    /// Lineage ids for lineages new to the library, consumed in closure
    /// order (fewer than it needs refuses, unused ones are ignored); drawn
    /// at random when omitted. A recorded session passes the ones it
    /// recorded, so its replay reproduces the folder and the origins.
    #[serde(default)]
    pub fresh: Option<Vec<SourceLineageId>>,
}

/// Import one exact revision and its closure from another project file.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportFromProjectParams {
    pub expected_revision: u64,
    /// The `.pcbcraft` file to read. It is not changed.
    pub path: String,
    /// A published revision in that file.
    pub root: DefinitionRef,
}

/// Write the production files of the open project.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FabricationParams {
    /// A new or empty folder, or with `zip` a new `.zip` file.
    pub output: String,
    #[serde(default)]
    pub zip: bool,
    /// The fabrication fab profile to check against, by its id in the
    /// user's profile library (Pcbcraft ships none). The project's own
    /// rules when omitted.
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub options: pcbcraft_core::fabrication::FabricationOptions,
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
    /// Apply document operations in order at an expected revision as one
    /// undo step; any failure changes nothing.
    #[serde(rename = "project.apply_all")]
    ApplyAll(ApplyAllParams),
    /// Return the document, drafts with their findings and design checks.
    #[serde(rename = "project.inspect")]
    Inspect(InspectParams),
    /// The exact schema of one operation, query or command, or the index.
    #[serde(rename = "project.schema")]
    Schema(SchemaParams),
    /// Answer one read-only core query (`component.inspect`,
    /// `component.check`, `component.check_drafts`, `library.search`,
    /// `library.facets`, `library.inspect`, `project.bom`,
    /// `library.resolve_groups`, `project.issues`), passed unchanged to
    /// `Engine::query`.
    #[serde(rename = "project.query")]
    Query(Query),
    /// Undo the latest step at an expected revision.
    #[serde(rename = "project.undo")]
    Undo(RevisionParams),
    /// Redo the latest undone step at an expected revision.
    #[serde(rename = "project.redo")]
    Redo(RevisionParams),
    /// Start a new, empty project. History is cleared.
    #[serde(rename = "project.new")]
    New(NewParams),
    /// Open a `.pcbcraft` file; a failure leaves the session unchanged.
    #[serde(rename = "project.open")]
    Open(OpenParams),
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
    /// Check the design and write its production files (Gerber X2,
    /// Excellon, job file, pick-and-place, BOM, PDF drawings).
    #[serde(rename = "project.export_fabrication")]
    ExportFabrication(FabricationParams),
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
    /// Read another project file and apply `library.import` with one exact
    /// revision and its closure: one undo step.
    #[serde(rename = "library.import_from_project")]
    ImportFromProject(ImportFromProjectParams),
    /// Read an SVG file and add its drawing to a draft through
    /// `library.edit_graphics`: one atomic undo step.
    #[serde(rename = "library.import_svg")]
    ImportSvg(ImportSvgParams),
    /// A2: inspect a folder item without importing it.
    #[serde(rename = "library.open_preview")]
    OpenPreview(BundleParams),
    /// C3: the typed pinout document schema.
    #[serde(rename = "library.pinout_schema")]
    PinoutSchema(NoParams),
    /// Check a datasheet review report against the drafts.
    #[serde(rename = "library.review_check")]
    ReviewCheck(crate::review::ReviewCheckParams),
    /// C4: the emitted firmware pin-configuration text.
    #[serde(rename = "firmware.export_preview")]
    FirmwarePreview(firmware::FirmwareParams),
    /// C4: write the emitted text to a new file.
    #[serde(rename = "firmware.export")]
    FirmwareExport(firmware::FirmwareParams),
    /// D2: inspect a STEP, GLB or .solidcraft model file.
    #[serde(rename = "library.inspect_model_file")]
    InspectModelFile(models::ModelFileParams),
    /// Set a component draft's marking with SVG artwork as text.
    #[serde(rename = "library.set_marking")]
    SetMarking(marking::SetMarkingParams),
    /// Print the schematic as a vector PDF.
    #[serde(rename = "schematic.export_pdf")]
    SchematicPdf(SchematicPdfParams),
    /// Where a package can be marked: model area, box faces, anchors.
    #[serde(rename = "library.marking_surfaces")]
    MarkingSurfaces(marking::MarkingSurfacesParams),
    /// Lean parts: evaluate and check a Solidcraft part file at values.
    #[serde(rename = "library.check_part_file")]
    CheckPartFile(part_files::CheckPartFileParams),
    /// Lean parts: a package draft uses a part file at values.
    #[serde(rename = "library.use_part_file")]
    UsePartFile(part_files::UsePartFileParams),
    /// Lean parts: new values for a package draft's part file.
    #[serde(rename = "library.set_part_values")]
    SetPartValues(part_files::SetPartValuesParams),
    /// Mechanical parts: the part files a project can place.
    #[serde(rename = "library.part_files")]
    PartFiles(NoParams),
    /// Mechanical parts: place a part file directly on the board.
    #[serde(rename = "layout.place_part_file")]
    PlacePartFile(Box<mechanical_parts::PlacePartFileParams>),
    /// Mechanical parts: new values (or flags) for a placed part.
    #[serde(rename = "layout.set_part_file_values")]
    SetPartFileValues(mechanical_parts::SetPartFileValuesParams),
}

// A new request chooses its permission class here. `project.apply` and
// `project.query` take their operation's or query's; a request that names a
// path needs at least Ask ([`Request::effective_class`]).
pcbcraft_core::declare_classes!(Request {
    Apply "project.apply" Edit "Apply an operation",
    ApplyAll "project.apply_all" Edit "Apply operations as one step",
    Inspect "project.inspect" Read "Inspect the project",
    Schema "project.schema" Read "Look up a schema",
    Query "project.query" Read "Run a query",
    Undo "project.undo" Edit "Undo",
    Redo "project.redo" Edit "Redo",
    New "project.new" Ask "New project",
    Open "project.open" Ask "Open a project",
    Save "project.save" Ask "Save the project",
    PreviewUpdate "project.preview_update" Read "Preview an update",
    ApplyUpdate "project.apply_update" Edit "Apply an update",
    ExportNative "project.export_native" Ask "Export the 3D model",
    ExportFabrication "project.export_fabrication" Ask "Export fabrication files",
    Sources "library.sources" Read "List library folders",
    SourceAdd "library.source.add" Never "Add a library folder",
    SourceCreate "library.source.create" Never "Create a library folder",
    SourceRemove "library.source.remove" Never "Remove a library folder",
    SourceEnable "library.source.enable" Never "Turn a library folder on or off",
    Rescan "library.rescan" Read "Rescan library folders",
    Favourite "library.favourite" Never "Mark a favourite",
    Bundle "library.bundle" Read "Read a folder bundle",
    ImportFrom "library.import_from_folder" Edit "Import from a folder",
    PlaceFrom "library.place_from_folder" Edit "Place from a folder",
    Publish "library.publish_to_folder" Ask "Publish to a folder",
    ImportFromProject "library.import_from_project" Ask "Import from another project",
    ImportSvg "library.import_svg" Ask "Import SVG artwork",
    OpenPreview "library.open_preview" Read "Preview a folder entry",
    PinoutSchema "library.pinout_schema" Read "Pinout document schema",
    ReviewCheck "library.review_check" Read "Check a datasheet review report",
    FirmwarePreview "firmware.export_preview" Read "Preview a firmware export",
    FirmwareExport "firmware.export" Ask "Export firmware pin configuration",
    InspectModelFile "library.inspect_model_file" Ask "Inspect a model file",
    SetMarking "library.set_marking" Edit "Set a component marking",
    SchematicPdf "schematic.export_pdf" Ask "Export the schematic as PDF",
    MarkingSurfaces "library.marking_surfaces" Read "List marking surfaces",
    CheckPartFile "library.check_part_file" Read "Check a part file",
    UsePartFile "library.use_part_file" Edit "Use a part file as the model",
    SetPartValues "library.set_part_values" Edit "Set part file values",
    PartFiles "library.part_files" Read "List part files",
    PlacePartFile "layout.place_part_file" Edit "Place a part file on the board",
    SetPartFileValues "layout.set_part_file_values" Edit "Change a placed part file",
});

impl Request {
    /// The class the gate acts on: the one listed, or the operation's or
    /// query's for those two requests, and at least Ask once a path is named.
    pub fn effective_class(&self) -> Class {
        let own = match self {
            Self::Apply(params) => params.operation.class(),
            // At least an edit, so an empty batch is no read.
            Self::ApplyAll(params) => {
                Class::strictest(params.operations.iter().map(Operation::class)).max(Class::Edit)
            }
            Self::Query(query) => query.class(),
            other => other.class(),
        };
        if self.paths().is_empty() {
            own
        } else {
            own.max(Class::Ask)
        }
    }

    /// What the gate reports as requested: the operation or query inside
    /// `project.apply` and `project.query`, else the request itself.
    pub fn subject(&self) -> &'static str {
        match self {
            Self::Apply(params) => params.operation.command(),
            // The strictest member speaks for the batch.
            Self::ApplyAll(params) => {
                let class = Class::strictest(params.operations.iter().map(Operation::class));
                params
                    .operations
                    .iter()
                    .find(|operation| operation.class() == class)
                    .map_or("project.apply_all", Operation::command)
            }
            Self::Query(query) => query.command(),
            other => other.command(),
        }
    }

    /// The requests whose [`Request::paths`] can be non-empty: what the
    /// catalog, tool descriptions and schema rows say about asking with a
    /// path. `gate/tests.rs` checks the two against each other.
    pub const NAMING_PATHS: &'static [&'static str] = &[
        "project.open",
        "project.save",
        "project.export_native",
        "project.export_fabrication",
        "library.source.add",
        "library.source.create",
        "library.source.remove",
        "library.source.enable",
        "library.import_from_project",
        "library.import_svg",
        "firmware.export_preview",
        "firmware.export",
        "library.inspect_model_file",
        "schematic.export_pdf",
        "library.check_part_file",
        "library.use_part_file",
        "layout.place_part_file",
    ];

    /// Every file or folder path this request names.
    pub fn paths(&self) -> Vec<&str> {
        match self {
            Self::Open(params) => vec![params.path.as_str()],
            Self::ExportNative(params) | Self::SourceRemove(params) => {
                vec![params.path.as_str()]
            }
            Self::Save(params) => params.path.iter().map(String::as_str).collect(),
            Self::ExportFabrication(params) => vec![params.output.as_str()],
            Self::SourceAdd(params) => vec![params.path.as_str()],
            Self::SourceCreate(params) => vec![params.path.as_str()],
            Self::SourceEnable(params) => vec![params.path.as_str()],
            Self::ImportFromProject(params) => vec![params.path.as_str()],
            Self::ImportSvg(params) => vec![params.path.as_str()],
            Self::FirmwarePreview(params) | Self::FirmwareExport(params) => params
                .path
                .iter()
                .chain(&params.template_file)
                .map(String::as_str)
                .collect(),
            Self::InspectModelFile(params) => vec![params.path.as_str()],
            Self::SchematicPdf(params) => vec![params.path.as_str()],
            Self::CheckPartFile(params) => params.path.iter().map(String::as_str).collect(),
            Self::UsePartFile(params) => params.path.iter().map(String::as_str).collect(),
            Self::PlacePartFile(params) => params.path.iter().map(String::as_str).collect(),
            Self::Apply(_)
            | Self::ApplyAll(_)
            | Self::Inspect(_)
            | Self::Schema(_)
            | Self::Query(_)
            | Self::Undo(_)
            | Self::Redo(_)
            | Self::New(_)
            | Self::PreviewUpdate(_)
            | Self::ApplyUpdate(_)
            | Self::Sources(_)
            | Self::Rescan(_)
            | Self::Favourite(_)
            | Self::Bundle(_)
            | Self::ImportFrom(_)
            | Self::PlaceFrom(_)
            | Self::Publish(_)
            | Self::OpenPreview(_)
            | Self::PinoutSchema(_)
            | Self::ReviewCheck(_)
            | Self::SetMarking(_)
            | Self::MarkingSurfaces(_)
            | Self::SetPartValues(_)
            | Self::PartFiles(_)
            | Self::SetPartFileValues(_) => Vec::new(),
        }
    }

    /// The revision a request that changes the document expects, checked
    /// once before it runs ([`Session::execute`]), so a stale one does
    /// nothing at all: no file read or written, no step taken or undone.
    /// `None` only for a request that changes no document (a query, a file
    /// export, the user's library settings).
    pub fn expected_revision(&self) -> Option<u64> {
        match self {
            Self::Apply(params) => Some(params.expected_revision),
            Self::ApplyAll(params) => Some(params.expected_revision),
            Self::Undo(params) | Self::Redo(params) => Some(params.expected_revision),
            Self::New(params) => Some(params.expected_revision),
            Self::Open(params) => Some(params.expected_revision),
            Self::ApplyUpdate(params) => Some(params.expected_revision),
            Self::ImportFrom(params) => Some(params.expected_revision),
            Self::PlaceFrom(params) => Some(params.expected_revision),
            Self::Publish(params) => Some(params.expected_revision),
            Self::ImportFromProject(params) => Some(params.expected_revision),
            Self::ImportSvg(params) => Some(params.expected_revision),
            Self::SetMarking(params) => Some(params.expected_revision),
            Self::UsePartFile(params) => Some(params.expected_revision),
            Self::SetPartValues(params) => Some(params.expected_revision),
            Self::PlacePartFile(params) => Some(params.expected_revision),
            Self::SetPartFileValues(params) => Some(params.expected_revision),
            Self::Inspect(_)
            | Self::Schema(_)
            | Self::Query(_)
            | Self::Save(_)
            | Self::PreviewUpdate(_)
            | Self::ExportNative(_)
            | Self::ExportFabrication(_)
            | Self::Sources(_)
            | Self::SourceAdd(_)
            | Self::SourceCreate(_)
            | Self::SourceRemove(_)
            | Self::SourceEnable(_)
            | Self::Rescan(_)
            | Self::Favourite(_)
            | Self::Bundle(_)
            | Self::OpenPreview(_)
            | Self::PinoutSchema(_)
            | Self::ReviewCheck(_)
            | Self::FirmwarePreview(_)
            | Self::FirmwareExport(_)
            | Self::InspectModelFile(_)
            | Self::SchematicPdf(_)
            | Self::MarkingSurfaces(_)
            | Self::CheckPartFile(_)
            | Self::PartFiles(_) => None,
        }
    }

    /// Every request ID with its one-line summary, description and parameter
    /// schema, generated from the Rust types the dispatcher deserializes.
    /// The summary says what the command is for in its first words, within
    /// [`metadata::MAX_SUMMARY`] bytes; the description adds the detail.
    pub fn catalog() -> Vec<CommandInfo> {
        fn info<T: JsonSchema>(
            id: &'static str,
            summary: &'static str,
            description: &'static str,
        ) -> CommandInfo {
            let params = serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null);
            CommandInfo {
                id,
                // Every listed request has one; the metadata tests check.
                title: Request::TITLES
                    .iter()
                    .find(|(name, _)| *name == id)
                    .map_or(id, |(_, title)| *title),
                summary,
                description,
                // Unlisted cannot pass the gate's tests; refused until then.
                class: Request::CLASSES
                    .iter()
                    .find(|(name, _)| *name == id)
                    .map_or(Class::Never, |(_, class)| *class),
                takes_path: Request::NAMING_PATHS.contains(&id),
                params,
            }
        }
        vec![
            info::<ApplyParams>(
                "project.apply",
                "Apply one typed document operation at expected_revision; a stale revision or invalid input changes nothing.",
                "Apply one typed document operation (library, instance including instance.assign_part, schematic, layout including layout.set_fabrication, layout.set_stackup, layout.edit_stackup and layout.add_via, or update) at an expected revision and return its outcome. Fails without changes on stale revisions or invalid input.",
            ),
            info::<ApplyAllParams>(
                "project.apply_all",
                "Apply typed document operations in order at expected_revision as one undo step; any failure changes nothing.",
                "Apply one to 256 typed document operations, in order, at an expected revision as one undo step and one revision, and return their outcomes in order. Each is checked against the document the ones before it made; a stale revision or any failing operation (named by its place) changes nothing.",
            ),
            info::<InspectParams>(
                "project.inspect",
                "Read the revision, object counts, drafts and open findings; at reads one part of the document.",
                "Return the revision, object counts, each draft's state and open findings by code. verbose: true returns the whole snapshot (project, drafts with their publish findings, ERC and board-check findings; about 0.5 MB on a small board); at: a JSON pointer into it (/project/vias, /project/nets) returns just what it names.",
            ),
            info::<SchemaParams>(
                "project.schema",
                "Look up one operation, query or command's exact schema and how to send it; without a name, the index.",
                "The exact JSON schema of one project.apply operation (layout.add_via), project.query query (library.search) or command, with how to send it; large definitions come separately through def. Without a name, the index of all of them with one line each. Not a document change.",
            ),
            info::<Query>(
                "project.query",
                "Answer one read-only query by name, such as project.issues, library.search or component.inspect.",
                "Answer one read-only query: component.inspect reports an instance's exact component pin, resolved symbol and implementation, per-unit states, effective field values with their origin, active and unresolved terminal functions, occurrence and placement counts and completeness findings; component.check reports the publish findings, with the stage each blocks, of one draft or published revision; component.check_drafts reports every draft at once; library.search ranks component lineages at their latest published revision for a one-line query (free text, pkg:/has:/missing: filters and unit-aware field predicates such as res>=10k, tol<=1% or C:1n..100n, whose keys resolve against the project's field definitions; parse errors name the term, e.g. V>=10k: 10 k has no unit of voltage); library.facets lists the fields, package labels and completeness badges the matching entries carry; library.inspect reports one revision of a lineage with its revisions, symbols, implementations and badges; project.bom lists the bill of materials, resolved instances grouped by their manufacturer Part and unresolved ones marked; library.footprint_preview reports what library.edit_footprint would produce for a footprint draft (pads, graphics and findings, blocking ones included); library.resolve_groups lists instances without an agreeing Part that share component revision, footprint and requirements, each group with the library.search query that finds Parts for it; library.updates_available lists project lineages copied from a library source that holds a newer revision or records whose hash changed (modified lists those source revisions); library.promote_preview reports the bundle and origins library.promote would write and record (library.publish_to_folder runs it); project.issues lists every issue of the project (scope all, an area of component, schematic or board, or a selection of objects), uncapped and most severe first, each with core's severity (error for anything that blocks a stage or breaks a board or electrical rule, warning, to_do for unrouted, unplaced and unpurchased work, info), group, one-line title naming objects by their names, cause and fix, the finding it comes from and, under also, the same-cause findings merged into it; elsewhere counts the issues of areas outside the scope. library.search and library.facets include the entries of every ready library source. Nothing is stored.",
            ),
            info::<RevisionParams>(
                "project.undo",
                "Undo the last document change at expected_revision.",
                "Undo the last document change at expected_revision. Refused without changes on a stale revision, so a newer change by someone else is never undone.",
            ),
            info::<RevisionParams>(
                "project.redo",
                "Redo the last undone document change at expected_revision.",
                "Redo the last undone document change at expected_revision. Refused without changes on a stale revision.",
            ),
            info::<NewParams>(
                "project.new",
                "Start a new empty project at expected_revision with its board's fab profile, clearing history.",
                "Start a new empty project at expected_revision; clears history. profile is the board's fab profile: a profile library id (project.query rules.fab_profiles lists them) or a whole profile file's profile; omitted, the user's default fab profile; with neither, rules.no_fab_profile refuses. Refused without changes on a stale revision.",
            ),
            info::<OpenParams>(
                "project.open",
                "Open a .pcbcraft project file in place of the document at expected_revision.",
                "Open a .pcbcraft project file of the current format version in place of the document at expected_revision; other versions are refused. Failure, a stale revision included, leaves the session unchanged.",
            ),
            info::<SaveParams>(
                "project.save",
                "Save the project atomically to a .pcbcraft file, by default its current path.",
                "Atomically save to a .pcbcraft file (defaults to the current path). Not undoable.",
            ),
            info::<PreviewParams>(
                "project.preview_update",
                "Preview a component-revision, instance, schematic or board-sync change and its routed-copper impact.",
                "Preview a revision-bound component-revision, instance, schematic or PCB-sync change and its routed-copper impact.",
            ),
            info::<updates::ApplyUpdate>(
                "project.apply_update",
                "Apply a previewed update at its revision, optionally removing the listed conflicting tracks.",
                "Apply a previewed update at its revision, optionally removing the listed conflicting tracks.",
            ),
            info::<PathParams>(
                "project.export_native",
                "Export the board as a native parametric .solidcraft 3D model to a new file.",
                "Export the board as a native parametric .solidcraft model to a new file.",
            ),
            info::<FabricationParams>(
                "project.export_fabrication",
                "Check the board for production, then write Gerber, drill, pick-and-place, BOM and drawing files.",
                "Run the production check (fabrication.preflight) under the chosen fab profile, then write the production files to a new or empty folder, or one new .zip: Gerber X2 copper, solder mask, paste, legend and profile layers with file, aperture and object attributes; plated and non-plated Excellon drill files with routed slots, a drill map and table; a Gerber job file; pick-and-place and BOM CSVs; fabrication and assembly drawings (PDF). Refused, writing nothing, while unwaived errors remain (waive with project.apply fabrication.waive). Returns the preflight report, the files with sizes and step timings. Not a document change.",
            ),
            info::<NoParams>(
                "library.sources",
                "List the configured library folders with their state, or why one is disabled.",
                "List the configured library folders in settings order: path, enabled, writable, state (ready with library id, name, entry and revision counts and how the index was obtained; disabled with the reason, e.g. path missing or unsupported format v2; failed with a diagnostic). Not a document change.",
            ),
            info::<SourceAddParams>(
                "library.source.add",
                "Add a library folder as a source in the person's settings.",
                "Add a library folder (pcbcraft-library.json manifest) as a source in the per-user settings. A missing folder, a malformed manifest, another format version, an unknown required feature, a repeated path or a repeated library id is refused and nothing changes. library.search and library.facets then include its entries (source library, with source_lineage).",
            ),
            info::<SourceCreateParams>(
                "library.source.create",
                "Create an empty library folder and add it as a writable source.",
                "Create an empty library folder and add it as a writable source. Its library id is random unless library supplies one (a recorded session passes the id it recorded, so a replay reproduces the folder).",
            ),
            info::<PathParams>(
                "library.source.remove",
                "Remove a library source from the settings; the folder stays untouched.",
                "Remove a library source from the settings. The folder and every project copy are untouched.",
            ),
            info::<SourceEnableParams>(
                "library.source.enable",
                "Turn a library source on or off.",
                "Turn a library source on or off; an off source stays listed as disabled (turned off).",
            ),
            info::<NoParams>(
                "library.rescan",
                "Re-read every library source's manifest and index, then list the sources.",
                "Re-read every source's manifest and catalog index (after a git pull or another writer), then list the sources.",
            ),
            info::<FavouriteParams>(
                "library.favourite",
                "Mark or unmark a library lineage as a favourite, which ranks search results.",
                "Mark or unmark a lineage, keyed by project lineage or by library id and source lineage, as a favourite. Favourites rank library.search results and persist per user, never in a project.",
            ),
            info::<BundleParams>(
                "library.bundle",
                "Read one revision of a folder lineage with its dependency closure and packed assets.",
                "Read one revision of a folder lineage (default the newest) with its complete dependency closure and packed assets, as the bundle library.import and library.place_from carry.",
            ),
            info::<ImportFromParams>(
                "library.import_from_folder",
                "Import a folder lineage's revision and its closure into the project in one undo step.",
                "Read a folder bundle and apply library.import with it at the expected revision: the closure is copied into the project under fresh project lineages, or appended as the next project revision of a lineage already imported from the same source; an equal origin is reused. One undo step; places nothing. Follow with project.preview_update to move instances to the new revision.",
            ),
            info::<ImportFromProjectParams>(
                "library.import_from_project",
                "Copy one published revision and its closure from another .pcbcraft file into this project.",
                "Read another .pcbcraft file (never changed) and copy one published revision (root, with its kind, id and revision in that file) and its complete closure into this project, packed assets included, as one library.import at expected_revision: one undo step, nothing placed. Each copy records its origin with the exact source revision and content hash, and every pin keeps naming the exact revision it names there. The project numbers its copies in import order. Ids are derived from the file's content, so a replay is reproducible and importing again reuses the copies (no undo step); a newer revision of a copied lineage becomes its next project revision. A draft in that file cannot be imported: publish it there first. An unknown root lists what the file holds. Refused without changes on a stale revision, a dangling reference, a model that may not be redistributed, or a revision that differs from the copy already held (library.source_modified).",
            ),
            info::<PlaceFromParams>(
                "library.place_from_folder",
                "Import a folder component if needed and place it, in one undo step.",
                "Read a folder bundle of a component lineage and apply library.place_from with it: import if needed, then place, as one transaction and one undo step. When the project holds another revision of that lineage, other_revision refuse (default) fails with code library.other_revision and keep_both imports the folder revision beside it.",
            ),
            info::<PublishParams>(
                "library.publish_to_folder",
                "Promote a published project revision and its closure to a writable library folder.",
                "Promote a published project revision and its closure to a writable library source: compute library.promote_preview with fresh lineage ids (random unless fresh supplies them, as a recorded session does so that a replay reproduces the folder and origins; too few refuses before anything is written), write the records and assets to the folder (an existing rNNNN.json with other content refuses the write before anything is written), then apply library.promote, which records the origins as one undo step. Returns the written and unchanged record paths.",
            ),
            info::<ImportSvgParams>(
                "library.import_svg",
                "Add a .svg file's drawing to a symbol unit or footprint layer as editable graphics.",
                "Read a .svg file (at most 1 MiB) and add its drawing to a symbol unit or a footprint's silkscreen or assembly layer as editable graphics, centred at `at` and scaled from the SVG's declared millimetre size: circles, circular arcs and axis-aligned rectangles stay exact, tangent arcs become corner radii, Béziers and ellipses flatten within tolerance_mm, filled closed shapes become solid fills and strokes keep their width. Filled regions with holes are imported as outlines; text, images and use references are skipped and listed in import.notes. Applied as one library.edit_graphics step at expected_revision: atomic and one undo step; the outcome lists the new graphic IDs.",
            ),
            info::<BundleParams>(
                "library.open_preview",
                "Inspect one revision of a folder lineage without importing it.",
                "Inspect one revision of a folder lineage (default the newest) without importing it: the same report library.inspect gives a project item. Not a document change.",
            ),
            info::<NoParams>(
                "library.pinout_schema",
                "The schema of the pinout document an agent fills from a datasheet, with its pin-rule API.",
                "The JSON schema of the typed pinout document (format 1) an agent fills from a datasheet: device, package variants, peripherals with roles, pins with explicit numbers per variant, GPIO, options, functions with explicit peripheral, role and mux code, source page, uncertain flags, and Rhai pin-rule scripts. Preview with project.query library.pinout_preview, show the review items to the user, then apply with project.apply library.pinout_apply; never publish without the user seeing the unreviewed count.",
            ),
            info::<crate::review::ReviewCheckParams>(
                "library.review_check",
                "Check a datasheet part's review report against the drafts: citations, confidence and open questions.",
                "Check the review report of a part made from datasheets (the datasheet_to_component prompt's last step) against the project's drafts: every value with a listed document, page, location (table, figure or section) and confidence (high: a table cell or dimensioned drawing; medium: derived or ambiguous; low: inferred, measured or not stated); distinct sources; open questions when any value is low confidence or any imported pin is uncertain; a component draft; and a source page on every imported function. Returns accepted, the problems to fix, each draft's blocking and warning finding codes, the unreviewed and uncertain pin counts and the report as Markdown for the person. Accepted means ready for a person's review, never verified or publishable. Not a document change.",
            ),
            info::<firmware::FirmwareParams>(
                "firmware.export_preview",
                "Emit an instance's firmware pin configuration as JSON, C, a Zephyr overlay, Rust or a template.",
                "Emit one instance's firmware pin configuration (project.pin_config) as JSON, a C header, a Zephyr overlay skeleton, Rust consts or a user template. Pin configuration only; a starting point, never a claim to configure clocks or registers.",
            ),
            info::<firmware::FirmwareParams>(
                "firmware.export",
                "Write a firmware pin-configuration export to a new file.",
                "Write firmware.export_preview's text atomically to a new file at path. Not a document change.",
            ),
            info::<marking::SetMarkingParams>(
                "library.set_marking",
                "Set or clear a component draft's display-only 3D marking: text, artwork or polarity marks.",
                "Set or clear a component draft's generic marking: display-only decals on the 3D model, never saved to .solidcraft or fabrication output. Items: text from templates ({mpn}, {designator}, {field:<id>}, {var:lot}), SVG artwork given as text with its licence (packed by SHA-256; never supply a logo you have no right to: a placeholder box marks where the user's own goes), polarity marks, a generated QR code (code2d: payload template, size_mm, anchor) and an electrolytic's µF/V text with its negative stripe. placement.on picks the surface: model (top marking area), a box face (top, bottom, pos_x, neg_x, pos_y, neg_y) or a named anchor from library.marking_surfaces (module: can_top, can_pos_x…, pcb_top, antenna_area; radial: top, sleeve; axial or MELF: body); then offset_mm, rotation_deg, scale, mirror, clip and, on a cylinder, wrap {seam_deg, wrap_deg, along_mm, repeat} with arc length kept. finish: laser_etch, laser_etch_metal (shield cans), ink_white (silkscreen), {ink: {colour}}, {pad_print: {colour}} or engraved. decals: up to four further decals, each with its own items, finish and placement. One library.pack_artwork (or library.edit) step at expected_revision; refused whole on any problem.",
            ),
            info::<marking::MarkingSurfacesParams>(
                "library.marking_surfaces",
                "List where a package can be marked: the model area, box faces and named anchors.",
                "List where a package draft or published package can be marked, for library.set_marking's placement.on: the model area, the six box faces and its family's named anchors (each with a description, plane or cylinder, and size; a cylinder's circumference and whether its u runs along the axis). Not a document change.",
            ),
            info::<SchematicPdfParams>(
                "schematic.export_pdf",
                "Print the schematic as a vector PDF, one page per sheet.",
                "Print the schematic as a vector PDF at path (written atomically, replacing a file there): one page per sheet in page order at its paper size, with the page frame and title block; sheets limits it to a selection (all when empty) and a sheet symbol links to its child sheet's page. Returns the page count. Not a document change.",
            ),
            info::<part_files::CheckPartFileParams>(
                "library.check_part_file",
                "Evaluate a SolidCraft part file at given parameter values and report its convention check.",
                concat!(
                    "Evaluate a SolidCraft part file (path to a .solidcraft design, or builtin: esp32-s31-wroom-3) at literal values for some of its user parameters ({\"can_height\": {\"mm\": 2.465}}; units mm, deg, count, number) and report the convention check (docs/part-templates.md §11): parameters with their values, bodies with the ",
                    pcbcraft_core::product_name!(),
                    " material each appearance names, marking surfaces from marking-<name> construction planes sized by marking_<name>_u/_v, package datums from opening-<name> construction planes (sized by <datum>_u and <datum>_v, optional <datum>_r, <datum>_depth and <datum>_clearance; the normal points out along the plug's way in) and axis-<name> construction axes (optional diameter <datum>_d), the pin1 point, bounds, timing and findings (part_file.read, part_file.structure, part_file.value, part_file.feature, part_file.seating, part_file.material, part_file.marking, part_file.datum blocking; part_file.origin, part_file.no_marking, part_file.import, part_file.slow advisory). Lists the material appearance names. Not a document change."
                ),
            ),
            info::<part_files::UsePartFileParams>(
                "library.use_part_file",
                "Make a SolidCraft part file a package draft's 3D model, at literal parameter values.",
                "Make a package draft's 3D model a SolidCraft part file at literal parameter values: the file (path, with its licence, or a builtin) is checked, packed as a Model of format solidcraft (or the same file's existing model is reused) and the package's model set to it, with the evaluated bounds, marking surfaces and declared datums stored (the datums become the package's, marked as the part file's), in one undo step at expected_revision. Refused whole on a blocking finding. The footprint, terminals and keep-outs stay the package's own. Returns the outcome and the check report.",
            ),
            info::<part_files::SetPartValuesParams>(
                "library.set_part_values",
                "Set new parameter values for the part file a package draft uses.",
                "Set new literal values for the part file a package draft uses (they replace the previous values), re-evaluated and checked, in one undo step at expected_revision; refused whole on a blocking finding. Returns the outcome and the check report.",
            ),
            info::<NoParams>(
                "library.part_files",
                "List the SolidCraft part files this project can place or use.",
                "List the SolidCraft part files this project can place or use (docs/mechanical-parts.md): the built-ins (name, title, designator prefix, kind pads, mechanical or package, parameters with units and defaults, licence) and the project's Models of format solidcraft (model, name, file, licence). Not a document change.",
            ),
            info::<mechanical_parts::PlacePartFileParams>(
                "layout.place_part_file",
                "Place a parametric 3D part such as a nut, standoff, spacer or heatsink on the board.",
                "Place a parametric 3D part (an SMD nut or standoff, spacer, heatsink, bracket: path with its licence, builtin or model) directly on the board at position (mm, the part origin), rotation_deg and layer, at literal parameter values. The file is checked (blocking findings refuse) and evaluated; lands come from lands, lands_from (a footprint's pads) or a built-in's land rules, else none. pads: mechanical (default, no terminal) or terminals (each copper land a terminal that can join a net through the part's symbol on a sheet). bom (default true) and pick_and_place (default: has copper lands). The variant (file, values, lands, flags) is reused or made as an ordinary component set with a generated footprint and courtyard, and a new instance placed with its symbol unit off-sheet, in one undo step at expected_revision. Returns the outcome (PartPlaced: instance, designator, component, created) and the check report.",
            ),
            info::<mechanical_parts::SetPartFileValuesParams>(
                "layout.set_part_file_values",
                "Give a placed mechanical part new parameter values, pads or BOM flags.",
                "Give a placed mechanical part new literal values (they replace the previous values) and optionally new pads, bom or pick_and_place: re-evaluated and checked, a built-in's lands re-derived (other lands kept), and the instance moved to the variant for them (reused or made), keeping its designator, placement and marking, in one undo step at expected_revision. Returns the snapshot, the outcome and the check report.",
            ),
            info::<models::ModelFileParams>(
                "library.inspect_model_file",
                "Read a STEP, GLB or .solidcraft model file and report its bodies, materials and bounds.",
                "Read a STEP AP214/242, GLB or .solidcraft model file and report its bodies (keys, source colours, glTF materials), bounds and whether it has textures, for the import wizard's explicit mapping. Not a document change.",
            ),
        ]
    }
}

impl Request {
    /// [`Request::catalog`], generated once.
    pub fn catalog_shared() -> &'static [CommandInfo] {
        static CATALOG: std::sync::OnceLock<Vec<CommandInfo>> = std::sync::OnceLock::new();
        CATALOG.get_or_init(Request::catalog)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CommandInfo {
    pub id: &'static str,
    /// A short name for people (the MCP tool `title`).
    pub title: &'static str,
    /// One line: what the command is for, without the detail.
    pub summary: &'static str,
    pub description: &'static str,
    pub class: Class,
    /// Whether it can name a path, which makes it at least Ask
    /// ([`Request::NAMING_PATHS`]).
    pub takes_path: bool,
    pub params: Value,
}

impl CommandInfo {
    /// The sentence a tool description ends with, so a model can see what
    /// will need asking.
    pub fn permission_note(&self) -> String {
        match (self.id, self.class, self.takes_path) {
            ("project.apply", ..) => {
                "Permission: edit; project_schema marks the operations that ask.".to_owned()
            }
            (_, class @ (Class::Read | Class::Edit), true) => {
                format!("Permission: {}, or ask with a path.", class.name())
            }
            (_, class, _) => format!("Permission: {}.", class.name()),
        }
    }
}

/// A headless editing session: one engine, the current file path and the
/// library sources.
#[derive(Default)]
pub struct Session {
    engine: Engine,
    path: Option<PathBuf>,
    sources: sources::Sources,
    /// Set while [`Session::respond`] runs a request: mutations answer
    /// without building the snapshot.
    compact: bool,
    baseline: Option<reply::Baseline>,
}

impl Session {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            ..Self::default()
        }
    }
    /// A session with the given library sources, e.g.
    /// [`sources::Sources::from_env`].
    pub fn with_sources(engine: Engine, sources: sources::Sources) -> Self {
        Self {
            engine,
            sources,
            ..Self::default()
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

    /// Run `f` on this session over a host's own engine (the desktop
    /// window's, for live MCP), then hand the engine back, its revision and
    /// undo history moved on by whatever `f` did. `path` is the host's file,
    /// which requests that name none (`project.save`) would use.
    pub fn lend<R>(
        &mut self,
        engine: &mut Engine,
        path: Option<&Path>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        std::mem::swap(&mut self.engine, engine);
        self.path = path.map(Path::to_path_buf);
        let result = f(self);
        std::mem::swap(&mut self.engine, engine);
        result
    }

    /// Execute one typed request.
    pub fn execute(&mut self, request: Request) -> Result<Value> {
        // The one revision check of every document-changing request.
        if let Some(expected) = request.expected_revision() {
            self.engine.check_revision(expected)?;
        }
        match request {
            Request::Apply(params) => {
                let outcome = self.engine.apply_hosted(
                    params.expected_revision,
                    params.operation,
                    hosted(),
                )?;
                let mut reply = self.document_reply();
                reply["outcome"] = serde_json::to_value(outcome)?;
                self.add_rerouted(&mut reply)?;
                return Ok(reply);
            }
            Request::ApplyAll(params) => {
                let outcomes = self.engine.apply_all_hosted(
                    params.expected_revision,
                    params.operations,
                    hosted(),
                )?;
                let mut reply = self.document_reply();
                reply["outcomes"] = serde_json::to_value(outcomes)?;
                self.add_rerouted(&mut reply)?;
                return Ok(reply);
            }
            Request::Inspect(params) => return self.inspect(params),
            Request::Schema(params) => return discovery::schema_reply(params),
            Request::Query(query) => {
                // A profile id reads as it does for an export: one of the
                // user's library.
                if let Query::RuleProfiles {} = query {
                    let mut profiles: Vec<_> = self
                        .engine
                        .project()
                        .rules
                        .profile
                        .iter()
                        .cloned()
                        .collect();
                    let dirs = pcbcraft_io::settings::UserDirs::from_env()?;
                    let listing =
                        pcbcraft_io::rule_profiles::ProfileLibrary::from_dirs(&dirs).list()?;
                    for p in listing.profiles {
                        if !profiles.iter().any(|q| q.id == p.id) {
                            profiles.push(p);
                        }
                    }
                    return Ok(serde_json::to_value(
                        pcbcraft_core::QueryResult::RuleProfiles(profiles),
                    )?);
                }
                let query = match query {
                    Query::Preflight {
                        profile: Some(ProfileChoice::Id(id)),
                    } => Query::Preflight {
                        profile: Some(ProfileChoice::Profile(Box::new(
                            pcbcraft_io::fabrication::resolve_profile(&id)?,
                        ))),
                    },
                    other => other,
                };
                return Ok(serde_json::to_value(self.engine.query_using(
                    query,
                    &self.sources.context(),
                    Some(&pcbcraft_solidcraft::footprint::SketchEvaluator),
                )?)?);
            }
            Request::Undo(params) => {
                let step = self.engine.undo_step();
                self.engine.undo_at(params.expected_revision)?;
                return self.stepped(step);
            }
            Request::Redo(params) => {
                let step = self.engine.redo_step();
                self.engine.redo_at(params.expected_revision)?;
                return self.stepped(step);
            }
            Request::New(params) => {
                let profile = match params.profile {
                    Some(ProfileChoice::Profile(profile)) => *profile,
                    Some(ProfileChoice::Id(id)) => {
                        pcbcraft_io::rule_profiles::for_new_board_from_env(Some(&id))?
                    }
                    None => pcbcraft_io::rule_profiles::for_new_board_from_env(None)?,
                };
                let mut project = Project::default();
                pcbcraft_core::rules::apply(
                    &mut project.rules,
                    pcbcraft_core::rules::RulesCommand::SetProfile {
                        profile: Box::new(profile),
                        keep_overrides: false,
                    },
                )?;
                self.engine.replace_document(EditorFile {
                    version: pcbcraft_core::format::FILE_VERSION,
                    project,
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
                    self.engine.preview_update_hosted(params.change, hosted()),
                )?);
            }
            Request::ApplyUpdate(update) => {
                self.engine.apply_hosted(
                    update.expected_revision,
                    Operation::Update(Box::new(update)),
                    hosted(),
                )?;
            }
            Request::ExportNative(params) => {
                pcbcraft_solidcraft::project::build(self.engine.project())?.save(&params.path)?;
                return Ok(json!({
                    "revision": self.engine.revision(),
                    "exported_path": params.path
                }));
            }
            Request::ExportFabrication(params) => {
                let profile = params
                    .profile
                    .as_deref()
                    .map(pcbcraft_io::fabrication::resolve_profile)
                    .transpose()?;
                let report = pcbcraft_io::fabrication::export(
                    self.engine.project(),
                    self.engine.drafts(),
                    &pcbcraft_io::fabrication::ExportRequest {
                        output: PathBuf::from(&params.output),
                        zip: params.zip,
                        profile,
                        options: params.options,
                    },
                    &mut |_| {},
                )?;
                let mut value = serde_json::to_value(report)?;
                value["revision"] = json!(self.engine.revision());
                return Ok(value);
            }
            Request::Sources(_) => return self.list_sources(),
            Request::SourceAdd(params) => {
                let added = self.sources.add(Path::new(&params.path), params.writable)?;
                return Ok(json!({"added": added, "sources": self.sources.list()}));
            }
            Request::SourceCreate(params) => {
                let created = match params.library {
                    Some(library) => {
                        self.sources
                            .create_as(Path::new(&params.path), &params.name, library)?
                    }
                    None => self.sources.create(Path::new(&params.path), &params.name)?,
                };
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
                let mut reply = self.apply(params.expected_revision, operation)?;
                // The placement is done; remembering it as recent is a
                // convenience whose failure is reported, never an error.
                if let Err(error) = self.sources.used(EntryKey::Library {
                    library: params.library,
                    lineage: params.lineage,
                }) {
                    reply["warnings"] = json!([format!("recents not saved: {error}")]);
                }
                return Ok(reply);
            }
            Request::ImportFromProject(params) => return self.import_from_project(params),
            Request::Publish(params) => return self.publish(params),
            Request::ImportSvg(params) => return self.import_svg(params),
            Request::OpenPreview(params) => return self.open_preview(params),
            Request::PinoutSchema(_) => return self.pinout_schema(),
            Request::ReviewCheck(params) => {
                return Ok(crate::review::check(&self.snapshot(), &params.report));
            }
            Request::FirmwarePreview(params) => return self.firmware(params, false),
            Request::FirmwareExport(params) => return self.firmware(params, true),
            Request::InspectModelFile(params) => return self.inspect_model_file(params),
            Request::SetMarking(params) => return self.set_marking(params),
            Request::SchematicPdf(params) => {
                let pages = pcbcraft_io::schematic_pdf::export_schematic_pdf(
                    self.engine.project(),
                    &params.sheets,
                    Path::new(&params.path),
                )?;
                return Ok(json!({
                    "revision": self.engine.revision(),
                    "path": params.path,
                    "pages": pages
                }));
            }
            Request::MarkingSurfaces(params) => return self.marking_surfaces(params),
            Request::CheckPartFile(params) => return self.check_part_file(params),
            Request::UsePartFile(params) => return self.use_part_file(params),
            Request::SetPartValues(params) => return self.set_part_values(params),
            Request::PartFiles(_) => return self.part_files(),
            Request::PlacePartFile(params) => return self.place_part_file(*params),
            Request::SetPartFileValues(params) => return self.set_part_file_values(params),
        }
        Ok(self.document_reply())
    }

    /// Apply one operation and return the document state with its outcome.
    fn apply(&mut self, expected_revision: u64, operation: Operation) -> Result<Value> {
        let outcome = self
            .engine
            .apply_hosted(expected_revision, operation, hosted())?;
        let mut reply = self.document_reply();
        reply["outcome"] = serde_json::to_value(outcome)?;
        self.add_rerouted(&mut reply)?;
        Ok(reply)
    }

    /// The document state after an undo or redo, with the `step` it took
    /// back or applied again: its title and area, as the desktop says them.
    fn stepped(&self, step: Option<pcbcraft_core::Step>) -> Result<Value> {
        let mut reply = self.document_reply();
        reply["step"] = serde_json::to_value(step)?;
        Ok(reply)
    }

    /// When the operation just applied moved parts whose dragged copper
    /// was re-routed or left unrouted: `rerouted` with the connections'
    /// nets and the one-line `summary` the desktop toasts.
    fn add_rerouted(&self, reply: &mut Value) -> Result<()> {
        let rerouted = self.engine.rerouted();
        if let Some(summary) = rerouted.line() {
            let mut value = serde_json::to_value(rerouted)?;
            value["summary"] = Value::String(summary);
            reply["rerouted"] = value;
        }
        Ok(())
    }

    /// `library.import_from_project`: read the file, take the exact closure
    /// from it and apply one `library.import` with that bundle inline.
    fn import_from_project(&mut self, params: ImportFromProjectParams) -> Result<Value> {
        let path = Path::new(&params.path);
        let file = pcbcraft_io::read_project(path)?;
        let library = &file.project.library;
        if library.definition(params.root).is_none() {
            let held = held_revisions(&file);
            let message = match file.drafts.get(params.root.id) {
                Some(draft) => format!(
                    "{} holds ‘{}’ as an unpublished draft. Only a published revision is immutable: open that project, publish it and save, then import it.",
                    params.path, draft.meta.name
                ),
                None => format!(
                    "{} does not hold {} {} of lineage {}. It holds: {held}",
                    params.path,
                    pcbcraft_core::library::names::noun(params.root.kind),
                    params.root.revision,
                    params.root.id
                ),
            };
            return Err(pcbcraft_core::Error::NotFound(message).into());
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| stem.len() <= pcbcraft_core::ids::MAX_LABEL)
            .unwrap_or("another project");
        let bundle = pcbcraft_core::library::import::project_bundle(library, params.root, name)?;
        let operation = Operation::Library(LibraryCommand::Import {
            bundle: Box::new(bundle),
        });
        self.apply(params.expected_revision, operation)
    }

    fn list_sources(&self) -> Result<Value> {
        Ok(json!({"sources": self.sources.list()}))
    }

    /// `library.publish_to_folder`: the three promote steps. The revision is
    /// checked before the folder is written, and the plan is recomputed by
    /// `library.promote` with the same arguments, so the recorded origins
    /// equal the records written.
    fn publish(&mut self, params: PublishParams) -> Result<Value> {
        let folder = self.sources.writable(params.library)?.clone();
        let members =
            pcbcraft_core::library::bundle::closure(&self.engine.project().library, params.root)?;
        let fresh = match params.fresh {
            Some(fresh) => fresh,
            None => (0..members.len())
                .map(|_| pcbcraft_io::library_folder::new_lineage_id())
                .collect::<pcbcraft_io::Result<Vec<_>>>()?,
        };
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
        let mut reply = self.document_reply();
        reply["outcome"] = serde_json::to_value(outcome)?;
        reply["import"] = json!({"size": artwork.size, "notes": artwork.notes});
        Ok(reply)
    }

    /// The whole document state, returned to in-process callers and to
    /// `verbose` requests. Each draft
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
            // Raw rule output for scripts that read rule codes directly.
            // The findings view people and agents act on is
            // `project.issues` (severity, wording, merging, locating).
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
        library: None,
    }))?;
    let id: LibraryId = serde_json::from_value(created["created"]["library"].clone())?;
    let mut written = Vec::new();
    for root in roots {
        let reply = session.execute(Request::Publish(PublishParams {
            expected_revision: session.engine().revision(),
            root,
            library: id,
            fresh: None,
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

/// What a project file holds that is worth importing, for a refusal: its
/// latest Parts and components by kind, id, revision and name.
fn held_revisions(file: &EditorFile) -> String {
    const SHOWN: usize = 16;
    let library = &file.project.library;
    let parts = library
        .latest_parts()
        .into_iter()
        .map(|p| ("part", p.id, p.revision, p.meta.name.clone()));
    let components = library
        .latest_components()
        .into_iter()
        .map(|c| ("component", c.id, c.revision, c.meta.name.clone()));
    let all: Vec<String> = parts
        .chain(components)
        .map(|(kind, id, revision, name)| format!("{kind} {} {revision} ‘{name}’", id.0))
        .collect();
    if all.is_empty() {
        return "no published Part or component.".into();
    }
    let more = all.len().saturating_sub(SHOWN);
    let shown = all.into_iter().take(SHOWN).collect::<Vec<_>>().join(", ");
    if more > 0 {
        format!("{shown} and {more} more.")
    } else {
        format!("{shown}.")
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

pub mod discovery;
mod firmware;
mod marking;
pub mod mechanical_parts;
pub mod metadata;
mod models;
pub mod part_files;
mod pinout;
mod preview;
mod reply;
pub(crate) use reply::take_verbose;
pub mod sources;

#[cfg(test)]
mod dimension_tests;
#[cfg(test)]
mod fabrication_tests;
#[cfg(test)]
mod firmware_tests;
#[cfg(test)]
mod folder_tests;
#[cfg(test)]
mod high_speed_tests;
#[cfg(test)]
mod issues_tests;
#[cfg(test)]
mod metadata_tests;
#[cfg(test)]
mod models_tests;
#[cfg(test)]
mod package_tests;
#[cfg(test)]
mod parts_tests;
#[cfg(test)]
mod pinout_tests;
#[cfg(test)]
mod project_import_tests;
#[cfg(test)]
mod reply_tests;
#[cfg(test)]
mod svg_tests;
#[cfg(test)]
mod templates_tests;
#[cfg(test)]
mod tests;

/// The shipped showcase ([`pcbcraft_core::Engine::showcase_hosted`]) on
/// SolidCraft's sketch engine: its sketched board, cutout, slot and
/// edge-held header included, on `profile`.
pub fn showcase(
    profile: pcbcraft_core::rules::RuleProfile,
) -> pcbcraft_core::Result<pcbcraft_core::Engine> {
    pcbcraft_core::Engine::showcase_hosted(hosted(), profile)
}

/// The host evaluators every applied operation gets: SolidCraft's sketch
/// engine for footprint sources and for the board shape.
pub fn hosted() -> pcbcraft_core::Evaluators<'static> {
    pcbcraft_core::Evaluators {
        footprint: Some(&pcbcraft_solidcraft::footprint::SketchEvaluator),
        board: Some(&pcbcraft_solidcraft::board::SketchBoardEvaluator),
    }
}
