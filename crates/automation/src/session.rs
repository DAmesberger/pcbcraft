//! The automation request contract. Each request is a document query, a
//! document mutation through [`Engine::apply`], or an external file effect.
//! File effects never enter undo history.
use crate::{Error, Result};
use pcbcraft_core::{Engine, Operation, Project, format::EditorFile, layout, schematic, updates};
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
    /// Return the document, draft and design checks.
    #[serde(rename = "project.inspect")]
    Inspect(NoParams),
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
                "Apply one typed document operation (part, schematic, layout or update) at an expected revision. Fails without changes on stale revisions or invalid input.",
            ),
            info::<NoParams>(
                "project.inspect",
                "Return the revision, project, component draft, ERC and board-check findings.",
            ),
            info::<NoParams>("project.undo", "Undo the last document change."),
            info::<NoParams>("project.redo", "Redo the last undone document change."),
            info::<NoParams>("project.new", "Start a new empty project; clears history."),
            info::<PathParams>(
                "project.open",
                "Open a .pcbcraft project file; older versions migrate. Failure leaves the session unchanged.",
            ),
            info::<SaveParams>(
                "project.save",
                "Atomically save to a .pcbcraft file (defaults to the current path). Not undoable.",
            ),
            info::<PreviewParams>(
                "project.preview_update",
                "Preview a revision-bound schematic, part-revision or PCB-sync change and its routed-copper impact.",
            ),
            info::<updates::ApplyUpdate>(
                "project.apply_update",
                "Apply a previewed update at its revision, optionally removing the listed conflicting tracks.",
            ),
            info::<PathParams>(
                "project.export_native",
                "Export the board as a native parametric .solidcraft model to a new file.",
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
                self.engine
                    .apply(params.expected_revision, params.operation)?;
            }
            Request::Inspect(_) => {}
            Request::Undo(_) => self.engine.undo()?,
            Request::Redo(_) => self.engine.redo()?,
            Request::New(_) => {
                self.engine.replace_document(EditorFile {
                    version: pcbcraft_core::format::FILE_VERSION,
                    project: Project::default(),
                    authoring_draft: None,
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
        }
        Ok(self.snapshot())
    }

    /// The document state every mutation and inspection returns.
    pub fn snapshot(&self) -> Value {
        let project = self.engine.project();
        json!({
            "revision": self.engine.revision(),
            "project": project,
            "draft": self.engine.draft(),
            "can_undo": self.engine.can_undo(),
            "can_redo": self.engine.can_redo(),
            "erc": schematic::checks(project),
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
mod tests;
