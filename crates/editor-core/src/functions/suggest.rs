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
use crate::diagnostics::{Error, Result};
use crate::ids::{ResourceId, RoleId, RuleId, TerminalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Search nodes one `functions.suggest` may visit (target: < 50 ms on a
/// 176-pin fixture; to be measured).
pub const SUGGEST_NODE_BUDGET: u64 = 100_000;
/// Ranked assignments one suggestion returns.
pub const MAX_SUGGESTIONS: usize = 8;
/// Peripherals one request enables.
pub const MAX_PERIPHERAL_REQUESTS: usize = 64;
/// Terminals one request avoids.
pub const MAX_AVOID: usize = 2048;

/// One peripheral to enable; `roles` restricts which roles must be placed
/// (all `required_when_used` roles otherwise).
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
/// instance's selections and the net hints.
pub struct SuggestInput<'a> {
    pub terminals: &'a [super::TerminalDeclaration<'a>],
    pub resources: &'a [super::Resource],
    pub constraints: &'a [super::FunctionConstraint],
    pub unavailable: Option<&'a [crate::ids::FunctionKey]>,
    pub rules: &'a super::rules::CompiledRules,
    pub selections: &'a super::FunctionSelections,
    pub hints: &'a [TerminalHint],
}

/// Search for assignments that enable `request`. Contract stub until C2.
pub fn suggest(_input: &SuggestInput<'_>, _request: &SuggestRequest) -> Result<SuggestReport> {
    Err(unsupported())
}

/// Contract stub until C2.
pub fn unsupported() -> Error {
    Error::Unsupported(
        "functions.suggest_unsupported: pin assignment suggestions are not available yet (slice C2)"
            .into(),
    )
}
