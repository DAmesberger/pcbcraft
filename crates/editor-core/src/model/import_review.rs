//! Where an imported design's board and its own schematic disagreed about
//! nets. The schematic's nets were kept; each disagreement stays a finding
//! while the schematic still disagrees with what the board had, so it is
//! never silent. Data and its findings only; an importer records them with
//! `schematic.set_net_disagreements`.
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::ids::{Endpoint, Id};
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Most disagreements a project keeps.
pub const MAX_DISAGREEMENTS: usize = 10_000;
/// Most terminals over all disagreements.
pub const MAX_TERMINALS: usize = 200_000;
/// Most nets or unconnected terminals one finding names.
const MAX_NAMED: usize = 8;

/// The finding of a board that joined nets the schematic keeps apart.
pub const BOARD_JOINS: &str = "import.board_joins_nets";
/// The finding of a net the schematic joins that the board had split.
pub const BOARD_SPLITS: &str = "import.board_splits_net";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DisagreementKind {
    /// One net of the board's joins terminals the schematic keeps on
    /// different nets (or leaves unconnected); the groups are the
    /// schematic's.
    BoardJoins,
    /// One net of the schematic's joins terminals the board had on
    /// different nets; the groups are the board's.
    BoardSplits,
}

/// One disagreement: its kind and the terminals involved, in groups.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NetDisagreement {
    pub kind: DisagreementKind,
    /// At least two groups, none empty.
    pub groups: Vec<Vec<Endpoint>>,
}

/// Bounds of a list of disagreements: at most [`MAX_DISAGREEMENTS`], each
/// of two or more non-empty groups, [`MAX_TERMINALS`] terminals in all.
pub fn check(list: &[NetDisagreement]) -> Result<()> {
    if list.len() > MAX_DISAGREEMENTS {
        return Err(Error::Limit(format!(
            "import.disagreements: at most {MAX_DISAGREEMENTS} net disagreements"
        )));
    }
    let mut terminals = 0usize;
    for d in list {
        if d.groups.len() < 2 || d.groups.iter().any(Vec::is_empty) {
            return Err(Error::Invalid(
                "import.disagreements: a disagreement needs two or more groups of terminals".into(),
            ));
        }
        terminals = terminals.saturating_add(d.groups.iter().map(Vec::len).sum());
    }
    if terminals > MAX_TERMINALS {
        return Err(Error::Limit(format!(
            "import.disagreements: at most {MAX_TERMINALS} terminals"
        )));
    }
    Ok(())
}

/// Where a terminal is in the schematic now: its net, or itself when it
/// is on none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Place {
    Net(Id),
    Alone(Endpoint),
}

fn place(project: &Project, end: Endpoint) -> Place {
    project
        .net_of(end)
        .map_or(Place::Alone(end), |net| Place::Net(net.id))
}

/// "R5 pin 2": a terminal in the user's words.
fn terminal_name(project: &Project, end: Endpoint) -> String {
    let Some(instance) = project.instance(end.instance) else {
        return "a removed part's pin".into();
    };
    let name = project
        .library
        .component(instance.component)
        .and_then(|c| c.definition.terminal(end.terminal))
        .map(|t| t.name.trim().to_owned())
        .filter(|n| !n.is_empty());
    match name {
        Some(name) => format!("{} pin {name}", instance.designator),
        None => format!("{} pin", instance.designator),
    }
}

/// "A, B and 3 more".
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] if names.len() <= 4 => format!("{} and {last}", init.join(", ")),
        _ => format!(
            "{} and {} more",
            names.get(..2).unwrap_or_default().join(", "),
            names.len() - 2
        ),
    }
}

/// The findings of the project's disagreements that still hold: a board
/// join while its groups lie on two or more places (nets, or unconnected
/// terminals) of the schematic, a board split while its terminals all lie
/// on one net. Terminals of removed parts are left out.
pub fn findings(project: &Project) -> Vec<Finding> {
    let mut out = Vec::new();
    for d in &project.net_disagreements {
        let groups: Vec<Vec<Endpoint>> = d
            .groups
            .iter()
            .map(|g| {
                g.iter()
                    .copied()
                    .filter(|e| project.instance(e.instance).is_some())
                    .collect::<Vec<_>>()
            })
            .filter(|g| !g.is_empty())
            .collect();
        if groups.len() < 2 {
            continue;
        }
        let places: BTreeSet<Place> = groups
            .iter()
            .flatten()
            .map(|e| place(project, *e))
            .collect();
        match d.kind {
            DisagreementKind::BoardJoins if places.len() >= 2 => {
                let mut objects = Vec::new();
                let mut names = Vec::new();
                for p in &places {
                    match *p {
                        Place::Net(id) => {
                            if objects.len() < MAX_NAMED {
                                objects.push(ObjectRef::Net { id });
                            }
                            names.push(project.net_name(id).to_owned());
                        }
                        Place::Alone(end) => {
                            if objects.len() < MAX_NAMED {
                                objects.push(ObjectRef::Terminal { endpoint: end });
                            }
                            names.push(format!("unconnected {}", terminal_name(project, end)));
                        }
                    }
                }
                out.push(Finding::new(
                    BOARD_JOINS,
                    format!(
                        "The imported board joined {} on one net; the schematic keeps them apart, and its nets were kept.",
                        listed(&names)
                    ),
                    objects,
                ));
            }
            DisagreementKind::BoardSplits if places.len() == 1 => {
                let Some(Place::Net(id)) = places.first().copied() else {
                    continue;
                };
                out.push(Finding::new(
                    BOARD_SPLITS,
                    format!(
                        "Net {} joins pins the imported board had on {} nets; the schematic's net was kept.",
                        project.net_name(id),
                        groups.len()
                    ),
                    vec![ObjectRef::Net { id }],
                ));
            }
            _ => {}
        }
    }
    out
}
