//! Instance configuration rules and choice resolution. An instance pins one
//! exact component revision; [`resolve`] turns its lineage choices into the
//! exact symbol, binding, footprint and package revisions it uses, so there is
//! one source of truth for each. [`check_instance`] is the one owner of the
//! per-instance invariants that commands, update plans and document
//! validation all apply. Field values are checked, applied and resolved
//! through `fields`, and function selections through `functions`; this
//! module only looks up the definitions they need.
use crate::diagnostics::{Error, Result};
use crate::fields::{
    self, EffectiveField, FieldOverride, InstanceFields, MAX_FIELDS, MAX_PROPERTY_EDITS,
    PropertyEdit, PropertyInput,
};
use crate::functions::{self, FunctionEdit, FunctionSelections, Resolution};
use crate::geometry::{Point, point_valid};
use crate::ids::{ComponentRef, DefinitionId, Endpoint, FieldRef, Id, TerminalId, UnitId};
use crate::library::{
    BindingDefinition, ComponentDefinition, FieldView, FootprintDefinition, Library,
    PackageDefinition, Published, Rotation, SymbolDefinition, designator_prefix_valid,
};
use crate::model::{ComponentInstance, PhysicalChoice, Project, SymbolOccurrence, WireEnd};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod part;
pub mod pins;
pub use part::{MAX_ASSIGN_INSTANCES, assign_part, binding_name, part_findings, requirements};

pub const MAX_INSTANCES: usize = 4096;
/// Symbol occurrences in one project; [`add_occurrence`] enforces it.
pub const MAX_OCCURRENCES: usize = 16384;
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
                "{} does not offer {}.",
                crate::library::names::capitalised(&library.describe(crate::ids::DefinitionRef {
                    kind: crate::ids::DefinitionKind::Component,
                    id: component.id,
                    revision: component.revision,
                })),
                library.describe_lineage(crate::ids::DefinitionKind::Symbol, lineage)
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
/// it exists and is electrical (not device NC or reserved).
pub fn wirable(component: &ComponentDefinition, terminal: TerminalId) -> bool {
    component
        .terminal(terminal)
        .is_some_and(|t| t.is_electrical())
}

/// Resolve an instance's function selections against its pinned component
/// and, when it has one, its chosen binding's unavailable functions. This is
/// the one place that builds the resolver's input from library data.
pub fn function_resolution(library: &Library, instance: &ComponentInstance) -> Result<Resolution> {
    let resolved = resolve(library, instance)?;
    resolve_functions(
        &resolved.component.definition,
        resolved.physical.as_ref(),
        &instance.functions,
        &instance.pin_config,
    )
}

fn resolve_functions(
    component: &ComponentDefinition,
    physical: Option<&PhysicalView<'_>>,
    selections: &FunctionSelections,
    config: &functions::PinConfigs,
) -> Result<Resolution> {
    functions::resolve_with_rules(
        &component.declarations(),
        &component.resources,
        &component.constraints,
        physical.map(|p| p.binding.definition.unavailable.as_slice()),
        selections,
        &component.pin_rules,
        config,
    )
}

/// The unit that draws `terminal`.
pub fn owning_unit(symbol: &SymbolDefinition, terminal: TerminalId) -> Option<UnitId> {
    symbol.pin(terminal).map(|(unit, _)| unit.id)
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
    check_properties(&project.library, component, instance)?;
    part::check_part(&project.library, instance)?;
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
            Some(t) if !t.is_electrical() => {
                return Err(invalid(format!(
                    "{name}: terminal ‘{}’ is device NC or reserved and needs no project no-connect.",
                    t.name
                )));
            }
            Some(_) => {}
        }
    }
    for endpoint in endpoints {
        let terminal = component
            .terminal(endpoint.terminal)
            .ok_or_else(|| invalid("A wire references a missing terminal.".into()))?;
        if !terminal.is_electrical() {
            return Err(Error::Conflict(
                "A device-NC or reserved terminal cannot be wired.".into(),
            ));
        }
        if instance.no_connect.contains(&terminal.id) {
            return Err(Error::Conflict(format!(
                "{name} terminal ‘{}’ is marked no-connect; a no-connect terminal cannot be wired.",
                terminal.name
            )));
        }
        let unit = owning_unit(symbol, terminal.id).ok_or_else(|| {
            invalid(format!(
                "{name}: terminal ‘{}’ has no pin in its representation.",
                terminal.name
            ))
        })?;
        if !placed.contains(&unit) {
            return Err(Error::Conflict(format!(
                "Place {} before wiring terminal ‘{}’.",
                display_label(project, instance, unit),
                terminal.name
            )));
        }
    }
    check_functions(
        name,
        component,
        resolved.physical.as_ref(),
        &instance.functions,
        &instance.pin_config,
    )
}

/// Selections are structurally valid and the resolution has no conflicts:
/// capacity, exclusion and availability in the chosen binding are
/// invariants, never findings.
fn check_functions(
    name: &str,
    component: &ComponentDefinition,
    physical: Option<&PhysicalView<'_>>,
    selections: &FunctionSelections,
    config: &functions::PinConfigs,
) -> Result<()> {
    if selections.len() > component.terminals.len() {
        return Err(invalid(format!(
            "{name}: more function selections than terminals."
        )));
    }
    let resolution = resolve_functions(component, physical, selections, config)
        .map_err(|e| invalid(format!("{name}: {e}")))?;
    if resolution.conflicts.is_empty() {
        return Ok(());
    }
    let messages: Vec<&str> = resolution.conflicts.iter().map(|c| c.message()).collect();
    Err(Error::Conflict(format!("{name}: {}", messages.join(" "))))
}

fn field_view(library: &Library, r: FieldRef) -> Result<FieldView<'_>> {
    library.field_view(r).ok_or_else(|| {
        invalid(format!(
            "Field {r} or its quantity kind is not in the project library."
        ))
    })
}

/// A field error naming the instance, the field and its quantity kind.
fn in_field(name: &str, view: &FieldView<'_>, error: Error) -> Error {
    let kind = view
        .kind
        .map(|k| format!(" ({})", k.meta.name))
        .unwrap_or_default();
    let message = format!("{name} {}{kind}: {error}", view.field.meta.name);
    match error {
        Error::Limit(_) => Error::Limit(message),
        _ => Error::Invalid(message),
    }
}

/// Overrides name fields the pinned component selects, and assigned values
/// satisfy those field revisions.
fn check_properties(
    library: &Library,
    component: &ComponentDefinition,
    instance: &ComponentInstance,
) -> Result<()> {
    let name = &instance.designator;
    if instance.properties.len() > MAX_FIELDS {
        return Err(invalid(format!("{name}: at most 64 field overrides.")));
    }
    for (lineage, value) in &instance.properties {
        let selection = component.field(*lineage).ok_or_else(|| {
            invalid(format!(
                "{name}: field {lineage} is not selected by component {}.",
                instance.component
            ))
        })?;
        if let FieldOverride::Assigned(value) = value {
            let view = field_view(library, selection.field)?;
            fields::check_value(value, &view.field.definition.value_type, view.kind_body())
                .map_err(|e| in_field(name, &view, e))?;
        }
    }
    Ok(())
}

/// The effective value of every field the pinned component selects, in
/// selection order.
pub fn effective_fields(
    library: &Library,
    instance: &ComponentInstance,
) -> Result<Vec<EffectiveField>> {
    let component = library.component(instance.component).ok_or_else(|| {
        invalid(format!(
            "{} pins component {}, which is not in the project library.",
            instance.designator, instance.component
        ))
    })?;
    component
        .definition
        .fields
        .iter()
        .map(|selection| {
            let view = field_view(library, selection.field)?;
            Ok(fields::effective(
                &view.field.meta.name,
                &view.field.definition,
                view.kind_body(),
                selection,
                instance.properties.get(&selection.field.id),
            ))
        })
        .collect()
}

/// The effective value of every field `component` selects when nothing is
/// assigned, in selection order: what an instance placed now would show.
pub fn default_fields(
    library: &Library,
    component: &ComponentDefinition,
) -> Result<Vec<EffectiveField>> {
    component
        .fields
        .iter()
        .map(|selection| {
            let view = field_view(library, selection.field)?;
            Ok(fields::effective(
                &view.field.meta.name,
                &view.field.definition,
                view.kind_body(),
                selection,
                None,
            ))
        })
        .collect()
}

/// Choices offered by the pinned component; unit IDs exist in the resolved
/// representation; placed and unused exclusive; no-connect only on
/// electrical terminals; every wire endpoint's unit placed, electrical and
/// not project no-connect, so no-connect and wiring exclude each other;
/// field values valid; function selections structurally valid and
/// conflict-free, including availability in the chosen binding.
pub fn check_instance(project: &Project, id: Id) -> Result<()> {
    let instance = project
        .instance(id)
        .ok_or_else(|| Error::NotFound("Unknown component instance.".into()))?;
    let endpoints = project
        .wires
        .iter()
        .flat_map(|w| [w.from, w.to])
        .filter_map(WireEnd::terminal);
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
        for endpoint in [wire.from, wire.to]
            .into_iter()
            .filter_map(WireEnd::terminal)
        {
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
    /// An explicit designator, validated like `instance.set_designator`.
    /// Omitted, the core numbers the instance under the pinned component
    /// revision's designator prefix: the lowest free `{prefix}{n}`, n ≥ 1,
    /// unique case-insensitively within the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub designator: Option<String>,
    /// Initial field values, applied as one `instance.set_properties` batch.
    #[serde(default)]
    pub properties: Vec<PropertyEdit>,
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
        rotation: Rotation::R0,
        mirror: false,
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
    let Some(component) = project.library.component(request.component) else {
        return Err(Error::NotFound(
            "Choose a published component revision first.".into(),
        ));
    };
    let designator = match request.designator {
        Some(designator) => designator,
        None => next_designator(project, &component.definition.designator_prefix)?,
    };
    let id = project.allocate()?;
    project.instances.push(ComponentInstance {
        id,
        component: request.component,
        designator,
        properties: InstanceFields::new(),
        representation: request.representation,
        physical: request.physical,
        unused_units: BTreeSet::new(),
        no_connect: BTreeSet::new(),
        functions: FunctionSelections::new(),
        part: None,
        pin_config: Default::default(),
        marking: Default::default(),
    });
    let occurrence = add_occurrence(project, id, request.unit, request.position)?;
    apply_properties(project, id, &request.properties)?;
    check_instance(project, id)?;
    check_designators(project)?;
    Ok((id, occurrence))
}

/// The lowest free designator `{prefix}{n}`, n ≥ 1, compared
/// case-insensitively with every designator in the project. Fails for a
/// prefix that is not 1–8 uppercase letters.
pub fn next_designator(project: &Project, prefix: &str) -> Result<String> {
    if !designator_prefix_valid(prefix) {
        return Err(invalid(format!(
            "The component's designator prefix ‘{prefix}’ is not 1–8 uppercase letters; give a designator."
        )));
    }
    // Numbers already taken under this prefix. At most MAX_INSTANCES are, so
    // a free number lies within 1..=MAX_INSTANCES + 1.
    let taken: BTreeSet<usize> = project
        .instances
        .iter()
        .filter_map(|i| {
            let (head, digits) = i.designator.split_at_checked(prefix.len())?;
            if !head.eq_ignore_ascii_case(prefix)
                || digits.is_empty()
                || digits.starts_with('0')
                || !digits.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            digits.parse().ok()
        })
        .collect();
    (1..=MAX_INSTANCES + 1)
        .find(|n| !taken.contains(n))
        .map(|n| format!("{prefix}{n}"))
        .ok_or_else(|| Error::Limit("No free designator number is left.".into()))
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
            .flat_map(|w| [w.from, w.to])
            .filter_map(WireEnd::terminal)
            .any(|e| e.instance == id)
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
    /// Field overrides carried by [`carry_properties`].
    pub properties: InstanceFields,
    /// Function selections carried by [`carry_functions`].
    pub functions: FunctionSelections,
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
    instance.properties = target.properties;
    instance.functions = target.functions;
    instance.no_connect.retain(|t| wirable(&component, *t));
    Ok(())
}

/// Reannotate. Every identity stays; only the label changes.
pub fn set_designator(project: &mut Project, id: Id, designator: String) -> Result<()> {
    instance_mut(project, id)?.designator = designator;
    check_designators(project)
}

/// Apply up to 64 field edits to one instance's overrides as one batch.
/// Each field must be selected by the pinned component; any invalid edit
/// fails the whole batch.
fn apply_properties(project: &mut Project, id: Id, edits: &[PropertyEdit]) -> Result<()> {
    if edits.len() > MAX_PROPERTY_EDITS {
        return Err(invalid("Change between 1 and 64 fields at a time.".into()));
    }
    let instance = project
        .instance(id)
        .ok_or_else(|| Error::NotFound("Unknown component instance.".into()))?;
    let library = &project.library;
    let component = library.component(instance.component).ok_or_else(|| {
        Error::NotFound("The pinned component revision is not in the project library.".into())
    })?;
    let mut overrides = instance.properties.clone();
    let mut seen = BTreeSet::new();
    for edit in edits {
        if !seen.insert(edit.field) {
            return Err(invalid(format!(
                "{}: field {} is edited twice.",
                instance.designator, edit.field
            )));
        }
        let selection = component.definition.field(edit.field).ok_or_else(|| {
            invalid(format!(
                "{}: field {} is not selected by component {}.",
                instance.designator, edit.field, instance.component
            ))
        })?;
        let view = field_view(library, selection.field)?;
        fields::apply_input(
            &mut overrides,
            edit.field,
            &edit.input,
            &view.field.definition,
            view.kind_body(),
        )
        .map_err(|e| in_field(&instance.designator, &view, e))?;
    }
    instance_mut(project, id)?.properties = overrides;
    Ok(())
}

/// `instance.set_properties`: apply 1–64 field edits atomically and return
/// every effective field of the instance.
pub fn set_properties(
    project: &mut Project,
    id: Id,
    edits: &[PropertyEdit],
) -> Result<Vec<EffectiveField>> {
    if edits.is_empty() {
        return Err(invalid("Change between 1 and 64 fields at a time.".into()));
    }
    apply_properties(project, id, edits)?;
    check_instance(project, id)?;
    let instance = project
        .instance(id)
        .ok_or_else(|| Error::NotFound("Unknown component instance.".into()))?;
    effective_fields(&project.library, instance)
}

/// The overrides an instance keeps when it moves to `target`, another
/// revision of its component lineage. A valid override is preserved. An
/// assigned value whose field the target no longer selects, whose value type
/// or quantity-kind lineage changes, or which no longer satisfies the target
/// field blocks until `resolutions` names that field; an explicit unset of a
/// removed field is dropped. Resolutions are applied as property edits
/// against the target fields; `Inherit` also resolves a removed field.
pub(crate) fn carry_properties(
    library: &Library,
    old: &ComponentInstance,
    target: &ComponentDefinition,
    resolutions: &[PropertyEdit],
) -> Result<InstanceFields> {
    let name = &old.designator;
    let current = library.component(old.component).ok_or_else(|| {
        Error::NotFound("The pinned component revision is not in the project library.".into())
    })?;
    let mut resolved = BTreeSet::new();
    for resolution in resolutions {
        if !resolved.insert(resolution.field) {
            return Err(invalid(format!(
                "{name}: field {} is resolved twice.",
                resolution.field
            )));
        }
    }
    let current_field = |lineage: DefinitionId| {
        current
            .definition
            .field(lineage)
            .and_then(|s| library.field(s.field))
    };
    let label = |lineage: DefinitionId| {
        current_field(lineage).map_or_else(|| lineage.to_string(), |f| f.meta.name.clone())
    };
    let mut next = InstanceFields::new();
    for (lineage, value) in &old.properties {
        if resolved.contains(lineage) {
            next.insert(*lineage, value.clone());
            continue;
        }
        let Some(selection) = target.field(*lineage) else {
            if matches!(value, FieldOverride::Assigned(_)) {
                return Err(Error::Conflict(format!(
                    "{name}: field ‘{}’ has a value of its own, but the new revision no longer has this field. Decide what happens to the value: drop it, or keep the field on the component (an update FieldResolution).",
                    label(*lineage)
                )));
            }
            continue;
        };
        if let FieldOverride::Assigned(assigned) = value {
            let view = field_view(library, selection.field)?;
            let new_type = &view.field.definition.value_type;
            let same_meaning = current_field(*lineage)
                .is_some_and(|f| fields::compatible(&f.definition.value_type, new_type));
            if !same_meaning || fields::check_value(assigned, new_type, view.kind_body()).is_err() {
                return Err(Error::Conflict(format!(
                    "{name}: the value of field ‘{}’ does not fit its new definition (field revision {}). Enter a value that fits, or drop it (an update FieldResolution).",
                    label(*lineage),
                    selection.field
                )));
            }
        }
        next.insert(*lineage, value.clone());
    }
    for resolution in resolutions {
        match target.field(resolution.field) {
            Some(selection) => {
                let view = field_view(library, selection.field)?;
                fields::apply_input(
                    &mut next,
                    resolution.field,
                    &resolution.input,
                    &view.field.definition,
                    view.kind_body(),
                )
                .map_err(|e| in_field(name, &view, e))?;
            }
            None if resolution.input == PropertyInput::Inherit => {
                next.remove(&resolution.field);
            }
            None => {
                return Err(invalid(format!(
                    "{name}: the target revision does not select field {}; only Inherit resolves it.",
                    resolution.field
                )));
            }
        }
    }
    Ok(next)
}

/// `instance.select_functions`: apply 1–256 selection edits atomically. The
/// final state is checked once, so a batch may move a resource role between
/// terminals without an intermediate state. Returns the new resolution.
pub fn select_functions(
    project: &mut Project,
    id: Id,
    edits: &[FunctionEdit],
) -> Result<Resolution> {
    let instance = project
        .instance(id)
        .ok_or_else(|| Error::NotFound("Unknown component instance.".into()))?;
    let selections = functions::apply_edits(&instance.functions, edits)?;
    instance_mut(project, id)?.functions = selections;
    check_instance(project, id)?;
    let instance = project
        .instance(id)
        .ok_or_else(|| Error::NotFound("Unknown component instance.".into()))?;
    function_resolution(&project.library, instance)
}

/// The selections an instance keeps when it moves to `target`, another
/// revision of its component lineage. Selections are keyed by ID, so a
/// reordered or relabeled function keeps its selection. A selection whose
/// function the target no longer offers on that terminal blocks until
/// `resolutions` names the terminal; resolutions are applied as selection
/// edits. Conflicts in the result are rejected by [`check_instance`].
pub(crate) fn carry_functions(
    old: &ComponentInstance,
    target: &ComponentDefinition,
    resolutions: &[FunctionEdit],
) -> Result<FunctionSelections> {
    let name = &old.designator;
    let mut next = FunctionSelections::new();
    for (terminal, function) in &old.functions {
        if resolutions.iter().any(|r| r.terminal == *terminal) {
            next.insert(*terminal, *function);
            continue;
        }
        let offered = target
            .terminal(*terminal)
            .and_then(|t| t.functions())
            .is_some_and(|f| f.function(*function).is_some());
        if !offered {
            return Err(Error::Conflict(format!(
                "{name} terminal {terminal}: selected function {function} is not offered by the target revision; supply a FunctionResolution."
            )));
        }
        next.insert(*terminal, *function);
    }
    if resolutions.is_empty() {
        return Ok(next);
    }
    functions::apply_edits(&next, resolutions).map_err(|e| invalid(format!("{name}: {e}")))
}

/// Carry a current unit's occurrence or unused flag to a unit of the target
/// representation, or drop it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnitTarget {
    Unit(UnitId),
    Drop,
}

/// Where an instance's occurrences and unused flags go in a target
/// representation.
pub(crate) struct UnitCarry {
    /// Occurrences that stay, with their target unit.
    pub moved: Vec<(Id, UnitId)>,
    /// Occurrences that leave the sheet.
    pub dropped: Vec<Id>,
    /// Unused flags in the target representation.
    pub unused: BTreeSet<UnitId>,
}

/// The unit-carry rule for moving `old` to `symbol`, another revision of its
/// representation or (when `lineage_changed`) another representation lineage.
/// Unit IDs are lineage-local: within a lineage a unit keeps its meaning and a
/// removed unused unit only loses its flag; across lineages every occurrence
/// and unused flag needs an explicit resolution. Each target unit is claimed
/// once. Pure: the caller applies the result.
pub(crate) fn carry_units(
    project: &Project,
    old: &ComponentInstance,
    symbol: &SymbolDefinition,
    lineage_changed: bool,
    resolutions: &[(UnitId, UnitTarget)],
) -> Result<UnitCarry> {
    let name = &old.designator;
    let mut keys = BTreeSet::new();
    for (unit, _) in resolutions {
        if !keys.insert(*unit) {
            return Err(invalid("A unit is resolved twice.".into()));
        }
        let used = project.occurrences_of(old.id).any(|o| o.unit == *unit)
            || old.unused_units.contains(unit);
        if !used {
            return Err(invalid(format!(
                "{name}: unit {unit} has no occurrence or unused flag to resolve."
            )));
        }
    }
    let resolution_for = |unit: UnitId| {
        resolutions
            .iter()
            .find(|(u, _)| *u == unit)
            .map(|(_, t)| *t)
    };
    let mut claimed = BTreeSet::new();
    let mut claim = |unit: UnitId| -> Result<()> {
        if symbol.unit(unit).is_none() {
            return Err(invalid(format!(
                "{name}: unit {unit} is not a unit of the target representation."
            )));
        }
        if !claimed.insert(unit) {
            return Err(Error::Conflict(format!(
                "{name}: unit {unit} of the target representation is claimed twice."
            )));
        }
        Ok(())
    };
    let mut carry = UnitCarry {
        moved: Vec::new(),
        dropped: Vec::new(),
        unused: BTreeSet::new(),
    };
    for occurrence in project.occurrences_of(old.id) {
        let target = match resolution_for(occurrence.unit) {
            Some(UnitTarget::Unit(unit)) => Some(unit),
            Some(UnitTarget::Drop) => None,
            None if !lineage_changed && symbol.unit(occurrence.unit).is_some() => {
                Some(occurrence.unit)
            }
            None => {
                return Err(Error::Conflict(format!(
                    "{}: placed unit {} has no counterpart in the target representation; supply a UnitResolution.",
                    display_label(project, old, occurrence.unit),
                    occurrence.unit
                )));
            }
        };
        match target {
            Some(unit) => {
                claim(unit)?;
                carry.moved.push((occurrence.id, unit));
            }
            None => carry.dropped.push(occurrence.id),
        }
    }
    for unit in &old.unused_units {
        match resolution_for(*unit) {
            Some(UnitTarget::Unit(target)) => {
                claim(target)?;
                carry.unused.insert(target);
            }
            Some(UnitTarget::Drop) => {}
            None if !lineage_changed => {
                if symbol.unit(*unit).is_some() {
                    claim(*unit)?;
                    carry.unused.insert(*unit);
                }
            }
            None => {
                return Err(Error::Conflict(format!(
                    "{name}: unused unit {unit} has no counterpart in the target representation; supply a UnitResolution."
                )));
            }
        }
    }
    Ok(carry)
}

/// Terminals of instance `id` that carry at least one wire.
pub(crate) fn wired_terminals(project: &Project, id: Id) -> BTreeSet<TerminalId> {
    project
        .wires
        .iter()
        .flat_map(|w| [w.from, w.to])
        .filter_map(WireEnd::terminal)
        .filter(|e| e.instance == id)
        .map(|e| e.terminal)
        .collect()
}

/// The decision for a wired terminal whose pin moves into a unit that is
/// not placed.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RegroupAction {
    /// Place the target unit here (clearing its unused flag).
    PlaceUnit { position: Point },
    /// Remove the terminal's wires; the plan lists them.
    AcceptDisconnect,
}

/// The decisions [`regroup`] collects for one instance.
pub(crate) struct Regroup {
    /// Units to place; the first placement named for a unit wins.
    pub places: Vec<(UnitId, Point)>,
    /// Terminals whose wires are removed.
    pub disconnects: BTreeSet<TerminalId>,
}

/// The regroup rule: every wired terminal of instance `id` whose pin in
/// `symbol` lies in a unit outside `placed` needs an explicit decision from
/// `decide`, either placing that unit or accepting the disconnect. Pure: the
/// caller applies the result through `schematic`.
pub(crate) fn regroup<'a>(
    project: &Project,
    id: Id,
    symbol: &SymbolDefinition,
    placed: &BTreeSet<UnitId>,
    decide: impl Fn(TerminalId) -> Option<&'a RegroupAction>,
) -> Result<Regroup> {
    let name = project
        .instance(id)
        .map(|i| i.designator.as_str())
        .unwrap_or("?");
    let mut result = Regroup {
        places: Vec::new(),
        disconnects: BTreeSet::new(),
    };
    for terminal in wired_terminals(project, id) {
        let Some(unit) = owning_unit(symbol, terminal) else {
            continue;
        };
        if placed.contains(&unit) || result.places.iter().any(|(u, _)| *u == unit) {
            continue;
        }
        match decide(terminal) {
            Some(RegroupAction::PlaceUnit { position }) => result.places.push((unit, *position)),
            Some(RegroupAction::AcceptDisconnect) => {
                result.disconnects.insert(terminal);
            }
            None => {
                return Err(Error::Conflict(format!(
                    "{name} terminal {terminal} is wired and moves to unit {}, which is not placed; supply a RegroupResolution to place the unit or accept disconnecting its wires.",
                    symbol.unit(unit).map(|u| u.label.as_str()).unwrap_or("?")
                )));
            }
        }
    }
    Ok(result)
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
