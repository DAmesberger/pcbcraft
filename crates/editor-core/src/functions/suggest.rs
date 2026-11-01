//! Slice C2 (contract C0): `functions.suggest`, a bounded deterministic
//! backtracking search that enables peripherals on one instance.
//!
//! The search assigns each requested role a candidate (terminal, function),
//! checks capacity, `Excludes`/`Requires` and the C1 rule scripts
//! incrementally (rules as bounded predicates under
//! [`MAX_SUGGEST_OPERATIONS`](super::rules::MAX_SUGGEST_OPERATIONS)), and
//! stops at [`SUGGEST_NODE_BUDGET`] nodes. Ranking: no change to existing
//! selections; terminals without a net, or whose net already reaches the
//! role's counterpart; declared order; terminal order. Pure and
//! wasm-safe; never applies anything.
use super::FunctionEdit;
use super::rules::{
    self, MAX_RESOLUTION_OPERATIONS, MAX_RULE_OPERATIONS, MAX_SUGGEST_OPERATIONS, RuleBudget,
    RuleFailureKind, RuleVerdict, RuleView, ViewFunction, ViewResource, ViewTerminal,
};
use crate::diagnostics::{Error, Result};
use crate::ids::{FunctionId, ResourceId, RoleId, RuleId, TerminalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Search nodes one `functions.suggest` may visit (target: < 50 ms on a
/// 176-pin fixture; to be measured).
pub const SUGGEST_NODE_BUDGET: u64 = 100_000;
/// Ranked assignments one suggestion returns.
pub const MAX_SUGGESTIONS: usize = 8;
/// Peripherals one request enables.
pub const MAX_PERIPHERAL_REQUESTS: usize = 64;
/// Terminals one request avoids.
pub const MAX_AVOID: usize = 2048;

/// One peripheral to enable; `roles` restricts which roles are placed
/// (a stepwise edit: required roles it leaves out may stay missing), else
/// every `required_when_used` role, else the first role.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PeripheralRequest {
    pub peripheral: ResourceId,
    #[serde(default)]
    pub roles: Option<Vec<RoleId>>,
}

/// The `functions.suggest` request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SuggestRequest {
    pub enable: Vec<PeripheralRequest>,
    /// Keep every existing explicit selection fixed.
    #[serde(default)]
    pub keep_existing: bool,
    #[serde(default)]
    pub avoid: Vec<TerminalId>,
    /// At most [`MAX_SUGGESTIONS`]; 1 when omitted.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One complete ranked assignment: the `instance.select_functions` batch
/// that applies it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Suggestion {
    pub edits: Vec<FunctionEdit>,
    /// Terminals whose active function changes.
    pub changed: Vec<TerminalId>,
}

/// What stops a peripheral from being enabled.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Blocker {
    pub peripheral: ResourceId,
    pub role: Option<RoleId>,
    /// The rule script that rejects every candidate, if one does.
    pub rule: Option<RuleId>,
    pub message: String,
}

/// The `functions.suggest` report.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SuggestReport {
    pub suggestions: Vec<Suggestion>,
    pub blockers: Vec<Blocker>,
    /// The node budget ran out before the search space did.
    pub exhausted: bool,
    pub nodes: u64,
}

/// Net facts the ranking uses, per terminal, built by `pins::suggest`
/// (functions sees no nets).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalHint {
    pub terminal: TerminalId,
    /// The terminal is on a net with another endpoint.
    pub connected: bool,
}

/// Everything the search reads: the component's declarations as
/// [`resolve`](super::resolve) takes them, the compiled rules, the
/// instance's selections and pin configuration, and the net hints.
pub struct SuggestInput<'a> {
    pub terminals: &'a [super::TerminalDeclaration<'a>],
    pub resources: &'a [super::Resource],
    pub constraints: &'a [super::FunctionConstraint],
    pub unavailable: Option<&'a [crate::ids::FunctionKey]>,
    pub rules: &'a super::rules::CompiledRules,
    pub selections: &'a super::FunctionSelections,
    /// C2 (additive): the instance's pin configuration, which rules read
    /// through `option(…)`.
    pub configs: &'a super::PinConfigs,
    pub hints: &'a [TerminalHint],
}

/// Solutions the search collects before it ranks them.
const MAX_COLLECTED: usize = 64;
/// Reasons one blocker message quotes.
const MAX_REASONS: usize = 3;

/// One role to place.
struct Slot {
    resource: usize,
    role: RoleId,
    candidates: Vec<Candidate>,
}

#[derive(Clone, Copy)]
struct Candidate {
    terminal: TerminalId,
    function: FunctionId,
    /// 0: already active; otherwise a change, dearer on a connected
    /// terminal and dearer still when it overrides an explicit selection.
    cost: u32,
}

/// One problem of a checked state: a conflict, a forbid verdict or a rule
/// failure (hard), or a requirement (soft).
#[derive(Clone, PartialEq)]
struct Item {
    hard: bool,
    rule: Option<RuleId>,
    /// A missing `required_when_used` role.
    missing: Option<(ResourceId, RoleId)>,
    message: String,
}

/// The first reason a slot's candidate was rejected.
#[derive(Clone)]
struct Reason {
    rule: Option<RuleId>,
    message: String,
}

struct Found {
    assignment: Vec<Candidate>,
    cost: u32,
}

struct Search<'s, 'a> {
    input: &'s SuggestInput<'a>,
    slots: Vec<Slot>,
    base: Vec<Item>,
    /// Roles of a requested peripheral that its request leaves out on
    /// purpose: their absence is not a new requirement.
    left_out: BTreeSet<(ResourceId, RoleId)>,
    base_active: BTreeMap<TerminalId, FunctionId>,
    selections: super::FunctionSelections,
    used: BTreeSet<TerminalId>,
    chosen: Vec<Candidate>,
    found: Vec<Found>,
    nodes: u64,
    operations: u64,
    exhausted: bool,
    /// Rejection reasons per slot, and the first slot every candidate of
    /// which was rejected.
    reasons: Vec<Vec<Reason>>,
    dead_end: Option<usize>,
}

/// Search for assignments that enable `request`. Deterministic: the same
/// input always yields the same report. Every returned suggestion is a
/// complete assignment: no conflict, forbid verdict or requirement that
/// the current selections do not already have.
pub fn suggest(input: &SuggestInput<'_>, request: &SuggestRequest) -> Result<SuggestReport> {
    let limit = check_request(input, request)?;
    let base_resolution = super::resolve(
        input.terminals,
        input.resources,
        input.constraints,
        input.unavailable,
        input.selections,
    )?;
    let mut operations = 0;
    let base = check_state(input, &base_resolution, &mut operations)?.unwrap_or_default();
    let base_active = base_resolution
        .active
        .iter()
        .map(|a| (a.terminal, a.function))
        .collect();
    let slots = slots(input, request, &base_active);
    let mut report = SuggestReport::default();
    for slot in slots.iter().filter(|s| s.candidates.is_empty()) {
        let resource = &input.resources[slot.resource];
        let role = resource
            .role(slot.role)
            .map_or_else(|| slot.role.to_string(), |r| format!("‘{}’", r.label));
        report.blockers.push(Blocker {
            peripheral: resource.id,
            role: Some(slot.role),
            rule: None,
            message: format!(
                "‘{}’ cannot be enabled: no terminal can take role {role} (none offers it, or every one that does is avoided, kept for another selection or unavailable in this implementation).",
                resource.label
            ),
        });
    }
    if !report.blockers.is_empty() {
        return Ok(report);
    }
    let count = slots.len();
    let placed: BTreeSet<(ResourceId, RoleId)> = slots
        .iter()
        .map(|s| (input.resources[s.resource].id, s.role))
        .collect();
    let left_out = request
        .enable
        .iter()
        .filter(|w| w.roles.is_some())
        .filter_map(|w| input.resources.iter().find(|r| r.id == w.peripheral))
        .flat_map(|r| r.roles.iter().map(move |role| (r.id, role.id)))
        .filter(|key| !placed.contains(key))
        .collect();
    let mut search = Search {
        input,
        slots,
        base,
        left_out,
        base_active,
        selections: input.selections.clone(),
        used: BTreeSet::new(),
        chosen: Vec::new(),
        found: Vec::new(),
        nodes: 0,
        operations,
        exhausted: false,
        reasons: vec![Vec::new(); count],
        dead_end: None,
    };
    search.descend(0)?;
    report.nodes = search.nodes;
    report.exhausted = search.exhausted;
    // Stable: equal costs keep the search's declared order.
    search.found.sort_by_key(|f| f.cost);
    for found in search.found.iter().take(limit) {
        let mut edits: Vec<FunctionEdit> = found
            .assignment
            .iter()
            .filter(|c| search.base_active.get(&c.terminal) != Some(&c.function))
            .map(|c| FunctionEdit {
                terminal: c.terminal,
                function: Some(c.function),
            })
            .collect();
        edits.sort_by_key(|e| e.terminal);
        let changed = edits.iter().map(|e| e.terminal).collect();
        report.suggestions.push(Suggestion { edits, changed });
    }
    if report.suggestions.is_empty() {
        report.blockers.push(search.blocker());
    }
    Ok(report)
}

fn invalid(message: String) -> Error {
    Error::Invalid(message)
}

/// Validate the request; returns the number of suggestions wanted.
fn check_request(input: &SuggestInput<'_>, request: &SuggestRequest) -> Result<usize> {
    if request.enable.is_empty() || request.enable.len() > MAX_PERIPHERAL_REQUESTS {
        return Err(invalid(format!(
            "Enable between 1 and {MAX_PERIPHERAL_REQUESTS} peripherals at a time."
        )));
    }
    if request.avoid.len() > MAX_AVOID {
        return Err(invalid(format!(
            "Avoid at most {MAX_AVOID} terminals at a time."
        )));
    }
    let limit = request.limit.unwrap_or(1);
    if !(1..=MAX_SUGGESTIONS).contains(&limit) {
        return Err(invalid(format!(
            "Ask for between 1 and {MAX_SUGGESTIONS} suggestions."
        )));
    }
    let mut seen = BTreeSet::new();
    for wanted in &request.enable {
        let resource = input
            .resources
            .iter()
            .find(|r| r.id == wanted.peripheral)
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "The component declares no peripheral {}.",
                    wanted.peripheral
                ))
            })?;
        if !seen.insert(wanted.peripheral) {
            return Err(invalid(format!(
                "Peripheral ‘{}’ is requested twice.",
                resource.label
            )));
        }
        if let Some(roles) = &wanted.roles {
            if roles.is_empty() || roles.len() > super::MAX_ROLES {
                return Err(invalid(format!(
                    "Name between 1 and {} roles of ‘{}’, or omit them.",
                    super::MAX_ROLES,
                    resource.label
                )));
            }
            let mut unique = BTreeSet::new();
            for role in roles {
                if resource.role(*role).is_none() {
                    return Err(Error::NotFound(format!(
                        "Peripheral ‘{}’ has no role {role}.",
                        resource.label
                    )));
                }
                if !unique.insert(*role) {
                    return Err(invalid(format!(
                        "Role {role} of ‘{}’ is named twice.",
                        resource.label
                    )));
                }
            }
        }
    }
    Ok(limit)
}

/// The roles a request places: those it names, else the peripheral's
/// `required_when_used` roles, else its first role. Each with its
/// candidates, cheapest first, then in declaration order.
fn slots(
    input: &SuggestInput<'_>,
    request: &SuggestRequest,
    active: &BTreeMap<TerminalId, FunctionId>,
) -> Vec<Slot> {
    let avoid: BTreeSet<TerminalId> = request.avoid.iter().copied().collect();
    let connected: BTreeSet<TerminalId> = input
        .hints
        .iter()
        .filter(|h| h.connected)
        .map(|h| h.terminal)
        .collect();
    let unavailable: BTreeSet<crate::ids::FunctionKey> =
        input.unavailable.into_iter().flatten().copied().collect();
    let mut out = Vec::new();
    for wanted in &request.enable {
        let Some(index) = input
            .resources
            .iter()
            .position(|r| r.id == wanted.peripheral)
        else {
            continue;
        };
        let resource = &input.resources[index];
        let roles: Vec<RoleId> = match &wanted.roles {
            Some(roles) => roles.clone(),
            None => {
                let required: Vec<RoleId> = resource
                    .roles
                    .iter()
                    .filter(|r| r.required_when_used)
                    .map(|r| r.id)
                    .collect();
                if required.is_empty() {
                    resource.roles.iter().map(|r| r.id).take(1).collect()
                } else {
                    required
                }
            }
        };
        for role in roles {
            let mut candidates = Vec::new();
            for declaration in input.terminals {
                let Some(declared) = declaration.functions else {
                    continue;
                };
                let terminal = declaration.terminal;
                if avoid.contains(&terminal) {
                    continue;
                }
                let explicit = input.selections.get(&terminal);
                for function in &declared.functions {
                    let takes = function
                        .resource_role
                        .is_some_and(|rr| rr.resource == resource.id && rr.role == role);
                    let key = crate::ids::FunctionKey {
                        terminal,
                        function: function.id,
                    };
                    if !takes || unavailable.contains(&key) {
                        continue;
                    }
                    let overrides = explicit.is_some_and(|f| *f != function.id);
                    if request.keep_existing && overrides {
                        continue;
                    }
                    let cost = if active.get(&terminal) == Some(&function.id) {
                        0
                    } else {
                        1 + u32::from(connected.contains(&terminal)) + 4 * u32::from(overrides)
                    };
                    candidates.push(Candidate {
                        terminal,
                        function: function.id,
                        cost,
                    });
                }
            }
            candidates.sort_by_key(|c| c.cost);
            out.push(Slot {
                resource: index,
                role,
                candidates,
            });
        }
    }
    out
}

/// The rule view of one resolution (C1's `RuleView`).
fn view(input: &SuggestInput<'_>, resolution: &super::Resolution) -> RuleView {
    let resource_label = |rr: super::ResourceRole| {
        let resource = input.resources.iter().find(|r| r.id == rr.resource);
        (
            resource.map(|r| r.label.clone()),
            resource
                .and_then(|r| r.role(rr.role))
                .map(|r| r.label.clone()),
        )
    };
    let mut taken: BTreeSet<(ResourceId, RoleId)> = BTreeSet::new();
    let terminals = input
        .terminals
        .iter()
        .map(|declaration| {
            let declared = declaration.functions;
            let active = resolution.active(declaration.terminal).and_then(|a| {
                let function = declared.and_then(|d| d.function(a.function))?;
                let (peripheral, role) = match function.resource_role {
                    Some(rr) => {
                        taken.insert((rr.resource, rr.role));
                        resource_label(rr)
                    }
                    None => (None, None),
                };
                Some(ViewFunction {
                    function: function.id,
                    label: function.label.clone(),
                    peripheral,
                    role,
                })
            });
            ViewTerminal {
                terminal: declaration.terminal,
                name: declaration.name.to_owned(),
                active,
                functions: declared
                    .map(|d| d.functions.iter().map(|f| f.label.clone()).collect())
                    .unwrap_or_default(),
                config: input.configs.get(&declaration.terminal).cloned(),
            }
        })
        .collect();
    let resources = input
        .resources
        .iter()
        .map(|r| ViewResource {
            resource: r.id,
            label: r.label.clone(),
            kind: r.kind.clone(),
            active_roles: r
                .roles
                .iter()
                .filter(|role| taken.contains(&(r.id, role.id)))
                .map(|role| role.label.clone())
                .collect(),
        })
        .collect();
    RuleView {
        terminals,
        resources,
    }
}

/// The problems of one resolution, rules included. `None` when the rule
/// budget of the whole search is spent.
fn check_state(
    input: &SuggestInput<'_>,
    resolution: &super::Resolution,
    operations: &mut u64,
) -> Result<Option<Vec<Item>>> {
    let mut items: Vec<Item> = resolution
        .conflicts
        .iter()
        .map(|c| Item {
            hard: true,
            missing: None,
            rule: match c {
                super::Conflict::Rule { rule, .. } => Some(*rule),
                _ => None,
            },
            message: c.message().to_owned(),
        })
        .chain(resolution.incomplete.iter().map(|r| Item {
            hard: false,
            missing: match r {
                super::Requirement::RoleMissing { resource, role, .. } => Some((*resource, *role)),
                _ => None,
            },
            rule: match r {
                super::Requirement::Rule { rule, .. } => Some(*rule),
                _ => None,
            },
            message: r.message().to_owned(),
        }))
        .collect();
    if input.rules.rules().is_empty() {
        return Ok(Some(items));
    }
    let remaining = MAX_SUGGEST_OPERATIONS.saturating_sub(*operations);
    if remaining == 0 {
        return Ok(None);
    }
    let budget = RuleBudget {
        per_rule: MAX_RULE_OPERATIONS,
        total: remaining.min(MAX_RESOLUTION_OPERATIONS),
    };
    let evaluation = rules::evaluate(input.rules, &view(input, resolution), budget)?;
    *operations = operations.saturating_add(evaluation.operations);
    for outcome in evaluation.outcomes {
        match outcome.verdict {
            RuleVerdict::Ok | RuleVerdict::Warn { .. } => {}
            RuleVerdict::Forbid { message, .. } => items.push(Item {
                hard: true,
                missing: None,
                rule: Some(outcome.rule),
                message,
            }),
            RuleVerdict::Require { message, .. } => items.push(Item {
                hard: false,
                missing: None,
                rule: Some(outcome.rule),
                message,
            }),
        }
    }
    for failure in evaluation.failures {
        if failure.kind == RuleFailureKind::Budget && *operations >= MAX_SUGGEST_OPERATIONS {
            return Ok(None);
        }
        items.push(Item {
            hard: true,
            missing: None,
            rule: Some(failure.rule),
            message: format!("A pin rule could not be evaluated: {}", failure.message),
        });
    }
    Ok(Some(items))
}

impl Search<'_, '_> {
    fn stop(&self) -> bool {
        self.exhausted || self.found.len() >= MAX_COLLECTED
    }

    /// Place slot `at` and everything after it.
    fn descend(&mut self, at: usize) -> Result<()> {
        if at == self.slots.len() {
            return Ok(());
        }
        let last = at + 1 == self.slots.len();
        let mut passed = false;
        let count = self.slots[at].candidates.len();
        for index in 0..count {
            if self.stop() {
                return Ok(());
            }
            let candidate = self.slots[at].candidates[index];
            if self.used.contains(&candidate.terminal) {
                continue;
            }
            if self.nodes >= SUGGEST_NODE_BUDGET {
                self.exhausted = true;
                return Ok(());
            }
            self.nodes += 1;
            let previous = self
                .selections
                .insert(candidate.terminal, candidate.function);
            let verdict = self.verdict(last);
            let outcome = match verdict {
                Ok(Some(None)) => {
                    passed = true;
                    self.used.insert(candidate.terminal);
                    self.chosen.push(candidate);
                    let result = if last {
                        self.found.push(Found {
                            assignment: self.chosen.clone(),
                            cost: self.chosen.iter().map(|c| c.cost).sum(),
                        });
                        Ok(())
                    } else {
                        self.descend(at + 1)
                    };
                    self.chosen.pop();
                    self.used.remove(&candidate.terminal);
                    result
                }
                Ok(Some(Some(reason))) => {
                    if self.reasons[at].len() < MAX_REASONS
                        && !self.reasons[at].iter().any(|r| r.message == reason.message)
                    {
                        self.reasons[at].push(reason);
                    }
                    Ok(())
                }
                Ok(None) => {
                    self.exhausted = true;
                    Ok(())
                }
                Err(e) => Err(e),
            };
            match previous {
                Some(f) => self.selections.insert(candidate.terminal, f),
                None => self.selections.remove(&candidate.terminal),
            };
            outcome?;
        }
        if !passed && !self.exhausted && self.dead_end.is_none() {
            self.dead_end = Some(at);
        }
        Ok(())
    }

    /// Check the current selections: `Some(None)` passes, `Some(Some(r))`
    /// is rejected for `r`, `None` means the rule budget is spent. Partial
    /// states are rejected only for new hard problems (a forbid verdict
    /// is treated as monotone); complete ones also for new requirements.
    fn verdict(&mut self, complete: bool) -> Result<Option<Option<Reason>>> {
        let resolution = super::resolve(
            self.input.terminals,
            self.input.resources,
            self.input.constraints,
            self.input.unavailable,
            &self.selections,
        )?;
        let Some(items) = check_state(self.input, &resolution, &mut self.operations)? else {
            return Ok(None);
        };
        let new = items
            .into_iter()
            .filter(|i| (i.hard || complete) && !self.base.contains(i))
            .filter(|i| i.missing.is_none_or(|m| !self.left_out.contains(&m)))
            // A forbid names its rule, so it explains best.
            .min_by_key(|i| (!i.hard, i.rule.is_none()));
        Ok(Some(new.map(|i| Reason {
            rule: i.rule,
            message: i.message,
        })))
    }

    /// Why nothing was found: the first slot with no surviving candidate,
    /// else the last slot.
    fn blocker(&self) -> Blocker {
        let at = self
            .dead_end
            .unwrap_or_else(|| self.slots.len().saturating_sub(1));
        let slot = self.slots.get(at);
        let resource = slot.and_then(|s| self.input.resources.get(s.resource));
        let label = resource.map_or_else(String::new, |r| r.label.clone());
        let role = slot.and_then(|s| resource.and_then(|r| r.role(s.role)));
        let reasons = self.reasons.get(at).cloned().unwrap_or_default();
        let message = if self.exhausted {
            format!(
                "‘{label}’: the search stopped after {} candidates without a complete assignment; avoid fewer terminals or enable fewer peripherals at once.",
                self.nodes
            )
        } else if reasons.is_empty() {
            format!("‘{label}’ cannot be enabled together with the other requested peripherals.")
        } else {
            let quoted: Vec<&str> = reasons.iter().map(|r| r.message.as_str()).collect();
            format!(
                "‘{label}’ cannot be enabled: every candidate for role ‘{}’ is rejected: {}",
                role.map_or("?", |r| r.label.as_str()),
                quoted.join(" ")
            )
        };
        Blocker {
            peripheral: resource.map_or_else(ResourceId::default, |r| r.id),
            role: slot.map(|s| s.role),
            rule: reasons.iter().find_map(|r| r.rule),
            message,
        }
    }
}

#[cfg(test)]
mod tests;
