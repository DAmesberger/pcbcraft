//! Slice C2: instance-side pin mutations.
//! `instance.assign_peripheral` compiles to one `select_functions` batch;
//! `instance.set_pin_config` validates against the terminals' declared
//! options. Both are one undo step and leave the project unchanged on any
//! failure (the engine stages on a candidate).
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::functions::pin_config::{self, MAX_PIN_CONFIG_EDITS, PinConfigEdit};
use crate::functions::suggest::{
    PeripheralRequest, SuggestInput, SuggestReport, SuggestRequest, TerminalHint,
};
use crate::functions::{self, FunctionEdit, FunctionSelections, Resolution};
use crate::ids::{Endpoint, Id, ResourceId, RoleId, TerminalId};
use crate::library::ComponentDefinition;
use crate::model::{ComponentInstance, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One role placement of `instance.assign_peripheral`: `None` lets the
/// solver choose the terminal.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RolePlacement {
    pub role: RoleId,
    pub terminal: Option<TerminalId>,
}

/// The `instance.assign_peripheral` request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AssignPeripheral {
    pub instance: Id,
    pub peripheral: ResourceId,
    #[serde(default)]
    pub roles: Vec<RolePlacement>,
    /// `false` clears every function of the peripheral.
    pub enable: bool,
}

fn not_found() -> Error {
    Error::NotFound("Unknown component instance.".into())
}

/// Net facts the solver ranks by: whether each electrical terminal of
/// `instance` is on a net with another endpoint.
pub fn terminal_hints(
    project: &Project,
    instance: &ComponentInstance,
    component: &ComponentDefinition,
) -> Vec<TerminalHint> {
    component
        .terminals
        .iter()
        .filter(|t| t.is_electrical())
        .map(|t| TerminalHint {
            terminal: t.id,
            connected: project
                .net_of(Endpoint {
                    instance: instance.id,
                    terminal: t.id,
                })
                .is_some_and(|n| n.terminals.len() >= 2),
        })
        .collect()
}

/// Run `functions.suggest` for one instance over `selections` (its own,
/// or a staged batch on top of them). The one place that builds the
/// solver's input from library data.
pub fn suggest_with(
    project: &Project,
    id: Id,
    selections: &FunctionSelections,
    request: &SuggestRequest,
) -> Result<SuggestReport> {
    let instance = project.instance(id).ok_or_else(not_found)?;
    let resolved = super::resolve(&project.library, instance)?;
    let component = &resolved.component.definition;
    let declarations = component.declarations();
    let rules = functions::rules::compile(&component.pin_rules)?;
    let hints = terminal_hints(project, instance, component);
    let input = SuggestInput {
        terminals: &declarations,
        resources: &component.resources,
        constraints: &component.constraints,
        unavailable: resolved
            .physical
            .as_ref()
            .map(|p| p.binding.definition.unavailable.as_slice()),
        rules: &rules,
        selections,
        configs: &instance.pin_config,
        hints: &hints,
    };
    functions::suggest::suggest(&input, request)
}

/// `instance.assign_peripheral`. Enabling places each named role on its
/// given terminal (the terminal's function that takes that role) and lets
/// the solver place the rest, plus every `required_when_used` role not
/// named; with no placements at all the solver places every role that has
/// a candidate. The solver keeps every other explicit selection. An explicit
/// placement of a role of capacity 1 is a move: the role's earlier
/// selected terminal is cleared in the same batch. Disabling
/// clears every explicit selection of the peripheral's functions. Either
/// way the edits are applied as one `select_functions` batch, so the
/// final state is checked once and nothing changes on failure.
pub fn assign_peripheral(project: &mut Project, request: &AssignPeripheral) -> Result<Resolution> {
    let id = request.instance;
    let instance = project.instance(id).ok_or_else(not_found)?;
    let name = instance.designator.clone();
    let resolved = super::resolve(&project.library, instance)?;
    let component = &resolved.component.definition;
    let resource = component
        .resources
        .iter()
        .find(|r| r.id == request.peripheral)
        .ok_or_else(|| {
            Error::NotFound(format!(
                "{name}: the component declares no peripheral {}.",
                request.peripheral
            ))
        })?;
    let takes =
        |f: &functions::Function| f.resource_role.is_some_and(|rr| rr.resource == resource.id);
    if !request.enable {
        if !request.roles.is_empty() {
            return Err(Error::Invalid(format!(
                "{name}: disabling ‘{}’ takes no role placements.",
                resource.label
            )));
        }
        let edits: Vec<FunctionEdit> = instance
            .functions
            .iter()
            .filter(|(terminal, function)| {
                component
                    .terminal(**terminal)
                    .and_then(|t| t.functions())
                    .and_then(|d| d.function(**function))
                    .is_some_and(takes)
            })
            .map(|(terminal, _)| FunctionEdit {
                terminal: *terminal,
                function: None,
            })
            .collect();
        if edits.is_empty() {
            return Err(Error::Invalid(format!(
                "{name}: no terminal has a selected function of ‘{}’.",
                resource.label
            )));
        }
        let resolution = super::select_functions(project, id, &edits)?;
        // A fixed function or design default can still take the
        // peripheral; that is the component's declaration, not a selection.
        return Ok(resolution);
    }
    if request.roles.len() > functions::MAX_ROLES {
        return Err(Error::Invalid(format!(
            "{name}: place at most {} roles at a time.",
            functions::MAX_ROLES
        )));
    }
    let mut explicit: Vec<FunctionEdit> = Vec::new();
    let mut named = BTreeSet::new();
    let mut solve: Vec<RoleId> = Vec::new();
    for placement in &request.roles {
        let role = resource.role(placement.role).ok_or_else(|| {
            Error::NotFound(format!(
                "{name}: ‘{}’ has no role {}.",
                resource.label, placement.role
            ))
        })?;
        if !named.insert(role.id) {
            return Err(Error::Invalid(format!(
                "{name}: role ‘{}’ of ‘{}’ is placed twice.",
                role.label, resource.label
            )));
        }
        let Some(terminal) = placement.terminal else {
            solve.push(role.id);
            continue;
        };
        let declared = component
            .terminal(terminal)
            .ok_or_else(|| Error::NotFound(format!("{name}: no terminal {terminal}.")))?;
        let function = declared
            .functions()
            .and_then(|d| {
                d.functions.iter().find(|f| {
                    f.resource_role
                        .is_some_and(|rr| rr.resource == resource.id && rr.role == role.id)
                })
            })
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "{name}: terminal ‘{}’ has no function that takes role ‘{}’ of ‘{}’.",
                    declared.name, role.label, resource.label
                ))
            })?;
        if explicit.iter().any(|e| e.terminal == terminal) {
            return Err(Error::Invalid(format!(
                "{name}: terminal ‘{}’ is given two roles.",
                declared.name
            )));
        }
        explicit.push(FunctionEdit {
            terminal,
            function: Some(function.id),
        });
    }
    // Move: a placed role of capacity 1 leaves its earlier selected
    // terminal, which returns to its default function.
    let mut moves: Vec<FunctionEdit> = Vec::new();
    for placement in &request.roles {
        let (Some(target), Some(role)) = (placement.terminal, resource.role(placement.role)) else {
            continue;
        };
        if role.capacity != 1 {
            continue;
        }
        for (terminal, function) in &instance.functions {
            let earlier = *terminal != target
                && !explicit.iter().any(|e| e.terminal == *terminal)
                && component
                    .terminal(*terminal)
                    .and_then(|t| t.functions())
                    .and_then(|d| d.function(*function))
                    .and_then(|f| f.resource_role)
                    .is_some_and(|rr| rr.resource == resource.id && rr.role == role.id);
            if earlier && !moves.iter().any(|m| m.terminal == *terminal) {
                moves.push(FunctionEdit {
                    terminal: *terminal,
                    function: None,
                });
            }
        }
    }
    explicit.extend(moves);
    for role in &resource.roles {
        if role.required_when_used && !named.contains(&role.id) {
            solve.push(role.id);
        }
    }
    // No placements: the solver's default, every role that has a
    // candidate (the required ones must be placed).
    let all_roles = request.roles.is_empty();
    let staged = if explicit.is_empty() {
        instance.functions.clone()
    } else {
        functions::apply_edits(&instance.functions, &explicit)?
    };
    let mut edits = explicit;
    if all_roles || !solve.is_empty() {
        let avoid = edits.iter().map(|e| e.terminal).collect();
        let report = suggest_with(
            project,
            id,
            &staged,
            &SuggestRequest {
                enable: vec![PeripheralRequest {
                    peripheral: resource.id,
                    roles: (!all_roles).then_some(solve),
                }],
                keep_existing: true,
                avoid,
                limit: Some(1),
            },
        )?;
        let Some(best) = report.suggestions.into_iter().next() else {
            let messages: Vec<String> = report.blockers.into_iter().map(|b| b.message).collect();
            return Err(Error::Conflict(format!("{name}: {}", messages.join(" "))));
        };
        edits.extend(best.edits);
    }
    if edits.is_empty() {
        // Every role is already where it should be.
        let instance = project.instance(id).ok_or_else(not_found)?;
        return super::function_resolution(&project.library, instance);
    }
    super::select_functions(project, id, &edits)
}

/// `instance.set_pin_config`: set or clear 1–256 terminals' configuration
/// atomically. Each terminal must be electrical, each set value must be
/// offered by the terminal's declared options
/// ([`check_config`](functions::pin_config::check_config)), and an empty
/// configuration clears the entry. Never changes function ERC directly,
/// but pin rules read it through `option(…)`: the final state is checked
/// by `check_instance`, so a `forbid` over an option refuses the edit and
/// a `require` becomes a `config.rule_unmet` finding.
pub fn set_pin_config(project: &mut Project, instance: Id, edits: &[PinConfigEdit]) -> Result<()> {
    if edits.is_empty() || edits.len() > MAX_PIN_CONFIG_EDITS {
        return Err(Error::Invalid(format!(
            "Change between 1 and {MAX_PIN_CONFIG_EDITS} pin configurations at a time."
        )));
    }
    let current = project.instance(instance).ok_or_else(not_found)?;
    let name = current.designator.clone();
    let resolved = super::resolve(&project.library, current)?;
    let component = &resolved.component.definition;
    let mut next = current.pin_config.clone();
    let mut seen = BTreeSet::new();
    for edit in edits {
        if !seen.insert(edit.terminal) {
            return Err(Error::Invalid(format!(
                "{name}: terminal {} is edited twice in one batch.",
                edit.terminal
            )));
        }
        let terminal = component
            .terminal(edit.terminal)
            .ok_or_else(|| Error::NotFound(format!("{name}: no terminal {}.", edit.terminal)))?;
        let Some(declared) = terminal.functions() else {
            return Err(Error::Invalid(format!(
                "{name}: terminal ‘{}’ is device NC or reserved and takes no configuration.",
                terminal.name
            )));
        };
        match &edit.config {
            Some(config) if !pin_config::config_is_empty(config) => {
                pin_config::check_config(&declared.options, config).map_err(|e| {
                    Error::Invalid(format!("{name} terminal ‘{}’: {e}", terminal.name))
                })?;
                next.insert(edit.terminal, config.clone());
            }
            _ => {
                next.remove(&edit.terminal);
            }
        }
    }
    super::instance_mut(project, instance)?.pin_config = next;
    super::check_instance(project, instance)
}

/// Structural invariants of a stored pin configuration, part of
/// `check_instance`: each entry names an electrical terminal and stays
/// within its persistence bounds. Whether its values are still offered
/// is a finding ([`pin_option_findings`]), because a library update may
/// withdraw an option.
pub(crate) fn check_configs(
    name: &str,
    component: &ComponentDefinition,
    configs: &functions::PinConfigs,
) -> Result<()> {
    for (terminal, config) in configs {
        if !super::wirable(component, *terminal) {
            return Err(Error::Invalid(format!(
                "{name}: a pin configuration names terminal {terminal}, which is not an electrical terminal."
            )));
        }
        if !pin_config::config_within_bounds(config) {
            return Err(Error::Limit(format!(
                "{name}: the pin configuration of terminal {terminal} exceeds its text limits."
            )));
        }
    }
    Ok(())
}

/// The configuration an instance keeps in `target`, another revision of
/// its lineage: entries are kept by terminal ID, except on terminals the
/// target removes or declares NC (dropped, as project no-connect markers
/// are). Values the target no longer offers stay and are reported by
/// [`pin_option_findings`], never silently reset.
pub(crate) fn carry_pin_config(
    configs: &functions::PinConfigs,
    target: &ComponentDefinition,
) -> functions::PinConfigs {
    configs
        .iter()
        .filter(|(terminal, _)| super::wirable(target, **terminal))
        .map(|(terminal, config)| (*terminal, config.clone()))
        .collect()
}

/// `config.pin_option` findings of one instance: stored values its pinned
/// component no longer offers (after a library update). Advisory: a pin
/// configuration never changes function ERC (rules may read it).
pub fn pin_option_findings(
    instance: &ComponentInstance,
    component: &ComponentDefinition,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for (terminal, config) in &instance.pin_config {
        let Some(declared) = component.terminal(*terminal) else {
            continue;
        };
        let options = declared
            .functions()
            .map(|f| f.options.as_slice())
            .unwrap_or_default();
        if let Err(e) = pin_config::check_config(options, config) {
            out.push(Finding::new(
                "config.pin_option",
                format!(
                    "{} terminal ‘{}’: its pin configuration is no longer valid: {e}.",
                    instance.designator, declared.name
                ),
                vec![ObjectRef::Terminal {
                    endpoint: Endpoint {
                        instance: instance.id,
                        terminal: *terminal,
                    },
                }],
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests;
