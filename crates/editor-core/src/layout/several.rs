//! Several board objects at once, as one undo step: `layout.delete_objects`
//! (a multi-selection's Delete, a dangling stub's tracks) and
//! `layout.lock_objects`. Each object is refused as its own command
//! refuses it (a locked track or via is not deleted), and one refusal
//! refuses the whole command.
use super::fill::{self, FillCommand};
use super::{RouteEditSet, apply_routes, via};
use crate::diagnostics::{Error, Result};
use crate::ids::Id;
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Most objects one command names.
pub const MAX_OBJECTS: usize = 10_000;

/// The board objects a command names, by kind.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Objects {
    /// Placed parts (only `layout.lock_objects` takes them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vias: Vec<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fills: Vec<Id>,
}

impl Objects {
    fn check(&self) -> Result<()> {
        let count = self.parts.len() + self.tracks.len() + self.vias.len() + self.fills.len();
        if count == 0 {
            return Err(Error::Invalid("name at least one object".into()));
        }
        if count > MAX_OBJECTS {
            return Err(Error::Limit(format!(
                "one command takes at most {MAX_OBJECTS} objects, not {count}"
            )));
        }
        for ids in [&self.parts, &self.tracks, &self.vias, &self.fills] {
            let mut seen = BTreeSet::new();
            if let Some(id) = ids.iter().find(|id| !seen.insert(**id)) {
                return Err(Error::Invalid(format!("object {id} is named twice")));
            }
        }
        Ok(())
    }
}

/// `layout.delete_objects`: remove the tracks, vias and fills.
pub fn delete(project: &mut Project, objects: &Objects) -> Result<()> {
    objects.check()?;
    if !objects.parts.is_empty() {
        return Err(Error::Invalid(
            "parts come from the schematic; remove them there".into(),
        ));
    }
    if !objects.tracks.is_empty() {
        apply_routes(
            project,
            RouteEditSet {
                remove: objects.tracks.clone(),
                ..Default::default()
            },
        )?;
    }
    for &id in &objects.vias {
        via::delete(project, id)?;
    }
    for &id in &objects.fills {
        fill::apply(project, FillCommand::Delete { id })?;
    }
    Ok(())
}

/// `layout.lock_objects`: lock or unlock every named object.
pub fn lock(project: &mut Project, objects: &Objects, locked: bool) -> Result<()> {
    objects.check()?;
    let missing = |kind: &str, id: Id| Error::NotFound(format!("unknown {kind} {id}"));
    for &id in &objects.parts {
        project
            .placements
            .iter_mut()
            .find(|p| p.instance == id)
            .ok_or_else(|| missing("placed part", id))?
            .locked = locked;
    }
    for &id in &objects.tracks {
        project
            .tracks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| missing("track", id))?
            .locked = locked;
    }
    for &id in &objects.vias {
        via::set_lock(project, id, locked)?;
    }
    for &id in &objects.fills {
        project
            .fills
            .iter_mut()
            .find(|f| f.id == id)
            .ok_or_else(|| missing("fill", id))?
            .locked = locked;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
