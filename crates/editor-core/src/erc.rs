//! Net-level electrical rules. Findings describe circuit state and never
//! block a component stage. A terminal alone on its net is not an ERC
//! condition: completeness reports it once as `instance.terminal_unaccounted`.
use crate::diagnostics::{Finding, ObjectRef};
use crate::ids::Endpoint;
use crate::library::PinKind;
use crate::model::Project;

fn kind(project: &Project, endpoint: Endpoint) -> Option<PinKind> {
    let instance = project.instance(endpoint.instance)?;
    project
        .library
        .component(instance.component)?
        .definition
        .terminal(endpoint.terminal)
        .map(|t| t.kind)
}

/// A deliberately small ERC subset. Drive strength, voltage domains and
/// function-aware rules come with terminal functions.
pub fn checks(project: &Project) -> Vec<Finding> {
    let mut issues = Vec::new();
    for net in &project.nets {
        let pins: Vec<_> = net
            .terminals
            .iter()
            .filter_map(|endpoint| kind(project, *endpoint).map(|kind| (*endpoint, kind)))
            .collect();
        let outputs = pins
            .iter()
            .filter(|(_, kind)| matches!(kind, PinKind::Output | PinKind::PowerOutput))
            .count();
        if outputs > 1 {
            issues.push(Finding::new(
                "erc.multiple_outputs",
                format!("{}: multiple output terminals are connected.", net.name),
                vec![ObjectRef::Net { id: net.id }],
            ));
        }
        for (endpoint, kind) in &pins {
            if *kind == PinKind::PowerInput
                && !pins.iter().any(|(_, kind)| *kind == PinKind::PowerOutput)
            {
                let designator = project
                    .instance(endpoint.instance)
                    .map(|i| i.designator.as_str())
                    .unwrap_or("?");
                issues.push(Finding::new(
                    "erc.undriven_power_input",
                    format!(
                        "{designator} terminal {} has no power-output driver on its net.",
                        endpoint.terminal
                    ),
                    vec![ObjectRef::Terminal {
                        endpoint: *endpoint,
                    }],
                ));
            }
        }
    }
    issues
}
