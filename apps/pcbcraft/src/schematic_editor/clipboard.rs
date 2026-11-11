//! Cut, copy and paste on the sheet. The buffer lives here as what the
//! `schematic.copy` query answered; pasting sends one `schematic.paste`.
//! What is copied is the selection: one object or the whole group.
use super::{Item, SchematicEditor, sheet_ui};
use pcbcraft_core::schematic::{ClipObject, SchematicCommand, SheetClip};
use pcbcraft_core::updates::{Change, InstanceChange};
use pcbcraft_core::{Engine, Operation, Point, Project, Query, QueryResult, Result};

/// What was copied, and where its anchor went last.
pub(super) struct Clipboard {
    clip: SheetClip,
    last: Point,
}

/// Grid steps a paste with no new point moves on from the last one.
const NUDGE_STEPS: f64 = 2.0;

impl SchematicEditor {
    /// The selected object, as the copy query names it; `Err` says why
    /// there is none to copy.
    pub(super) fn clip_object(&self, project: &Project) -> std::result::Result<ClipObject, String> {
        if let Some(id) = self.selected.filter(|id| project.instance(*id).is_some()) {
            return if project.occurrences_of(id).any(|o| o.sheet == self.sheet) {
                Ok(ClipObject::Instance { id })
            } else {
                Err("Only a symbol on this sheet can be copied".into())
            };
        }
        if let Some(id) = self.selected_wire.filter(|id| project.wire(*id).is_some()) {
            return Ok(ClipObject::Wire { id });
        }
        match self.selected_item(project) {
            Some(Item::Label(id)) => Ok(ClipObject::Label { id }),
            Some(Item::Drawing(id)) => Ok(ClipObject::Drawing { id }),
            Some(Item::SheetSymbol(_) | Item::SheetPin(_)) => {
                Err("Sheet symbols and pins cannot be copied yet".into())
            }
            None => Err("Select a symbol, label, drawing or wire first".into()),
        }
    }

    /// The selection, as the copy query names it.
    fn clip_objects(&self, project: &Project) -> std::result::Result<Vec<ClipObject>, String> {
        if self.group.is_empty() {
            self.clip_object(project).map(|object| vec![object])
        } else {
            Ok(self.group.objects())
        }
    }

    pub fn can_copy(&self, project: &Project) -> std::result::Result<(), String> {
        self.clip_objects(project).map(|_| ())
    }

    pub fn can_cut(&self, project: &Project) -> std::result::Result<(), String> {
        if self.update.is_open() {
            return Err("Finish the review first".into());
        }
        self.can_copy(project)
    }

    pub fn can_paste(&self) -> std::result::Result<(), String> {
        if self.update.is_open() {
            return Err("Finish the review first".into());
        }
        match &self.clipboard {
            Some(_) => Ok(()),
            None => Err("Copy something on the sheet first".into()),
        }
    }

    /// Put the selection in the buffer. Its name also goes to the system
    /// clipboard, which is how a field or another app sees a copy.
    pub fn copy_selection(&mut self, ctx: &egui::Context, engine: &Engine) -> Result<()> {
        let project = engine.project();
        let objects = self
            .clip_objects(project)
            .map_err(pcbcraft_core::Error::Invalid)?;
        let QueryResult::SheetClip(clip) = engine.query(Query::SheetCopy {
            sheet: Some(self.sheet),
            objects: objects.clone(),
        })?
        else {
            return Err(pcbcraft_core::Error::Invalid(
                "The copy query answered something else.".into(),
            ));
        };
        let last = clip.anchor().unwrap_or_default();
        ctx.copy_text(match objects[..] {
            [object] => shown(project, object),
            _ => self.group.summary(project),
        });
        self.clipboard = Some(Clipboard { clip: *clip, last });
        Ok(())
    }

    /// Copy the selection, then remove it as Delete or its menu row would:
    /// a symbol, wire or group through the review, so `None` until accepted.
    pub fn cut_selection(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
    ) -> Result<Option<Operation>> {
        self.copy_selection(ctx, engine)?;
        if !self.group.is_empty() {
            return Ok(self.remove_group(engine));
        }
        let project = engine.project();
        let object = self
            .clip_object(project)
            .map_err(pcbcraft_core::Error::Invalid)?;
        Ok(match object {
            ClipObject::Instance { id: instance } => self.stage_update(
                engine,
                Change::Instance {
                    edit: InstanceChange::Remove { instance },
                },
            ),
            ClipObject::Wire { id: wire } => self.stage_update(
                engine,
                Change::Schematic {
                    command: SchematicCommand::Disconnect { wire },
                },
            ),
            ClipObject::Label { id } => {
                self.item = None;
                Some(sheet_ui::removed(Item::Label(id)))
            }
            ClipObject::Drawing { id } => {
                self.item = None;
                Some(sheet_ui::removed(Item::Drawing(id)))
            }
        })
    }

    /// The one operation that pastes the buffer: its anchor at `at`, or
    /// moved on from the last paste when `at` is off the canvas or that
    /// same spot, so a copy never lands exactly on what it was copied from.
    pub fn paste_at(&mut self, engine: &Engine, at: Option<Point>) -> Option<Operation> {
        let sheet = self.sheet;
        let step = engine.drafts().grid.symbol_mm() * NUDGE_STEPS;
        let buffer = self.clipboard.as_mut()?;
        let anchor = buffer.clip.anchor()?;
        let from = buffer.last;
        let target = at
            .filter(|p| p != &from)
            .unwrap_or([from[0] + step, from[1] - step]);
        buffer.last = target;
        let operation = Operation::Schematic(SchematicCommand::Paste {
            sheet: Some(sheet),
            clip: Box::new(buffer.clip.clone()),
            offset: super::sheet_nm([target[0] - anchor[0], target[1] - anchor[1]])?,
        });
        // A tool in progress ends: the result is selected, as after a click.
        let _ = self.on_tool("select", engine);
        Some(operation)
    }

    /// The pointer on the canvas, snapped to the grid.
    pub fn paste_at_pointer(&mut self, engine: &Engine) -> Option<Operation> {
        self.paste_at(engine, self.pointer)
    }
}

/// The copied object's name, as text.
fn shown(project: &Project, object: ClipObject) -> String {
    match object {
        ClipObject::Instance { id } => project.instance(id).map(|i| i.designator.clone()),
        ClipObject::Label { id } => project.label(id).map(|l| l.name.clone()),
        ClipObject::Drawing { .. } => Some("Drawing".to_owned()),
        ClipObject::Wire { .. } => Some("Wire".to_owned()),
    }
    .unwrap_or_default()
}

#[cfg(test)]
#[path = "clipboard_tests.rs"]
mod tests;
