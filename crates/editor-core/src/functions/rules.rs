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
//! Script contract (C1 implements it; documented in
//! `docs/library-pins-3d-contract.md`): the script body evaluates to a
//! verdict built with `ok()`, `forbid(message)`, `require(message)` or
//! `warn(message)` (each optionally with an array of terminal names), and
//! reads the view only through helpers: `active(terminal, function)`,
//! `used(peripheral)`, `role(peripheral, role)`, `option(terminal, name)`,
//! `excludes(a, b)` and `requires(a, b)` over `"TERM.function"` strings.
//!
//! Sandbox: no modules, no I/O, no time, no randomness; operation, call
//! depth, string, array and map limits below; deterministic and
//! wasm-compatible. A compile error, runtime error or budget hit is a
//! [`RuleFailure`] (a finding, never a panic) and leaves the document
//! unchanged. Scripts compile once per revision and are cached by the
//! hash of their sources ([`CompiledRules`]). The solver
//! (`functions::suggest`) calls [`evaluate`] as a bounded predicate with
//! a total [`RuleBudget`] and explains the first failing rule.
use super::pin_config::{PinConfig, SourceSpan};
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::ids::{DefinitionId, FunctionId, ResourceId, RuleId, TerminalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

/// A component revision's rules compiled once and cached by the hash of
/// their sources. Opaque; C1 decides what it holds.
#[derive(Clone, Debug, Default)]
pub struct CompiledRules {
    rules: Vec<RuleId>,
}

impl CompiledRules {
    /// The rules, in declaration order.
    pub fn rules(&self) -> &[RuleId] {
        &self.rules
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

/// Compile a component revision's rules. Contract stub until C1.
pub fn compile(rules: &[PinRule]) -> Result<CompiledRules> {
    if rules.is_empty() {
        return Ok(CompiledRules::default());
    }
    Err(unsupported())
}

/// Evaluate every compiled rule over `view` within `budget`. Contract
/// stub until C1: no rules give an empty evaluation.
pub fn evaluate(
    compiled: &CompiledRules,
    _view: &RuleView,
    _budget: RuleBudget,
) -> Result<RuleEvaluation> {
    if compiled.rules.is_empty() {
        return Ok(RuleEvaluation::default());
    }
    Err(unsupported())
}

/// Publication findings of a component's rules: bounds, unique labels,
/// compilation, and (C1) a default resolution no hard rule rejects.
/// Contract: any rule is refused with `rule.unsupported` until C1.
pub fn declaration_findings(component: DefinitionId, rules: &[PinRule]) -> Vec<Finding> {
    let this = vec![ObjectRef::Definition { id: component }];
    let mut out = Vec::new();
    if !within_bounds(rules) {
        out.push(Finding::blocking(
            "rule.bounds",
            "The component's pin rules exceed the supported count or size.".into(),
            this.clone(),
            Stage::Publish,
        ));
    }
    if !rules.is_empty() {
        out.push(Finding::blocking(
            "rule.unsupported",
            "Pin rule scripts cannot be published yet.".into(),
            this,
            Stage::Publish,
        ));
    }
    out
}

fn unsupported() -> Error {
    Error::Unsupported("rule.unsupported: pin rule scripts are not available yet (slice C1)".into())
}
