//! Unfinished definitions. Drafts live outside the project, so editing one
//! neither revalidates the project nor changes the board model. Only
//! persistence bounds and finite geometry are enforced.
use super::graphics::Grid;
use super::symbol_style::SymbolConventions;
use super::{Definition, DefinitionMeta, Library, check_bounds};
use crate::diagnostics::{Error, Result};
use crate::ids::{DefinitionId, DefinitionKind, Revision};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_DRAFTS: usize = 64;
/// Draft bodies one `library.edit` replaces together.
pub const MAX_EDITS: usize = 16;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Drafts {
    pub items: Vec<Draft>,
    /// The authoring grid: saved and undoable with the drafts, never part of
    /// revision data.
    pub grid: Grid,
    /// How generated symbols are drawn (the resistor style): an authoring
    /// setting saved and undoable with the drafts, never revision data.
    #[serde(default, skip_serializing_if = "SymbolConventions::is_default")]
    pub symbols: SymbolConventions,
}

/// At most one draft per lineage. Its pending revision is `base + 1`, or 1.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub id: DefinitionId,
    pub base: Option<Revision>,
    pub meta: DefinitionMeta,
    pub definition: Definition,
}

impl Draft {
    /// The revision publishing this draft would create.
    pub fn pending(&self) -> Option<Revision> {
        match self.base {
            Some(base) => base.next(),
            None => Some(Revision::FIRST),
        }
    }
}

/// Replace one draft's envelope and body.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DraftEdit {
    pub id: DefinitionId,
    pub meta: DefinitionMeta,
    pub definition: Definition,
}

impl Drafts {
    pub fn get(&self, id: DefinitionId) -> Option<&Draft> {
        self.items.iter().find(|d| d.id == id)
    }
    pub(super) fn set_base(&mut self, id: DefinitionId, revision: Revision) {
        if let Some(draft) = self.items.iter_mut().find(|d| d.id == id) {
            draft.base = Some(revision);
        }
    }
}

/// Add a draft of a reserved new lineage (`base` is `None`) or of a published
/// revision of an existing lineage.
pub fn create(
    drafts: &mut Drafts,
    library: &Library,
    id: DefinitionId,
    base: Option<Revision>,
    meta: DefinitionMeta,
    definition: Definition,
) -> Result<()> {
    check_bounds(&meta, &definition)?;
    if drafts.items.len() >= MAX_DRAFTS {
        return Err(Error::Limit("a project holds at most 64 drafts".into()));
    }
    if drafts.get(id).is_some() {
        return Err(Error::Conflict(format!(
            "{} already has a draft",
            super::names::capitalised(&library.describe_lineage(definition.kind(), id))
        )));
    }
    check_lineage(library, id, base, definition.kind())?;
    drafts.items.push(Draft {
        id,
        base,
        meta,
        definition,
    });
    Ok(())
}

fn check_lineage(
    library: &Library,
    id: DefinitionId,
    base: Option<Revision>,
    kind: DefinitionKind,
) -> Result<()> {
    match (library.latest(id), base) {
        (None, None) => Ok(()),
        (Some((stored, _)), _) if stored != kind => Err(Error::Invalid(format!(
            "{} is a {}, not a {}",
            super::names::capitalised(&library.describe_lineage(stored, id)),
            super::names::noun(stored),
            super::names::noun(kind)
        ))),
        (Some((_, latest)), Some(base)) if base.0 >= 1 && base <= latest => Ok(()),
        (Some(_), None) => Err(Error::Invalid(format!(
            "A draft of the published {} needs its base revision",
            library.describe_lineage(kind, id)
        ))),
        _ => Err(Error::Invalid(format!(
            "The draft of {} names a base revision that does not exist",
            library.describe_lineage(kind, id)
        ))),
    }
}

/// Replace several draft bodies atomically. Kinds never change.
pub fn edit(drafts: &mut Drafts, edits: Vec<DraftEdit>) -> Result<()> {
    if edits.is_empty() || edits.len() > MAX_EDITS {
        return Err(Error::Invalid(
            "edit between 1 and 16 drafts at a time".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    for edit in &edits {
        if !seen.insert(edit.id) {
            return Err(Error::Invalid(format!(
                "The draft ‘{}’ is edited twice",
                edit.meta.name
            )));
        }
        let draft = drafts.get(edit.id).ok_or_else(|| {
            Error::NotFound(format!("The draft ‘{}’ no longer exists", edit.meta.name))
        })?;
        if draft.definition.kind() != edit.definition.kind() {
            return Err(Error::Invalid(format!(
                "The draft ‘{}’ is a {}; its kind cannot change",
                draft.meta.name,
                super::names::noun(draft.definition.kind())
            )));
        }
        check_bounds(&edit.meta, &edit.definition)?;
    }
    for edit in edits {
        if let Some(draft) = drafts.items.iter_mut().find(|d| d.id == edit.id) {
            draft.meta = edit.meta;
            draft.definition = edit.definition;
        }
    }
    Ok(())
}

/// Replace the authoring grid. Geometry never moves.
pub fn set_grid(drafts: &mut Drafts, grid: Grid) -> Result<()> {
    if !grid.valid() {
        return Err(Error::Invalid(
            "grid steps must be finite and 0.01–10 mm".into(),
        ));
    }
    drafts.grid = grid;
    Ok(())
}

/// Remove a draft. A reserved new-lineage identity stays allocated.
pub fn discard(drafts: &mut Drafts, id: DefinitionId) -> Result<()> {
    let before = drafts.items.len();
    drafts.items.retain(|d| d.id != id);
    if drafts.items.len() == before {
        return Err(Error::NotFound("That draft no longer exists".into()));
    }
    Ok(())
}

/// Bounds, unique lineages and kind/base consistency with the snapshot.
pub fn validate(drafts: &Drafts, library: &Library) -> Result<()> {
    if drafts.items.len() > MAX_DRAFTS {
        return Err(Error::Invalid("too many drafts".into()));
    }
    if !drafts.grid.valid() {
        return Err(Error::Invalid(
            "grid steps must be finite and 0.01–10 mm".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    for draft in &drafts.items {
        if draft.id.0 == 0 || !seen.insert(draft.id) {
            return Err(Error::Invalid(
                "draft lineages must be nonzero and have one draft each".into(),
            ));
        }
        check_bounds(&draft.meta, &draft.definition)?;
        check_lineage(library, draft.id, draft.base, draft.definition.kind())?;
    }
    Ok(())
}
