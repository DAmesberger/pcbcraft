//! Revision-bound, speculative changes to electronics and their PCB impact.
//! Plans are derived from exact snapshots and electrical IDs, never names.
//! [`stage`] is the single composition of consequential instance edits; a
//! plan is blocked whenever a current choice is not offered by its target
//! until an explicit resolution is supplied. Nothing is inferred from names,
//! order or pad count.
use crate::diagnostics::{Error, Result};
use crate::fields::{EffectiveField, PropertyEdit, PropertyInput};
use crate::geometry::Point;
use crate::ids::{ComponentRef, DefinitionId, Endpoint, Id, TerminalId, UnitId};
use crate::instances::{self, Resolved};
use crate::library::{
    BindingDefinition, ComponentDefinition, FootprintDefinition, PackageDefinition, Pad, PinKind,
    SymbolDefinition,
};
use crate::model::{ComponentInstance, EditPolicy, PhysicalChoice, Project, SymbolOccurrence};
use crate::{erc, layout, nets, schematic, validate};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Instances one component-revision update may change.
pub const MAX_UPDATE_INSTANCES: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "params", deny_unknown_fields)]
pub enum Change {
    /// Move instances to another revision of the same component lineage.
    ComponentRevision {
        instances: Vec<Id>,
        target: ComponentRef,
        #[serde(default)]
        resolutions: Resolutions,
    },
    /// A consequential instance edit (remove, change implementation).
    Instance {
        edit: InstanceChange,
    },
    /// A consequential schematic edit, e.g. unplacing a wired unit.
    Schematic {
        command: schematic::SchematicCommand,
    },
    SyncPcb {},
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "params", deny_unknown_fields)]
pub enum InstanceChange {
    Remove {
        instance: Id,
    },
    SetPhysical {
        instance: Id,
        choice: Option<PhysicalChoice>,
    },
}

/// Explicit decisions a component-revision plan needs. Each names an
/// instance in the same update.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Resolutions {
    #[serde(default)]
    pub choices: Vec<ChoiceResolution>,
    #[serde(default)]
    pub units: Vec<UnitResolution>,
    #[serde(default)]
    pub regroups: Vec<RegroupResolution>,
    #[serde(default)]
    pub fields: Vec<FieldResolution>,
}

/// An explicit decision for an instance field whose assigned value the
/// target revision no longer accepts: a new input against the target field,
/// or `Inherit` to drop the override.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldResolution {
    pub instance: Id,
    /// The field lineage.
    pub field: DefinitionId,
    pub input: PropertyInput,
}

fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Replace a lineage choice the target no longer offers. `physical` absent
/// keeps the current choice; `null` clears it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChoiceResolution {
    pub instance: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<DefinitionId>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub physical: Option<Option<PhysicalChoice>>,
}

/// Carry a current unit's occurrence or unused flag to a unit of the target
/// representation, or drop it. Unit IDs are lineage-local, so a
/// representation-lineage change needs one for every occurrence and flag.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnitResolution {
    pub instance: Id,
    /// The current unit, in the current representation.
    pub unit: UnitId,
    pub target: UnitTarget,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnitTarget {
    Unit(UnitId),
    Drop,
}

/// A wired terminal whose anchor moves into a unit that is not placed.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegroupResolution {
    pub instance: Id,
    pub terminal: TerminalId,
    pub action: RegroupAction,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RegroupAction {
    /// Place the target unit here (clearing its unused flag).
    PlaceUnit { position: Point },
    /// Remove the terminal's wires; the plan lists them.
    AcceptDisconnect,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyUpdate {
    pub expected_revision: u64,
    pub change: Change,
    pub remove_conflicting_tracks: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdatePlan {
    pub revision: u64,
    pub change: Change,
    pub changes: Vec<String>,
    pub affected_wires: Vec<Id>,
    pub removed_wires: Vec<Id>,
    /// Placement IDs are their stable instance IDs.
    pub affected_placements: Vec<Id>,
    pub removed_placements: Vec<Id>,
    pub conflicting_tracks: Vec<Id>,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

impl UpdatePlan {
    fn detail(&mut self, message: String) {
        if self.changes.len() < 10_000 {
            self.changes.push(message);
        } else {
            const LIMIT: &str = "Preview detail limit exceeded; update fewer instances at a time.";
            if !self.blockers.iter().any(|b| b == LIMIT) {
                self.blockers.push(LIMIT.into());
            }
        }
    }
}

fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
fn not_found(message: &str) -> Error {
    Error::NotFound(message.into())
}

fn endpoint_label(project: &Project, endpoint: Endpoint) -> String {
    let designator = project
        .instance(endpoint.instance)
        .map(|i| i.designator.as_str())
        .unwrap_or("removed");
    format!("{designator}.{}", endpoint.terminal)
}

fn members(project: &Project, terminals: &[Endpoint]) -> String {
    if terminals.is_empty() {
        return "removed".into();
    }
    let mut labels = terminals
        .iter()
        .take(128)
        .map(|e| endpoint_label(project, *e))
        .collect::<Vec<_>>()
        .join(", ");
    if terminals.len() > 128 {
        labels.push_str(&format!(" and {} more terminals", terminals.len() - 128));
    }
    labels
}

fn local_pad(pad: Option<&Pad>) -> String {
    pad.map(|p| {
        format!(
            "at {:?} mm, size {:?} mm, package terminal {}",
            p.position, p.size, p.terminal
        )
    })
    .unwrap_or_else(|| "removed".into())
}

fn world_pad(pad: Option<&layout::WorldPad>) -> String {
    pad.map(|p| {
        format!(
            "at {:?} mm, size {:?} mm, {}°, {:?}, net {}",
            p.position, p.size, p.rotation_deg, p.layer, p.net
        )
    })
    .unwrap_or_else(|| "absent".into())
}

/// The single composition of consequential instance edits. `component.remove`
/// and `instance.set_physical` run it with `Direct`; reviewed updates with
/// `Consequential`.
pub fn stage(project: &mut Project, change: &InstanceChange, policy: EditPolicy) -> Result<()> {
    match change {
        InstanceChange::Remove { instance } => {
            if project.instance(*instance).is_none() {
                return Err(not_found("Unknown component instance."));
            }
            schematic::detach_instance(project, *instance, policy)?;
            layout::detach_instance(project, *instance, policy)?;
            instances::remove(project, *instance)
        }
        InstanceChange::SetPhysical { instance, choice } => {
            layout::set_physical(project, *instance, *choice, policy)
        }
    }
}

fn stage_change(project: &Project, change: &Change) -> Result<Project> {
    let mut next = project.clone();
    match change {
        Change::ComponentRevision {
            instances,
            target,
            resolutions,
        } => revise(&mut next, instances, *target, resolutions)?,
        Change::Instance { edit } => stage(&mut next, edit, EditPolicy::Consequential)?,
        Change::Schematic { command } => {
            schematic::apply(&mut next, command.clone(), EditPolicy::Consequential)?;
        }
        Change::SyncPcb {} => layout::apply(&mut next, layout::LayoutCommand::Sync)?,
    }
    nets::reconcile(&mut next)?;
    Ok(next)
}

/// Move instances to `target`, applying only explicit resolutions.
fn revise(
    next: &mut Project,
    ids: &[Id],
    target: ComponentRef,
    resolutions: &Resolutions,
) -> Result<()> {
    if ids.is_empty() || ids.len() > MAX_UPDATE_INSTANCES {
        return Err(invalid(
            "Select between 1 and 128 instances for one revision update.",
        ));
    }
    let target_component = next.library.component(target).cloned().ok_or_else(|| {
        not_found("Target exact component revision is not in the project snapshot.")
    })?;
    let selected: BTreeSet<Id> = ids.iter().copied().collect();
    if selected.len() != ids.len() {
        return Err(invalid("An instance occurs twice in the update."));
    }
    let named = resolutions
        .choices
        .iter()
        .map(|r| r.instance)
        .chain(resolutions.units.iter().map(|r| r.instance))
        .chain(resolutions.regroups.iter().map(|r| r.instance))
        .chain(resolutions.fields.iter().map(|r| r.instance));
    for instance in named {
        if !selected.contains(&instance) {
            return Err(invalid(
                "A resolution names an instance that is not part of this update.",
            ));
        }
    }
    for id in ids {
        let instance = next
            .instance(*id)
            .ok_or_else(|| not_found("Update refers to an unknown instance."))?;
        if instance.component.id != target.id {
            return Err(Error::Conflict(
                "Revision updates must retain the component definition lineage. Matching names or pin numbers do not establish electrical compatibility.".into(),
            ));
        }
        revise_instance(next, *id, target, &target_component.definition, resolutions)?;
    }
    for id in ids {
        instances::check_instance(next, *id)?;
    }
    Ok(())
}

fn revise_instance(
    next: &mut Project,
    id: Id,
    target: ComponentRef,
    target_component: &ComponentDefinition,
    resolutions: &Resolutions,
) -> Result<()> {
    let old = next
        .instance(id)
        .cloned()
        .ok_or_else(|| not_found("Update refers to an unknown instance."))?;
    let name = old.designator.clone();
    let field_resolutions: Vec<PropertyEdit> = resolutions
        .fields
        .iter()
        .filter(|r| r.instance == id)
        .map(|r| PropertyEdit {
            field: r.field,
            input: r.input.clone(),
        })
        .collect();
    let properties =
        instances::carry_properties(&next.library, &old, target_component, &field_resolutions)?;
    let choice = resolutions.choices.iter().find(|c| c.instance == id);
    let representation = choice
        .and_then(|c| c.representation)
        .unwrap_or(old.representation);
    let Some(symbol_ref) = target_component.representation(representation) else {
        return Err(Error::Conflict(format!(
            "{name}: representation {representation} is not offered by {target}; supply a ChoiceResolution."
        )));
    };
    let new_symbol = next
        .library
        .symbol(symbol_ref)
        .map(|s| s.definition.clone())
        .ok_or_else(|| invalid("Target representation is missing from the snapshot."))?;
    let physical = match choice.and_then(|c| c.physical) {
        Some(choice) => choice,
        None => old.physical,
    };
    if let Some(choice) = physical
        && instances::resolve_physical(&next.library, target_component, choice).is_err()
    {
        return Err(Error::Conflict(format!(
            "{name}: implementation {} with footprint {} is not offered by {target}; supply a ChoiceResolution.",
            choice.binding, choice.footprint
        )));
    }
    let lineage_changed = representation != old.representation;
    let unit_resolutions: Vec<&UnitResolution> = resolutions
        .units
        .iter()
        .filter(|r| r.instance == id)
        .collect();
    let mut keys = BTreeSet::new();
    for resolution in &unit_resolutions {
        if !keys.insert(resolution.unit) {
            return Err(invalid("A unit is resolved twice."));
        }
        let used = next.occurrences_of(id).any(|o| o.unit == resolution.unit)
            || old.unused_units.contains(&resolution.unit);
        if !used {
            return Err(Error::Invalid(format!(
                "{name}: unit {} has no occurrence or unused flag to resolve.",
                resolution.unit
            )));
        }
    }
    let resolution_for = |unit: UnitId| {
        unit_resolutions
            .iter()
            .find(|r| r.unit == unit)
            .map(|r| r.target)
    };
    let mut claimed = BTreeSet::new();
    let mut claim = |unit: UnitId| -> Result<()> {
        if new_symbol.unit(unit).is_none() {
            return Err(Error::Invalid(format!(
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
    let occurrences: Vec<SymbolOccurrence> = next.occurrences_of(id).cloned().collect();
    let mut moved: Vec<(Id, UnitId)> = Vec::new();
    let mut dropped: Vec<Id> = Vec::new();
    for occurrence in occurrences {
        let target_unit = match resolution_for(occurrence.unit) {
            Some(UnitTarget::Unit(unit)) => Some(unit),
            Some(UnitTarget::Drop) => None,
            None if !lineage_changed && new_symbol.unit(occurrence.unit).is_some() => {
                Some(occurrence.unit)
            }
            None => {
                return Err(Error::Conflict(format!(
                    "{}: placed unit {} has no counterpart in the target representation; supply a UnitResolution.",
                    instances::display_label(next, &old, occurrence.unit),
                    occurrence.unit
                )));
            }
        };
        match target_unit {
            Some(unit) => {
                claim(unit)?;
                moved.push((occurrence.id, unit));
            }
            None => dropped.push(occurrence.id),
        }
    }
    let mut unused = BTreeSet::new();
    for unit in &old.unused_units {
        match resolution_for(*unit) {
            Some(UnitTarget::Unit(target_unit)) => {
                claim(target_unit)?;
                unused.insert(target_unit);
            }
            Some(UnitTarget::Drop) => {}
            None if !lineage_changed => {
                // A same-lineage unit keeps its meaning; a removed unused unit
                // only loses its flag, which the plan lists.
                if new_symbol.unit(*unit).is_some() {
                    claim(*unit)?;
                    unused.insert(*unit);
                }
            }
            None => {
                return Err(Error::Conflict(format!(
                    "{name}: unused unit {unit} has no counterpart in the target representation; supply a UnitResolution."
                )));
            }
        }
    }
    // Dropped units leave the sheet with their wires while the old symbol
    // still resolves.
    for occurrence in dropped {
        schematic::unplace_unit(next, occurrence, EditPolicy::Consequential)?;
    }
    // Terminals the target removes or declares NC keep no wires.
    let unwirable: BTreeSet<TerminalId> = wired_terminals(next, id)
        .into_iter()
        .filter(|t| !instances::wirable(target_component, *t))
        .collect();
    schematic::disconnect_terminals(next, id, &unwirable, EditPolicy::Consequential)?;
    let placed: BTreeSet<UnitId> = moved.iter().map(|(_, unit)| *unit).collect();
    let Regroup {
        places,
        disconnects,
    } = regroup(next, id, &name, &new_symbol, &placed, resolutions)?;
    schematic::disconnect_terminals(next, id, &disconnects, EditPolicy::Consequential)?;
    // A unit placed to keep its wires is no longer unused.
    unused.retain(|unit| !places.iter().any(|(placed, _)| placed == unit));
    instances::retarget(
        next,
        id,
        instances::Retarget {
            component: target,
            representation,
            physical,
            unused_units: unused,
            units: moved,
            properties,
        },
    )?;
    if physical.is_none() {
        layout::detach_instance(next, id, EditPolicy::Consequential)?;
    }
    if !places.is_empty() {
        schematic::place_units(next, id, &places)?;
    }
    Ok(())
}

fn wired_terminals(project: &Project, id: Id) -> BTreeSet<TerminalId> {
    project
        .wires
        .iter()
        .flat_map(|w| [w.from, w.to])
        .filter(|e| e.instance == id)
        .map(|e| e.terminal)
        .collect()
}

/// The decisions [`regroup`] collects for one instance.
struct Regroup {
    /// Units to place; the first placement named for a unit wins.
    places: Vec<(UnitId, Point)>,
    /// Terminals whose wires are removed.
    disconnects: BTreeSet<TerminalId>,
}

/// Wired terminals whose anchor moved into a unit that is not placed need an
/// explicit decision: place that unit, or accept disconnecting their wires.
fn regroup(
    next: &Project,
    id: Id,
    name: &str,
    symbol: &SymbolDefinition,
    placed: &BTreeSet<UnitId>,
    resolutions: &Resolutions,
) -> Result<Regroup> {
    let mut places: Vec<(UnitId, Point)> = Vec::new();
    let mut disconnects = BTreeSet::new();
    for terminal in wired_terminals(next, id) {
        let Some(unit) = instances::owning_unit(symbol, terminal) else {
            continue;
        };
        if placed.contains(&unit) || places.iter().any(|(u, _)| *u == unit) {
            continue;
        }
        let action = resolutions
            .regroups
            .iter()
            .find(|r| r.instance == id && r.terminal == terminal)
            .map(|r| &r.action);
        match action {
            Some(RegroupAction::PlaceUnit { position }) => places.push((unit, *position)),
            Some(RegroupAction::AcceptDisconnect) => {
                disconnects.insert(terminal);
            }
            None => {
                return Err(Error::Conflict(format!(
                    "{name} terminal {terminal} is wired and moves to unit {}, which is not placed; supply a RegroupResolution to place the unit or accept disconnecting its wires.",
                    symbol.unit(unit).map(|u| u.label.as_str()).unwrap_or("?")
                )));
            }
        }
    }
    Ok(Regroup {
        places,
        disconnects,
    })
}

/// What an instance resolves to, for comparing two project states.
struct View<'a> {
    name: &'a str,
    component: &'a ComponentDefinition,
    symbol: &'a SymbolDefinition,
    binding: Option<&'a BindingDefinition>,
    footprint: Option<&'a FootprintDefinition>,
    package: Option<&'a PackageDefinition>,
}

fn view<'a>(project: &'a Project, instance: &ComponentInstance) -> Option<View<'a>> {
    let Resolved {
        component,
        symbol,
        physical,
    } = instances::resolve(&project.library, instance).ok()?;
    Some(View {
        name: &component.meta.name,
        component: &component.definition,
        symbol: &symbol.definition,
        binding: physical.map(|p| &p.binding.definition),
        footprint: physical.map(|p| &p.footprint.definition),
        package: physical.map(|p| &p.package.definition),
    })
}

fn unit_label(symbol: &SymbolDefinition, unit: UnitId) -> String {
    symbol
        .unit(unit)
        .map(|u| u.label.clone())
        .unwrap_or_else(|| unit.to_string())
}

/// Definition differences reachable from two resolved states of one instance.
fn definition_details(
    plan: &mut UpdatePlan,
    designator: &str,
    instance: Id,
    a: &View<'_>,
    b: &View<'_>,
    mapping_endpoints: &mut BTreeSet<Endpoint>,
) {
    if a.name != b.name {
        plan.detail(format!(
            "{designator}: component name ‘{}’ → ‘{}’.",
            a.name, b.name
        ));
    }
    if let (Some(pa), Some(pb)) = (a.package, b.package) {
        plan.detail(format!(
            "{designator}: model dimensions {:?} → {:?} mm; model offset {:?} → {:?} mm.",
            pa.body_size, pb.body_size, pa.model_offset, pb.model_offset
        ));
    }
    // Units and anchors.
    for unit in &a.symbol.units {
        if b.symbol.unit(unit.id).is_none() {
            plan.detail(format!(
                "{designator}: unit {} ({}) is removed.",
                unit.label, unit.id
            ));
        }
    }
    for unit in &b.symbol.units {
        if a.symbol.unit(unit.id).is_none() {
            plan.detail(format!(
                "{designator}: add unit {} ({}).",
                unit.label, unit.id
            ));
        }
    }
    for terminal in &b.component.terminals {
        let before = a.symbol.anchor(terminal.id);
        let after = b.symbol.anchor(terminal.id);
        match (before, after) {
            (Some((ua, aa)), Some((ub, ab))) if ua.id != ub.id => plan.detail(format!(
                "{designator} terminal {} moves from unit {} to unit {}; anchor {:?} → {:?} mm.",
                terminal.id,
                unit_label(a.symbol, ua.id),
                unit_label(b.symbol, ub.id),
                aa.position,
                ab.position
            )),
            (Some((_, aa)), Some((ub, ab))) if aa.position != ab.position => plan.detail(format!(
                "{designator} terminal {} anchor in unit {} {:?} → {:?} mm.",
                terminal.id,
                unit_label(b.symbol, ub.id),
                aa.position,
                ab.position
            )),
            _ => {}
        }
    }
    if a.symbol.units.iter().map(|u| (u.id, u.body_size)).ne(b
        .symbol
        .units
        .iter()
        .map(|u| (u.id, u.body_size)))
    {
        plan.detail(format!(
            "{designator}: symbol geometry changes; anchors remain bound by terminal ID."
        ));
    }
    // Explicit binding mapping, keyed by package terminal.
    let map = |binding: Option<&BindingDefinition>| -> BTreeMap<_, _> {
        binding
            .map(|b| {
                b.mappings
                    .iter()
                    .map(|m| (m.package_terminal, m.terminal))
                    .collect()
            })
            .unwrap_or_default()
    };
    let am = map(a.binding);
    let bm = map(b.binding);
    let none = || "none".to_string();
    for physical in am.keys().chain(bm.keys()).copied().collect::<BTreeSet<_>>() {
        if am.get(&physical) != bm.get(&physical) {
            plan.detail(format!(
                "{designator}: package terminal {physical} maps electrical {} → {}.",
                am.get(&physical)
                    .map(|t| t.to_string())
                    .unwrap_or_else(none),
                bm.get(&physical)
                    .map(|t| t.to_string())
                    .unwrap_or_else(none)
            ));
            for terminal in am.get(&physical).into_iter().chain(bm.get(&physical)) {
                mapping_endpoints.insert(Endpoint {
                    instance,
                    terminal: *terminal,
                });
            }
        }
    }
    // Terminals.
    for terminal in &a.component.terminals {
        let replacement = b.component.terminal(terminal.id);
        if replacement.is_some_and(|t| {
            (t.kind == PinKind::NotConnected) != (terminal.kind == PinKind::NotConnected)
        }) {
            mapping_endpoints.insert(Endpoint {
                instance,
                terminal: terminal.id,
            });
        }
        if replacement != Some(terminal) {
            plan.detail(format!(
                "{designator}: terminal {} ‘{}’ ({:?}) → {}.",
                terminal.id,
                terminal.name,
                terminal.kind,
                replacement
                    .map(|t| format!("‘{}’ ({:?})", t.name, t.kind))
                    .unwrap_or_else(|| "removed".into())
            ));
            if replacement.is_some_and(|t| t.name != terminal.name) {
                plan.warnings.push(format!(
                    "{designator}.{} is renamed; its stable electrical identity is retained. Confirm the new label does not represent a different signal.",
                    terminal.id
                ));
            }
        }
    }
    for terminal in &b.component.terminals {
        if a.component.terminal(terminal.id).is_none() {
            plan.detail(format!(
                "{designator}: add electrical terminal {} ({:?}).",
                terminal.id, terminal.kind
            ));
        }
    }
    // Pads. A removed pad or changed pad terminal creates an explicit routing
    // obligation even when its net stays the same.
    let empty = Vec::new();
    let a_pads = a.footprint.map_or(&empty, |f| &f.pads);
    let b_pads = b.footprint.map_or(&empty, |f| &f.pads);
    for pad in a_pads {
        let replacement = b_pads.iter().find(|p| p.id == pad.id);
        if replacement.is_none_or(|p| p.terminal != pad.terminal) {
            if let Some(terminal) = am.get(&pad.terminal) {
                mapping_endpoints.insert(Endpoint {
                    instance,
                    terminal: *terminal,
                });
            }
            if let Some(replacement) = replacement
                && let Some(terminal) = bm.get(&replacement.terminal)
            {
                mapping_endpoints.insert(Endpoint {
                    instance,
                    terminal: *terminal,
                });
            }
        }
        if replacement != Some(pad) {
            plan.detail(format!(
                "{designator}: footprint pad {} {} → {}.",
                pad.id,
                local_pad(Some(pad)),
                local_pad(replacement)
            ));
        }
    }
    for pad in b_pads {
        if !a_pads.iter().any(|p| p.id == pad.id) {
            plan.detail(format!(
                "{designator}: add footprint pad {} {}.",
                pad.id,
                local_pad(Some(pad))
            ));
        }
    }
    let datum = |f: Option<&FootprintDefinition>| f.map(|f| f.datum_offset);
    if datum(a.footprint) != datum(b.footprint) {
        plan.detail(format!(
            "{designator}: footprint datum {:?} → {:?} mm.",
            datum(a.footprint),
            datum(b.footprint)
        ));
    }
}

fn instance_details(
    plan: &mut UpdatePlan,
    project: &Project,
    next: &Project,
    old: &ComponentInstance,
    new: &ComponentInstance,
    mapping_endpoints: &mut BTreeSet<Endpoint>,
) {
    let designator = old.designator.as_str();
    if old.designator != new.designator {
        plan.detail(format!(
            "Instance {}: designator {:?} → {:?}.",
            old.id, old.designator, new.designator
        ));
    }
    if old.component != new.component || old.properties != new.properties {
        field_details(plan, project, next, old, new);
    }
    if old.component != new.component {
        plan.detail(format!(
            "{designator}: component {} revision {} → {}.",
            old.component.id, old.component.revision, new.component.revision
        ));
    }
    if old.representation != new.representation {
        plan.detail(format!(
            "{designator}: representation {} → {}.",
            old.representation, new.representation
        ));
    }
    if old.physical != new.physical {
        let show = |c: Option<PhysicalChoice>| {
            c.map(|c| format!("binding {} footprint {}", c.binding, c.footprint))
                .unwrap_or_else(|| "none".into())
        };
        plan.detail(format!(
            "{designator}: physical implementation {} → {}.",
            show(old.physical),
            show(new.physical)
        ));
    }
    if (old.component, old.representation, old.physical)
        != (new.component, new.representation, new.physical)
        && let (Some(a), Some(b)) = (view(project, old), view(next, new))
    {
        definition_details(plan, designator, old.id, &a, &b, mapping_endpoints);
    }
    for unit in old.unused_units.difference(&new.unused_units) {
        plan.detail(format!(
            "{designator}: unit {unit} is no longer marked unused."
        ));
    }
    for unit in new.unused_units.difference(&old.unused_units) {
        plan.detail(format!("{designator}: unit {unit} is marked unused."));
    }
    for terminal in old.no_connect.difference(&new.no_connect) {
        plan.detail(format!(
            "{designator}: no-connect marker on terminal {terminal} is removed."
        ));
    }
    for terminal in new.no_connect.difference(&old.no_connect) {
        plan.detail(format!(
            "{designator}: terminal {terminal} is marked no-connect."
        ));
    }
}

/// Effective-value changes per field lineage, including label-only field
/// revisions, which keep their assignments.
fn field_details(
    plan: &mut UpdatePlan,
    project: &Project,
    next: &Project,
    old: &ComponentInstance,
    new: &ComponentInstance,
) {
    let designator = old.designator.as_str();
    let fields = |p: &Project, i: &ComponentInstance| -> BTreeMap<DefinitionId, EffectiveField> {
        instances::effective_fields(&p.library, i)
            .unwrap_or_default()
            .into_iter()
            .map(|f| (f.field.id, f))
            .collect()
    };
    let before = fields(project, old);
    let after = fields(next, new);
    let show = |f: Option<&EffectiveField>| {
        f.map_or_else(
            || "not selected".to_string(),
            |f| {
                format!(
                    "{} ({:?})",
                    f.text.as_deref().unwrap_or("no value"),
                    f.origin
                )
            },
        )
    };
    for lineage in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
        let (a, b) = (before.get(lineage), after.get(lineage));
        if let (Some(a), Some(b)) = (a, b)
            && a.label != b.label
        {
            plan.detail(format!(
                "{designator}: field ‘{}’ is renamed ‘{}’; its assignment is kept.",
                a.label, b.label
            ));
        }
        if a.map(|f| (&f.value, f.origin)) != b.map(|f| (&f.value, f.origin)) {
            let label = b
                .or(a)
                .map_or_else(|| lineage.to_string(), |f| f.label.clone());
            plan.detail(format!(
                "{designator}: field ‘{label}’ {} → {}.",
                show(a),
                show(b)
            ));
        }
    }
}

fn occurrence_details(plan: &mut UpdatePlan, project: &Project, next: &Project) {
    let label = |p: &Project, o: &SymbolOccurrence| {
        p.instance(o.instance)
            .map(|i| instances::display_label(p, i, o.unit))
            .unwrap_or_else(|| "removed instance".into())
    };
    for occurrence in &project.occurrences {
        match next.occurrence(occurrence.id) {
            None => plan.detail(format!(
                "Remove symbol occurrence {} ({}).",
                occurrence.id,
                label(project, occurrence)
            )),
            Some(after) if after.unit != occurrence.unit => plan.detail(format!(
                "Occurrence {} draws {} instead of {}.",
                occurrence.id,
                label(next, after),
                label(project, occurrence)
            )),
            Some(after) if after.position != occurrence.position => plan.detail(format!(
                "{}: schematic position {:?} → {:?} mm; electrical identities stay unchanged.",
                label(project, occurrence),
                occurrence.position,
                after.position
            )),
            Some(_) => {}
        }
    }
    for occurrence in &next.occurrences {
        if project.occurrence(occurrence.id).is_none() {
            plan.detail(format!(
                "Place {} at {:?} mm (occurrence {}).",
                label(next, occurrence),
                occurrence.position,
                occurrence.id
            ));
        }
    }
}

/// Preview never mutates the source. Apply recomputes this plan at its bound revision.
pub(crate) fn preview(
    project: &Project,
    revision: u64,
    change: Change,
) -> (UpdatePlan, Option<Project>) {
    let mut plan = UpdatePlan {
        revision,
        change: change.clone(),
        changes: vec![],
        affected_wires: vec![],
        removed_wires: vec![],
        affected_placements: vec![],
        removed_placements: vec![],
        conflicting_tracks: vec![],
        blockers: vec![],
        warnings: vec![],
    };
    let mut next = match stage_change(project, &change) {
        Ok(next) => next,
        Err(error) => {
            plan.blockers.push(error.to_string());
            return (plan, None);
        }
    };
    let old_nets: BTreeMap<Id, &[Endpoint]> = project
        .nets
        .iter()
        .map(|net| (net.id, net.terminals.as_slice()))
        .collect();
    let new_nets: BTreeMap<Id, &[Endpoint]> = next
        .nets
        .iter()
        .map(|net| (net.id, net.terminals.as_slice()))
        .collect();
    let old_net_by_terminal: BTreeMap<Endpoint, Id> = project
        .nets
        .iter()
        .flat_map(|net| net.terminals.iter().map(move |t| (*t, net.id)))
        .collect();
    let new_net_by_terminal: BTreeMap<Endpoint, Id> = next
        .nets
        .iter()
        .flat_map(|net| net.terminals.iter().map(move |t| (*t, net.id)))
        .collect();
    // Net IDs are persistent, so a net that only gains terminals keeps its
    // copper valid. Nets that lose terminals, disappear or merge away are
    // electrical changes whose copper must be removed explicitly.
    let mut electrical_changes = BTreeSet::new();
    for (id, terminals) in &old_nets {
        let name = project.net_name(*id);
        match new_nets.get(id) {
            Some(after) if after == terminals => {}
            Some(after) if terminals.iter().all(|t| after.binary_search(t).is_ok()) => {
                plan.detail(format!(
                    "Net {name} gains terminals: {} → {}.",
                    members(project, terminals),
                    members(&next, after)
                ));
            }
            after => {
                electrical_changes.insert(*id);
                plan.detail(format!(
                    "Net {name}: {} → {}.",
                    members(project, terminals),
                    members(&next, after.copied().unwrap_or(&[]))
                ));
            }
        }
    }
    for (id, terminals) in &new_nets {
        if !old_nets.contains_key(id) {
            plan.detail(format!(
                "Add net {}: {}.",
                next.net_name(*id),
                members(&next, terminals)
            ));
        }
    }
    let mut mapping_endpoints = BTreeSet::new();
    for old in &project.instances {
        let Some(new) = next.instance(old.id) else {
            plan.detail(format!(
                "Remove instance {} (ID {}).",
                old.designator, old.id
            ));
            continue;
        };
        instance_details(&mut plan, project, &next, old, new, &mut mapping_endpoints);
    }
    for instance in &next.instances {
        if project.instance(instance.id).is_none() {
            plan.detail(format!(
                "Add instance {} (ID {}), component {}.",
                instance.designator, instance.id, instance.component
            ));
        }
    }
    occurrence_details(&mut plan, project, &next);
    for endpoint in mapping_endpoints {
        for net in old_net_by_terminal
            .get(&endpoint)
            .into_iter()
            .chain(new_net_by_terminal.get(&endpoint))
        {
            electrical_changes.insert(*net);
        }
    }
    for wire in &project.wires {
        let replacement = next.wires.iter().find(|w| w.id == wire.id);
        if replacement.is_none() {
            plan.removed_wires.push(wire.id);
            plan.detail(format!(
                "Remove wire {}: {} → {}.",
                wire.id,
                endpoint_label(project, wire.from),
                endpoint_label(project, wire.to)
            ));
        }
        if replacement != Some(wire)
            || [wire.from, wire.to]
                .into_iter()
                .any(|e| schematic::pin_position(project, e) != schematic::pin_position(&next, e))
        {
            plan.affected_wires.push(wire.id);
        }
    }
    for wire in &next.wires {
        if !project.wires.iter().any(|w| w.id == wire.id) {
            plan.affected_wires.push(wire.id);
            plan.detail(format!(
                "Add wire {}: {} → {}.",
                wire.id,
                endpoint_label(&next, wire.from),
                endpoint_label(&next, wire.to)
            ));
        }
    }
    for placement in &project.placements {
        let replacement = next
            .placements
            .iter()
            .find(|p| p.instance == placement.instance);
        let pins = |p: &Project| {
            p.instance(placement.instance)
                .map(|i| (i.component, i.physical))
        };
        let part_changed = pins(project) != pins(&next);
        if replacement != Some(placement) || part_changed {
            plan.affected_placements.push(placement.instance);
            if placement.locked {
                plan.blockers.push(format!(
                    "Placement {} is locked; unlock it before updating.",
                    placement.instance
                ));
            }
            if replacement.is_none() {
                plan.removed_placements.push(placement.instance);
                plan.detail(format!(
                    "Remove PCB placement {} at {:?} mm.",
                    placement.instance, placement.position
                ));
            } else {
                plan.detail(format!(
                    "Placement {} stays at {:?} mm, rotation {}°, {:?}; its exact component snapshot changes.",
                    placement.instance, placement.position, placement.rotation_deg, placement.layer
                ));
            }
        }
    }
    for placement in &next.placements {
        if !project
            .placements
            .iter()
            .any(|p| p.instance == placement.instance)
        {
            plan.affected_placements.push(placement.instance);
            plan.detail(format!(
                "Add PCB placement {} at {:?} mm, {}°, {:?}.",
                placement.instance, placement.position, placement.rotation_deg, placement.layer
            ));
        }
    }
    let old_pads: BTreeMap<_, _> = layout::pads(project)
        .into_iter()
        .map(|pad| ((pad.instance, pad.pad), pad))
        .collect();
    let new_pads: BTreeMap<_, _> = layout::pads(&next)
        .into_iter()
        .map(|pad| ((pad.instance, pad.pad), pad))
        .collect();
    let pad_ids: BTreeSet<_> = old_pads.keys().chain(new_pads.keys()).copied().collect();
    let mut conflicts: BTreeSet<_> = project
        .tracks
        .iter()
        .filter(|t| electrical_changes.contains(&t.net))
        .map(|t| t.id)
        .collect();
    let mut affected_tracks = conflicts.clone();
    let mut pairs = 0usize;
    for key in pad_ids {
        let before = old_pads.get(&key);
        let after = new_pads.get(&key);
        if before == after {
            continue;
        }
        plan.detail(format!(
            "{} pad {} world geometry: {} → {}.",
            project
                .instance(key.0)
                .or_else(|| next.instance(key.0))
                .map(|i| i.designator.as_str())
                .unwrap_or("Removed instance"),
            key.1,
            world_pad(before),
            world_pad(after)
        ));
        for track in &project.tracks {
            pairs += 1;
            if pairs > 100_000 {
                plan.blockers.push(
                    "Impact geometry budget exceeded; update fewer instances at a time.".into(),
                );
                break;
            }
            let touched_before = before.is_some_and(|p| layout::track_touches_pad(track, p));
            let touched_after = after.is_some_and(|p| layout::track_touches_pad(track, p));
            if touched_before || touched_after {
                affected_tracks.insert(track.id);
            }
            if after.is_some_and(|p| layout::track_pad_clearance_conflict(track, p)) {
                conflicts.insert(track.id);
                affected_tracks.insert(track.id);
            }
            if touched_before && !touched_after && !conflicts.contains(&track.id) {
                plan.warnings.push(format!(
                    "Track {} is detached from instance {} pad {} after its geometry changes; copper stays in place for rerouting.",
                    track.id, key.0, key.1
                ));
            }
        }
        if pairs > 100_000 {
            break;
        }
    }
    for track in &project.tracks {
        if affected_tracks.contains(&track.id) && track.locked {
            plan.blockers.push(format!(
                "Track {} is locked and affected; unlock it before updating.",
                track.id
            ));
        }
    }
    plan.conflicting_tracks = conflicts.into_iter().collect();
    if !plan.conflicting_tracks.is_empty() {
        plan.warnings.push(format!(
            "Explicit removal required for tracks {:?}; retained tracks are never retagged.",
            plan.conflicting_tracks
        ));
    }
    next.tracks
        .retain(|t| !plan.conflicting_tracks.contains(&t.id));
    if let Err(error) = validate::edited(project, &next) {
        plan.blockers.push(error.to_string());
    }
    for check in layout::checks(&next) {
        plan.warnings
            .push(format!("Candidate PCB check: {}", check.message));
    }
    for check in erc::checks(&next) {
        plan.warnings
            .push(format!("Candidate electrical check: {}", check.message));
    }
    plan.affected_wires.sort_unstable();
    plan.affected_wires.dedup();
    plan.affected_placements.sort_unstable();
    plan.affected_placements.dedup();
    plan.blockers.sort();
    plan.blockers.dedup();
    plan.warnings.sort();
    plan.warnings.dedup();
    if plan.changes.is_empty() {
        plan.warnings
            .push("This update makes no document changes.".into());
    }
    (plan, Some(next))
}

pub(crate) fn candidate(project: &Project, revision: u64, update: ApplyUpdate) -> Result<Project> {
    if update.expected_revision != revision {
        return Err(Error::Stale {
            expected: update.expected_revision,
            actual: revision,
        });
    }
    let (plan, candidate) = preview(project, revision, update.change);
    if !plan.blockers.is_empty() {
        return Err(Error::Conflict(plan.blockers.join("\n")));
    }
    if !plan.conflicting_tracks.is_empty() && !update.remove_conflicting_tracks {
        return Err(Error::Conflict(format!(
            "Update requires explicit removal of conflicting tracks {:?}.",
            plan.conflicting_tracks
        )));
    }
    candidate.ok_or_else(|| invalid("Update could not be staged."))
}

#[cfg(test)]
mod tests;
