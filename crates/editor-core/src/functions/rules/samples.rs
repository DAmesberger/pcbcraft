//! Rules evaluated over sample assignments, so a script that only fails
//! on a branch the default assignment never takes (an unknown helper, a
//! missing `else`, a typo in a computed name) is found when the rule is
//! written, not when a designer first selects that function.
//!
//! The samples of one rule are the default assignment, one assignment per
//! function, peripheral or role the script names in a literal helper call
//! (that function made active on its pin; for a peripheral or role, the
//! first function that takes it), and all of those together. Every sample
//! is the default view with the named functions swapped in, so a sample
//! is never refused for conflicts: rules see it as they would any
//! assignment. [`MAX_RULE_SAMPLES`] bounds the work per rule.
use super::{
    CompiledRule, CompiledRules, Detailed, Evaluator, RuleBudget, RuleFailureKind, RuleVerdict,
    RuleView, ViewFunction, evaluate_in, scan,
};
use crate::functions::{Resource, TerminalDeclaration};
use crate::ids::{FunctionId, RuleId, TerminalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Sample assignments one rule is evaluated over, the default included.
pub const MAX_RULE_SAMPLES: usize = 34;

/// One rule's result over its samples (`library.pinout_preview`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleCheck {
    pub label: String,
    /// The verdict without explicit selections, e.g. `ok` or `warn: …`,
    /// or `failed: …`.
    pub default: String,
    /// Assignments the rule was evaluated over, the default included.
    pub samples: u32,
    /// Distinct verdicts over the samples (`ok`, `forbid`, `require`,
    /// `warn`), so an author sees whether the rule ever fires.
    pub verdicts: Vec<String>,
    /// Each sample the rule failed in: the sample and the error.
    pub failures: Vec<String>,
}

/// One sample's outcome for one rule.
pub(super) struct SampleFailure {
    pub sample: String,
    pub kind: RuleFailureKind,
    pub reference: bool,
    pub message: String,
}

/// A function made active in a sample.
#[derive(Clone)]
struct Activation {
    terminal: TerminalId,
    function: ViewFunction,
    /// "IO36.gpio_out" for messages.
    name: String,
}

fn fold(text: &str) -> String {
    text.trim().to_lowercase()
}

/// The view function of `terminal`'s function `id`.
fn view_function(
    declaration: &TerminalDeclaration<'_>,
    id: FunctionId,
    resources: &[Resource],
) -> Option<Activation> {
    let f = declaration.functions?.function(id)?;
    let resource = f
        .resource_role
        .and_then(|rr| resources.iter().find(|r| r.id == rr.resource));
    let role = f
        .resource_role
        .zip(resource)
        .and_then(|(rr, r)| r.role(rr.role));
    Some(Activation {
        terminal: declaration.terminal,
        function: ViewFunction {
            function: f.id,
            label: f.label.clone(),
            peripheral: resource.map(|r| r.label.clone()),
            role: role.map(|r| r.label.clone()),
        },
        name: format!("{}.{}", declaration.name.trim(), f.label),
    })
}

/// Terminals with this exact (trimmed) name: the rule helpers' lookup.
fn named<'a, 'b>(
    terminals: &'a [TerminalDeclaration<'b>],
    name: &str,
) -> Option<&'a TerminalDeclaration<'b>> {
    let mut found = terminals.iter().filter(|t| t.name.trim() == name.trim());
    match (found.next(), found.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

fn by_label(
    terminals: &[TerminalDeclaration<'_>],
    resources: &[Resource],
    terminal: &str,
    function: &str,
) -> Option<Activation> {
    let declaration = named(terminals, terminal)?;
    let wanted = fold(function);
    let f = declaration
        .functions?
        .functions
        .iter()
        .find(|f| fold(&f.label) == wanted)?;
    view_function(declaration, f.id, resources)
}

/// `"TERM.function"`, trying each dot from the right like the helpers.
fn by_ref(
    terminals: &[TerminalDeclaration<'_>],
    resources: &[Resource],
    text: &str,
) -> Option<Activation> {
    text.rmatch_indices('.')
        .find_map(|(at, _)| by_label(terminals, resources, text.get(..at)?, text.get(at + 1..)?))
}

/// The first function (in declaration order) taking a role of
/// `peripheral`, or exactly `role` when given.
fn taking(
    terminals: &[TerminalDeclaration<'_>],
    resources: &[Resource],
    peripheral: &str,
    role: Option<&str>,
) -> Option<Activation> {
    let resource = resources
        .iter()
        .find(|r| fold(&r.label) == fold(peripheral))?;
    let role_id = match role {
        Some(label) => Some(
            resource
                .roles
                .iter()
                .find(|r| fold(&r.label) == fold(label))?
                .id,
        ),
        None => None,
    };
    terminals.iter().find_map(|declaration| {
        let f = declaration.functions?.functions.iter().find(|f| {
            f.resource_role.is_some_and(|rr| {
                rr.resource == resource.id && role_id.is_none_or(|r| r == rr.role)
            })
        })?;
        view_function(declaration, f.id, resources)
    })
}

/// The activations a rule's literal helper calls name, without repeats.
fn activations(
    source: &str,
    terminals: &[TerminalDeclaration<'_>],
    resources: &[Resource],
) -> Vec<Activation> {
    let mut out: Vec<Activation> = Vec::new();
    let mut push = |a: Option<Activation>| {
        if let Some(a) = a
            && !out
                .iter()
                .any(|x| x.terminal == a.terminal && x.function.function == a.function.function)
        {
            out.push(a);
        }
    };
    for call in scan::literal_calls(source) {
        let args: Vec<&str> = call.args.iter().map(String::as_str).collect();
        match (call.helper.as_str(), args.as_slice()) {
            ("active", [terminal, function]) => {
                push(by_label(terminals, resources, terminal, function));
            }
            ("active", [fref]) => push(by_ref(terminals, resources, fref)),
            ("active_count" | "excludes" | "requires", frefs) => {
                for fref in frefs {
                    push(by_ref(terminals, resources, fref));
                }
            }
            ("function", [terminal]) => {
                // Each function the pin declares: a rule on function(…)
                // usually branches on it.
                if let Some(d) = named(terminals, terminal)
                    && let Some(declared) = d.functions
                {
                    for f in &declared.functions {
                        push(view_function(d, f.id, resources));
                    }
                }
            }
            ("used", [peripheral]) => push(taking(terminals, resources, peripheral, None)),
            ("role", [peripheral, role]) => {
                push(taking(terminals, resources, peripheral, Some(role)));
            }
            _ => {}
        }
    }
    out
}

/// `base` with `active` swapped in, peripheral roles recomputed.
fn with(base: &RuleView, active: &[&Activation]) -> RuleView {
    let mut view = base.clone();
    for a in active {
        if let Some(t) = view.terminals.iter_mut().find(|t| t.terminal == a.terminal) {
            t.active = Some(a.function.clone());
        }
    }
    let taken: BTreeSet<(String, String)> = view
        .terminals
        .iter()
        .filter_map(|t| t.active.as_ref())
        .filter_map(|f| Some((fold(f.peripheral.as_ref()?), fold(f.role.as_ref()?))))
        .collect();
    for resource in &mut view.resources {
        let label = fold(&resource.label);
        resource.active_roles = resource
            .roles
            .iter()
            .filter(|r| taken.contains(&(label.clone(), fold(r))))
            .cloned()
            .collect();
    }
    view
}

/// One compiled rule as its own set.
fn only(compiled: &CompiledRules, rule: RuleId) -> CompiledRules {
    let one: Vec<CompiledRule> = compiled
        .compiled
        .iter()
        .filter(|r| r.id == rule)
        .cloned()
        .collect();
    CompiledRules {
        rules: vec![rule],
        compiled: Arc::new(one),
    }
}

fn verdict_name(verdict: &RuleVerdict) -> &'static str {
    match verdict {
        RuleVerdict::Ok => "ok",
        RuleVerdict::Forbid { .. } => "forbid",
        RuleVerdict::Require { .. } => "require",
        RuleVerdict::Warn { .. } => "warn",
    }
}

fn verdict_text(verdict: &RuleVerdict) -> String {
    match verdict {
        RuleVerdict::Ok => "ok".into(),
        RuleVerdict::Forbid { message, .. }
        | RuleVerdict::Require { message, .. }
        | RuleVerdict::Warn { message, .. } => format!("{}: {message}", verdict_name(verdict)),
    }
}

/// One rule over its samples: the default verdict text, the verdict kinds
/// seen, the samples evaluated and each failure. `None` when the rule does
/// not compile (reported as `rule.script`) or the view is out of bounds.
pub(super) fn run(
    evaluator: &mut Evaluator,
    compiled: &CompiledRules,
    rule: RuleId,
    source: &str,
    base: &RuleView,
    terminals: &[TerminalDeclaration<'_>],
    resources: &[Resource],
) -> Option<(String, BTreeSet<&'static str>, u32, Vec<SampleFailure>)> {
    if compiled.compile_error(rule).is_some() {
        return None;
    }
    let one = only(compiled, rule);
    let named = activations(source, terminals, resources);
    let mut samples: Vec<(String, RuleView)> = vec![("without selections".into(), base.clone())];
    for a in named.iter().take(MAX_RULE_SAMPLES.saturating_sub(2)) {
        samples.push((format!("with {} active", a.name), with(base, &[a])));
    }
    if named.len() > 1 {
        let all: Vec<&Activation> = named.iter().collect();
        samples.push((
            "with every function it names active".into(),
            with(base, &all),
        ));
    }
    let mut default = String::new();
    let mut kinds = BTreeSet::new();
    let mut failures = Vec::new();
    for (k, (sample, view)) in samples.iter().enumerate() {
        let Detailed {
            evaluation,
            references,
        } = evaluate_in(evaluator, &one, view.clone(), RuleBudget::default()).ok()?;
        if let Some(outcome) = evaluation.outcomes.first() {
            kinds.insert(verdict_name(&outcome.verdict));
            if k == 0 {
                default = verdict_text(&outcome.verdict);
            }
        }
        if let Some(failure) = evaluation.failures.first() {
            if k == 0 {
                default = format!("failed: {}", failure.message);
            }
            failures.push(SampleFailure {
                sample: sample.clone(),
                kind: failure.kind,
                reference: references.contains(&rule),
                message: failure.message.clone(),
            });
        }
    }
    Some((default, kinds, samples.len() as u32, failures))
}

/// Every rule of a component over its samples, in declaration order
/// (`library.pinout_preview` lists them so an author sees what each rule
/// says and where it fails). Empty when the rules exceed their bounds or
/// the declarations do not resolve.
pub fn check_samples(
    terminals: &[TerminalDeclaration<'_>],
    resources: &[Resource],
    rules: &[super::PinRule],
) -> Vec<RuleCheck> {
    let Ok(compiled) = super::compile(rules) else {
        return Vec::new();
    };
    let Some(base) = super::default_view(terminals, resources) else {
        return Vec::new();
    };
    let mut evaluator = Evaluator::new();
    rules
        .iter()
        .map(|rule| {
            let label = rule.label.clone();
            if let Some(error) = compiled.compile_error(rule.id) {
                return RuleCheck {
                    label,
                    default: format!("failed: {error}"),
                    samples: 0,
                    verdicts: Vec::new(),
                    failures: vec![error.to_owned()],
                };
            }
            match run(
                &mut evaluator,
                &compiled,
                rule.id,
                &rule.script.source,
                &base,
                terminals,
                resources,
            ) {
                Some((default, kinds, samples, failures)) => RuleCheck {
                    label,
                    default,
                    samples,
                    verdicts: kinds.into_iter().map(str::to_owned).collect(),
                    failures: failures
                        .into_iter()
                        .map(|f| format!("{}: {}", f.sample, f.message))
                        .collect(),
                },
                None => RuleCheck {
                    label,
                    default: "not evaluated: the view is out of bounds".into(),
                    samples: 0,
                    verdicts: Vec::new(),
                    failures: Vec::new(),
                },
            }
        })
        .collect()
}
