//! A small revisioned harness around the parametric probe model, used to
//! verify regeneration, atomic rejection and native save/reopen.
use super::{Parameters, Snapshot, call, measure, probe_model};
use crate::{Error, Result, native::validate_path};
use serde::Deserialize;
use serde_json::{Value, json};
use solidcraft_engine::Session;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
    pub parameters: Parameters,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub expected_revision: u64,
    pub parameters: Parameters,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePath {
    pub path: String,
}

#[derive(Debug, serde::Serialize)]
pub struct Reply {
    pub revision: u64,
    pub snapshot: Snapshot,
    pub saved_path: Option<String>,
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
                let candidate = probe_model::build(&request.parameters)?;
                self.commit(candidate, false)?;
            }
            Command::Update(request) => {
                if request.expected_revision != self.revision {
                    return Err(Error::Invalid(format!(
                        "stale revision: expected {}, current {}",
                        request.expected_revision, self.revision
                    )));
                }
                request.parameters.validate()?;
                let original = self.session()?;
                let mut candidate = Session::new((*original.doc).clone());
                call(
                    &mut candidate,
                    "parameters.change",
                    json!({"changes": request.parameters.expressions()}),
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
                if Path::new(&request.path).try_exists()? {
                    return Err(Error::Invalid(
                        "output already exists; choose a new path".into(),
                    ));
                }
                let mut copy = Session::new((*self.session()?.doc).clone());
                call(
                    &mut copy,
                    "file.save_as",
                    json!({"path": request.path, "format": "container", "cache": false,
                        "preview": false}),
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

#[cfg(test)]
mod tests;
