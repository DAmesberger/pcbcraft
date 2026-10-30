//! Instance configuration rules and choice resolution. An instance pins one
//! exact component revision; [`resolve`] turns its lineage choices into the
//! exact symbol, binding, footprint and package revisions it uses, so there is
//! one source of truth for each. [`check_instance`] is the one owner of the
//! per-instance invariants that commands, update plans and document
//! validation all apply.
use crate::diagnostics::{Error, Result};
use crate::geometry::{Point, point_valid};
use crate::ids::{ComponentRef, DefinitionId, Endpoint, Id, TerminalId, UnitId};
use crate::library::{
    BindingDefinition, ComponentDefinition, FootprintDefinition, Library, PackageDefinition,
    PinKind, Published, SymbolDefinition,
};
use crate::model::{ComponentInstance, PhysicalChoice, Project, SymbolOccurrence};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_INSTANCES: usize = 4096;
/// Symbol occurrences in one project; [`add_occurrence`] enforces it.
pub const MAX_OCCURRENCES: usize = 16384;
pub const MAX_VALUE: usize = 256;
/// Terminals one `instance.set_no_connect` changes.
pub const MAX_NO_CONNECT_EDIT: usize = 256;

/// The exact revisions an instance uses.
#[derive(Clone, Copy)]
pub struct Resolved<'a> {
    pub component: &'a Published<ComponentDefinition>,
    pub symbol: &'a Published<SymbolDefinition>,
    pub physical: Option<PhysicalView<'a>>,
}

/// The exact physical implementation of a physical choice.
#[derive(Clone, Copy)]
pub struct PhysicalView<'a> {
    pub binding: &'a Published<BindingDefinition>,
    pub footprint: &'a Published<FootprintDefinition>,
    pub package: &'a Published<PackageDefinition>,
}

/// A unit's derived state. Placed and unused are exclusive.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UnitState {
    Placed { occurrence: Id },
    Unplaced,
    Unused,
}

fn invalid(message: String) -> Error {
    Error::Invalid(message)
}

/// Resolve every choice through the pinned component revision. Fails when a
/// choice is not offered by that revision or a revision is missing.
pub fn resolve<'a>(library: &'a Library, instance: &ComponentInstance) -> Result<Resolved<'a>> {
    let component = library.component(instance.component).ok_or_else(|| {
        invalid(format!(
            "{} pins component {}, which is not in the project library.",
            instance.designator, instance.component
        ))
    })?;
    let symbol = representation(library, component, instance.representation)?;
    let physical = instance
        .physical
        .map(|choice| resolve_physical(library, &component.definition, choice))
        .transpose()?;
    Ok(Resolved {
        component,
        symbol,
        physical,
    })
}

/// The exact symbol revision a component revision lists for a lineage.
pub fn representation<'a>(
    library: &'a Library,
    component: &Published<ComponentDefinition>,
    lineage: DefinitionId,
) -> Result<&'a Published<SymbolDefinition>> {
    let r = component
        .definition
        .representation(lineage)
        .ok_or_else(|| {
            Error::Conflict(format!(
                "Component {} {} does not offer representation {lineage}.",
                component.id, component.revision
            ))
        })?;
    library
        .symbol(r)
        .ok_or_else(|| invalid(format!("Symbol {r} is not in the project library.")))
}

/// The exact binding, footprint and package of a physical choice offered by
/// `component`.
pub fn resolve_physical<'a>(
    library: &'a Library,
    component: &ComponentDefinition,
    choice: PhysicalChoice,
) -> Result<PhysicalView<'a>> {
    let binding_ref = component.implementation(choice.binding).ok_or_else(|| {
        Error::Conflict(format!(
            "The component revision does not offer implementation {}.",
            choice.binding
        ))
    })?;
    let binding = library.binding(binding_ref).ok_or_else(|| {
        invalid(format!(
            "Binding {binding_ref} is not in the project library."
        ))
    })?;
    let footprint_ref = binding
        .definition
        .footprint(choice.footprint)
        .ok_or_else(|| {
            Error::Conflict(format!(
                "Implementation {binding_ref} does not offer footprint {}.",
                choice.footprint
            ))
        })?;
    let footprint = library.footprint(footprint_ref).ok_or_else(|| {
        invalid(format!(
            "Footprint {footprint_ref} is not in the project library."
        ))
    })?;
    let package = library.package(binding.definition.package).ok_or_else(|| {
        invalid(format!(
            "Package {} is not in the project library.",
            binding.definition.package
        ))
    })?;
    Ok(PhysicalView {
        binding,
        footprint,
        package,
    })
}

/// Whether a terminal of `component` may carry wires or a project no-connect:
/// it exists and is not device-declared NC.
pub fn wirable(component: &ComponentDefinition, terminal: TerminalId) -> bool {
    component
        .terminal(terminal)
        .is_some_and(|t| t.kind != PinKind::NotConnected)
}

/// The unit that draws `terminal`.
pub fn owning_unit(symbol: &SymbolDefinition, terminal: TerminalId) -> Option<UnitId> {
    symbol.anchor(terminal).map(|(unit, _)| unit.id)
}

/// Every unit of the resolved representation with its derived state.
pub fn unit_states(
    project: &Project,
    instance: &ComponentInstance,
) -> Result<Vec<(UnitId, UnitState)>> {
    let resolved = resolve(&project.library, instance)?;
    Ok(resolved
        .symbol
        .definition
        .units
        .iter()
        .map(|unit| {
            let state = match project
                .occurrences_of(instance.id)
                .find(|o| o.unit == unit.id)
            {
                Some(o) => UnitState::Placed { occurrence: o.id },
                None if instance.unused_units.contains(&unit.id) => UnitState::Unused,
                None => UnitState::Unplaced,
            };
            (unit.id, state)
        })
        .collect())
}

/// Designator plus unit label (`Q7` + `A` → `Q7A`) when the representation
/// has more than one unit. The label is presentation only.
pub fn display_label(project: &Project, instance: &ComponentInstance, unit: UnitId) -> String {
    match resolve(&project.library, instance) {
        Ok(resolved) if resolved.symbol.definition.units.len() > 1 => {
            let label = resolved
                .symbol
                .definition
                .unit(unit)
                .map(|u| u.label.as_str())
                .unwrap_or("?");
            format!("{}{label}", instance.designator)
        }
        _ => instance.designator.clone(),
    }
}

/// 1–64 bytes without whitespace or control characters.
pub fn designator_valid(designator: &str) -> bool {
    !designator.is_empty()
        && designator.len() <= 64
        && !designator
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
}

fn check_with<'a>(
    project: &Project,
    instance: &ComponentInstance,
    occurrences: impl Iterator<Item = &'a SymbolOccurrence>,
    endpoints: impl Iterator<Item = Endpoint>,
) -> Result<()> {
    let resolved = resolve(&project.library, instance)?;
    let component = &resolved.component.definition;
    let symbol = &resolved.symbol.definition;
    let name = &instance.designator;
    if instance.value.len() > MAX_VALUE || instance.value.chars().any(char::is_control) {
        return Err(invalid(format!(
            "{name}: value must be at most 256 bytes without control characters."
        )));
    }
    let mut placed = BTreeSet::new();
    for occurrence in occurrences {
        if symbol.unit(occurrence.unit).is_none() {
            return Err(invalid(format!(
                "{name}: unit {} is not a unit of its representation.",
                occurrence.unit
            )));
        }
        if !placed.insert(occurrence.unit) {
            return Err(Error::Conflict(format!(
                "{name}: unit {} is already placed.",
                occurrence.unit
            )));
        }
        if !point_valid(occurrence.position) {
            return Err(invalid(
                "Schematic position must be finite and within ±10000 mm.".into(),
            ));
        }
    }
    for unit in &instance.unused_units {
        if symbol.unit(*unit).is_none() {
            return Err(invalid(format!(
                "{name}: unit {unit} is not a unit of its representation."
            )));
        }
        if placed.contains(unit) {
            return Err(Error::Conflict(format!(
                "{name}: unit {unit} cannot be both placed and unused; clear the unused flag first."
            )));
        }
    }
    for terminal in &instance.no_connect {
        match component.terminal(*terminal) {
            None => {
                return Err(invalid(format!(
                    "{name}: no-connect names unknown terminal {terminal}."
                )));
            }
            Some(t) if t.kind == PinKind::NotConnected => {
                return Err(invalid(format!(
                    "{name}: terminal {terminal} is device-declared NC and needs no project no-connect."
                )));
            }
            Some(_) => {}
        }
    }
    for endpoint in endpoints {
        let terminal = component
            .terminal(endpoint.terminal)
            .ok_or_else(|| invalid("A wire references a missing terminal.".into()))?;
        if terminal.kind == PinKind::NotConnected {
            return Err(Error::Conflict(
                "A device-declared NC terminal cannot be wired.".into(),
            ));
        }
        if instance.no_connect.contains(&terminal.id) {
            return Err(Error::Conflict(format!(
                "{name} terminal {} is marked no-connect; a no-connect terminal cannot be wired.",
                terminal.id
            )));
        }
        let unit = owning_unit(symbol, terminal.id).ok_or_else(|| {
            invalid(format!(
                "{name}: terminal {} has no anchor in its representation.",
                terminal.id
            ))
        })?;
        if !placed.contains(&unit) {
            return Err(Error::Conflict(format!(
                "Place {} before wiring terminal {} ‘{}’.",
                display_label(project, instance, unit),
                terminal.id,
                terminal.name
            )));
        }
    }
    Ok(())
}

/// Choices offered by the pinned component; unit IDs exist in the resolved
/// representation; placed and unused exclusive; no-connect only on terminals
/// that are not device NC; every wire endpoint's unit placed, not device NC
/// and not project no-connect, so no-connect and wiring exclude each other.
pub fn check_instance(project: &Project, id: Id) -> Result<()> {
    let instance = project
        .instance(id)
        .ok_or_else(|| Error::NotFound("Unknown component instance.".into()))?;
    let endpoints = project.wires.iter().flat_map(|w| [w.from, w.to]);
    check_with(
        project,
        instance,
        project.occurrences_of(id),
        endpoints.filter(move |e| e.instance == id),
    )
}

/// [`check_instance`] for every instance, with one pass over occurrences and
/// wires.
pub fn check_all(project: &Project) -> Result<()> {
    let mut occurrences: BTreeMap<Id, Vec<&SymbolOccurrence>> = BTreeMap::new();
    for occurrence in &project.occurrences {
        occurrences
            .entry(occurrence.instance)
            .or_default()
            .push(occurrence);
    }
    let mut endpoints: BTreeMap<Id, Vec<Endpoint>> = BTreeMap::new();
    for wire in &project.wires {
        for endpoint in [wire.from, wire.to] {
            endpoints
                .entry(endpoint.instance)
                .or_default()
                .push(endpoint);
        }
    }
    for instance in &project.instances {
        check_with(
            project,
            instance,
            occurrences.get(&instance.id).into_iter().flatten().copied(),
            endpoints.get(&instance.id).into_iter().flatten().copied(),
        )?;
    }
    Ok(())
}

/// Designators are well formed and unique case-insensitively.
pub fn check_designators(project: &Project) -> Result<()> {
    let mut seen = BTreeSet::new();
    for instance in &project.instances {
        if !designator_valid(&instance.designator) {
            return Err(invalid(
                "Designators must be 1–64 bytes without spaces or control characters.".into(),
            ));
        }
        if !seen.insert(instance.designator.to_lowercase()) {
            return Err(Error::Conflict(format!(
                "Designator {} is already used.",
                instance.designator
            )));
        }
    }
    Ok(())
}

/// What `component.place` creates: one instance and its first occurrence.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewInstance {
    pub component: ComponentRef,
    pub designator: String,
    pub value: String,
    pub representation: DefinitionId,
    pub unit: UnitId,
    pub position: Point,
    pub physical: Option<PhysicalChoice>,
}

pub(crate) fn instance_mut(project: &mut Project, id: Id) -> Result<&mut ComponentInstance> {
    project
        .instances
        .iter_mut()
        .find(|i| i.id == id)
        .ok_or_else(|| Error::NotFound("Unknown component instance.".into()))
}

/// The one occurrence constructor: allocate and record an occurrence within
/// [`MAX_OCCURRENCES`]. Callers check the instance afterwards.
pub(crate) fn add_occurrence(
    project: &mut Project,
    instance: Id,
    unit: UnitId,
    position: Point,
) -> Result<Id> {
    if project.occurrences.len() >= MAX_OCCURRENCES {
        return Err(Error::Limit(
            "This edition supports at most 16384 symbol occurrences.".into(),
        ));
    }
    let id = project.allocate()?;
    project.occurrences.push(SymbolOccurrence {
        id,
        instance,
        unit,
        position,
    });
    Ok(id)
}

/// Create an instance and the occurrence of its first unit. Callers stage
/// this on a candidate project they discard on failure.
pub fn place(project: &mut Project, request: NewInstance) -> Result<(Id, Id)> {
    if project.instances.len() >= MAX_INSTANCES {
        return Err(Error::Limit(
            "This edition supports at most 4096 component instances.".into(),
        ));
    }
    if project.library.component(request.component).is_none() {
        return Err(Error::NotFound(
            "Choose a published component revision first.".into(),
        ));
    }
    let id = project.allocate()?;
    project.instances.push(ComponentInstance {
        id,
        component: request.component,
        designator: request.designator,
        value: request.value,
        representation: request.representation,
        physical: request.physical,
        unused_units: BTreeSet::new(),
        no_connect: BTreeSet::new(),
    });
    let occurrence = add_occurrence(project, id, request.unit, request.position)?;
    check_instance(project, id)?;
    check_designators(project)?;
    Ok((id, occurrence))
}

/// Remove an instance with nothing attached. Occurrences, wires and the
/// placement are detached first by `schematic` and `layout`.
pub fn remove(project: &mut Project, id: Id) -> Result<()> {
    if project.instance(id).is_none() {
        return Err(Error::NotFound("Unknown component instance.".into()));
    }
    if project.occurrences_of(id).next().is_some()
        || project.placement_of(id).is_some()
        || project
            .wires
            .iter()
            .any(|w| w.from.instance == id || w.to.instance == id)
    {
        return Err(Error::Conflict(
            "Detach the instance's occurrences, wires and placement before removing it.".into(),
        ));
    }
    project.instances.retain(|i| i.id != id);
    Ok(())
}

/// The configuration an instance takes on in another revision of its
/// component lineage, with its units already mapped by the caller.
pub(crate) struct Retarget {
    pub component: ComponentRef,
    pub representation: DefinitionId,
    pub physical: Option<PhysicalChoice>,
    pub unused_units: BTreeSet<UnitId>,
    /// Each remaining occurrence of the instance and the target unit it draws.
    pub units: Vec<(Id, UnitId)>,
}

/// Pin an instance to another revision of its lineage. Project no-connect
/// markers on terminals the target removes or declares NC are dropped; the
/// caller has already unplaced dropped units and removed wires on such
/// terminals through `schematic`, and checks the instance once its schematic
/// is regrouped.
pub(crate) fn retarget(project: &mut Project, id: Id, target: Retarget) -> Result<()> {
    let component = project
        .library
        .component(target.component)
        .map(|c| c.definition.clone())
        .ok_or_else(|| {
            Error::NotFound(
                "Target exact component revision is not in the project snapshot.".into(),
            )
        })?;
    if let Some(stray) = target
        .units
        .iter()
        .find(|(o, _)| project.occurrence(*o).is_none_or(|o| o.instance != id))
    {
        return Err(Error::Invalid(format!(
            "Occurrence {} is not an occurrence of this instance.",
            stray.0
        )));
    }
    for occurrence in &mut project.occurrences {
        if let Some((_, unit)) = target.units.iter().find(|(o, _)| *o == occurrence.id) {
            occurrence.unit = *unit;
        }
    }
    let instance = instance_mut(project, id)?;
    instance.component = target.component;
    instance.representation = target.representation;
    instance.physical = target.physical;
    instance.unused_units = target.unused_units;
    instance.no_connect.retain(|t| wirable(&component, *t));
    Ok(())
}

/// Reannotate. Every identity stays; only the label changes.
pub fn set_designator(project: &mut Project, id: Id, designator: String) -> Result<()> {
    instance_mut(project, id)?.designator = designator;
    check_designators(project)
}

/// Slice 1 free-text value; replaced by typed fields in slice 2.
pub fn set_value(project: &mut Project, id: Id, value: String) -> Result<()> {
    instance_mut(project, id)?.value = value;
    check_instance(project, id)
}

/// Mark a unit unused or clear the mark. Rejected while the unit is placed;
/// never changes no-connect markers.
pub fn set_unit_unused(project: &mut Project, id: Id, unit: UnitId, unused: bool) -> Result<()> {
    let instance = instance_mut(project, id)?;
    if unused {
        instance.unused_units.insert(unit);
    } else {
        instance.unused_units.remove(&unit);
    }
    check_instance(project, id)
}

/// Record or clear project no-connect decisions on terminals.
pub fn set_no_connect(
    project: &mut Project,
    id: Id,
    terminals: &[TerminalId],
    no_connect: bool,
) -> Result<()> {
    if terminals.is_empty() || terminals.len() > MAX_NO_CONNECT_EDIT {
        return Err(invalid(
            "Change between 1 and 256 no-connect markers at a time.".into(),
        ));
    }
    let instance = project
        .instance(id)
        .ok_or_else(|| Error::NotFound("Unknown component instance.".into()))?;
    let component = &resolve(&project.library, instance)?.component.definition;
    if let Some(unknown) = terminals.iter().find(|t| component.terminal(**t).is_none()) {
        return Err(invalid(format!("Unknown terminal {unknown}.")));
    }
    let instance = instance_mut(project, id)?;
    for terminal in terminals {
        if no_connect {
            instance.no_connect.insert(*terminal);
        } else {
            instance.no_connect.remove(terminal);
        }
    }
    check_instance(project, id)
}
