//! Single-sheet capture. Wire endpoints reference electrical identities; drawn
//! crossings and coincident symbol pins never imply an electrical connection.
use crate::{
    Endpoint, Error, Id, Instance, Net, PartRef, PinKind, Point, Project, Result, Wire, point_valid,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum SchematicCommand {
    #[serde(rename = "schematic.place")]
    Place {
        part: PartRef,
        reference: String,
        value: String,
        position: Point,
    },
    #[serde(rename = "schematic.move")]
    Move { instance: Id, position: Point },
    #[serde(rename = "schematic.edit")]
    Edit {
        instance: Id,
        reference: String,
        value: String,
    },
    #[serde(rename = "schematic.connect")]
    Connect { from: Endpoint, to: Endpoint },
    #[serde(rename = "schematic.disconnect")]
    Disconnect { wire: Id },
    #[serde(rename = "schematic.remove")]
    Remove { instance: Id },
}

fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}

/// Also atomic when invoked directly, rather than through Engine::apply.
pub fn apply(project: &mut Project, command: SchematicCommand) -> Result<()> {
    let mut candidate = project.clone();
    apply_candidate(&mut candidate, command, false)?;
    protect_routed_nets(project, &candidate)?;
    *project = candidate;
    Ok(())
}

/// Speculative edits only. The update planner validates and commits the result.
pub(crate) fn apply_candidate(
    candidate: &mut Project,
    command: SchematicCommand,
    allow_placed_removal: bool,
) -> Result<()> {
    match command {
        SchematicCommand::Place {
            part,
            reference,
            value,
            position,
        } => {
            if candidate.part(part).is_none() {
                return Err(invalid("Choose a published part revision first."));
            }
            if candidate.instances.len() >= 4096 {
                return Err(invalid(
                    "This edition supports at most 4096 schematic instances.",
                ));
            }
            let id = candidate.allocate()?;
            candidate.instances.push(Instance {
                id,
                part,
                reference,
                value,
                position,
            });
        }
        SchematicCommand::Move { instance, position } => {
            let item = candidate
                .instances
                .iter_mut()
                .find(|i| i.id == instance)
                .ok_or_else(|| invalid("Unknown schematic instance."))?;
            item.position = position;
        }
        SchematicCommand::Edit {
            instance,
            reference,
            value,
        } => {
            let item = candidate
                .instances
                .iter_mut()
                .find(|i| i.id == instance)
                .ok_or_else(|| invalid("Unknown schematic instance."))?;
            item.reference = reference;
            item.value = value;
        }
        SchematicCommand::Connect { from, to } => {
            if from == to {
                return Err(invalid("A wire must connect two different terminals."));
            }
            for endpoint in [from, to] {
                if terminal_kind(candidate, endpoint).is_none() {
                    return Err(invalid("Unknown wire terminal."));
                }
                if terminal_kind(candidate, endpoint) == Some(PinKind::NotConnected) {
                    return Err(invalid("A device-declared NC terminal cannot be wired."));
                }
            }
            if candidate
                .wires
                .iter()
                .any(|w| (w.from == from && w.to == to) || (w.from == to && w.to == from))
            {
                return Err(invalid("These terminals are already connected by a wire."));
            }
            if candidate.wires.len() >= 16384 {
                return Err(invalid("This edition supports at most 16384 wires."));
            }
            let id = candidate.allocate()?;
            candidate.wires.push(Wire { id, from, to });
        }
        SchematicCommand::Disconnect { wire } => {
            if !candidate.wires.iter().any(|w| w.id == wire) {
                return Err(invalid("Unknown schematic wire."));
            }
            candidate.wires.retain(|w| w.id != wire);
        }
        SchematicCommand::Remove { instance } => {
            if candidate.instance(instance).is_none() {
                return Err(invalid("Unknown schematic instance."));
            }
            if !allow_placed_removal && candidate.placements.iter().any(|p| p.instance == instance)
            {
                return Err(invalid(
                    "Remove the PCB placement before deleting this instance.",
                ));
            }
            candidate.placements.retain(|p| p.instance != instance);
            candidate.instances.retain(|i| i.id != instance);
            candidate
                .wires
                .retain(|w| w.from.instance != instance && w.to.instance != instance);
        }
    }
    validate(candidate)
}

fn protect_routed_nets(before: &Project, after: &Project) -> Result<()> {
    if before.tracks.is_empty() {
        return Ok(());
    }
    let old = nets(before);
    let new = nets(after);
    for track in &before.tracks {
        let a = old.iter().find(|net| net.id == track.net);
        let b = new.iter().find(|net| net.id == track.net);
        if a.map(|net| &net.terminals) != b.map(|net| &net.terminals) {
            return Err(invalid(
                "This connectivity change affects routed copper. Preview it with project.preview_update and apply the reviewed track removals through project.apply_update.",
            ));
        }
    }
    Ok(())
}

pub fn validate(project: &Project) -> Result<()> {
    if project.instances.len() > 4096 || project.wires.len() > 16384 {
        return Err(invalid("Schematic exceeds supported item limits."));
    }
    let mut ids = BTreeSet::new();
    let mut labels = BTreeSet::new();
    for instance in &project.instances {
        if instance.id == 0 || !ids.insert(instance.id) {
            return Err(invalid(
                "Schematic instance identities must be nonzero and unique.",
            ));
        }
        if project.part(instance.part).is_none() {
            return Err(invalid(
                "An instance references a missing exact part revision.",
            ));
        }
        if !point_valid(instance.position) {
            return Err(invalid(
                "Schematic position must be finite and within ±10000 mm.",
            ));
        }
        if instance.reference.trim().is_empty()
            || instance.reference.len() > 64
            || instance.reference.chars().any(char::is_whitespace)
            || instance.reference.chars().any(char::is_control)
        {
            return Err(invalid(
                "Reference must be 1–64 bytes without spaces or control characters.",
            ));
        }
        if !labels.insert(instance.reference.to_lowercase()) {
            return Err(invalid("Reference designators must be unique."));
        }
        if instance.value.len() > 256 || instance.value.chars().any(char::is_control) {
            return Err(invalid(
                "Value must be at most 256 bytes without control characters.",
            ));
        }
    }
    let mut wire_ids = BTreeSet::new();
    let mut pairs = BTreeSet::new();
    for wire in &project.wires {
        if wire.id == 0 || ids.contains(&wire.id) || !wire_ids.insert(wire.id) {
            return Err(invalid(
                "Wire identities must be unique and distinct from instances.",
            ));
        }
        if wire.from == wire.to {
            return Err(invalid("A wire must connect two different terminals."));
        }
        for endpoint in [wire.from, wire.to] {
            match terminal_kind(project, endpoint) {
                None => return Err(invalid("A wire references a missing terminal.")),
                Some(PinKind::NotConnected) => {
                    return Err(invalid("Device-declared NC terminals cannot be wired."));
                }
                _ => {}
            }
        }
        let pair = if wire.from < wire.to {
            (wire.from, wire.to)
        } else {
            (wire.to, wire.from)
        };
        if !pairs.insert(pair) {
            return Err(invalid("Duplicate schematic connection."));
        }
    }
    Ok(())
}

pub fn terminal_kind(project: &Project, endpoint: Endpoint) -> Option<PinKind> {
    let instance = project.instance(endpoint.instance)?;
    project
        .part(instance.part)?
        .component
        .terminals
        .iter()
        .find(|t| t.id == endpoint.terminal)
        .map(|t| t.kind)
}

/// Global schematic anchor in millimetres; changing a symbol position affects
/// only wire presentation, while its electrical Endpoint remains identical.
pub fn pin_position(project: &Project, endpoint: Endpoint) -> Option<Point> {
    let instance = project.instance(endpoint.instance)?;
    let pin = project
        .part(instance.part)?
        .symbol
        .pins
        .iter()
        .find(|p| p.terminal == endpoint.terminal)?;
    Some([
        instance.position[0] + pin.position[0],
        instance.position[1] + pin.position[1],
    ])
}

pub fn nets(project: &Project) -> Vec<Net> {
    let mut graph: BTreeMap<Endpoint, BTreeSet<Endpoint>> = BTreeMap::new();
    for instance in &project.instances {
        if let Some(part) = project.part(instance.part) {
            for terminal in &part.component.terminals {
                graph
                    .entry(Endpoint {
                        instance: instance.id,
                        terminal: terminal.id,
                    })
                    .or_default();
            }
        }
    }
    for wire in &project.wires {
        if graph.contains_key(&wire.from) && graph.contains_key(&wire.to) {
            graph.entry(wire.from).or_default().insert(wire.to);
            graph.entry(wire.to).or_default().insert(wire.from);
        }
    }
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    for first in graph.keys().copied() {
        if seen.contains(&first) {
            continue;
        }
        let mut queue = VecDeque::from([first]);
        let mut members = BTreeSet::new();
        while let Some(endpoint) = queue.pop_front() {
            if !seen.insert(endpoint) {
                continue;
            }
            members.insert(endpoint);
            if let Some(neighbors) = graph.get(&endpoint) {
                queue.extend(neighbors.iter().copied().filter(|n| !seen.contains(n)));
            }
        }
        output.push(Net {
            id: format!("net:{}:{}", first.instance, first.terminal),
            terminals: members.into_iter().collect(),
        });
    }
    output
}

/// A deliberately small ERC subset. Passive and unspecified drive compatibility,
/// voltage domains and project no-connect decisions remain future work.
pub fn checks(project: &Project) -> Vec<String> {
    let mut issues = Vec::new();
    for net in nets(project) {
        let pins: Vec<_> = net
            .terminals
            .iter()
            .filter_map(|endpoint| terminal_kind(project, *endpoint).map(|kind| (*endpoint, kind)))
            .collect();
        let outputs = pins
            .iter()
            .filter(|(_, kind)| matches!(kind, PinKind::Output | PinKind::PowerOutput))
            .count();
        if outputs > 1 {
            issues.push(format!(
                "{}: multiple output terminals are connected.",
                net.id
            ));
        }
        for (endpoint, kind) in &pins {
            let reference = project
                .instance(endpoint.instance)
                .map(|i| i.reference.as_str())
                .unwrap_or("?");
            if pins.len() == 1 && matches!(kind, PinKind::Input | PinKind::PowerInput) {
                issues.push(format!(
                    "{reference} terminal {} is unconnected.",
                    endpoint.terminal
                ));
            } else if *kind == PinKind::PowerInput
                && !pins.iter().any(|(_, kind)| *kind == PinKind::PowerOutput)
            {
                issues.push(format!(
                    "{reference} terminal {} has no power-output driver on its net.",
                    endpoint.terminal
                ));
            }
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn project() -> Project {
        let part = Part {
            id: 1,
            revision: 1,
            name: "Test".into(),
            component: Component {
                terminals: vec![
                    Terminal {
                        id: 1,
                        name: "A".into(),
                        kind: PinKind::Input,
                    },
                    Terminal {
                        id: 2,
                        name: "B".into(),
                        kind: PinKind::Output,
                    },
                ],
            },
            symbol: Symbol {
                pins: vec![
                    SymbolPin {
                        terminal: 1,
                        position: [-5.0, 0.0],
                    },
                    SymbolPin {
                        terminal: 2,
                        position: [5.0, 0.0],
                    },
                ],
                body_size: [6.0, 4.0],
            },
            package: Package {
                terminals: vec![
                    PackageTerminal {
                        id: 1,
                        number: "1".into(),
                    },
                    PackageTerminal {
                        id: 2,
                        number: "2".into(),
                    },
                ],
                body_size: [2.0, 1.0, 0.5],
                model_offset: [0.0; 3],
            },
            footprint: Footprint {
                pads: vec![
                    Pad {
                        id: 1,
                        terminal: 1,
                        position: [-1.0, 0.0],
                        size: [0.5, 0.5],
                    },
                    Pad {
                        id: 2,
                        terminal: 2,
                        position: [1.0, 0.0],
                        size: [0.5, 0.5],
                    },
                ],
                datum_offset: [0.0; 2],
            },
            binding: Binding {
                mappings: vec![
                    TerminalMap {
                        electrical: 1,
                        physical: 1,
                    },
                    TerminalMap {
                        electrical: 2,
                        physical: 2,
                    },
                ],
            },
            provenance: "authored test".into(),
        };
        Project {
            parts: vec![part],
            next_id: 2,
            ..Project::default()
        }
    }
    fn place(p: &mut Project, reference: &str, position: Point) -> Result<Id> {
        let id = p.next_id;
        apply(
            p,
            SchematicCommand::Place {
                part: PartRef { id: 1, revision: 1 },
                reference: reference.into(),
                value: "1k".into(),
                position,
            },
        )?;
        Ok(id)
    }
    #[test]
    fn topology_uses_ids_and_survives_moving_and_reannotation() -> Result<()> {
        let mut p = project();
        let a = place(&mut p, "U1", [0.0, 0.0])?;
        let b = place(&mut p, "U2", [0.0, 0.0])?;
        assert_eq!(
            nets(&p).len(),
            4,
            "coincident symbols do not short terminals"
        );
        apply(
            &mut p,
            SchematicCommand::Connect {
                from: Endpoint {
                    instance: a,
                    terminal: 2,
                },
                to: Endpoint {
                    instance: b,
                    terminal: 1,
                },
            },
        )?;
        let before = nets(&p);
        apply(
            &mut p,
            SchematicCommand::Move {
                instance: a,
                position: [20.0, 10.0],
            },
        )?;
        apply(
            &mut p,
            SchematicCommand::Edit {
                instance: a,
                reference: "AMP7".into(),
                value: "new".into(),
            },
        )?;
        assert_eq!(nets(&p), before);
        assert_eq!(p.instances[0].part, PartRef { id: 1, revision: 1 });
        assert_eq!(
            pin_position(
                &p,
                Endpoint {
                    instance: a,
                    terminal: 2
                }
            ),
            Some([25.0, 10.0])
        );
        Ok(())
    }
    #[test]
    fn malformed_references_and_routed_connectivity_changes_leave_state_unchanged() -> Result<()> {
        let mut p = project();
        let a = place(&mut p, "U1", [0.0, 0.0])?;
        let b = place(&mut p, "U2", [20.0, 0.0])?;
        let from = Endpoint {
            instance: a,
            terminal: 2,
        };
        let to = Endpoint {
            instance: b,
            terminal: 1,
        };
        let before = p.clone();
        assert!(
            apply(
                &mut p,
                SchematicCommand::Connect {
                    from,
                    to: Endpoint {
                        instance: 999,
                        terminal: 1
                    }
                }
            )
            .is_err()
        );
        assert_eq!(p, before);
        assert!(
            apply(
                &mut p,
                SchematicCommand::Move {
                    instance: a,
                    position: [f64::NAN, 0.0]
                }
            )
            .is_err()
        );
        assert_eq!(p, before);
        apply(&mut p, SchematicCommand::Connect { from, to })?;
        let net = nets(&p)
            .into_iter()
            .find(|n| n.terminals.contains(&from))
            .ok_or_else(|| invalid("missing test net"))?;
        p.tracks.push(Track {
            id: 9,
            net: net.id,
            start: [0.0, 0.0],
            end: [5.0, 0.0],
            width: 0.5,
            layer: Layer::Top,
            locked: false,
        });
        let before = p.clone();
        let wire = p.wires[0].id;
        assert!(apply(&mut p, SchematicCommand::Disconnect { wire }).is_err());
        assert_eq!(p, before);
        Ok(())
    }
    #[test]
    fn erc_reports_unconnected_input_and_multiple_outputs() -> Result<()> {
        let mut p = project();
        let a = place(&mut p, "U1", [0.0, 0.0])?;
        let b = place(&mut p, "U2", [20.0, 0.0])?;
        assert_eq!(checks(&p).len(), 2);
        apply(
            &mut p,
            SchematicCommand::Connect {
                from: Endpoint {
                    instance: a,
                    terminal: 2,
                },
                to: Endpoint {
                    instance: b,
                    terminal: 2,
                },
            },
        )?;
        assert!(checks(&p).iter().any(|m| m.contains("multiple output")));
        Ok(())
    }

    #[test]
    fn exact_revisions_and_connectivity_survive_engine_history() -> Result<()> {
        let mut engine = Engine::default();
        engine.apply(
            0,
            Operation::Part(parts::PartCommand::Publish {
                draft: parts::passive_template("Resistor"),
            }),
        )?;
        for (reference, position) in [("R1", [0.0, 0.0]), ("R2", [20.0, 0.0]), ("R3", [40.0, 0.0])]
        {
            engine.apply(
                engine.revision(),
                Operation::Schematic(SchematicCommand::Place {
                    part: PartRef { id: 1, revision: 1 },
                    reference: reference.into(),
                    value: "10k".into(),
                    position,
                }),
            )?;
        }
        let endpoints = [
            Endpoint {
                instance: 2,
                terminal: 2,
            },
            Endpoint {
                instance: 3,
                terminal: 1,
            },
            Endpoint {
                instance: 4,
                terminal: 1,
            },
        ];
        for pair in endpoints.windows(2) {
            engine.apply(
                engine.revision(),
                Operation::Schematic(SchematicCommand::Connect {
                    from: pair[0],
                    to: pair[1],
                }),
            )?;
        }
        let connected = nets(engine.project());
        assert_eq!(
            connected
                .iter()
                .filter(|net| net.terminals.len() == 3)
                .count(),
            1
        );
        engine.undo()?;
        assert_eq!(nets(engine.project()).len(), connected.len() + 1);
        engine.redo()?;
        assert_eq!(nets(engine.project()), connected);
        let anchor = pin_position(engine.project(), endpoints[0]);
        let mut newer = engine.project().parts[0].clone();
        newer.symbol.pins[1].position = [15.0, 0.0];
        engine.apply(
            engine.revision(),
            Operation::Part(parts::PartCommand::Publish { draft: newer }),
        )?;
        assert_eq!(engine.project().parts.len(), 2);
        assert_eq!(pin_position(engine.project(), endpoints[0]), anchor);
        assert_eq!(engine.project().instances[0].part.revision, 1);
        assert_eq!(nets(engine.project()), connected);
        let before = engine.project().clone();
        let revision = engine.revision();
        assert!(
            engine
                .apply(
                    revision,
                    Operation::Schematic(SchematicCommand::Place {
                        part: PartRef {
                            id: 1,
                            revision: 99
                        },
                        reference: "R99".into(),
                        value: String::new(),
                        position: [0.0, 0.0],
                    })
                )
                .is_err()
        );
        assert_eq!(engine.project(), &before);
        assert_eq!(engine.revision(), revision);
        Ok(())
    }
}
