//! Pcbcraft's first native-model probe. All geometry and persistence belong to Solidcraft.

pub mod copper;
mod fixture;
mod measure;
pub mod project;
pub use solidcraft_engine::{geom, render};

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solidcraft_engine::Session;
use std::path::Path;
use std::sync::Arc;

pub use fixture::Parameters;
pub use measure::{BodyMeasurement, Snapshot};

pub const SOLIDCRAFT_REVISION: &str = "4f3ac07aec69489183185071f07d7fbcf4f51e64";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("stale revision: expected {expected}, current {actual}")]
    Stale { expected: u64, actual: u64 },
    #[error("Solidcraft command {command} failed: {message}")]
    Engine { command: String, message: String },
    #[error("model verification failed: {0}")]
    Verification(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    pub fn diagnostic(&self) -> Value {
        let code = match self {
            Self::Invalid(_) | Self::Json(_) => "invalid_request",
            Self::Stale { .. } => "stale_revision",
            Self::Engine { .. } => "engine_error",
            Self::Verification(_) => "verification_failed",
            Self::Io(_) => "io_error",
        };
        let mut value = json!({"code": code, "message": self.to_string()});
        if let Self::Stale { expected, actual } = self {
            value["expected_revision"] = json!(expected);
            value["actual_revision"] = json!(actual);
        }
        value
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// One request contract for automation now and the UI later. Updates require a
/// session revision; complete parameter sets keep this early probe deliberately small.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum Command {
    #[serde(rename = "probe.create")]
    Create(Create),
    #[serde(rename = "probe.update")]
    Update(Update),
    #[serde(rename = "probe.inspect")]
    Inspect(Empty),
    #[serde(rename = "probe.undo")]
    Undo(Empty),
    #[serde(rename = "probe.redo")]
    Redo(Empty),
    #[serde(rename = "probe.save")]
    Save(FilePath),
    #[serde(rename = "probe.open")]
    Open(FilePath),
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Create {
    pub parameters: Parameters,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub expected_revision: u64,
    pub parameters: Parameters,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilePath {
    pub path: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Reply {
    pub revision: u64,
    pub snapshot: Snapshot,
    pub saved_path: Option<String>,
}

/// Generates the same contracts used by dispatch, without a hand-written command list.
pub fn schemas() -> Value {
    json!({"input": schema_for!(Command), "output": schema_for!(Reply),
        "solidcraft_revision": SOLIDCRAFT_REVISION})
}

#[derive(Default)]
pub struct Probe {
    session: Option<Session>,
    revision: u64,
    undo: Vec<Arc<solidcraft_engine::doc::Document>>,
    redo: Vec<Arc<solidcraft_engine::doc::Document>>,
}

impl Probe {
    pub fn dispatch(&mut self, request: Value) -> Result<Reply> {
        self.execute(serde_json::from_value(request)?)
    }

    pub fn execute(&mut self, command: Command) -> Result<Reply> {
        let mut saved_path = None;
        match command {
            Command::Create(request) => {
                let candidate = fixture::build(&request.parameters)?;
                self.commit(candidate, false)?;
            }
            Command::Update(request) => {
                if request.expected_revision != self.revision {
                    return Err(Error::Stale {
                        expected: request.expected_revision,
                        actual: self.revision,
                    });
                }
                request.parameters.validate()?;
                // Stage the complete edit: upstream parameter edits can leave failed
                // features in a document, so check evaluation before replacing ours.
                let original = self.session()?;
                let mut candidate = Session::new((*original.doc).clone());
                call(
                    &mut candidate,
                    "parameters.change",
                    json!({
                        "changes": request.parameters.expressions()
                    }),
                )?;
                measure::verify(&candidate, &request.parameters)?;
                self.commit(candidate, true)?;
            }
            Command::Open(request) => {
                validate_path(&request.path)?;
                let mut candidate = Session::default();
                call(
                    &mut candidate,
                    "doc.open",
                    json!({"path": request.path, "read_only": true}),
                )?;
                let parameters = measure::parameters(&candidate)?;
                measure::verify(&candidate, &parameters)?;
                self.commit(candidate, false)?;
            }
            Command::Save(request) => {
                validate_path(&request.path)?;
                // Saving is an external effect, never a document mutation or undo step.
                // The probe refuses existing destinations rather than overwriting work.
                if Path::new(&request.path).try_exists()? {
                    return Err(Error::Invalid(
                        "output already exists; choose a new path".into(),
                    ));
                }
                let mut copy = Session::new((*self.session()?.doc).clone());
                call(
                    &mut copy,
                    "file.save_as",
                    json!({
                        "path": request.path, "format": "container", "cache": false,
                        "preview": false
                    }),
                )?;
                saved_path = Some(request.path);
            }
            Command::Undo(_) => self.restore(true)?,
            Command::Redo(_) => self.restore(false)?,
            Command::Inspect(_) => {}
        }
        Ok(Reply {
            revision: self.revision,
            snapshot: measure::snapshot(self.session()?)?,
            saved_path,
        })
    }

    fn session(&self) -> Result<&Session> {
        self.session
            .as_ref()
            .ok_or_else(|| Error::Invalid("create or open a probe first".into()))
    }

    fn restore(&mut self, undo: bool) -> Result<()> {
        let history = if undo { &self.undo } else { &self.redo };
        let doc = history
            .last()
            .ok_or_else(|| Error::Invalid("history is empty".into()))?;
        let candidate = Session::new((**doc).clone());
        let parameters = measure::parameters(&candidate)?;
        measure::verify(&candidate, &parameters)?;
        let previous = self.session()?.doc.clone();
        let next = self
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("revision exhausted".into()))?;
        if undo {
            self.undo.pop();
            self.redo.push(previous);
        } else {
            self.redo.pop();
            self.undo.push(previous);
        }
        self.session = Some(candidate);
        self.revision = next;
        Ok(())
    }

    fn commit(&mut self, candidate: Session, undoable: bool) -> Result<()> {
        let next = self
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("revision exhausted".into()))?;
        // Compute the reply before mutation as part of the transaction boundary.
        measure::snapshot(&candidate)?;
        if undoable {
            let previous = self.session()?.doc.clone();
            if self.undo.len() == 100 {
                self.undo.remove(0);
            }
            self.undo.push(previous);
        } else {
            self.undo.clear();
        }
        self.redo.clear();
        self.session = Some(candidate);
        self.revision = next;
        Ok(())
    }
}

fn validate_path(path: &str) -> Result<()> {
    if path.len() > 4095
        || Path::new(path)
            .extension()
            .is_none_or(|ext| ext != "solidcraft")
    {
        return Err(Error::Invalid(
            "use a .solidcraft path shorter than 4096 bytes".into(),
        ));
    }
    Ok(())
}

fn call(session: &mut Session, command: &str, params: Value) -> Result<Value> {
    session
        .execute(command, &params)
        .map_err(|error| Error::Engine {
            command: command.into(),
            message: error.to_string(),
        })
}

fn id(reply: &Value, key: &str) -> Result<u64> {
    reply
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Verification(format!("missing {key} in {reply}")))
}

#[cfg(test)]
mod tests;
