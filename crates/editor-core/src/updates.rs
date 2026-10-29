//! Revision-bound, speculative changes to electronics and their PCB impact.
//! Plans are derived from exact snapshots and electrical IDs, never names.
use crate::{Endpoint, Error, Id, PartRef, PinKind, Project, Result, layout, nets, schematic};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "params", deny_unknown_fields)]
pub enum Change {
    PartRevision {
        instances: Vec<Id>,
        target: PartRef,
    },
    Schematic {
        command: schematic::SchematicCommand,
    },
    SyncPcb {},
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
    let reference = project
        .instance(endpoint.instance)
        .map(|i| i.reference.as_str())
        .unwrap_or("removed");
    format!("{reference}.{}", endpoint.terminal)
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

fn local_pad(pad: Option<&crate::Pad>) -> String {
    pad.map(|p| {
        format!(
            "at {:?} mm, size {:?} mm, physical terminal {}",
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

fn stage(project: &Project, change: &Change) -> Result<Project> {
    let mut next = project.clone();
    match change {
        Change::PartRevision { instances, target } => {
            if instances.is_empty() || instances.len() > 128 {
                return Err(invalid(
                    "Select between 1 and 128 instances for one revision update.",
                ));
            }
            let target_part = project.part(*target).ok_or_else(|| {
                not_found("Target exact component revision is not in the project snapshot.")
            })?;
            crate::parts::validate_part(target_part)?;
            let mut selected = BTreeSet::new();
            for id in instances {
                if !selected.insert(*id) {
                    return Err(invalid("An instance occurs twice in the update."));
                }
                let instance = next
                    .instances
                    .iter_mut()
                    .find(|i| i.id == *id)
                    .ok_or_else(|| not_found("Update refers to an unknown instance."))?;
                if instance.part.id != target.id {
                    return Err(Error::Conflict(
                        "Revision updates must retain the component definition ID. Matching names or pin numbers do not establish electrical compatibility.".into(),
                    ));
                }
                instance.part = *target;
            }
            // Missing/NC terminals cannot retain wires. The complete removal list
            // is surfaced before apply and committed together with the revision.
            let retained: BTreeSet<_> = next
                .wires
                .iter()
                .filter(|wire| {
                    [wire.from, wire.to].into_iter().all(|endpoint| {
                        schematic::terminal_kind(&next, endpoint)
                            .is_some_and(|kind| kind != PinKind::NotConnected)
                    })
                })
                .map(|wire| wire.id)
                .collect();
            next.wires.retain(|wire| retained.contains(&wire.id));
            nets::reconcile(&mut next)?;
            schematic::validate(&next)?;
        }
        Change::Schematic { command } => {
            schematic::apply_candidate(&mut next, command.clone(), true)?
        }
        Change::SyncPcb {} => layout::apply(&mut next, layout::LayoutCommand::Sync)?,
    }
    Ok(next)
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
    let mut next = match stage(project, &change) {
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
                old.reference, old.id
            ));
            continue;
        };
        if old.reference != new.reference || old.value != new.value {
            plan.detail(format!(
                "Instance {}: reference/value {:?} / {:?} → {:?} / {:?}.",
                old.id, old.reference, old.value, new.reference, new.value
            ));
        }
        if old.position != new.position {
            plan.detail(format!(
                "{}: schematic position {:?} → {:?} mm; electrical identities stay unchanged.",
                old.reference, old.position, new.position
            ));
        }
        if old.part != new.part {
            plan.detail(format!(
                "{}: component {} revision {} → {}.",
                old.reference, old.part.id, old.part.revision, new.part.revision
            ));
            if let Some((a, b)) = project.part(old.part).zip(next.part(new.part)) {
                if a.name != b.name {
                    plan.detail(format!(
                        "{}: component name ‘{}’ → ‘{}’.",
                        old.reference, a.name, b.name
                    ));
                }
                plan.detail(format!(
                    "{}: model dimensions {:?} → {:?} mm; model offset {:?} → {:?} mm.",
                    old.reference,
                    a.package.body_size,
                    b.package.body_size,
                    a.package.model_offset,
                    b.package.model_offset
                ));
                if a.symbol != b.symbol {
                    plan.detail(format!(
                        "{}: symbol geometry changes; pins remain bound by terminal ID.",
                        old.reference
                    ));
                }
                let am: BTreeMap<_, _> = a
                    .binding
                    .mappings
                    .iter()
                    .map(|m| (m.physical, m.electrical))
                    .collect();
                let bm: BTreeMap<_, _> = b
                    .binding
                    .mappings
                    .iter()
                    .map(|m| (m.physical, m.electrical))
                    .collect();
                for physical in am.keys().chain(bm.keys()).copied().collect::<BTreeSet<_>>() {
                    if am.get(&physical) != bm.get(&physical) {
                        plan.detail(format!(
                            "{}: physical terminal {physical} maps electrical {} → {}.",
                            old.reference,
                            am.get(&physical)
                                .map(Id::to_string)
                                .unwrap_or_else(|| "none".into()),
                            bm.get(&physical)
                                .map(Id::to_string)
                                .unwrap_or_else(|| "none".into())
                        ));
                        for terminal in am.get(&physical).into_iter().chain(bm.get(&physical)) {
                            mapping_endpoints.insert(Endpoint {
                                instance: old.id,
                                terminal: *terminal,
                            });
                        }
                    }
                }
                for terminal in &a.component.terminals {
                    let replacement = b.component.terminals.iter().find(|t| t.id == terminal.id);
                    if replacement.is_some_and(|t| {
                        (t.kind == PinKind::NotConnected)
                            != (terminal.kind == PinKind::NotConnected)
                    }) {
                        mapping_endpoints.insert(Endpoint {
                            instance: old.id,
                            terminal: terminal.id,
                        });
                    }
                    if replacement != Some(terminal) {
                        plan.detail(format!(
                            "{}: terminal {} ‘{}’ ({:?}) → {}.",
                            old.reference,
                            terminal.id,
                            terminal.name,
                            terminal.kind,
                            replacement
                                .map(|t| format!("‘{}’ ({:?})", t.name, t.kind))
                                .unwrap_or_else(|| "removed".into())
                        ));
                        if replacement.is_some_and(|t| t.name != terminal.name) {
                            plan.warnings.push(format!("{}.{} is renamed; its stable electrical identity is retained. Confirm the new label does not represent a different signal.",old.reference,terminal.id));
                        }
                    }
                }
                for terminal in &b.component.terminals {
                    if !a.component.terminals.iter().any(|t| t.id == terminal.id) {
                        plan.detail(format!(
                            "{}: add electrical terminal {} ({:?}).",
                            old.reference, terminal.id, terminal.kind
                        ));
                    }
                }
                // A removed pad or changed physical pad identity creates an explicit
                // routing obligation even when its electrical net remains the same.
                for pad in &a.footprint.pads {
                    let replacement = b.footprint.pads.iter().find(|p| p.id == pad.id);
                    if replacement.is_none_or(|p| p.terminal != pad.terminal) {
                        if let Some(terminal) = am.get(&pad.terminal) {
                            mapping_endpoints.insert(Endpoint {
                                instance: old.id,
                                terminal: *terminal,
                            });
                        }
                        if let Some(replacement) = replacement
                            && let Some(terminal) = bm.get(&replacement.terminal)
                        {
                            mapping_endpoints.insert(Endpoint {
                                instance: old.id,
                                terminal: *terminal,
                            });
                        }
                    }
                    if replacement != Some(pad) {
                        plan.detail(format!(
                            "{}: footprint pad {} {} → {}.",
                            old.reference,
                            pad.id,
                            local_pad(Some(pad)),
                            local_pad(replacement)
                        ));
                    }
                }
                for pad in &b.footprint.pads {
                    if !a.footprint.pads.iter().any(|p| p.id == pad.id) {
                        plan.detail(format!(
                            "{}: add footprint pad {} {}.",
                            old.reference,
                            pad.id,
                            local_pad(Some(pad))
                        ));
                    }
                }
                if a.footprint.datum_offset != b.footprint.datum_offset {
                    plan.detail(format!(
                        "{}: footprint datum {:?} → {:?} mm.",
                        old.reference, a.footprint.datum_offset, b.footprint.datum_offset
                    ));
                }
            }
        }
    }
    for instance in &next.instances {
        if project.instance(instance.id).is_none() {
            plan.detail(format!(
                "Add instance {} (ID {}), component {} revision {}.",
                instance.reference, instance.id, instance.part.id, instance.part.revision
            ));
        }
    }
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
        let part_changed = project.instance(placement.instance).map(|i| i.part)
            != next.instance(placement.instance).map(|i| i.part);
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
                plan.detail(format!("Placement {} stays at {:?} mm, rotation {}°, {:?}; its exact component snapshot changes.",placement.instance,placement.position,placement.rotation_deg,placement.layer));
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
                .map(|i| i.reference.as_str())
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
                plan.warnings.push(format!("Track {} is detached from instance {} pad {} after its geometry changes; copper stays in place for rerouting.",track.id,key.0,key.1));
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
    if let Err(error) = crate::validate(&next) {
        plan.blockers.push(error.to_string());
    }
    for check in layout::checks(&next) {
        plan.warnings
            .push(format!("Candidate PCB check: {}", check.message));
    }
    for check in schematic::checks(&next) {
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
