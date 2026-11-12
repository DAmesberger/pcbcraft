//! The pin-mux panel's data, built from the pinned component revision and
//! the instance's selections: peripherals with their roles and candidate
//! pins, the pins, the package pads, and a preview of staged selections
//! through the core's one resolution rule (`instances::function_resolution`,
//! which also runs the component's pin rules). Pure: no egui, no mutation.
//!
//! Candidates come only from declared `resource_role` data. A function's
//! label, a peripheral's kind or a net name never makes a terminal a
//! candidate (AGENTS.md: never infer electrical compatibility from names).
use crate::kit::severity::Level;
use crate::refusal;
use pcbcraft_core::functions::{
    Conflict, FunctionEdit, MAX_FUNCTION_EDITS, Requirement, Resolution, ResourceRole, apply_edits,
};
use pcbcraft_core::ids::{FunctionId, ResourceId, RoleId, RuleId, TerminalId};
use pcbcraft_core::instances;
use pcbcraft_core::issues::MAX_TITLE;
use pcbcraft_core::{ComponentInstance, Endpoint, Project, Result};
use std::collections::BTreeMap;

/// Explicit selections the panel has staged and not applied: `None` clears
/// a terminal's selection (back to its fixed function or design default).
pub type Staged = BTreeMap<TerminalId, Option<FunctionId>>;

/// One function of a terminal, as the panel shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionView {
    pub id: FunctionId,
    pub label: String,
    /// The mux table and code as people read them ("LP IO MUX F0").
    pub mux: Option<String>,
    pub role: Option<ResourceRole>,
}

/// One electrical terminal with functions.
#[derive(Clone, Debug, PartialEq)]
pub struct Pin {
    pub terminal: TerminalId,
    pub name: String,
    pub functions: Vec<FunctionView>,
    /// The function used without an explicit selection, if any.
    pub implicit: Option<FunctionId>,
}

impl Pin {
    pub fn function(&self, id: FunctionId) -> Option<&FunctionView> {
        self.functions.iter().find(|f| f.id == id)
    }
    /// More than one function to choose from.
    pub fn multi(&self) -> bool {
        self.functions.len() > 1
    }
}

/// A terminal function that can take a role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub terminal: TerminalId,
    pub function: FunctionId,
}

/// One role of a peripheral and the functions that declare it.
#[derive(Clone, Debug, PartialEq)]
pub struct RoleView {
    pub id: RoleId,
    pub label: String,
    pub required: bool,
    pub candidates: Vec<Candidate>,
}

/// One peripheral (resource) of the component.
#[derive(Clone, Debug, PartialEq)]
pub struct Peripheral {
    pub id: ResourceId,
    pub label: String,
    pub kind: Option<String>,
    pub roles: Vec<RoleView>,
}

/// One pad of the chosen footprint, in footprint millimetres.
#[derive(Clone, Debug, PartialEq)]
pub struct PadView {
    pub number: String,
    pub center: [f64; 2],
    /// The axis-aligned size of the rotated pad.
    pub size: [f64; 2],
    pub round: bool,
    pub terminal: Option<TerminalId>,
}

/// Everything the panel shows that does not depend on staged edits.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub designator: String,
    pub component: String,
    pub peripherals: Vec<Peripheral>,
    pub pins: Vec<Pin>,
    /// The resolution of the instance as the document holds it.
    pub current: Resolution,
    pub rules: BTreeMap<RuleId, String>,
    /// Pads of the chosen footprint; empty without a physical choice.
    pub pads: Vec<PadView>,
    pub physical: bool,
}

impl Model {
    pub fn pin(&self, terminal: TerminalId) -> Option<&Pin> {
        self.pins.iter().find(|p| p.terminal == terminal)
    }
    /// The position of a peripheral in declaration order, for its tint.
    pub fn tint_index(&self, id: ResourceId) -> Option<usize> {
        self.peripherals.iter().position(|p| p.id == id)
    }
    /// The peripheral role an active function takes, if any.
    pub fn role_of(&self, terminal: TerminalId, function: FunctionId) -> Option<ResourceRole> {
        self.pin(terminal)?.function(function)?.role
    }
    /// Pad numbers that land `terminal`.
    pub fn pads_of(&self, terminal: TerminalId) -> Vec<&str> {
        self.pads
            .iter()
            .filter(|p| p.terminal == Some(terminal))
            .map(|p| p.number.as_str())
            .collect()
    }
}

/// Build the model of `instance`, or the reason it has none.
pub fn build(project: &Project, instance: &ComponentInstance) -> Result<Model> {
    let resolved = instances::resolve(&project.library, instance)?;
    let component = &resolved.component.definition;
    let pins: Vec<Pin> = component
        .terminals
        .iter()
        .filter_map(|t| {
            let declared = t.functions()?;
            Some(Pin {
                terminal: t.id,
                name: t.name.clone(),
                functions: declared
                    .functions
                    .iter()
                    .map(|f| FunctionView {
                        id: f.id,
                        label: f.label.clone(),
                        mux: f.mux_text(),
                        role: f.resource_role,
                    })
                    .collect(),
                implicit: declared.implicit().map(|(f, _)| f.id),
            })
        })
        .collect();
    let peripherals = component
        .resources
        .iter()
        .map(|r| Peripheral {
            id: r.id,
            label: r.label.clone(),
            kind: r.kind.clone(),
            roles: r
                .roles
                .iter()
                .map(|role| RoleView {
                    id: role.id,
                    label: role.label.clone(),
                    required: role.required_when_used,
                    candidates: pins
                        .iter()
                        .flat_map(|p| {
                            p.functions
                                .iter()
                                .filter(|f| {
                                    f.role
                                        == Some(ResourceRole {
                                            resource: r.id,
                                            role: role.id,
                                        })
                                })
                                .map(|f| Candidate {
                                    terminal: p.terminal,
                                    function: f.id,
                                })
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();
    let pads = resolved.physical.map_or_else(Vec::new, |physical| {
        physical
            .footprint
            .definition
            .pads
            .iter()
            .map(|pad| {
                let extent = pad.shape.extent();
                let (sin, cos) = pad.rotation_deg().to_radians().sin_cos();
                let size = [
                    (extent[0] * cos).abs() + (extent[1] * sin).abs(),
                    (extent[0] * sin).abs() + (extent[1] * cos).abs(),
                ];
                PadView {
                    number: pad.number.clone(),
                    center: pad.position(),
                    size,
                    round: matches!(
                        pad.shape,
                        pcbcraft_core::library::PadShape::Circle { .. }
                            | pcbcraft_core::library::PadShape::Oblong { .. }
                    ),
                    terminal: pad
                        .terminal
                        .and_then(|pt| physical.binding.definition.terminal_of(pt)),
                }
            })
            .collect()
    });
    Ok(Model {
        designator: instance.designator.clone(),
        component: resolved.component.meta.name.clone(),
        peripherals,
        pins,
        current: instances::function_resolution(&project.library, instance)?,
        rules: component
            .pin_rules
            .iter()
            .map(|r| (r.id, r.label.clone()))
            .collect(),
        physical: resolved.physical.is_some(),
        pads,
    })
}

/// The `instance.select_functions` edits that turn the document's
/// selections into the staged ones: entries equal to the document's are
/// left out, in terminal order.
pub fn edits(instance: &ComponentInstance, staged: &Staged) -> Vec<FunctionEdit> {
    staged
        .iter()
        .filter(|(terminal, function)| instance.functions.get(terminal).copied() != **function)
        .map(|(terminal, function)| FunctionEdit {
            terminal: *terminal,
            function: *function,
        })
        .collect()
}

/// Drop staged entries the document already holds (after an Apply, an
/// undo or another edit).
pub fn settle(instance: &ComponentInstance, staged: &mut Staged) {
    staged.retain(|terminal, function| instance.functions.get(terminal).copied() != *function);
}

/// One explanation line of the preview.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub level: Level,
    /// One line, at most [`MAX_TITLE`] characters, naming objects.
    pub text: String,
    pub terminals: Vec<TerminalId>,
}

/// The document after the staged edits, as the core resolves it.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub edits: Vec<FunctionEdit>,
    pub resolution: Resolution,
    /// Terminals whose active function differs from the document's.
    pub changed: Vec<TerminalId>,
    /// Rejections first (they gate Apply), then unmet requirements, then
    /// rule notes.
    pub lines: Vec<Line>,
}

impl Preview {
    /// Why Apply is unavailable, or `None` when it may run.
    pub fn gate(&self) -> Option<&'static str> {
        if self.edits.is_empty() {
            Some("Nothing to apply: choose peripherals or pins first")
        } else if self.edits.len() > MAX_FUNCTION_EDITS {
            Some("Apply at most 256 pin changes at a time")
        } else if !self.resolution.conflicts.is_empty() {
            Some("Resolve the rejected combination above to apply")
        } else {
            None
        }
    }

    /// The role placements of `peripheral` that are active, in terminal
    /// order.
    pub fn placements(&self, model: &Model, peripheral: ResourceId) -> Vec<(RoleId, TerminalId)> {
        self.resolution
            .active
            .iter()
            .filter_map(|a| {
                let role = model.role_of(a.terminal, a.function)?;
                (role.resource == peripheral).then_some((role.role, a.terminal))
            })
            .collect()
    }

    /// The active peripheral role of `terminal`, if any.
    pub fn role(&self, model: &Model, terminal: TerminalId) -> Option<ResourceRole> {
        let active = self.resolution.active(terminal)?;
        model.role_of(terminal, active.function)
    }
}

/// Resolve the staged selections. A structural refusal (a function that
/// no longer exists) is the error.
pub fn preview(
    project: &Project,
    instance: &ComponentInstance,
    model: &Model,
    staged: &Staged,
) -> Result<Preview> {
    let edits = edits(instance, staged);
    let resolution = if edits.is_empty() || edits.len() > MAX_FUNCTION_EDITS {
        model.current.clone()
    } else {
        let mut scratch = instance.clone();
        scratch.functions = apply_edits(&instance.functions, &edits)?;
        instances::function_resolution(&project.library, &scratch)?
    };
    let active = |r: &Resolution, t: TerminalId| r.active(t).map(|a| a.function);
    let changed = model
        .pins
        .iter()
        .map(|p| p.terminal)
        .filter(|t| active(&model.current, *t) != active(&resolution, *t))
        .collect();
    let lines = explain(model, &resolution);
    Ok(Preview {
        edits,
        resolution,
        changed,
        lines,
    })
}

/// The preview's explanation lines, worded by the core: a rule script's
/// verdict is prefixed with the rule's label.
pub fn explain(model: &Model, resolution: &Resolution) -> Vec<Line> {
    let ruled = |rule: Option<RuleId>, message: &str| {
        let text = match rule.and_then(|r| model.rules.get(&r)) {
            Some(label) => format!("{label}: {message}"),
            None => message.to_owned(),
        };
        refusal::clip(&text, MAX_TITLE)
    };
    let mut lines = Vec::new();
    for conflict in &resolution.conflicts {
        let (rule, terminals) = match conflict {
            Conflict::Rule {
                rule, terminals, ..
            } => (Some(*rule), terminals.clone()),
            Conflict::Capacity { functions, .. } => {
                (None, functions.iter().map(|k| k.terminal).collect())
            }
            Conflict::Excluded { a, b, .. } => (None, vec![a.terminal, b.terminal]),
            Conflict::Unavailable { function, .. } => (None, vec![function.terminal]),
        };
        lines.push(Line {
            level: Level::Error,
            text: ruled(rule, conflict.message()),
            terminals,
        });
    }
    for requirement in &resolution.incomplete {
        let rule = match requirement {
            Requirement::Rule { rule, .. } => Some(*rule),
            _ => None,
        };
        lines.push(Line {
            level: Level::Warning,
            text: ruled(rule, requirement.message()),
            terminals: requirement.terminals(),
        });
    }
    for note in &resolution.notes {
        lines.push(Line {
            level: Level::Info,
            text: ruled(Some(note.rule), &note.message),
            terminals: note.terminals.clone(),
        });
    }
    lines
}

/// The staged change that frees `terminal` of its peripheral function:
/// clear its selection when what it falls back to takes no role of
/// `peripheral`, else select its first function that takes none. `None`
/// when the terminal cannot leave the peripheral.
fn release(pin: &Pin, peripheral: ResourceId) -> Option<Option<FunctionId>> {
    let outside = |f: &FunctionView| f.role.is_none_or(|r| r.resource != peripheral);
    match pin.implicit.and_then(|id| pin.function(id)) {
        Some(f) if !outside(f) => pin
            .functions
            .iter()
            .find(|f| outside(f))
            .map(|f| Some(f.id)),
        // Without an implicit function the terminal becomes unresolved,
        // which completeness reports; still nothing of the peripheral.
        _ => Some(None),
    }
}

/// Stage turning `peripheral` off: every terminal that takes one of its
/// roles in the preview is released. Returns the terminals that cannot
/// leave it (a fixed function of that peripheral).
pub fn disable(
    model: &Model,
    preview: &Preview,
    peripheral: ResourceId,
    staged: &mut Staged,
) -> Vec<TerminalId> {
    let mut stuck = Vec::new();
    for (_, terminal) in preview.placements(model, peripheral) {
        let released = model.pin(terminal).and_then(|p| release(p, peripheral));
        match released {
            Some(function) => {
                staged.insert(terminal, function);
            }
            None => stuck.push(terminal),
        }
    }
    stuck
}

/// Stage placing `role` of `peripheral` on `target` (or nowhere): the
/// terminals that take the role now are released first.
pub fn place(
    model: &Model,
    preview: &Preview,
    peripheral: ResourceId,
    role: RoleId,
    target: Option<&Candidate>,
    staged: &mut Staged,
) {
    for (placed, terminal) in preview.placements(model, peripheral) {
        if placed != role || target.is_some_and(|c| c.terminal == terminal) {
            continue;
        }
        if let Some(function) = model.pin(terminal).and_then(|p| release(p, peripheral)) {
            staged.insert(terminal, function);
        }
    }
    if let Some(candidate) = target {
        staged.insert(candidate.terminal, Some(candidate.function));
    }
}

/// Stage a suggestion's edits over `base`.
pub fn stage_suggestion(base: &Staged, edits: &[FunctionEdit]) -> Staged {
    let mut staged = base.clone();
    for edit in edits {
        staged.insert(edit.terminal, edit.function);
    }
    staged
}

/// "PA9 · usart1_tx · AF7": a candidate's label in a role's choice.
pub fn candidate_label(model: &Model, candidate: &Candidate) -> String {
    let Some(pin) = model.pin(candidate.terminal) else {
        return String::new();
    };
    match pin.function(candidate.function) {
        Some(f) => match &f.mux {
            Some(mux) => format!("{} · {} · {mux}", pin.name, f.label),
            None => format!("{} · {}", pin.name, f.label),
        },
        None => pin.name.clone(),
    }
}

/// The net of `terminal` and the other endpoints on it ("UART_TX → U3.RXD,
/// R5.1"), at most `limit` named and the rest counted. Facts only: no
/// endpoint is judged compatible.
pub fn net_line(
    project: &Project,
    instance: &ComponentInstance,
    terminal: TerminalId,
    limit: usize,
) -> Option<String> {
    let net = project.net_of(Endpoint {
        instance: instance.id,
        terminal,
    })?;
    let others: Vec<String> = net
        .terminals
        .iter()
        .filter(|e| !(e.instance == instance.id && e.terminal == terminal))
        .map(|e| {
            let Some(other) = project.instance(e.instance) else {
                return "?".to_owned();
            };
            let name = instances::resolve(&project.library, other)
                .ok()
                .and_then(|r| {
                    r.component
                        .definition
                        .terminals
                        .iter()
                        .find(|t| t.id == e.terminal)
                        .map(|t| t.name.clone())
                })
                .unwrap_or_else(|| e.terminal.to_string());
            format!("{}.{name}", other.designator)
        })
        .collect();
    if others.is_empty() {
        return Some(net.name.clone());
    }
    let shown: Vec<&str> = others.iter().take(limit).map(String::as_str).collect();
    let more = others.len().saturating_sub(shown.len());
    let tail = if more > 0 {
        format!(" +{more}")
    } else {
        String::new()
    };
    Some(format!("{} → {}{tail}", net.name, shown.join(", ")))
}
