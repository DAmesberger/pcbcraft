pub mod layout;
pub mod model;
pub mod parts;
pub mod persistence;
pub mod recovery;
pub mod schematic;
#[cfg(test)]
mod tests;
pub mod updates;
pub use model::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("stale revision: expected {expected}, current {actual}")]
    Stale { expected: u64, actual: u64 },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "editor", content = "operation", deny_unknown_fields)]
pub enum Operation {
    Part(parts::PartCommand),
    Schematic(schematic::SchematicCommand),
    Layout(layout::LayoutCommand),
    Update(Box<updates::ApplyUpdate>),
}

#[derive(Default)]
pub struct Engine {
    project: Project,
    authoring_draft: Option<Part>,
    revision: u64,
    undo: Vec<Project>,
    redo: Vec<Project>,
}
impl Engine {
    pub fn demo() -> Result<Self> {
        let mut engine = Self::default();
        engine.apply(
            0,
            Operation::Part(parts::PartCommand::Publish {
                draft: parts::passive_template("Resistor"),
            }),
        )?;
        let part = PartRef { id: 1, revision: 1 };
        for (reference, position) in [("R1", [-12.0, 0.0]), ("R2", [12.0, 0.0])] {
            engine.apply(
                engine.revision,
                Operation::Schematic(schematic::SchematicCommand::Place {
                    part,
                    reference: reference.into(),
                    value: "10k".into(),
                    position,
                }),
            )?;
        }
        engine.apply(
            engine.revision,
            Operation::Schematic(schematic::SchematicCommand::Connect {
                from: Endpoint {
                    instance: 2,
                    terminal: 2,
                },
                to: Endpoint {
                    instance: 3,
                    terminal: 1,
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
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        persistence::write(path, &self.document(self.authoring_draft.clone()))
    }
    pub fn open(&mut self, path: &std::path::Path) -> Result<()> {
        self.replace_document(persistence::read(path)?)
    }
    /// The UI supplies its current draft; None explicitly means no draft.
    pub fn document(&self, authoring_draft: Option<Part>) -> persistence::EditorFile {
        persistence::EditorFile {
            version: persistence::FILE_VERSION,
            project: self.project.clone(),
            authoring_draft,
        }
    }
    pub fn authoring_draft(&self) -> Option<&Part> {
        self.authoring_draft.as_ref()
    }
    pub fn replace_document(&mut self, file: persistence::EditorFile) -> Result<()> {
        file.validate()?;
        self.commit(file.project)?;
        self.authoring_draft = file.authoring_draft;
        self.undo.clear();
        self.redo.clear();
        Ok(())
    }
    pub fn project(&self) -> &Project {
        &self.project
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
    pub fn apply(&mut self, expected_revision: u64, operation: Operation) -> Result<()> {
        if expected_revision != self.revision {
            return Err(Error::Stale {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        let mut candidate = self.project.clone();
        match operation {
            Operation::Part(c) => parts::apply(&mut candidate, c)?,
            Operation::Schematic(c) => schematic::apply(&mut candidate, c)?,
            Operation::Layout(c) => layout::apply(&mut candidate, c)?,
            Operation::Update(update) => {
                candidate = updates::candidate(&self.project, self.revision, *update)?;
            }
        }
        validate(&candidate)?;
        self.commit(candidate)?;
        Ok(())
    }
    pub fn preview_update(&self, change: updates::Change) -> updates::UpdatePlan {
        updates::preview(&self.project, self.revision, change).0
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
    }
    fn commit(&mut self, candidate: Project) -> Result<()> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("revision exhausted".into()))?;
        if self.undo.len() == 100 {
            self.undo.remove(0);
        }
        self.undo
            .push(std::mem::replace(&mut self.project, candidate));
        self.redo.clear();
        self.revision = revision;
        Ok(())
    }
    pub fn undo(&mut self) -> Result<()> {
        let candidate = self
            .undo
            .last()
            .ok_or_else(|| Error::Invalid("nothing to undo".into()))?
            .clone();
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("revision exhausted".into()))?;
        self.redo
            .push(std::mem::replace(&mut self.project, candidate));
        self.undo.pop();
        self.revision = revision;
        Ok(())
    }
    pub fn redo(&mut self) -> Result<()> {
        let candidate = self
            .redo
            .last()
            .ok_or_else(|| Error::Invalid("nothing to redo".into()))?
            .clone();
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("revision exhausted".into()))?;
        self.undo
            .push(std::mem::replace(&mut self.project, candidate));
        self.redo.pop();
        self.revision = revision;
        Ok(())
    }
}
pub fn validate(p: &Project) -> Result<()> {
    if p.version != 1 {
        return Err(Error::Invalid("unsupported project version".into()));
    }
    if p.name.trim().is_empty() || p.name.len() > 256 {
        return Err(Error::Invalid("project name is empty or too long".into()));
    }
    let mut ids = std::collections::BTreeSet::new();
    for id in p
        .parts
        .iter()
        .map(|p| p.id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .chain(p.instances.iter().map(|i| i.id))
        .chain(p.wires.iter().map(|w| w.id))
        .chain(p.tracks.iter().map(|t| t.id))
    {
        if id == 0 || id >= p.next_id || !ids.insert(id) {
            return Err(Error::Invalid("invalid global identity allocation".into()));
        }
    }
    parts::validate(p)?;
    schematic::validate(p)?;
    layout::validate(p)?;
    Ok(())
}
pub fn point_valid(p: Point) -> bool {
    p.iter().all(|v| v.is_finite() && v.abs() <= 10000.0)
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum Request {
    #[serde(rename = "project.apply")]
    Apply {
        expected_revision: u64,
        operation: Box<Operation>,
    },
    #[serde(rename = "project.inspect")]
    Inspect {},
    #[serde(rename = "project.undo")]
    Undo {},
    #[serde(rename = "project.redo")]
    Redo {},
    #[serde(rename = "project.save")]
    Save { path: String },
    #[serde(rename = "project.open")]
    Open { path: String },
    #[serde(rename = "project.preview_update")]
    PreviewUpdate { change: updates::Change },
    #[serde(rename = "project.apply_update")]
    ApplyUpdate(updates::ApplyUpdate),
}
impl Engine {
    pub fn dispatch(&mut self, value: serde_json::Value) -> Result<serde_json::Value> {
        match serde_json::from_value::<Request>(value)? {
            Request::Apply {
                expected_revision,
                operation,
            } => self.apply(expected_revision, *operation)?,
            Request::Inspect {} => {}
            Request::Undo {} => self.undo()?,
            Request::Redo {} => self.redo()?,
            Request::Save { path } => self.save(std::path::Path::new(&path))?,
            Request::Open { path } => self.open(std::path::Path::new(&path))?,
            Request::PreviewUpdate { change } => {
                return Ok(serde_json::to_value(self.preview_update(change))?);
            }
            Request::ApplyUpdate(update) => self.apply_update(
                update.expected_revision,
                update.change,
                update.remove_conflicting_tracks,
            )?,
        }
        Ok(
            serde_json::json!({"revision":self.revision,"project":self.project,"erc":schematic::checks(&self.project),"board_checks":layout::checks(&self.project)}),
        )
    }
}
pub fn schemas() -> serde_json::Value {
    serde_json::json!({"input":schemars::schema_for!(Request),"project":schemars::schema_for!(Project),"update_plan":schemars::schema_for!(updates::UpdatePlan),"editor_file":schemars::schema_for!(persistence::EditorFile)})
}
