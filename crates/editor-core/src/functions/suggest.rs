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
//!
//! A peripheral requested without a role list gets every role that has a
//! candidate: its `required_when_used` roles (or its first role when none
//! is) must be placed, the others are optional and may be left out. A
//! suggestion that leaves an optional role out ranks after every complete
//! one and lists the role in [`Suggestion::unplaced`].
//!
//! One Rhai engine serves the whole search ([`rules::Evaluator`]); each
//! node swaps its view in.
use super::FunctionEdit;
use super::rules::{
    self, MAX_RESOLUTION_OPERATIONS, MAX_RULE_OPERATIONS, MAX_SUGGEST_OPERATIONS, RuleBudget,
    RuleFailureKind, RuleVerdict, RuleView,
};
use crate::diagnostics::{Error, Result};
use crate::ids::{FunctionId, ResourceId, RoleId, RuleId, TerminalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Search nodes one `functions.suggest` may visit (target: < 50 ms on a
/// 176-pin fixture; see the ignored `suggest_benchmark` test for the
/// measurement).
pub const SUGGEST_NODE_BUDGET: u64 = 100_000;
/// Ranked assignments one suggestion returns.
pub const MAX_SUGGESTIONS: usize = 8;
/// Peripherals one request enables.
pub const MAX_PERIPHERAL_REQUESTS: usize = 64;
/// Terminals one request avoids.
pub const MAX_AVOID: usize = 2048;

/// One peripheral to enable; `roles` restricts which roles are placed
/// (a stepwise edit: required roles it leaves out may stay missing), else
/// every role: the `required_when_used` ones (or the first role when none
/// is) must be placed, the others are placed where a candidate allows.
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
    /// Terminals the suggestion places an optional role on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional: Vec<TerminalId>,
    /// Optional roles of the enabled peripherals this suggestion leaves
    /// out; empty for a complete assignment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unplaced: Vec<super::ResourceRole>,
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
    /// The existing selection whose removal unblocks the peripheral, when
    /// the search finds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub culprit: Option<crate::ids::FunctionKey>,
    /// A complete alternative that moves the culprit's role to another
    /// terminal and enables the peripheral, when the search finds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternative: Option<Suggestion>,
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
    /// The role may be left out (at [`SKIP_COST`]).
    optional: bool,
    candidates: Vec<Candidate>,
}

/// The cost of leaving an optional role out: dearer than any placement,
/// so complete assignments rank first.
const SKIP_COST: u32 = 8;
/// Existing selections the blocker analysis tries removing.
const MAX_CULPRIT_TRIES: usize = 16;

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
    /// The slot of each placed candidate.
    slots: Vec<usize>,
    /// Optional slots left out.
    skipped: Vec<usize>,
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
    /// Optional slots of the current branch left out.
    skipped: Vec<usize>,
    /// Slots placed for the current branch, parallel to `chosen`.
    chosen_slots: Vec<usize>,
    found: Vec<Found>,
    evaluator: rules::Evaluator,
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
    let mut evaluator = rules::Evaluator::new();
    let mut spent = Spent::default();
    let mut report = search(input, request, limit, &mut evaluator, &mut spent)?;
    if report.suggestions.is_empty() && !report.exhausted {
        explain_blocker(input, request, &mut evaluator, &mut spent, &mut report)?;
    }
    Ok(report)
}

/// Nodes and rule operations spent so far by one `functions.suggest`,
/// across the main search and the blocker analysis: both share
/// [`SUGGEST_NODE_BUDGET`] and [`MAX_SUGGEST_OPERATIONS`].
#[derive(Clone, Copy, Default)]
struct Spent {
    nodes: u64,
    operations: u64,
}

/// A blocked request: find the existing selection that blocks it (the
/// first, in terminal order, whose removal lets the request succeed) and
/// name it with the rule involved; where the culprit takes a peripheral
/// role, look for a complete alternative that moves that role elsewhere.
/// Bounded by [`MAX_CULPRIT_TRIES`] and the shared budgets.
fn explain_blocker(
    input: &SuggestInput<'_>,
    request: &SuggestRequest,
    evaluator: &mut rules::Evaluator,
    spent: &mut Spent,
    report: &mut SuggestReport,
) -> Result<()> {
    let requested: BTreeSet<ResourceId> = request.enable.iter().map(|w| w.peripheral).collect();
    let declared = |terminal: TerminalId, function: FunctionId| {
        input
            .terminals
            .iter()
            .find(|d| d.terminal == terminal)
            .and_then(|d| {
                d.functions
                    .and_then(|f| f.function(function))
                    .map(|f| (d.name, f))
            })
    };
    let mut tries = 0;
    for (terminal, function) in input.selections {
        let Some((terminal_name, declared)) = declared(*terminal, *function) else {
            continue;
        };
        if declared
            .resource_role
            .is_some_and(|rr| requested.contains(&rr.resource))
        {
            continue;
        }
        if tries == MAX_CULPRIT_TRIES || spent.nodes >= SUGGEST_NODE_BUDGET {
            break;
        }
        tries += 1;
        let mut without = input.selections.clone();
        without.remove(terminal);
        let trial_input = SuggestInput {
            selections: &without,
            ..*input
        };
        let trial = search(&trial_input, request, 1, evaluator, spent)?;
        if trial.suggestions.is_empty() {
            continue;
        }
        let Some(blocker) = report.blockers.first_mut() else {
            return Ok(());
        };
        let role = declared.resource_role.and_then(|rr| {
            let resource = input.resources.iter().find(|r| r.id == rr.resource)?;
            Some((rr, resource, resource.role(rr.role)?))
        });
        let what = role.map_or_else(String::new, |(_, resource, role)| {
            format!(" ({} {})", resource.label, role.label)
        });
        let rule = blocker
            .rule
            .and_then(|r| input.rules.label(r))
            .map_or_else(String::new, |label| format!(" under rule ‘{label}’"));
        blocker.culprit = Some(crate::ids::FunctionKey {
            terminal: *terminal,
            function: *function,
        });
        blocker.message.push_str(&format!(
            " It conflicts with ‘{}’{what} on terminal ‘{}’{rule}; without that selection it can be enabled.",
            declared.label,
            terminal_name.trim()
        ));
        if let Some((rr, resource, _)) = role
            && request.enable.len() < MAX_PERIPHERAL_REQUESTS
            && request.avoid.len() < MAX_AVOID
        {
            // The culprit's role elsewhere first, then the peripheral's
            // other roles in declaration order.
            let roles = std::iter::once(rr.role).chain(
                resource
                    .roles
                    .iter()
                    .map(|r| r.id)
                    .filter(|r| *r != rr.role),
            );
            for alternative_role in roles {
                if spent.nodes >= SUGGEST_NODE_BUDGET {
                    break;
                }
                let mut moved = request.clone();
                moved.enable.push(PeripheralRequest {
                    peripheral: rr.resource,
                    roles: Some(vec![alternative_role]),
                });
                moved.avoid.push(*terminal);
                let alternative = search(&trial_input, &moved, 1, evaluator, spent)?;
                let Some(mut found) = alternative.suggestions.into_iter().next() else {
                    continue;
                };
                let wanted = super::ResourceRole {
                    resource: rr.resource,
                    role: alternative_role,
                };
                let target = found
                    .edits
                    .iter()
                    .filter_map(|e| Some((e.terminal, e.function?)))
                    .find(|(t, f)| declared_role(input, *t, *f) == Some(wanted))
                    .and_then(|(t, _)| input.terminals.iter().find(|d| d.terminal == t))
                    .map(|d| d.name.trim().to_owned());
                found.edits.push(FunctionEdit {
                    terminal: *terminal,
                    function: None,
                });
                found.edits.sort_by_key(|e| e.terminal);
                if !found.changed.contains(terminal) {
                    found.changed.push(*terminal);
                    found.changed.sort();
                }
                let label = resource
                    .role(alternative_role)
                    .map_or_else(|| alternative_role.to_string(), |r| r.label.clone());
                if let Some(target) = target {
                    if alternative_role == rr.role {
                        blocker.message.push_str(&format!(
                            " Alternative: move ‘{}’ ‘{label}’ to terminal ‘{target}’.",
                            resource.label
                        ));
                    } else {
                        blocker.message.push_str(&format!(
                            " Alternative: use ‘{}’ ‘{label}’ on terminal ‘{target}’ instead.",
                            resource.label
                        ));
                    }
                }
                blocker.alternative = Some(found);
                break;
            }
        }
        return Ok(());
    }
    Ok(())
}

/// The peripheral role `function` of `terminal` takes, if any.
fn declared_role(
    input: &SuggestInput<'_>,
    terminal: TerminalId,
    function: FunctionId,
) -> Option<super::ResourceRole> {
    input
        .terminals
        .iter()
        .find(|d| d.terminal == terminal)?
        .functions?
        .function(function)?
        .resource_role
}

/// One search over `input` (the request already checked).
fn search(
    input: &SuggestInput<'_>,
    request: &SuggestRequest,
    limit: usize,
    evaluator: &mut rules::Evaluator,
    spent: &mut Spent,
) -> Result<SuggestReport> {
    let base_resolution = super::resolve(
        input.terminals,
        input.resources,
        input.constraints,
        input.unavailable,
        input.selections,
    )?;
    let mut operations = spent.operations;
    let base =
        check_state(input, &base_resolution, &mut operations, evaluator)?.unwrap_or_default();
    let base_active = base_resolution
        .active
        .iter()
        .map(|a| (a.terminal, a.function))
        .collect();
    let slots = slots(input, request, &base_active);
    let mut report = SuggestReport::default();
    for slot in slots
        .iter()
        .filter(|s| s.candidates.is_empty() && !s.optional)
    {
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
            culprit: None,
            alternative: None,
        });
    }
    if !report.blockers.is_empty() {
        return Ok(report);
    }
    // An optional role nothing can take is simply left out.
    let mut unplaceable = Vec::new();
    let slots: Vec<Slot> = slots
        .into_iter()
        .filter(|s| {
            let keep = !s.candidates.is_empty();
            if !keep {
                unplaceable.push(super::ResourceRole {
                    resource: input.resources[s.resource].id,
                    role: s.role,
                });
            }
            keep
        })
        .collect();
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
        skipped: Vec::new(),
        chosen_slots: Vec::new(),
        found: Vec::new(),
        evaluator: std::mem::take(evaluator),
        nodes: spent.nodes,
        operations,
        exhausted: false,
        reasons: vec![Vec::new(); count],
        dead_end: None,
    };
    let descended = search.descend(0);
    *evaluator = std::mem::take(&mut search.evaluator);
    descended?;
    report.nodes = search.nodes.saturating_sub(spent.nodes);
    spent.nodes = search.nodes;
    spent.operations = search.operations;
    report.exhausted = search.exhausted;
    // Complete assignments first, then by cost. Stable: equal ranks keep
    // the search's declared order.
    search.found.sort_by_key(|f| (f.skipped.len(), f.cost));
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
        let mut optional: Vec<TerminalId> = found
            .assignment
            .iter()
            .zip(&found.slots)
            .filter(|(_, slot)| search.slots[**slot].optional)
            .map(|(c, _)| c.terminal)
            .collect();
        optional.sort();
        let mut unplaced: Vec<super::ResourceRole> = found
            .skipped
            .iter()
            .map(|slot| super::ResourceRole {
                resource: input.resources[search.slots[*slot].resource].id,
                role: search.slots[*slot].role,
            })
            .chain(unplaceable.iter().copied())
            .collect();
        unplaced.sort_by_key(|r| (r.resource, r.role));
        report.suggestions.push(Suggestion {
            edits,
            changed,
            optional,
            unplaced,
        });
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
        // (role, optional)
        let roles: Vec<(RoleId, bool)> = match &wanted.roles {
            Some(roles) => roles.iter().map(|r| (*r, false)).collect(),
            None => {
                let any_required = resource.roles.iter().any(|r| r.required_when_used);
                resource
                    .roles
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let mandatory = if any_required {
                            r.required_when_used
                        } else {
                            i == 0
                        };
                        (r.id, !mandatory)
                    })
                    .collect()
            }
        };
        for (role, optional) in roles {
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
                optional,
                candidates,
            });
        }
    }
    out
}

/// The rule view of one resolution, built as the resolution builds it.
fn view(input: &SuggestInput<'_>, resolution: &super::Resolution) -> RuleView {
    rules::view(input.terminals, input.resources, resolution, input.configs)
}

/// The problems of one resolution, rules included. `None` when the rule
/// budget of the whole search is spent.
fn check_state(
    input: &SuggestInput<'_>,
    resolution: &super::Resolution,
    operations: &mut u64,
    evaluator: &mut rules::Evaluator,
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
    let evaluation = rules::evaluate_with(evaluator, input.rules, view(input, resolution), budget)?;
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

    /// Record the current branch as a complete assignment.
    fn record(&mut self) {
        let cost = self
            .chosen
            .iter()
            .map(|c| c.cost)
            .sum::<u32>()
            .saturating_add(
                SKIP_COST.saturating_mul(u32::try_from(self.skipped.len()).unwrap_or(u32::MAX)),
            );
        self.found.push(Found {
            assignment: self.chosen.clone(),
            slots: self.chosen_slots.clone(),
            skipped: self.skipped.clone(),
            cost,
        });
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
                    self.chosen_slots.push(at);
                    let result = if last {
                        self.record();
                        Ok(())
                    } else {
                        self.descend(at + 1)
                    };
                    self.chosen.pop();
                    self.chosen_slots.pop();
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
        // An optional role may be left out; tried after every placement so
        // complete assignments are found first.
        if self.slots[at].optional && !self.stop() {
            self.skipped.push(at);
            let result = if last {
                if self.nodes >= SUGGEST_NODE_BUDGET {
                    self.exhausted = true;
                    Ok(())
                } else {
                    self.nodes += 1;
                    match self.verdict(true) {
                        Ok(Some(None)) => {
                            passed = true;
                            self.record();
                            Ok(())
                        }
                        Ok(Some(Some(_))) => Ok(()),
                        Ok(None) => {
                            self.exhausted = true;
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
            } else {
                passed = true;
                self.descend(at + 1)
            };
            self.skipped.pop();
            result?;
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
        let Some(items) = check_state(
            self.input,
            &resolution,
            &mut self.operations,
            &mut self.evaluator,
        )?
        else {
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
            culprit: None,
            alternative: None,
        }
    }
}

#[cfg(test)]
mod tests;
