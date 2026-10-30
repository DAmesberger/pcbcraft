//! Net-level electrical rules over active terminal behavior. Each terminal's
//! behavior is its active function from `instances::function_resolution`;
//! an unresolved terminal is reported and excluded from the drive rules, so
//! ERC never assumes all alternatives at once and never picks one.
//!
//! Findings describe circuit state and never block a component stage. A
//! terminal alone on its net is not an ERC condition: completeness reports it
//! once as `instance.terminal_unaccounted`.
use crate::diagnostics::{Finding, ObjectRef};
use crate::functions::{ActiveFunction, Direction, Drive, Resolution};
use crate::ids::{Endpoint, Id};
use crate::instances;
use crate::model::{Net, Project};
use std::collections::BTreeMap;

/// What one net terminal contributes to the net rules.
enum Behavior<'a> {
    Active(&'a ActiveFunction),
    Unresolved,
}

fn designator(project: &Project, endpoint: Endpoint) -> &str {
    project
        .instance(endpoint.instance)
        .map(|i| i.designator.as_str())
        .unwrap_or("?")
}

fn behavior<'a>(
    resolutions: &'a BTreeMap<Id, Resolution>,
    endpoint: Endpoint,
) -> Option<Behavior<'a>> {
    let resolution = resolutions.get(&endpoint.instance)?;
    if let Some(active) = resolution.active(endpoint.terminal) {
        return Some(Behavior::Active(active));
    }
    resolution
        .unresolved
        .contains(&endpoint.terminal)
        .then_some(Behavior::Unresolved)
}

fn net_checks(
    project: &Project,
    net: &Net,
    resolutions: &BTreeMap<Id, Resolution>,
    out: &mut Vec<Finding>,
) {
    let this = ObjectRef::Net { id: net.id };
    let mut active = Vec::new();
    let mut unresolved = Vec::new();
    for endpoint in &net.terminals {
        match behavior(resolutions, *endpoint) {
            Some(Behavior::Active(function)) => active.push((*endpoint, function)),
            Some(Behavior::Unresolved) => unresolved.push(*endpoint),
            None => {}
        }
    }
    let connected = net.terminals.len() >= 2;
    if connected {
        for endpoint in &unresolved {
            out.push(Finding::new(
                "erc.unchecked_terminal",
                format!(
                    "{}: {} terminal {} has no active function and is not checked.",
                    net.name,
                    designator(project, *endpoint),
                    endpoint.terminal
                ),
                vec![
                    this,
                    ObjectRef::Terminal {
                        endpoint: *endpoint,
                    },
                ],
            ));
        }
        let drivers = |drive: Drive| {
            active
                .iter()
                .filter(|(_, f)| {
                    matches!(f.direction, Direction::Output | Direction::Bidirectional)
                        && f.drive == drive
                })
                .count()
        };
        let push_pull = drivers(Drive::PushPull);
        let open_source = drivers(Drive::OpenSource);
        let open_drain = drivers(Drive::OpenDrain);
        if push_pull + open_source >= 2 {
            out.push(Finding::new(
                "erc.driver_conflict",
                format!(
                    "{}: {} push-pull or open-source drivers are connected.",
                    net.name,
                    push_pull + open_source
                ),
                vec![this],
            ));
        }
        if push_pull >= 1 && open_drain >= 1 {
            out.push(Finding::new(
                "erc.open_drain_contention",
                format!(
                    "{}: a push-pull driver shares the net with an open-drain driver.",
                    net.name
                ),
                vec![this],
            ));
        }
        let has = |direction: Direction| active.iter().any(|(_, f)| f.direction == direction);
        if has(Direction::Input)
            && !has(Direction::Output)
            && !has(Direction::Bidirectional)
            && !has(Direction::Passive)
        {
            out.push(Finding::new(
                "erc.input_only_net",
                format!("{}: inputs are connected with no driver.", net.name),
                vec![this],
            ));
        }
    }
    if !active
        .iter()
        .any(|(_, f)| f.direction == Direction::PowerOutput)
    {
        for (endpoint, _) in active
            .iter()
            .filter(|(_, f)| f.direction == Direction::PowerInput)
        {
            out.push(Finding::new(
                "erc.undriven_power_input",
                format!(
                    "{} terminal {} has no power-output driver on its net.",
                    designator(project, *endpoint),
                    endpoint.terminal
                ),
                vec![ObjectRef::Terminal {
                    endpoint: *endpoint,
                }],
            ));
        }
    }
}

/// Every ERC finding of the project, net by net.
pub fn checks(project: &Project) -> Vec<Finding> {
    let resolutions: BTreeMap<Id, Resolution> = project
        .instances
        .iter()
        .filter_map(|i| {
            instances::function_resolution(&project.library, i)
                .ok()
                .map(|r| (i.id, r))
        })
        .collect();
    let mut out = Vec::new();
    for net in &project.nets {
        net_checks(project, net, &resolutions, &mut out);
    }
    out
}
