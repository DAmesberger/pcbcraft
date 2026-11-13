//! Whole-document invariants. This module aggregates the domain validators;
//! it restates none of their rules.
use crate::diagnostics::{Error, Result};
use crate::ids::Id;
use crate::instances::{self, MAX_INSTANCES};
use crate::library::{self, Drafts};
use crate::model::Project;
use crate::{layout, nets, schematic};
use std::collections::BTreeSet;

/// Every project invariant, as checked on decode.
pub fn project(p: &Project) -> Result<()> {
    library::validate(&p.library)?;
    project_except_library(p)
}

/// [`project`] after an edit of an already valid `before`. The library
/// snapshot changes only on `library.publish`, so its full revalidation,
/// which reruns every revision's publish findings, is skipped while it is
/// unchanged.
pub fn edited(before: &Project, after: &Project) -> Result<()> {
    if before.library != after.library {
        library::validate(&after.library)?;
    }
    project_except_library(after)
}

fn project_except_library(p: &Project) -> Result<()> {
    if p.material_table != crate::materials::BUILTIN_REVISION {
        return Err(Error::Unsupported(format!(
            "material table revision {} is not this build's ({}); package looks would change",
            p.material_table,
            crate::materials::BUILTIN_REVISION
        )));
    }
    if p.name.trim().is_empty() || p.name.len() > 256 {
        return Err(Error::Invalid("project name is empty or too long".into()));
    }
    let lineages: BTreeSet<Id> = p.library.revisions().iter().map(|r| r.id.0).collect();
    let mut ids = BTreeSet::new();
    for id in lineages
        .into_iter()
        .chain(p.instances.iter().map(|i| i.id))
        .chain(p.occurrences.iter().map(|o| o.id))
        .chain(p.wires.iter().map(|w| w.id))
        .chain(p.junctions.iter().map(|j| j.id))
        .chain(p.labels.iter().map(|l| l.id))
        .chain(p.sheet_symbols.iter().map(|s| s.id))
        .chain(
            p.sheet_symbols
                .iter()
                .flat_map(|s| s.pins.iter().map(|pin| pin.id)),
        )
        .chain(p.drawings.iter().map(|d| d.id))
        .chain(p.tracks.iter().map(|t| t.id))
        .chain(p.vias.iter().map(|v| v.id))
        .chain(p.fills.iter().map(|f| f.id))
        .chain(p.keepouts.iter().map(|k| k.id))
        .chain(p.part_dimensions.iter().map(|d| d.id))
    {
        if id == 0 || id >= p.next_id || !ids.insert(id) {
            return Err(Error::Invalid("invalid global identity allocation".into()));
        }
    }
    if p.instances.len() > MAX_INSTANCES {
        return Err(Error::Invalid("too many component instances".into()));
    }
    p.rules.validate()?;
    if !p.production.within_bounds() {
        return Err(Error::Invalid(
            "fabrication.waiver_bounds: waivers need unique keys and bounded, printable text"
                .into(),
        ));
    }
    crate::interface::check_table(&p.interface)?;
    if let Some(link) = p.assembly_link.as_deref() {
        crate::interface::assembly_link::check_link(link)?;
    }
    instances::check_all(p)?;
    instances::check_designators(p)?;
    schematic::validate(p)?;
    nets::validate(p)?;
    layout::validate(p)?;
    Ok(())
}

/// The project plus its drafts: bounds, kinds, bases and allocated lineages.
pub fn document(p: &Project, drafts: &Drafts) -> Result<()> {
    project(p)?;
    drafts_only(p, drafts)
}

/// [`document`] after an edit of an already valid `before`; see [`edited`].
pub fn edited_document(before: &Project, after: &Project, drafts: &Drafts) -> Result<()> {
    edited(before, after)?;
    drafts_only(after, drafts)
}

/// Draft invariants against an already valid project.
pub fn drafts_only(p: &Project, drafts: &Drafts) -> Result<()> {
    library::drafts::validate(drafts, &p.library)?;
    let objects: BTreeSet<Id> = p
        .instances
        .iter()
        .map(|i| i.id)
        .chain(p.occurrences.iter().map(|o| o.id))
        .chain(p.wires.iter().map(|w| w.id))
        .chain(p.junctions.iter().map(|j| j.id))
        .chain(p.labels.iter().map(|l| l.id))
        .chain(p.sheet_symbols.iter().map(|s| s.id))
        .chain(
            p.sheet_symbols
                .iter()
                .flat_map(|s| s.pins.iter().map(|pin| pin.id)),
        )
        .chain(p.drawings.iter().map(|d| d.id))
        .chain(p.tracks.iter().map(|t| t.id))
        .chain(p.vias.iter().map(|v| v.id))
        .collect();
    for draft in &drafts.items {
        if draft.id.0 >= p.next_id || objects.contains(&draft.id.0) {
            return Err(Error::Invalid(format!(
                "draft {} does not name an allocated definition lineage",
                draft.id
            )));
        }
    }
    Ok(())
}
