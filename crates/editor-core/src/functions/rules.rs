//! Slice C1 (contract C0): pin rules as sandboxed Rhai scripts (founder
//! decision F2).
//!
//! A component revision stores its rules ([`PinRule`]) next to its typed
//! `Excludes`/`Requires` constraints; the script source is part of the
//! revision, so it is part of snapshot identity. During resolution each
//! rule is a pure function over a typed, read-only [`RuleView`] of the
//! instance's assignment and returns one [`RuleVerdict`] with a message
//! for people. The verdicts feed the existing resolution buckets:
//!
//! | Verdict | Bucket | Handling |
//! | --- | --- | --- |
//! | `ok` | none | |
//! | `forbid` | `conflicts` ([`Conflict::Rule`](super::Conflict::Rule)) | rejected, like `Excludes` |
//! | `require` | `incomplete` ([`Requirement::Rule`](super::Requirement::Rule)) | finding `config.rule_unmet` |
//! | `warn` | `notes` ([`RuleNote`]) | finding `config.rule_note`, never blocks |
//!
//! # Script contract
//!
//! A rule's body is a Rhai script whose last expression is one verdict:
//!
//! - `ok()`;
//! - `forbid(message)`, `require(message)`, `warn(message)`, each also as
//!   `forbid(message, ["PA9", "PB6"])` naming the terminals involved.
//!   Messages are 1–512 bytes.
//!
//! It reads the view only through these helpers. A terminal is named by
//! its exact name (`"PA9"`) or by number (`"#12"`); a function, peripheral
//! or role label matches without case. `"TERM.function"` names a function
//! of a terminal (`"PA9.usart1_tx"`; a terminal name may contain dots).
//!
//! | Helper | Returns |
//! | --- | --- |
//! | `active(terminal, function)`, `active("TERM.function")` | whether that function is active |
//! | `active_count(["A.f", "B.g", …])` | how many of the functions are active |
//! | `function(terminal)` | the active function's label, or `""` |
//! | `used(peripheral)` | whether any role of the peripheral is taken |
//! | `role(peripheral, role)` | whether that role is taken |
//! | `kind(peripheral)` | the peripheral's kind, or `""` |
//! | `option(terminal, name)` | the instance's `pull` (`none`/`up`/`down`), `speed`, `initial` (`low`/`high`), `schmitt` (`true`/`false`) or `name`, or `""` when unset |
//! | `excludes("A.f", "B.g")` | `forbid` with a generated message when both are active, else `ok()` |
//! | `requires("A.f", "B.g")` | `require` with a generated message when `A.f` is active and `B.g` is not, else `ok()` |
//!
//! Naming a terminal, function, peripheral, role or option that does not
//! exist is a reference error: a runtime failure, and `rule.reference`
//! when publishing (string-literal arguments are also checked statically,
//! so a typo on a branch the default assignment never takes is found).
//!
//! Example (synthetic part):
//!
//! ```text
//! if active("PA13.gpio") || active("PA14.gpio") {
//!     warn("Using the SWD pins as GPIO disables debugging.", ["PA13", "PA14"])
//! } else if active_count(["PA0.adc_in0", "PA1.adc_in1", "PA2.adc_in2"]) > 2 {
//!     forbid("At most two ADC inputs on this package.")
//! } else {
//!     requires("PA5.spi1_sck", "PA7.spi1_mosi")
//! }
//! ```
//!
//! # Sandbox
//!
//! No modules, no I/O, no time, no randomness, no `eval`, `print` or
//! `debug`; strict variables; the operation, call-depth, string, array and
//! map limits below (Rhai checks a string, array or map size when the
//! value is next used; growth in between is bounded by the operation
//! limit). A rule is deterministic and compiles for wasm. A
//! compile error, runtime error or budget hit is a [`RuleFailure`] (a
//! finding, never a panic) and leaves the document unchanged. Scripts
//! compile once per revision and are cached by the hash of their IDs,
//! labels and sources ([`CompiledRules`]). The solver
//! (`functions::suggest`) calls [`evaluate`] as a bounded predicate with
//! a total [`RuleBudget`] and explains the first failing rule.
mod sandbox;
mod scan;
#[cfg(test)]
mod tests;

use super::pin_config::{PinConfig, PinConfigs, SourceSpan};
use super::{Conflict, Requirement, Resolution, Resource, TerminalDeclaration};
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::ids::{DefinitionId, FunctionId, ResourceId, RuleId, TerminalId};
use sandbox::{ErrorClass, ScriptVerdict, ViewIndex};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// Rules one component revision declares (target).
pub const MAX_PIN_RULES: usize = 1024;
/// Source bytes of one rule script.
pub const MAX_RULE_SOURCE: usize = 4 * 1024;
/// Source bytes of all rule scripts of one component revision.
pub const MAX_RULES_SOURCE_TOTAL: usize = 256 * 1024;
/// Rhai operations one rule evaluation may take.
pub const MAX_RULE_OPERATIONS: u64 = 10_000;
/// Rhai operations all rules of one resolution may take together.
pub const MAX_RESOLUTION_OPERATIONS: u64 = 1_000_000;
/// Rhai operations one `functions.suggest` search may spend on rules
/// (target; measured by C2).
pub const MAX_SUGGEST_OPERATIONS: u64 = 20_000_000;
/// Rhai call depth inside a rule.
pub const MAX_RULE_CALL_DEPTH: usize = 16;
/// Rhai string length inside a rule, bytes.
pub const MAX_RULE_STRING: usize = 1024;
/// Rhai array length inside a rule.
pub const MAX_RULE_ARRAY: usize = 1024;
/// Rhai map size inside a rule.
pub const MAX_RULE_MAP: usize = 256;
/// Bytes of a verdict message.
pub const MAX_VERDICT_MESSAGE: usize = 512;

/// The Rhai source of one rule.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PinRuleScript {
    /// At most [`MAX_RULE_SOURCE`] bytes.
    pub source: String,
}

/// One named pin rule of a component revision.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PinRule {
    pub id: RuleId,
    /// Unique printable label within the component ("usart1_map").
    pub label: String,
    pub script: PinRuleScript,
    /// Where an imported rule came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceSpan>,
}

/// The active function of one terminal, as a rule sees it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ViewFunction {
    pub function: FunctionId,
    pub label: String,
    /// The peripheral (resource) label and role label it takes, if any.
    pub peripheral: Option<String>,
    pub role: Option<String>,
}

/// One terminal as a rule sees it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ViewTerminal {
    pub terminal: TerminalId,
    pub name: String,
    pub active: Option<ViewFunction>,
    /// Labels of every function the terminal declares.
    pub functions: Vec<String>,
    pub config: Option<PinConfig>,
}

/// One peripheral (resource) as a rule sees it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ViewResource {
    pub resource: ResourceId,
    pub label: String,
    pub kind: Option<String>,
    /// Labels of every role the peripheral declares.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Labels of the roles taken by an active function.
    pub active_roles: Vec<String>,
}

/// The typed, read-only view of one assignment that rules evaluate.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleView {
    pub terminals: Vec<ViewTerminal>,
    pub resources: Vec<ViewResource>,
}

/// What one rule says about one assignment.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum RuleVerdict {
    Ok,
    /// Never allowed: a conflict.
    Forbid {
        message: String,
        terminals: Vec<TerminalId>,
    },
    /// A prerequisite not met yet: incomplete.
    Require {
        message: String,
        terminals: Vec<TerminalId>,
    },
    /// Advisory: a note.
    Warn {
        message: String,
        terminals: Vec<TerminalId>,
    },
}

/// A `warn` verdict in a resolution.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleNote {
    pub rule: RuleId,
    pub message: String,
    pub terminals: Vec<TerminalId>,
}

/// Why a rule produced no verdict.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleFailureKind {
    /// The source does not compile (finding `rule.script`).
    Compile,
    /// Operation or size limits hit (finding `rule.budget`).
    Budget,
    /// The script raised an error or returned no verdict (`rule.runtime`).
    Runtime,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleFailure {
    pub rule: RuleId,
    pub kind: RuleFailureKind,
    pub message: String,
}

/// One rule's verdict.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleOutcome {
    pub rule: RuleId,
    pub verdict: RuleVerdict,
}

/// Every rule's verdict over one view, in rule order, with failures and
/// the operations spent.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleEvaluation {
    pub outcomes: Vec<RuleOutcome>,
    pub failures: Vec<RuleFailure>,
    pub operations: u64,
}

/// Operation limits of one evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuleBudget {
    pub per_rule: u64,
    pub total: u64,
}

impl Default for RuleBudget {
    fn default() -> Self {
        Self {
            per_rule: MAX_RULE_OPERATIONS,
            total: MAX_RESOLUTION_OPERATIONS,
        }
    }
}

/// Terminals one view holds: the component terminal limit (2048), stated
/// here because `functions` does not read `library`.
const MAX_VIEW_TERMINALS: usize = 2048;
/// Compiled rule sets kept by the process-wide cache.
const MAX_CACHED_RULE_SETS: usize = 64;

/// One compiled rule: its program, or the compile error it reports.
#[derive(Clone, Debug)]
struct CompiledRule {
    id: RuleId,
    label: String,
    program: std::result::Result<Arc<rhai::AST>, String>,
}

/// A component revision's rules compiled once and cached by the hash of
/// their IDs, labels and sources. A rule that does not compile is kept
/// with its error, which [`evaluate`] reports as a
/// [`RuleFailureKind::Compile`] failure.
#[derive(Clone, Debug, Default)]
pub struct CompiledRules {
    rules: Vec<RuleId>,
    compiled: Arc<Vec<CompiledRule>>,
}

impl CompiledRules {
    /// The rules, in declaration order.
    pub fn rules(&self) -> &[RuleId] {
        &self.rules
    }
    /// A rule's label.
    pub fn label(&self, rule: RuleId) -> Option<&str> {
        self.compiled
            .iter()
            .find(|r| r.id == rule)
            .map(|r| r.label.as_str())
    }
    /// The compile error of a rule that does not compile.
    pub fn compile_error(&self, rule: RuleId) -> Option<&str> {
        self.compiled
            .iter()
            .find(|r| r.id == rule)
            .and_then(|r| r.program.as_ref().err())
            .map(String::as_str)
    }
}

/// Rules within their persistence bounds.
pub fn within_bounds(rules: &[PinRule]) -> bool {
    let total: usize = rules.iter().map(|r| r.script.source.len()).sum();
    rules.len() <= MAX_PIN_RULES
        && total <= MAX_RULES_SOURCE_TOTAL
        && rules.iter().all(|r| {
            r.script.source.len() <= MAX_RULE_SOURCE && r.label.len() <= crate::ids::MAX_LABEL
        })
}

fn bounds_error() -> Error {
    Error::Limit(format!(
        "rule.bounds: a component has at most {MAX_PIN_RULES} pin rules of at most {MAX_RULE_SOURCE} bytes each and {MAX_RULES_SOURCE_TOTAL} bytes in total"
    ))
}

type CacheKey = [u8; 32];

fn cache_key(rules: &[PinRule]) -> CacheKey {
    let mut hash = Sha256::new();
    for rule in rules {
        hash.update(rule.id.0.to_le_bytes());
        for text in [&rule.label, &rule.script.source] {
            hash.update((text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
        }
    }
    hash.finalize().into()
}

type Cache = Vec<(CacheKey, Arc<Vec<CompiledRule>>)>;
static CACHE: Mutex<Cache> = Mutex::new(Vec::new());

fn cached(key: &CacheKey) -> Option<Arc<Vec<CompiledRule>>> {
    let cache = CACHE.lock().ok()?;
    cache.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn remember(key: CacheKey, compiled: Arc<Vec<CompiledRule>>) {
    // A poisoned cache is skipped, never trusted or unwrapped.
    if let Ok(mut cache) = CACHE.lock() {
        if cache.iter().any(|(k, _)| *k == key) {
            return;
        }
        if cache.len() >= MAX_CACHED_RULE_SETS {
            cache.remove(0);
        }
        cache.push((key, compiled));
    }
}

/// Whether a component revision's rules are in the compile cache (for
/// tests and diagnostics).
pub fn is_cached(rules: &[PinRule]) -> bool {
    cached(&cache_key(rules)).is_some()
}

fn compile_one(engine: &rhai::Engine, source: &str) -> std::result::Result<rhai::AST, String> {
    if source.trim().is_empty() {
        return Err("The script is empty; it must end with a verdict.".into());
    }
    engine
        .compile(source)
        .map_err(|e| bounded(format!("The script does not compile: {e}.")))
}

/// Compile a component revision's rules, once per distinct set of
/// sources. Only bounds are errors; a rule that does not compile is kept
/// and reported by [`evaluate`].
pub fn compile(rules: &[PinRule]) -> Result<CompiledRules> {
    if !within_bounds(rules) {
        return Err(bounds_error());
    }
    let ids = rules.iter().map(|r| r.id).collect();
    if rules.is_empty() {
        return Ok(CompiledRules::default());
    }
    let key = cache_key(rules);
    if let Some(compiled) = cached(&key) {
        return Ok(CompiledRules {
            rules: ids,
            compiled,
        });
    }
    let engine = sandbox::engine();
    let compiled: Arc<Vec<CompiledRule>> = Arc::new(
        rules
            .iter()
            .map(|r| CompiledRule {
                id: r.id,
                label: r.label.clone(),
                program: compile_one(&engine, &r.script.source).map(Arc::new),
            })
            .collect(),
    );
    remember(key, compiled.clone());
    Ok(CompiledRules {
        rules: ids,
        compiled,
    })
}

/// The syntax check of one rule's source, for an editor: `None` when it
/// compiles, else the error with its position.
pub fn syntax_error(source: &str) -> Option<String> {
    if source.len() > MAX_RULE_SOURCE {
        return Some(format!(
            "A pin rule script has at most {MAX_RULE_SOURCE} bytes."
        ));
    }
    compile_one(&sandbox::engine(), source).err()
}

/// Clip `text` to [`MAX_VERDICT_MESSAGE`] bytes on a character boundary.
fn bounded(mut text: String) -> String {
    if text.len() > MAX_VERDICT_MESSAGE {
        let mut end = MAX_VERDICT_MESSAGE;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// An evaluation with the rules whose runtime failure was a reference
/// error, which publishing reports separately.
struct Detailed {
    evaluation: RuleEvaluation,
    references: BTreeSet<RuleId>,
}

fn verdict(script: ScriptVerdict) -> RuleVerdict {
    match script {
        ScriptVerdict::Ok => RuleVerdict::Ok,
        ScriptVerdict::Forbid(message, terminals) => RuleVerdict::Forbid { message, terminals },
        ScriptVerdict::Require(message, terminals) => RuleVerdict::Require { message, terminals },
        ScriptVerdict::Warn(message, terminals) => RuleVerdict::Warn { message, terminals },
    }
}

/// A Rhai engine with every rule helper registered, reused across
/// evaluations: each one swaps its view in. A search that evaluates the
/// rules at many nodes builds it once instead of once per node.
pub struct Evaluator {
    engine: rhai::Engine,
    spent: Arc<std::sync::atomic::AtomicU64>,
    slot: sandbox::ViewSlot,
}

impl Evaluator {
    pub fn new() -> Self {
        let slot: sandbox::ViewSlot = Arc::new(std::sync::RwLock::new(Arc::new(ViewIndex::new(
            RuleView::default(),
        ))));
        let (engine, spent) = sandbox::evaluator(slot.clone());
        Self {
            engine,
            spent,
            slot,
        }
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}

fn evaluate_detailed(
    compiled: &CompiledRules,
    view: &RuleView,
    budget: RuleBudget,
) -> Result<Detailed> {
    if compiled.compiled.is_empty() {
        return Ok(Detailed {
            evaluation: RuleEvaluation::default(),
            references: BTreeSet::new(),
        });
    }
    evaluate_in(&mut Evaluator::new(), compiled, view.clone(), budget)
}

fn evaluate_in(
    evaluator: &mut Evaluator,
    compiled: &CompiledRules,
    view: RuleView,
    budget: RuleBudget,
) -> Result<Detailed> {
    let mut out = Detailed {
        evaluation: RuleEvaluation::default(),
        references: BTreeSet::new(),
    };
    if compiled.compiled.is_empty() {
        return Ok(out);
    }
    if view.terminals.len() > MAX_VIEW_TERMINALS || view.resources.len() > super::MAX_RESOURCES {
        return Err(Error::Limit(
            "rule.bounds: the view exceeds the supported terminal or peripheral count".into(),
        ));
    }
    match evaluator.slot.write() {
        Ok(mut current) => *current = Arc::new(ViewIndex::new(view)),
        Err(_) => {
            return Err(Error::Invalid(
                "rule.bounds: the rule evaluator is unusable after an earlier failure".into(),
            ));
        }
    }
    let engine = &mut evaluator.engine;
    let spent = &evaluator.spent;
    let evaluation = &mut out.evaluation;
    for rule in compiled.compiled.iter() {
        let fail = |kind, message: String| RuleFailure {
            rule: rule.id,
            kind,
            message: bounded(message),
        };
        let program = match &rule.program {
            Ok(program) => program,
            Err(error) => {
                evaluation
                    .failures
                    .push(fail(RuleFailureKind::Compile, error.clone()));
                continue;
            }
        };
        let remaining = budget.total.saturating_sub(evaluation.operations);
        let limit = budget.per_rule.min(remaining);
        if limit == 0 {
            evaluation.failures.push(fail(
                RuleFailureKind::Budget,
                "The pin rules ran out of operations before this rule.".into(),
            ));
            continue;
        }
        // Rhai treats 0 as unlimited; `limit` is at least 1 here.
        engine.set_max_operations(limit);
        spent.store(0, Ordering::Relaxed);
        let result = engine.eval_ast::<rhai::Dynamic>(program);
        let used = spent.load(Ordering::Relaxed).min(limit);
        evaluation.operations = evaluation.operations.saturating_add(used);
        match result {
            Ok(value) => match value.try_cast::<ScriptVerdict>() {
                Some(script) => evaluation.outcomes.push(RuleOutcome {
                    rule: rule.id,
                    verdict: verdict(script),
                }),
                None => evaluation.failures.push(fail(
                    RuleFailureKind::Runtime,
                    "The rule must end with ok(), forbid(…), require(…) or warn(…).".into(),
                )),
            },
            Err(error) => {
                let failure = match sandbox::classify(&error) {
                    ErrorClass::Budget(m) => fail(RuleFailureKind::Budget, m),
                    ErrorClass::Reference(m) => {
                        out.references.insert(rule.id);
                        fail(RuleFailureKind::Runtime, m)
                    }
                    ErrorClass::Runtime(m) => fail(RuleFailureKind::Runtime, m),
                };
                evaluation.failures.push(failure);
            }
        }
    }
    Ok(out)
}

/// Evaluate every compiled rule over `view` within `budget`: the verdicts
/// in rule order, a failure for each rule that does not compile, raises an
/// error, returns no verdict or runs out of operations, and the operations
/// spent. Errors only when the view itself is out of bounds.
pub fn evaluate(
    compiled: &CompiledRules,
    view: &RuleView,
    budget: RuleBudget,
) -> Result<RuleEvaluation> {
    evaluate_detailed(compiled, view, budget).map(|d| d.evaluation)
}

/// [`evaluate`] with a reused [`Evaluator`], taking the view by value.
pub fn evaluate_with(
    evaluator: &mut Evaluator,
    compiled: &CompiledRules,
    view: RuleView,
    budget: RuleBudget,
) -> Result<RuleEvaluation> {
    evaluate_in(evaluator, compiled, view, budget).map(|d| d.evaluation)
}

/// The view rules see of one resolution: every terminal with its declared
/// function labels, its active function and its configuration, and every
/// peripheral with its declared and taken roles.
pub fn view(
    terminals: &[TerminalDeclaration<'_>],
    resources: &[Resource],
    resolution: &Resolution,
    config: &PinConfigs,
) -> RuleView {
    let mut taken: BTreeSet<(ResourceId, crate::ids::RoleId)> = BTreeSet::new();
    let mut out = RuleView::default();
    for declaration in terminals {
        let declared = declaration.functions;
        let active = resolution.active(declaration.terminal).map(|a| {
            let role = declared
                .and_then(|d| d.function(a.function))
                .and_then(|f| f.resource_role);
            let resource = role.and_then(|rr| resources.iter().find(|r| r.id == rr.resource));
            if let Some(rr) = role {
                taken.insert((rr.resource, rr.role));
            }
            ViewFunction {
                function: a.function,
                label: a.label.clone(),
                peripheral: resource.map(|r| r.label.clone()),
                role: role
                    .zip(resource)
                    .and_then(|(rr, r)| r.role(rr.role))
                    .map(|r| r.label.clone()),
            }
        });
        out.terminals.push(ViewTerminal {
            terminal: declaration.terminal,
            name: declaration.name.to_owned(),
            active,
            functions: declared
                .map(|d| d.functions.iter().map(|f| f.label.clone()).collect())
                .unwrap_or_default(),
            config: config.get(&declaration.terminal).cloned(),
        });
    }
    for resource in resources {
        out.resources.push(ViewResource {
            resource: resource.id,
            label: resource.label.clone(),
            kind: resource.kind.clone(),
            roles: resource.roles.iter().map(|r| r.label.clone()).collect(),
            active_roles: resource
                .roles
                .iter()
                .filter(|r| taken.contains(&(resource.id, r.id)))
                .map(|r| r.label.clone())
                .collect(),
        });
    }
    out
}

fn rule_name(compiled: &CompiledRules, rule: RuleId) -> String {
    compiled
        .label(rule)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map_or_else(|| format!("Pin rule {rule}"), |l| format!("Pin rule ‘{l}’"))
}

/// Put an evaluation's verdicts into the resolution's buckets: `forbid`
/// is a [`Conflict::Rule`], `require` a [`Requirement::Rule`], `warn` a
/// [`RuleNote`], and every failure a [`Resolution::rule_failures`] entry.
/// Messages are prefixed with the rule's label.
pub fn apply(resolution: &mut Resolution, compiled: &CompiledRules, evaluation: &RuleEvaluation) {
    for outcome in &evaluation.outcomes {
        let rule = outcome.rule;
        match &outcome.verdict {
            RuleVerdict::Ok => {}
            RuleVerdict::Forbid { message, terminals } => {
                resolution.conflicts.push(Conflict::Rule {
                    rule,
                    terminals: terminals.clone(),
                    message: format!("{}: {message}", rule_name(compiled, rule)),
                });
            }
            RuleVerdict::Require { message, terminals } => {
                resolution.incomplete.push(Requirement::Rule {
                    rule,
                    terminals: terminals.clone(),
                    message: format!("{}: {message}", rule_name(compiled, rule)),
                });
            }
            RuleVerdict::Warn { message, terminals } => resolution.notes.push(RuleNote {
                rule,
                message: format!("{}: {message}", rule_name(compiled, rule)),
                terminals: terminals.clone(),
            }),
        }
    }
    resolution
        .rule_failures
        .extend(evaluation.failures.iter().map(|f| RuleFailure {
            message: bounded(format!("{}: {}", rule_name(compiled, f.rule), f.message)),
            ..f.clone()
        }));
}

/// One rule's result for the explain panel.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleExplanation {
    pub rule: RuleId,
    pub label: String,
    /// The verdict, or `None` when the rule failed.
    pub verdict: Option<RuleVerdict>,
    pub failure: Option<RuleFailure>,
}

/// Every rule's result over one view, in declaration order.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleExplanations {
    pub rules: Vec<RuleExplanation>,
    pub operations: u64,
}

/// Explain each rule over `view`: what it says, or why it failed.
pub fn explain(
    compiled: &CompiledRules,
    view: &RuleView,
    budget: RuleBudget,
) -> Result<RuleExplanations> {
    let evaluation = evaluate(compiled, view, budget)?;
    Ok(explanations(compiled, &evaluation))
}

/// Each rule's result in an evaluation, in declaration order.
pub fn explanations(compiled: &CompiledRules, evaluation: &RuleEvaluation) -> RuleExplanations {
    let rules = compiled
        .compiled
        .iter()
        .map(|r| RuleExplanation {
            rule: r.id,
            label: r.label.clone(),
            verdict: evaluation
                .outcomes
                .iter()
                .find(|o| o.rule == r.id)
                .map(|o| o.verdict.clone()),
            failure: evaluation.failures.iter().find(|f| f.rule == r.id).cloned(),
        })
        .collect();
    RuleExplanations {
        rules,
        operations: evaluation.operations,
    }
}

fn publish_issue(code: &str, message: String, objects: Vec<ObjectRef>) -> Finding {
    Finding::blocking(code, message, objects, Stage::Publish)
}

/// Publication findings of a component's rules without its declarations:
/// bounds, unique printable labels and compilation. This is what the
/// component publish calls at C0; [`publish_findings`] adds the reference
/// and default-resolution checks that need the terminals and peripherals.
pub fn declaration_findings(component: DefinitionId, rules: &[PinRule]) -> Vec<Finding> {
    publish_findings(component, None, rules)
}

/// Publication findings of a component's rules: bounds (`rule.bounds`),
/// unique printable labels and known references (`rule.reference`),
/// compilation (`rule.script`), and, with the declarations, a default
/// resolution every rule evaluates without error (`rule.runtime`,
/// `rule.budget`) and no rule forbids (`rule.default_conflict`).
pub fn publish_findings(
    component: DefinitionId,
    declarations: Option<(&[TerminalDeclaration<'_>], &[Resource])>,
    rules: &[PinRule],
) -> Vec<Finding> {
    let this = ObjectRef::Definition { id: component };
    let at = |rule: RuleId| {
        vec![
            this,
            ObjectRef::LibraryObject {
                definition: component,
                local: rule.0,
            },
        ]
    };
    if !within_bounds(rules) {
        return vec![publish_issue(
            "rule.bounds",
            format!(
                "The component's pin rules exceed {MAX_PIN_RULES} rules, {MAX_RULE_SOURCE} bytes per script or {MAX_RULES_SOURCE_TOTAL} bytes in total."
            ),
            vec![this],
        )];
    }
    let mut out = Vec::new();
    let mut labels = BTreeSet::new();
    for rule in rules {
        if !crate::ids::printable(&rule.label) || !labels.insert(rule.label.trim().to_lowercase()) {
            out.push(publish_issue(
                "rule.reference",
                "Each pin rule needs a unique printable label.".into(),
                at(rule.id),
            ));
        }
    }
    let Ok(compiled) = compile(rules) else {
        return out;
    };
    for rule in rules {
        if let Some(error) = compiled.compile_error(rule.id) {
            out.push(publish_issue(
                "rule.script",
                bounded(format!("{}: {error}", rule_name(&compiled, rule.id))),
                at(rule.id),
            ));
        }
    }
    let Some((terminals, resources)) = declarations else {
        return out;
    };
    let Ok(resolution) = super::resolve(
        terminals,
        resources,
        &[],
        None,
        &super::FunctionSelections::new(),
    ) else {
        return out;
    };
    let view = view(terminals, resources, &resolution, &PinConfigs::new());
    let index = ViewIndex::new(view.clone());
    let mut referenced = BTreeSet::new();
    for rule in rules {
        if compiled.compile_error(rule.id).is_some() {
            continue;
        }
        let problems: BTreeSet<String> = scan::literal_calls(&rule.script.source)
            .iter()
            .filter_map(|call| check_literal(&index, call).err())
            .collect();
        for problem in problems {
            referenced.insert(rule.id);
            out.push(publish_issue(
                "rule.reference",
                bounded(format!("{}: {problem}", rule_name(&compiled, rule.id))),
                at(rule.id),
            ));
        }
    }
    let Ok(detailed) = evaluate_detailed(&compiled, &view, RuleBudget::default()) else {
        return out;
    };
    for failure in &detailed.evaluation.failures {
        let code = match failure.kind {
            RuleFailureKind::Compile => continue,
            RuleFailureKind::Budget => "rule.budget",
            RuleFailureKind::Runtime if detailed.references.contains(&failure.rule) => {
                if referenced.contains(&failure.rule) {
                    continue;
                }
                "rule.reference"
            }
            RuleFailureKind::Runtime => "rule.runtime",
        };
        out.push(publish_issue(
            code,
            bounded(format!(
                "{} fails without explicit selections: {}",
                rule_name(&compiled, failure.rule),
                failure.message
            )),
            at(failure.rule),
        ));
    }
    for outcome in &detailed.evaluation.outcomes {
        if let RuleVerdict::Forbid { message, .. } = &outcome.verdict {
            out.push(publish_issue(
                "rule.default_conflict",
                bounded(format!(
                    "Without explicit selections the fixed functions and design defaults break {}: {message}",
                    rule_name(&compiled, outcome.rule).replacen("Pin rule", "pin rule", 1)
                )),
                at(outcome.rule),
            ));
        }
    }
    out
}

/// Check one literal helper call against the default view's names.
fn check_literal(index: &ViewIndex, call: &scan::LiteralCall) -> std::result::Result<(), String> {
    let args: Vec<&str> = call.args.iter().map(String::as_str).collect();
    match (call.helper.as_str(), args.as_slice()) {
        ("active", [terminal, function]) => {
            let t = index.terminal(terminal)?;
            index.declares(t, function)
        }
        ("active", [fref]) => index.function_ref(fref).map(|_| ()),
        ("excludes" | "requires", [a, b]) => {
            index.function_ref(a)?;
            index.function_ref(b).map(|_| ())
        }
        ("active_count", frefs) => frefs
            .iter()
            .try_for_each(|f| index.function_ref(f).map(|_| ())),
        ("function", [terminal]) => index.terminal(terminal).map(|_| ()),
        ("option", [terminal, name]) => {
            let t = index.terminal(terminal)?;
            ViewIndex::option(t, name).map(|_| ())
        }
        ("used" | "kind", [resource]) => index.resource(resource).map(|_| ()),
        ("role", [resource, role]) => index.role(resource, role).map(|_| ()),
        // A wrong argument count is a runtime error, reported by evaluation.
        _ => Ok(()),
    }
}
