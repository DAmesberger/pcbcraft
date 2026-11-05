//! Mechanical parts placed directly on the board (docs/mechanical-parts.md
//! M4): `layout.place_part` finds or makes the variant of a part spec and
//! places a new off-sheet instance of it; `layout.set_part` moves a placed
//! mechanical instance to another variant; `layout.remove_part` removes a
//! board-only one. Each runs on the engine's candidate, so a refusal
//! changes nothing.
use crate::diagnostics::{Error, Result};
use crate::geometry::{Point, point_valid};
use crate::ids::{ComponentRef, DefinitionId, Endpoint, Id, TerminalId};
use crate::instances;
use crate::library::mechanical::{self, PartSpec, VariantRefs};
use crate::model::{Layer, PhysicalChoice, Placement, Project};
use std::collections::BTreeSet;

fn variant(project: &mut Project, part: &PartSpec) -> Result<VariantRefs> {
    let mut library = std::mem::take(&mut project.library);
    let mut allocate = || project.allocate().map(DefinitionId);
    let made = mechanical::make_variant(&mut library, part, &mut allocate);
    project.library = library;
    Ok(made?.0)
}

fn physical(refs: &VariantRefs) -> PhysicalChoice {
    PhysicalChoice {
        binding: refs.binding.id,
        footprint: refs.footprint.id,
    }
}

/// `layout.place_part`: the new instance.
pub(super) fn place(
    project: &mut Project,
    part: &PartSpec,
    position: Point,
    rotation_deg: f64,
    layer: Layer,
    designator: Option<String>,
) -> Result<Id> {
    if !point_valid(position) || !rotation_deg.is_finite() || rotation_deg.abs() > 3600.0 {
        return Err(Error::Invalid(
            "layout.place_part: a finite position and rotation".into(),
        ));
    }
    let refs = variant(project, part)?;
    let instance = instances::place_off_sheet(
        project,
        refs.component,
        refs.symbol,
        physical(&refs),
        designator,
    )?;
    project.placements.push(Placement {
        instance,
        position,
        rotation_deg,
        layer,
        locked: false,
    });
    Ok(instance)
}

/// Whether any terminal of `instance` shares a net with another terminal.
fn connected(project: &Project, instance: Id) -> bool {
    project.nets.iter().any(|net| {
        net.terminals.len() >= 2
            && net
                .terminals
                .iter()
                .any(|t: &Endpoint| t.instance == instance)
    })
}

fn mechanical_instance(project: &Project, instance: Id) -> Result<()> {
    let found = project
        .instance(instance)
        .ok_or_else(|| Error::NotFound("unknown component instance".into()))?;
    if mechanical::mechanical_of(&project.library, found.component).is_none() {
        return Err(Error::Conflict(format!(
            "part.not_mechanical: {} is not a part placed from a part file",
            found.designator
        )));
    }
    Ok(())
}

fn terminals(project: &Project, component: ComponentRef) -> BTreeSet<TerminalId> {
    project
        .library
        .component(component)
        .map(|c| c.definition.terminals.iter().map(|t| t.id).collect())
        .unwrap_or_default()
}

/// `layout.set_part`: the same instance, now of the spec's variant.
pub(super) fn set(project: &mut Project, instance: Id, part: &PartSpec) -> Result<Id> {
    mechanical_instance(project, instance)?;
    if project
        .placement_of(instance)
        .is_some_and(|placement| placement.locked)
    {
        return Err(super::locked(project, instance, "change"));
    }
    let refs = variant(project, part)?;
    let current = project
        .instance(instance)
        .ok_or_else(|| Error::NotFound("unknown component instance".into()))?;
    if current.component == refs.component {
        return Ok(instance);
    }
    if current.part.is_some() {
        return Err(Error::Conflict(format!(
            "part.manufacturer_part: {} has a manufacturer part of its current variant; clear it first",
            current.designator
        )));
    }
    if terminals(project, current.component) != terminals(project, refs.component)
        && (project.occurrences_of(instance).next().is_some() || connected(project, instance))
    {
        return Err(Error::Conflict(format!(
            "part.terminals: the new variant of {} has other terminals, and its symbol is on a sheet or a terminal is connected",
            current.designator
        )));
    }
    let units: BTreeSet<_> = project
        .library
        .symbol(refs.symbol)
        .map(|s| s.definition.units.iter().map(|u| u.id).collect())
        .unwrap_or_default();
    let on_sheet = project.occurrences_of(instance).next().is_some();
    let target = instances::instance_mut(project, instance)?;
    target.component = refs.component;
    target.representation = refs.symbol.id;
    target.physical = Some(physical(&refs));
    if !on_sheet {
        target.unused_units = units;
    }
    let keep = terminals(project, refs.component);
    let target = instances::instance_mut(project, instance)?;
    target.no_connect.retain(|t| keep.contains(t));
    target.functions.retain(|t, _| keep.contains(t));
    instances::check_instance(project, instance)?;
    Ok(instance)
}

/// `layout.remove_part`: remove a board-only mechanical instance and its
/// placement.
pub(super) fn remove(project: &mut Project, instance: Id) -> Result<()> {
    mechanical_instance(project, instance)?;
    if project.occurrences_of(instance).next().is_some() || connected(project, instance) {
        return Err(Error::Conflict(
            "part.connected: the part is on a sheet or connected; preview its removal with project.preview_update (Change::Instance)".into(),
        ));
    }
    if project
        .placement_of(instance)
        .is_some_and(|placement| placement.locked)
    {
        return Err(super::locked(project, instance, "remove"));
    }
    project.placements.retain(|p| p.instance != instance);
    instances::remove(project, instance)
}
