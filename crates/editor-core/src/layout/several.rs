//! Several board objects at once, as one undo step: `layout.delete_objects`
//! (a multi-selection's Delete, a dangling stub's tracks) and
//! `layout.lock_objects`. Each object is refused as its own command
//! refuses it (a locked track or via is not deleted), and one refusal
//! refuses the whole command.
use super::fill::{self, FillCommand};
use super::{RouteEditSet, apply_routes};
use crate::diagnostics::{Error, Result};
use crate::ids::Id;
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

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
        for ids in [&self.parts, &self.tracks, &self.vias, &self.fills] {
            let mut seen = BTreeSet::new();
            if let Some(id) = ids.iter().find(|id| !seen.insert(**id)) {
                return Err(Error::Invalid(format!("object {id} is named twice")));
            }
        }
        Ok(())
    }
}

/// A part `layout.delete_objects` was named and kept: parts come from the
/// schematic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeptPart {
    pub instance: Id,
    pub designator: String,
}

/// Designators a kept-parts line names before it shortens to "…".
const NAMED_KEPT: usize = 5;

/// What a person reads about kept parts: "Kept 2 parts (R1, R2): delete
/// them in the schematic".
pub fn kept_line(kept: &[KeptPart]) -> String {
    let mut names: Vec<&str> = kept
        .iter()
        .take(NAMED_KEPT)
        .map(|k| k.designator.as_str())
        .collect();
    if kept.len() > NAMED_KEPT {
        names.push("…");
    }
    let (count, them) = match kept.len() {
        1 => ("1 part".to_owned(), "it"),
        n => (format!("{n} parts"), "them"),
    };
    format!(
        "Kept {count} ({}): delete {them} in the schematic",
        names.join(", ")
    )
}

/// `layout.delete_objects`: remove the tracks, vias and fills. Named parts
/// stay, as they come from the schematic, and are returned; naming only
/// parts is refused.
pub fn delete(project: &mut Project, objects: &Objects) -> Result<Vec<KeptPart>> {
    objects.check()?;
    if objects.tracks.is_empty() && objects.vias.is_empty() && objects.fills.is_empty() {
        return Err(Error::Invalid(
            "parts come from the schematic; remove them there".into(),
        ));
    }
    let mut kept = Vec::with_capacity(objects.parts.len());
    for &instance in &objects.parts {
        if project.placement_of(instance).is_none() {
            return Err(Error::NotFound(format!("unknown placed part {instance}")));
        }
        let designator = project
            .instance(instance)
            .map(|i| i.designator.clone())
            .unwrap_or_default();
        kept.push(KeptPart {
            instance,
            designator,
        });
    }
    // One routing edit removes them all, by identity: a whole board's
    // copper costs one pass, not one per object.
    if !objects.tracks.is_empty() || !objects.vias.is_empty() {
        apply_routes(
            project,
            RouteEditSet {
                remove: objects.tracks.clone(),
                remove_vias: objects.vias.clone(),
                ..Default::default()
            },
        )?;
    }
    for &id in &objects.fills {
        fill::apply(project, FillCommand::Delete { id })?;
    }
    Ok(kept)
}

/// `layout.lock_objects`: lock or unlock every named object.
pub fn lock(project: &mut Project, objects: &Objects, locked: bool) -> Result<()> {
    objects.check()?;
    let missing = |kind: &str, id: Id| Error::NotFound(format!("unknown {kind} {id}"));
    // By identity through one index per kind, so locking a whole board
    // is linear in it.
    fn set<T>(
        items: &mut [T],
        ids: &[Id],
        key: impl Fn(&T) -> Id,
        mut lock: impl FnMut(&mut T),
        missing: impl Fn(Id) -> Error,
    ) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let at: HashMap<Id, usize> = items.iter().enumerate().map(|(k, t)| (key(t), k)).collect();
        for &id in ids {
            let item = at
                .get(&id)
                .and_then(|k| items.get_mut(*k))
                .ok_or_else(|| missing(id))?;
            lock(item);
        }
        Ok(())
    }
    if !objects.parts.is_empty() {
        set(
            &mut project.placements,
            &objects.parts,
            |p| p.instance,
            |p| p.locked = locked,
            |id| missing("placed part", id),
        )?;
    }
    if !objects.tracks.is_empty() {
        set(
            &mut project.tracks,
            &objects.tracks,
            |t| t.id,
            |t| t.locked = locked,
            |id| missing("track", id),
        )?;
    }
    if !objects.vias.is_empty() {
        set(
            &mut project.vias,
            &objects.vias,
            |v| v.id,
            |v| v.locked = locked,
            |_| Error::NotFound("unknown via".into()),
        )?;
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
