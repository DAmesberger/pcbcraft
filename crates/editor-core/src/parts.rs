//! Project-local, immutable component snapshots. A draft publishes a new revision;
//! instances continue to refer to the exact revision they already use.
use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum PartCommand {
    /// Replace the component draft being authored. Drafts may be unfinished;
    /// only resource bounds and finite geometry are enforced.
    #[serde(rename = "part.draft_set")]
    SetDraft { draft: Part },
    /// Drop the component draft.
    #[serde(rename = "part.draft_discard")]
    DiscardDraft {},
    /// Publish a draft as an immutable project snapshot. Without `draft`, the
    /// current authoring draft is published. Afterwards the draft is the
    /// published revision, ready for the next edit.
    #[serde(rename = "part.publish")]
    Publish {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        draft: Option<Part>,
    },
}

pub(crate) fn apply(document: &mut Document, command: PartCommand) -> Result<()> {
    match command {
        PartCommand::SetDraft { draft } => {
            validate_draft(&draft)?;
            document.draft = Some(Arc::new(draft));
            Ok(())
        }
        PartCommand::DiscardDraft {} => {
            if document.draft.take().is_none() {
                return Err(Error::NotFound("there is no component draft".into()));
            }
            Ok(())
        }
        PartCommand::Publish { draft } => {
            let draft = match draft {
                Some(draft) => draft,
                None => document
                    .draft
                    .as_deref()
                    .cloned()
                    .ok_or_else(|| Error::NotFound("there is no component draft".into()))?,
            };
            let published = publish(Arc::make_mut(&mut document.project), draft)?;
            document.draft = Some(Arc::new(published));
            Ok(())
        }
    }
}

fn publish(project: &mut Project, mut draft: Part) -> Result<Part> {
    validate_part(&draft)?;
    if project.parts.len() >= 4096 {
        return Err(Error::Limit(
            "project component snapshot limit reached".into(),
        ));
    }
    if draft.id == 0 {
        if draft.revision != 0 {
            return Err(Error::Invalid(
                "a new component draft starts at revision zero".into(),
            ));
        }
        draft.id = project.allocate()?;
        draft.revision = 1;
    } else {
        let latest = project
            .parts
            .iter()
            .filter(|p| p.id == draft.id)
            .max_by_key(|p| p.revision)
            .ok_or_else(|| {
                Error::NotFound("component draft refers to an unknown definition".into())
            })?;
        if latest.revision != draft.revision {
            return Err(Error::Conflict(
                "a newer component revision exists; load it before publishing".into(),
            ));
        }
        draft.revision = latest
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::Limit("component revision exhausted".into()))?;
    }
    project.parts.push(draft.clone());
    Ok(draft)
}

/// Drafts are not published components: empty names, missing mappings, duplicate
/// local IDs and zero/negative dimensions may be legitimate interrupted edits.
/// Only resource bounds and finite geometry are required for safe persistence.
pub fn validate_draft(part: &Part) -> Result<()> {
    if part.name.len() > 256
        || part.provenance.len() > 4096
        || part.component.terminals.len() > 256
        || part.symbol.pins.len() > 256
        || part.package.terminals.len() > 512
        || part.footprint.pads.len() > 1024
        || part.binding.mappings.len() > 512
        || part.component.terminals.iter().any(|t| t.name.len() > 256)
        || part.package.terminals.iter().any(|t| t.number.len() > 256)
    {
        return Err(Error::Limit(
            "authoring draft exceeds supported text or item limits".into(),
        ));
    }
    let finite = |values: &[f64]| values.iter().all(|v| v.is_finite() && v.abs() <= 10000.0);
    if !finite(&part.symbol.body_size)
        || !finite(&part.package.body_size)
        || !finite(&part.package.model_offset)
        || !finite(&part.footprint.datum_offset)
        || part.symbol.pins.iter().any(|p| !finite(&p.position))
        || part
            .footprint
            .pads
            .iter()
            .any(|p| !finite(&p.position) || !finite(&p.size))
    {
        return Err(Error::Invalid(
            "authoring draft geometry must be finite and within ±10000 mm".into(),
        ));
    }
    Ok(())
}

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::Invalid(message.into()))
    }
}
fn text_valid(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn size_valid(size: &[f64]) -> bool {
    size.iter()
        .all(|v| v.is_finite() && (0.01..=10000.0).contains(v))
}

/// Structural validation, not a claim that a component has been checked against
/// its datasheet. Package numbers and terminal names never imply a binding.
pub fn validate_part(part: &Part) -> Result<()> {
    require(
        text_valid(&part.name),
        "component name must contain 1–256 printable bytes",
    )?;
    require(
        part.provenance.len() <= 4096,
        "component provenance is too long",
    )?;
    require(
        (1..=256).contains(&part.component.terminals.len()),
        "component needs 1–256 electrical terminals",
    )?;
    require(
        (1..=512).contains(&part.package.terminals.len()),
        "package needs 1–512 physical terminals",
    )?;
    require(
        (1..=1024).contains(&part.footprint.pads.len()),
        "footprint needs 1–1024 pads",
    )?;
    require(
        part.symbol.pins.len() == part.component.terminals.len(),
        "symbol must have exactly one pin for each electrical terminal",
    )?;
    require(
        part.binding.mappings.len() == part.package.terminals.len(),
        "binding must have exactly one mapping for each package terminal",
    )?;
    require(
        size_valid(&part.symbol.body_size),
        "symbol body dimensions must be finite and positive",
    )?;
    require(
        size_valid(&part.package.body_size),
        "package envelope dimensions must be finite and positive",
    )?;
    require(
        part.package
            .model_offset
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 10000.0),
        "package model offset is invalid",
    )?;
    require(
        point_valid(part.footprint.datum_offset),
        "footprint datum offset is invalid",
    )?;
    let mut electrical = BTreeSet::new();
    for terminal in &part.component.terminals {
        require(
            terminal.id > 0 && electrical.insert(terminal.id),
            "electrical terminal IDs must be unique and nonzero",
        )?;
        require(
            text_valid(&terminal.name),
            "electrical terminal names must be printable and nonempty",
        )?;
    }
    let mut physical = BTreeSet::new();
    let mut numbers = BTreeSet::new();
    for terminal in &part.package.terminals {
        require(
            terminal.id > 0 && physical.insert(terminal.id),
            "package terminal IDs must be unique and nonzero",
        )?;
        require(
            text_valid(&terminal.number) && numbers.insert(&terminal.number),
            "package terminal numbers must be unique and nonempty",
        )?;
    }
    let mut drawn = BTreeSet::new();
    for pin in &part.symbol.pins {
        require(
            electrical.contains(&pin.terminal) && drawn.insert(pin.terminal),
            "each symbol pin must reference a distinct electrical terminal",
        )?;
        require(point_valid(pin.position), "symbol pin position is invalid")?;
    }
    require(
        drawn == electrical,
        "symbol must expose every electrical terminal",
    )?;
    let mut bound_physical = BTreeSet::new();
    let mut bound_electrical = BTreeSet::new();
    for mapping in &part.binding.mappings {
        require(
            electrical.contains(&mapping.electrical),
            "binding refers to an unknown electrical terminal",
        )?;
        require(
            physical.contains(&mapping.physical) && bound_physical.insert(mapping.physical),
            "each physical terminal must have exactly one explicit electrical mapping",
        )?;
        bound_electrical.insert(mapping.electrical);
    }
    require(
        bound_physical == physical && bound_electrical == electrical,
        "binding must cover every electrical and physical terminal",
    )?;
    let mut pads = BTreeSet::new();
    let mut landed = BTreeSet::new();
    for pad in &part.footprint.pads {
        require(
            pad.id > 0 && pads.insert(pad.id),
            "pad IDs must be unique and nonzero",
        )?;
        require(
            physical.contains(&pad.terminal),
            "pad refers to an unknown package terminal",
        )?;
        require(
            point_valid(pad.position) && size_valid(&pad.size),
            "pad dimensions and position must be finite and valid",
        )?;
        landed.insert(pad.terminal);
    }
    require(
        landed == physical,
        "every package terminal needs a footprint pad",
    )
}

pub fn validate(project: &Project) -> Result<()> {
    require(project.parts.len() <= 4096, "too many component snapshots")?;
    let mut revisions = BTreeSet::new();
    for part in &project.parts {
        require(
            part.id > 0 && part.revision > 0 && revisions.insert((part.id, part.revision)),
            "published component identities and revisions must be unique and nonzero",
        )?;
        validate_part(part)?;
    }
    Ok(())
}

/// An original, unverified generic two-terminal envelope, ready for local editing.
/// IDs inside a snapshot belong to their own definition's scope.
pub fn passive_template(name: &str) -> Part {
    Part {
        id: 0,
        revision: 0,
        name: name.into(),
        component: Component {
            terminals: vec![
                Terminal {
                    id: 1,
                    name: "1".into(),
                    kind: PinKind::Passive,
                },
                Terminal {
                    id: 2,
                    name: "2".into(),
                    kind: PinKind::Passive,
                },
            ],
        },
        symbol: Symbol {
            body_size: [6.0, 3.0],
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
            body_size: [2.0, 1.2, 0.6],
            model_offset: [0.0, 0.0, 0.05],
        },
        footprint: Footprint {
            pads: vec![
                Pad {
                    id: 1,
                    terminal: 1,
                    position: [-1.2, 0.0],
                    size: [1.0, 1.4],
                },
                Pad {
                    id: 2,
                    terminal: 2,
                    position: [1.2, 0.0],
                    size: [1.0, 1.4],
                },
            ],
            datum_offset: [0.0, 0.0],
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
        provenance:
            "Locally authored generic template; verify against the selected component datasheet."
                .into(),
    }
}

pub fn connector_template(name: &str) -> Part {
    let mut part = passive_template(name);
    part.symbol.body_size = [4.0, 6.0];
    part.symbol.pins = vec![
        SymbolPin {
            terminal: 1,
            position: [-5.0, 1.5],
        },
        SymbolPin {
            terminal: 2,
            position: [-5.0, -1.5],
        },
    ];
    part.package.body_size = [5.0, 3.5, 4.0];
    part.footprint.pads = vec![
        Pad {
            id: 1,
            terminal: 1,
            position: [-1.27, 0.0],
            size: [1.5, 2.0],
        },
        Pad {
            id: 2,
            terminal: 2,
            position: [1.27, 0.0],
            size: [1.5, 2.0],
        },
    ];
    part
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_or_ambiguous_mapping_is_rejected_before_allocation() {
        let mut project = Project::default();
        let original = project.clone();
        let mut missing = passive_template("R");
        missing.binding.mappings.pop();
        assert!(publish(&mut project, missing).map(|_| ()).is_err());
        let mut duplicate = passive_template("R");
        duplicate.binding.mappings.push(TerminalMap {
            electrical: 2,
            physical: 1,
        });
        assert!(publish(&mut project, duplicate).map(|_| ()).is_err());
        assert_eq!(project, original);
    }

    #[test]
    fn new_revision_does_not_mutate_placed_snapshot() -> Result<()> {
        let mut project = Project::default();
        publish(&mut project, passive_template("R")).map(|_| ())?;
        let original = project.parts[0].clone();
        project.instances.push(Instance {
            id: 2,
            part: PartRef {
                id: original.id,
                revision: 1,
            },
            reference: "R1".into(),
            value: "10k".into(),
            position: [0.0, 0.0],
        });
        let mut draft = original.clone();
        draft.name = "Renamed resistor".into();
        draft.footprint.pads[0].position[0] -= 0.4;
        publish(&mut project, draft).map(|_| ())?;
        assert_eq!(project.parts[0], original);
        assert_eq!(project.parts[1].revision, 2);
        assert_eq!(project.instance(2).map(|i| i.part.revision), Some(1));
        assert_eq!(
            project.part(PartRef {
                id: original.id,
                revision: 1
            }),
            Some(&original)
        );
        Ok(())
    }

    #[test]
    fn stale_draft_cannot_publish_over_a_newer_revision() -> Result<()> {
        let mut project = Project::default();
        publish(&mut project, passive_template("R")).map(|_| ())?;
        let draft = project.parts[0].clone();
        publish(&mut project, draft.clone()).map(|_| ())?;
        let before = project.clone();
        assert!(publish(&mut project, draft).map(|_| ()).is_err());
        assert_eq!(project, before);
        Ok(())
    }

    #[test]
    fn one_electrical_terminal_may_bind_multiple_physical_terminals_and_pads() -> Result<()> {
        let mut part = passive_template("Supply test");
        part.package.terminals.push(PackageTerminal {
            id: 3,
            number: "EP".into(),
        });
        part.binding.mappings.push(TerminalMap {
            electrical: 1,
            physical: 3,
        });
        part.footprint.pads.push(Pad {
            id: 3,
            terminal: 3,
            position: [0.0, 0.0],
            size: [0.6, 0.8],
        });
        part.footprint.pads.push(Pad {
            id: 4,
            terminal: 3,
            position: [0.0, 1.0],
            size: [0.4, 0.4],
        });
        validate_part(&part)
    }

    #[test]
    fn malformed_geometry_and_symbol_references_are_rejected() {
        let mut part = passive_template("R");
        part.footprint.pads[0].size[0] = f64::NAN;
        assert!(validate_part(&part).is_err());
        let mut part = passive_template("R");
        part.symbol.pins[0].terminal = 99;
        assert!(validate_part(&part).is_err());
    }
}
