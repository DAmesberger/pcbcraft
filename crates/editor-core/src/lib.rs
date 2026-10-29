//! Pcbcraft's electronics document and its single typed command engine.
//!
//! This crate is pure and wasm-compatible: no files, threads, windows or
//! transports. `pcbcraft-io` reads and writes documents; transports (UI, CLI,
//! MCP) translate requests into [`Operation`]s applied by [`Engine`].
pub mod diagnostics;
pub mod format;
pub mod layout;
pub mod model;
pub mod nets;
pub mod parts;
pub mod schematic;
#[cfg(test)]
mod tests;
pub mod updates;
pub use diagnostics::{Diagnostic, Error, Finding, ObjectRef, Result};
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
    Part(parts::PartCommand),
    Schematic(schematic::SchematicCommand),
    Layout(layout::LayoutCommand),
    Update(Box<updates::ApplyUpdate>),
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

    /// Two connected resistors, placed on the board, with a resistor draft.
    pub fn demo() -> Result<Self> {
        let mut engine = Self::default();
        engine.apply(
            0,
            Operation::Part(parts::PartCommand::Publish {
                draft: Some(parts::passive_template("Resistor")),
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
    pub fn draft(&self) -> Option<&Part> {
        self.document.draft.as_deref()
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

    pub fn apply(&mut self, expected_revision: u64, operation: Operation) -> Result<()> {
        if expected_revision != self.revision {
            return Err(Error::Stale {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        let mut candidate = self.document.clone();
        match operation {
            Operation::Part(command) => parts::apply(&mut candidate, command)?,
            Operation::Schematic(command) => schematic::apply_staged(
                &self.document.project,
                Arc::make_mut(&mut candidate.project),
                command,
            )?,
            Operation::Layout(command) => {
                layout::apply_staged(Arc::make_mut(&mut candidate.project), command)?
            }
            Operation::Update(update) => {
                candidate.project = Arc::new(updates::candidate(
                    &self.document.project,
                    self.revision,
                    *update,
                )?);
            }
        }
        let project_changed = !Arc::ptr_eq(&candidate.project, &self.document.project);
        if project_changed {
            validate(&candidate.project)?;
        } else if candidate.draft == self.document.draft {
            return Ok(());
        }
        self.commit(candidate)
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

pub fn validate(p: &Project) -> Result<()> {
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
    nets::validate(p)?;
    layout::validate(p)?;
    Ok(())
}
pub fn point_valid(p: Point) -> bool {
    p.iter().all(|v| v.is_finite() && v.abs() <= 10000.0)
}

/// JSON schemas of the document types every transport shares.
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "operation": schemars::schema_for!(Operation),
        "project": schemars::schema_for!(Project),
        "update_plan": schemars::schema_for!(updates::UpdatePlan),
        "editor_file": schemars::schema_for!(format::EditorFile),
        "finding": schemars::schema_for!(Finding),
        "diagnostic": schemars::schema_for!(Diagnostic),
    })
}
