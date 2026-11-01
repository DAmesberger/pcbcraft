//! Slice C2: the pin queries that need nets.
//!
//! - `project.pin_report { instance }` ([`pin_report`]): per terminal, the
//!   pads, GPIO, active function with peripheral, role and mux code, pin
//!   config, net, and every other endpoint on the net with its active
//!   function, component and Part. It reports facts and never claims
//!   compatibility; matching labels never count.
//! - `functions.suggest { instance, request }` ([`suggest`]): builds the
//!   [`SuggestInput`](crate::functions::suggest::SuggestInput) (with net
//!   hints) and runs the pure search.
use crate::diagnostics::{Error, Result};
use crate::functions::suggest::{SuggestReport, SuggestRequest};
use crate::functions::{PinConfig, Pull, Resolution};
use crate::ids::{Endpoint, Id, ResourceId, RoleId, TerminalId};
use crate::instances;
use crate::library::ComponentDefinition;
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The active function of a terminal, as reports show it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReportFunction {
    pub label: String,
    #[serde(default)]
    pub peripheral: Option<ResourceId>,
    #[serde(default)]
    pub peripheral_label: Option<String>,
    #[serde(default)]
    pub peripheral_kind: Option<String>,
    #[serde(default)]
    pub role: Option<RoleId>,
    #[serde(default)]
    pub role_label: Option<String>,
    #[serde(default)]
    pub signal: Option<String>,
    #[serde(default)]
    pub mux: Option<String>,
    /// Whether someone selected this function on the instance; `false`
    /// when it is active only as the terminal's fixed or default function.
    #[serde(default)]
    pub selected: bool,
}

/// Another endpoint on a terminal's net.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReportEndpoint {
    pub instance: Id,
    pub designator: String,
    pub terminal: TerminalId,
    pub terminal_name: String,
    pub component: String,
    #[serde(default)]
    pub mpn: Option<String>,
    #[serde(default)]
    pub function: Option<ReportFunction>,
}

/// One terminal of the reported instance.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReportPin {
    pub terminal: TerminalId,
    pub name: String,
    pub pads: Vec<String>,
    #[serde(default)]
    pub gpio: Option<crate::functions::GpioRef>,
    #[serde(default)]
    pub function: Option<ReportFunction>,
    #[serde(default)]
    pub config: Option<PinConfig>,
    #[serde(default)]
    pub net: Option<Id>,
    #[serde(default)]
    pub net_name: Option<String>,
    pub endpoints: Vec<ReportEndpoint>,
}

/// `project.pin_report`'s result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinReport {
    pub instance: Id,
    pub designator: String,
    pub pins: Vec<ReportPin>,
}

/// `project.pin_report`: every terminal of `instance` in declaration
/// order with its pads (package terminal designations of the chosen
/// binding), GPIO, active function, pin configuration, net and the other
/// endpoints on that net. Facts only: it never pairs signals by name.
pub fn pin_report(project: &Project, instance: Id) -> Result<PinReport> {
    let item = project
        .instance(instance)
        .ok_or_else(|| Error::NotFound("Unknown component instance.".into()))?;
    let resolved = instances::resolve(&project.library, item)?;
    let component = &resolved.component.definition;
    let resolution = instances::function_resolution(&project.library, item)?;
    let mut others: BTreeMap<Id, Option<Endpoints<'_>>> = BTreeMap::new();
    let mut pins = Vec::with_capacity(component.terminals.len());
    for terminal in &component.terminals {
        let endpoint = Endpoint {
            instance,
            terminal: terminal.id,
        };
        let pads = resolved
            .physical
            .as_ref()
            .map(|p| {
                let mut pads: Vec<String> = Vec::new();
                for map in p
                    .binding
                    .definition
                    .mappings
                    .iter()
                    .filter(|m| m.terminal == terminal.id)
                {
                    let designation = p
                        .package
                        .definition
                        .terminal(map.package_terminal)
                        .map_or_else(
                            || map.package_terminal.to_string(),
                            |t| t.designation.clone(),
                        );
                    if !pads.contains(&designation) {
                        pads.push(designation);
                    }
                }
                pads
            })
            .unwrap_or_default();
        let function = report_function(component, &resolution, terminal.id);
        let net = project.net_of(endpoint);
        let mut endpoints = Vec::new();
        for other in net.into_iter().flat_map(|n| n.terminals.iter()) {
            if *other == endpoint {
                continue;
            }
            let entry = others
                .entry(other.instance)
                .or_insert_with(|| Endpoints::of(project, other.instance));
            let Some(facts) = entry else {
                continue;
            };
            let other_terminal = facts.component.terminal(other.terminal);
            endpoints.push(ReportEndpoint {
                instance: other.instance,
                designator: facts.designator.clone(),
                terminal: other.terminal,
                terminal_name: other_terminal
                    .map_or_else(|| other.terminal.to_string(), |t| t.name.clone()),
                component: facts.name.clone(),
                mpn: facts.mpn.clone(),
                function: facts
                    .resolution
                    .as_ref()
                    .and_then(|r| report_function(facts.component, r, other.terminal)),
            });
        }
        pins.push(ReportPin {
            terminal: terminal.id,
            name: terminal.name.clone(),
            pads,
            gpio: terminal.functions().and_then(|f| f.gpio.clone()),
            function,
            config: item.pin_config.get(&terminal.id).cloned(),
            net: net.map(|n| n.id),
            net_name: net.map(|n| n.name.clone()),
            endpoints,
        });
    }
    Ok(PinReport {
        instance,
        designator: item.designator.clone(),
        pins,
    })
}

/// What a report says about another instance on a net.
struct Endpoints<'a> {
    designator: String,
    name: String,
    mpn: Option<String>,
    component: &'a ComponentDefinition,
    resolution: Option<Resolution>,
}

impl<'a> Endpoints<'a> {
    fn of(project: &'a Project, id: Id) -> Option<Self> {
        let instance = project.instance(id)?;
        let component = project.library.component(instance.component)?;
        Some(Self {
            designator: instance.designator.clone(),
            name: component.meta.name.clone(),
            mpn: instance
                .part
                .and_then(|p| project.library.part(p))
                .map(|p| p.definition.mpn.clone()),
            component: &component.definition,
            resolution: instances::function_resolution(&project.library, instance).ok(),
        })
    }
}

/// The active function of `terminal` with its peripheral, role, signal
/// and mux code, as declared.
fn report_function(
    component: &ComponentDefinition,
    resolution: &Resolution,
    terminal: TerminalId,
) -> Option<ReportFunction> {
    let active = resolution.active(terminal)?;
    let function = component
        .terminal(terminal)
        .and_then(|t| t.functions())
        .and_then(|f| f.function(active.function))?;
    let resource = function
        .resource_role
        .and_then(|rr| component.resources.iter().find(|r| r.id == rr.resource));
    let role = function
        .resource_role
        .and_then(|rr| resource.and_then(|r| r.role(rr.role)));
    Some(ReportFunction {
        label: function.label.clone(),
        peripheral: resource.map(|r| r.id),
        peripheral_label: resource.map(|r| r.label.clone()),
        peripheral_kind: resource.and_then(|r| r.kind.clone()),
        role: role.map(|r| r.id),
        role_label: role.map(|r| r.label.clone()),
        signal: role.and_then(|r| r.signal.clone()),
        mux: function.mux.as_ref().map(|m| m.0.clone()),
        selected: active.origin == crate::functions::ActiveOrigin::Selected,
    })
}

/// `functions.suggest`: the pure search over the instance's current
/// selections, pin configuration and net hints. Never applies anything.
pub fn suggest(project: &Project, instance: Id, request: &SuggestRequest) -> Result<SuggestReport> {
    let item = project
        .instance(instance)
        .ok_or_else(|| Error::NotFound("Unknown component instance.".into()))?;
    instances::pins::suggest_with(project, instance, &item.functions, request)
}

/// The pull a report shows for a terminal: its config, else none.
pub fn pull_of(config: Option<&PinConfig>) -> Option<Pull> {
    config.and_then(|c| c.pull)
}
