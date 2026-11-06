//! Terminal functions: what each electrical terminal can do, which shared
//! resources those functions use, and the single resolution rule that turns
//! an instance's explicit selections into one active function per terminal.
//!
//! Inputs are plain data: this module never sees `library` or `model` types.
//! `library` stores declarations and calls [`declaration_findings`] and
//! [`binding_findings`] when it publishes; `instances` builds the
//! [`resolve`] input from an instance's pinned component and chosen binding.
//! The resolution's buckets are consumed separately: conflicts are rejected
//! by `instances::check_instance`, unresolved and incomplete terminals are
//! completeness findings, and ERC applies drive rules to active functions.
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::ids::{
    DefinitionId, FunctionId, FunctionKey, MAX_LABEL, ResourceId, RoleId, TerminalId, printable,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub mod pin_config;
pub mod rules;
pub mod suggest;

pub use pin_config::{GpioRef, Level, MuxCode, PinConfig, PinConfigs, PinOption, Pull, SourceSpan};
pub use rules::{PinRule, PinRuleScript, RuleFailure, RuleNote};

/// Alternative functions of one terminal.
pub const MAX_FUNCTIONS: usize = 32;
/// Resources (peripheral instances) one component declares.
pub const MAX_RESOURCES: usize = 256;
/// Roles of one resource: room for wide parallel buses (a 24-bit RGB LCD
/// with its controls, MII Ethernet, a camera port). The suggestion solver
/// stays bounded by its operation budget.
pub const MAX_ROLES: usize = 64;
/// Constraints one component declares.
pub const MAX_CONSTRAINTS: usize = 1024;
/// Bytes of a peripheral kind or role signal name.
pub const MAX_KIND: usize = 32;
/// Edits one `instance.select_functions` applies.
pub const MAX_FUNCTION_EDITS: usize = 256;
/// Functions one binding marks unavailable.
pub const MAX_UNAVAILABLE: usize = 1024;

/// The functions an electrical terminal can take. A terminal with exactly one
/// function is fixed; otherwise its active function comes from an explicit
/// selection or the design default.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalFunctions {
    /// 1–32 functions. Order is presentation only.
    pub functions: Vec<Function>,
    /// The function used without an explicit selection. A reset-state
    /// description is never a default.
    pub design_default: Option<FunctionId>,
    /// C2: the terminal's GPIO identity, stated explicitly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpio: Option<GpioRef>,
    /// C2: settable non-electrical options and their allowed values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<PinOption>,
}

/// One function a terminal can take.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Function {
    pub id: FunctionId,
    pub label: String,
    pub direction: Direction,
    pub signal: SignalKind,
    pub drive: Drive,
    /// The role of a shared resource this function takes while active.
    pub resource_role: Option<ResourceRole>,
    /// C2: the vendor's alternate-function selector, opaque data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mux: Option<MuxCode>,
    /// Which of the vendor's mux tables `mux` belongs to ("IO MUX",
    /// "LP IO MUX", "analog") when a part numbers several tables from the
    /// same codes; opaque data like the code, at most
    /// [`pin_config::MAX_MUX_TABLE`] bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mux_table: Option<String>,
    /// C3: where an imported function came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceSpan>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Input,
    Output,
    Bidirectional,
    Passive,
    PowerInput,
    PowerOutput,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    Digital,
    Analog,
    Power,
    Unspecified,
}

/// How a driving function drives its net.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Drive {
    None,
    PushPull,
    OpenDrain,
    OpenSource,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResourceRole {
    pub resource: ResourceId,
    pub role: RoleId,
}

/// A shared hardware resource whose roles functions take.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub id: ResourceId,
    pub label: String,
    /// C2: free grouping text ("uart", "spi", "timer") for search and
    /// firmware templates; never used for compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 1–16 roles.
    pub roles: Vec<Role>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Role {
    pub id: RoleId,
    pub label: String,
    /// How many active functions may take this role at once (≥ 1).
    pub capacity: u16,
    /// Once any role of the resource is active, this role must be too.
    pub required_when_used: bool,
    /// C2: the datasheet signal name ("TX", "SCK", "CH1N"), data only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
}

/// A rule between functions of one instance.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum FunctionConstraint {
    /// The two functions are never active together.
    Excludes { a: FunctionKey, b: FunctionKey },
    /// While `selected` is active, `requires` must be active too.
    Requires {
        selected: FunctionKey,
        requires: FunctionKey,
    },
}

/// An instance's explicit selections. A missing terminal uses its fixed
/// function or design default.
pub type FunctionSelections = BTreeMap<TerminalId, FunctionId>;

/// Select a function on a terminal, or clear its explicit selection.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FunctionEdit {
    pub terminal: TerminalId,
    /// `None` clears the explicit selection.
    pub function: Option<FunctionId>,
}

/// One terminal as the resolver sees it.
#[derive(Clone, Copy, Debug)]
pub struct TerminalDeclaration<'a> {
    pub terminal: TerminalId,
    /// The terminal's name, for messages.
    pub name: &'a str,
    /// `None` for device-NC and reserved terminals, which have no functions.
    pub functions: Option<&'a TerminalFunctions>,
}

/// Why a terminal has its active function.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActiveOrigin {
    Selected,
    Fixed,
    DesignDefault,
}

/// The active function of one terminal, with the behavior ERC uses.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ActiveFunction {
    pub terminal: TerminalId,
    pub function: FunctionId,
    pub label: String,
    pub direction: Direction,
    pub signal: SignalKind,
    pub drive: Drive,
    pub origin: ActiveOrigin,
}

/// A state that is never allowed: rejected by direct commands and blocking
/// in plans.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "conflict", rename_all = "snake_case")]
pub enum Conflict {
    /// More active functions take a role than its capacity allows.
    Capacity {
        resource: ResourceId,
        role: RoleId,
        capacity: u16,
        functions: Vec<FunctionKey>,
        message: String,
    },
    /// Both functions of an `Excludes` pair are active.
    Excluded {
        a: FunctionKey,
        b: FunctionKey,
        message: String,
    },
    /// An active function is unavailable in the chosen binding.
    Unavailable {
        function: FunctionKey,
        message: String,
    },
    /// C1: a pin rule script's `forbid` verdict.
    Rule {
        rule: crate::ids::RuleId,
        terminals: Vec<TerminalId>,
        message: String,
    },
}

impl Conflict {
    pub fn message(&self) -> &str {
        match self {
            Self::Capacity { message, .. }
            | Self::Excluded { message, .. }
            | Self::Unavailable { message, .. }
            | Self::Rule { message, .. } => message,
        }
    }
}

/// A prerequisite that is not met yet. Stepwise edits may pass through it;
/// completeness reports it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "requirement", rename_all = "snake_case")]
pub enum Requirement {
    /// A `required_when_used` role is absent while another role of its
    /// resource is active.
    RoleMissing {
        resource: ResourceId,
        role: RoleId,
        /// The active functions that use the resource.
        used_by: Vec<FunctionKey>,
        message: String,
    },
    /// A `Requires` constraint whose prerequisite is not active.
    Requires {
        selected: FunctionKey,
        requires: FunctionKey,
        message: String,
    },
    /// C1: a pin rule script's `require` verdict.
    Rule {
        rule: crate::ids::RuleId,
        terminals: Vec<TerminalId>,
        message: String,
    },
}

impl Requirement {
    pub fn message(&self) -> &str {
        match self {
            Self::RoleMissing { message, .. }
            | Self::Requires { message, .. }
            | Self::Rule { message, .. } => message,
        }
    }
    /// The terminals whose functions take part.
    pub fn terminals(&self) -> Vec<TerminalId> {
        let keys: Vec<FunctionKey> = match self {
            Self::RoleMissing { used_by, .. } => used_by.clone(),
            Self::Requires {
                selected, requires, ..
            } => vec![*selected, *requires],
            Self::Rule { terminals, .. } => {
                let set: BTreeSet<TerminalId> = terminals.iter().copied().collect();
                return set.into_iter().collect();
            }
        };
        let set: BTreeSet<TerminalId> = keys.into_iter().map(|k| k.terminal).collect();
        set.into_iter().collect()
    }
}

/// The outcome of [`resolve`], in four buckets.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Resolution {
    /// The active function per terminal, in terminal declaration order.
    pub active: Vec<ActiveFunction>,
    /// Electrical terminals without an active function.
    pub unresolved: Vec<TerminalId>,
    pub conflicts: Vec<Conflict>,
    pub incomplete: Vec<Requirement>,
    /// C1: `warn` verdicts of pin rule scripts; never blocking.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<RuleNote>,
    /// C1: pin rules that gave no verdict (compile error, runtime error,
    /// exhausted budget). Each is a finding that blocks electrical
    /// resolution; it never rejects an edit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_failures: Vec<RuleFailure>,
}

impl Resolution {
    pub fn active(&self, terminal: TerminalId) -> Option<&ActiveFunction> {
        self.active.iter().find(|a| a.terminal == terminal)
    }
}

impl Direction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Bidirectional => "bidirectional",
            Self::Passive => "passive",
            Self::PowerInput => "power input",
            Self::PowerOutput => "power output",
        }
    }
    fn is_power(self) -> bool {
        matches!(self, Self::PowerInput | Self::PowerOutput)
    }
    fn drives(self) -> bool {
        matches!(self, Self::Output | Self::Bidirectional)
    }
}

impl SignalKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Digital => "digital",
            Self::Analog => "analog",
            Self::Power => "power",
            Self::Unspecified => "unspecified",
        }
    }
}

impl Drive {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "no",
            Self::PushPull => "push-pull",
            Self::OpenDrain => "open-drain",
            Self::OpenSource => "open-source",
        }
    }
}

impl Function {
    /// Direction, signal and drive form a valid combination: only outputs and
    /// bidirectional functions drive, and only power directions carry power.
    pub fn combination_valid(&self) -> bool {
        let drive_ok = if self.direction.drives() {
            self.drive != Drive::None
        } else {
            self.drive == Drive::None
        };
        drive_ok && (self.signal == SignalKind::Power) == self.direction.is_power()
    }

    /// The mux selector as people read it: the table and the code ("LP IO
    /// MUX F0"), the code alone ("AF7") when no table is named, the table
    /// alone when it has no code; `None` without either.
    pub fn mux_text(&self) -> Option<String> {
        mux_text(
            self.mux_table.as_deref(),
            self.mux.as_ref().map(|m| m.0.as_str()),
        )
    }
    fn key(&self, terminal: TerminalId) -> FunctionKey {
        FunctionKey {
            terminal,
            function: self.id,
        }
    }
}

/// [`Function::mux_text`] of a table and a code.
pub fn mux_text(table: Option<&str>, code: Option<&str>) -> Option<String> {
    let table = table.map(str::trim).filter(|t| !t.is_empty());
    let code = code.map(str::trim).filter(|c| !c.is_empty());
    match (table, code) {
        (Some(t), Some(c)) => Some(format!("{t} {c}")),
        (Some(t), None) => Some(t.to_owned()),
        (None, Some(c)) => Some(c.to_owned()),
        (None, None) => None,
    }
}

impl TerminalFunctions {
    /// One fixed function with the conventional behavior of `direction`:
    /// push-pull for driving directions, power signal for power directions,
    /// otherwise no drive and an unspecified signal.
    pub fn fixed(id: FunctionId, direction: Direction) -> Self {
        let signal = if direction.is_power() {
            SignalKind::Power
        } else {
            SignalKind::Unspecified
        };
        let drive = if direction.drives() {
            Drive::PushPull
        } else {
            Drive::None
        };
        Self {
            functions: vec![Function {
                id,
                label: direction.label().into(),
                direction,
                signal,
                drive,
                resource_role: None,
                mux: None,
                mux_table: None,
                source: None,
            }],
            design_default: None,
            gpio: None,
            options: Vec::new(),
        }
    }

    pub fn function(&self, id: FunctionId) -> Option<&Function> {
        self.functions.iter().find(|f| f.id == id)
    }

    /// The function used without an explicit selection: the only function of
    /// a fixed terminal, else the design default.
    pub fn implicit(&self) -> Option<(&Function, ActiveOrigin)> {
        match self.functions.as_slice() {
            [only] => Some((only, ActiveOrigin::Fixed)),
            _ => self
                .design_default
                .and_then(|d| self.function(d))
                .map(|f| (f, ActiveOrigin::DesignDefault)),
        }
    }

    /// Persistence bounds a draft must satisfy.
    pub fn within_bounds(&self) -> bool {
        self.functions.len() <= MAX_FUNCTIONS
            && self.functions.iter().all(|f| {
                f.label.len() <= MAX_LABEL
                    && pin_config::function_within_bounds(
                        f.mux.as_ref(),
                        f.mux_table.as_deref(),
                        f.source.as_ref(),
                    )
            })
            && pin_config::terminal_within_bounds(self.gpio.as_ref(), &self.options)
    }

    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.functions.iter().map(|f| f.id.0)
    }
}

impl Resource {
    pub fn role(&self, id: RoleId) -> Option<&Role> {
        self.roles.iter().find(|r| r.id == id)
    }
    /// Persistence bounds a draft must satisfy.
    pub fn within_bounds(&self) -> bool {
        self.label.len() <= MAX_LABEL
            && self.kind.as_ref().is_none_or(|k| k.len() <= MAX_KIND)
            && self.roles.len() <= MAX_ROLES
            && self.roles.iter().all(|r| {
                r.label.len() <= MAX_LABEL && r.signal.as_ref().is_none_or(|s| s.len() <= MAX_KIND)
            })
    }
    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        std::iter::once(self.id.0).chain(self.roles.iter().map(|r| r.id.0))
    }
}

/// Resources and constraints within their persistence bounds.
pub fn declarations_within_bounds(
    resources: &[Resource],
    constraints: &[FunctionConstraint],
) -> bool {
    resources.len() <= MAX_RESOURCES
        && constraints.len() <= MAX_CONSTRAINTS
        && resources.iter().all(Resource::within_bounds)
}

fn publish_issue(code: &str, message: String, objects: Vec<ObjectRef>) -> Finding {
    Finding::blocking(code, message, objects, Stage::Publish)
}

fn local(definition: DefinitionId, local: u32) -> ObjectRef {
    ObjectRef::LibraryObject { definition, local }
}

/// Whether `key` names a function of its own terminal.
fn names_own_function(terminals: &[TerminalDeclaration<'_>], key: FunctionKey) -> bool {
    terminals
        .iter()
        .find(|t| t.terminal == key.terminal)
        .and_then(|t| t.functions)
        .is_some_and(|f| f.function(key.function).is_some())
}

/// A terminal for a message: its quoted name, or its number when it has
/// none or is not declared.
fn terminal_name(terminals: &[TerminalDeclaration<'_>], terminal: TerminalId) -> String {
    terminals
        .iter()
        .find(|t| t.terminal == terminal)
        .map(|t| t.name.trim())
        .filter(|name| !name.is_empty())
        .map_or_else(|| terminal.to_string(), |name| format!("‘{name}’"))
}

/// A function for a message: its quoted label, or "a function" when the
/// terminal does not declare it.
fn function_name(terminals: &[TerminalDeclaration<'_>], key: FunctionKey) -> String {
    terminals
        .iter()
        .find(|t| t.terminal == key.terminal)
        .and_then(|t| t.functions)
        .and_then(|f| f.function(key.function))
        .map_or_else(|| "a function".into(), |f| format!("‘{}’", f.label))
}

fn unique_label(seen: &mut BTreeSet<String>, label: &str) -> bool {
    printable(label) && seen.insert(label.trim().to_lowercase())
}

/// Publication findings of a component's function declarations: function
/// counts, labels and combinations; design defaults in their own list;
/// resources and roles well formed and referenced correctly; constraint keys
/// naming functions of their own terminal; and a conflict-free default
/// resolution. Unique local IDs are the library's local-ID rule.
pub fn declaration_findings(
    component: DefinitionId,
    terminals: &[TerminalDeclaration<'_>],
    resources: &[Resource],
    constraints: &[FunctionConstraint],
) -> Vec<Finding> {
    let mut out = Vec::new();
    let this = ObjectRef::Definition { id: component };
    for declaration in terminals {
        let Some(declared) = declaration.functions else {
            continue;
        };
        let terminal = declaration.terminal;
        let name = terminal_name(terminals, terminal);
        if !(1..=MAX_FUNCTIONS).contains(&declared.functions.len()) {
            out.push(publish_issue(
                "function.count",
                format!("Electrical terminal {name} needs 1–32 functions."),
                vec![local(component, terminal.0)],
            ));
        }
        let mut labels = BTreeSet::new();
        for function in &declared.functions {
            let at = vec![local(component, function.id.0)];
            if !unique_label(&mut labels, &function.label) {
                out.push(publish_issue(
                    "function.label",
                    format!("A function of terminal {name} needs a unique printable label."),
                    at.clone(),
                ));
            }
            if !function.combination_valid() {
                // Lead with the pin, as ERC rows do, so truncated rows differ.
                let pin = Some(declaration.name.trim())
                    .filter(|n| !n.is_empty())
                    .map_or_else(|| terminal.to_string(), str::to_owned);
                out.push(publish_issue(
                    "function.combination",
                    format!(
                        "{pin}: function ‘{}’ is {} with {} signal and {} drive, not a valid combination.",
                        function.label,
                        function.direction.label(),
                        function.signal.label(),
                        function.drive.label()
                    ),
                    at.clone(),
                ));
            }
            if let Some(rr) = function.resource_role {
                let known = resources
                    .iter()
                    .find(|r| r.id == rr.resource)
                    .is_some_and(|r| r.role(rr.role).is_some());
                if !known {
                    out.push(publish_issue(
                        "function.resource_unknown",
                        format!(
                            "Function ‘{}’ of terminal {name} takes a resource role that is not declared.",
                            function.label
                        ),
                        at,
                    ));
                }
            }
        }
        if let Some(default) = declared.design_default
            && declared.function(default).is_none()
        {
            out.push(publish_issue(
                "function.design_default",
                format!("The design default of terminal {name} is not one of its functions."),
                vec![local(component, terminal.0)],
            ));
        }
    }
    let mut resource_labels = BTreeSet::new();
    for resource in resources {
        let at = vec![local(component, resource.id.0)];
        if !unique_label(&mut resource_labels, &resource.label) {
            out.push(publish_issue(
                "resource.label",
                "A resource needs a unique printable label.".into(),
                at.clone(),
            ));
        }
        if !(1..=MAX_ROLES).contains(&resource.roles.len()) {
            out.push(publish_issue(
                "resource.role_count",
                format!("Resource ‘{}’ needs 1–16 roles.", resource.label),
                at,
            ));
        }
        let mut role_labels = BTreeSet::new();
        for role in &resource.roles {
            let at = vec![local(component, role.id.0)];
            if !unique_label(&mut role_labels, &role.label) {
                out.push(publish_issue(
                    "resource.role_label",
                    format!(
                        "A role of resource ‘{}’ needs a unique printable label.",
                        resource.label
                    ),
                    at.clone(),
                ));
            }
            if role.capacity == 0 {
                out.push(publish_issue(
                    "resource.role_capacity",
                    format!(
                        "Role ‘{}’ of resource ‘{}’ needs a capacity of at least 1.",
                        role.label, resource.label
                    ),
                    at,
                ));
            }
        }
    }
    for constraint in constraints {
        let keys = match constraint {
            FunctionConstraint::Excludes { a, b } => [*a, *b],
            FunctionConstraint::Requires { selected, requires } => [*selected, *requires],
        };
        for key in keys {
            if !names_own_function(terminals, key) {
                out.push(publish_issue(
                    "function.constraint_key",
                    format!(
                        "A constraint names a function that terminal {} does not have.",
                        terminal_name(terminals, key.terminal)
                    ),
                    vec![this],
                ));
            }
        }
    }
    // The default resolution must be placeable: no explicit selections, no
    // physical choice. Bindings are covered by `binding_findings`.
    if let Ok(resolution) = resolve(
        terminals,
        resources,
        constraints,
        None,
        &FunctionSelections::new(),
    ) {
        for conflict in &resolution.conflicts {
            out.push(publish_issue(
                "function.default_conflict",
                format!(
                    "Without explicit selections the fixed functions and design defaults conflict: {}",
                    conflict.message()
                ),
                vec![this],
            ));
        }
    }
    out
}

/// Publication findings of one binding a component lists: every unavailable
/// key names a function of its own terminal, and none is a fixed function or
/// a design default, so the default resolution stays available in every
/// listed implementation.
pub fn binding_findings(
    component: DefinitionId,
    binding: DefinitionId,
    terminals: &[TerminalDeclaration<'_>],
    unavailable: &[FunctionKey],
) -> Vec<Finding> {
    let mut out = Vec::new();
    let objects = vec![
        ObjectRef::Definition { id: component },
        ObjectRef::Definition { id: binding },
    ];
    for key in unavailable {
        let declared = terminals
            .iter()
            .find(|t| t.terminal == key.terminal)
            .and_then(|t| t.functions);
        let Some(declared) = declared.filter(|d| d.function(key.function).is_some()) else {
            out.push(publish_issue(
                "binding.unavailable_unknown",
                format!(
                    "A binding marks a function unavailable that terminal {} does not have.",
                    terminal_name(terminals, key.terminal)
                ),
                objects.clone(),
            ));
            continue;
        };
        if declared
            .implicit()
            .is_some_and(|(f, _)| f.id == key.function)
        {
            out.push(publish_issue(
                "binding.unavailable_required",
                format!(
                    "A binding marks the fixed function or design default {} of terminal {} unavailable.",
                    function_name(terminals, *key),
                    terminal_name(terminals, key.terminal)
                ),
                objects.clone(),
            ));
        }
    }
    out
}

/// Apply 1–256 edits to a copy of `selections`. A terminal may be edited
/// once per batch; the final state is validated once by [`resolve`], so a
/// batch can move a role between terminals without an intermediate state.
pub fn apply_edits(
    selections: &FunctionSelections,
    edits: &[FunctionEdit],
) -> Result<FunctionSelections> {
    if edits.is_empty() || edits.len() > MAX_FUNCTION_EDITS {
        return Err(Error::Invalid(
            "Change between 1 and 256 function selections at a time.".into(),
        ));
    }
    let mut next = selections.clone();
    let mut seen = BTreeSet::new();
    for edit in edits {
        if !seen.insert(edit.terminal) {
            return Err(Error::Invalid(format!(
                "Terminal number {} is edited twice in one batch.",
                edit.terminal
            )));
        }
        match edit.function {
            Some(function) => next.insert(edit.terminal, function),
            None => next.remove(&edit.terminal),
        };
    }
    Ok(next)
}

/// The one resolution rule. Each electrical terminal's active function is,
/// in order: its explicit selection, its only function, its design default;
/// otherwise it is unresolved. Structural errors (an unknown terminal, a
/// function of another terminal, a selection on a terminal without
/// functions) are `invalid_input`. Capacity, exclusion and availability
/// violations are conflicts; missing required roles and unmet requirements
/// are incomplete.
pub fn resolve(
    terminals: &[TerminalDeclaration<'_>],
    resources: &[Resource],
    constraints: &[FunctionConstraint],
    unavailable: Option<&[FunctionKey]>,
    selections: &FunctionSelections,
) -> Result<Resolution> {
    for (terminal, function) in selections {
        let declaration = terminals
            .iter()
            .find(|t| t.terminal == *terminal)
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "A selection names an unknown terminal ({terminal})."
                ))
            })?;
        let declared = declaration.functions.ok_or_else(|| {
            Error::Invalid(format!(
                "Terminal {} is not electrical and takes no function.",
                terminal_name(terminals, *terminal)
            ))
        })?;
        if declared.function(*function).is_none() {
            return Err(Error::Invalid(format!(
                "Terminal {} has no such function.",
                terminal_name(terminals, *terminal)
            )));
        }
    }
    let mut resolution = Resolution::default();
    let mut roles: BTreeMap<(ResourceId, RoleId), Vec<FunctionKey>> = BTreeMap::new();
    for declaration in terminals {
        let Some(declared) = declaration.functions else {
            continue;
        };
        let terminal = declaration.terminal;
        let chosen = match selections.get(&terminal) {
            Some(id) => declared.function(*id).map(|f| (f, ActiveOrigin::Selected)),
            None => declared.implicit(),
        };
        let Some((function, origin)) = chosen else {
            resolution.unresolved.push(terminal);
            continue;
        };
        if let Some(rr) = function.resource_role {
            roles
                .entry((rr.resource, rr.role))
                .or_default()
                .push(function.key(terminal));
        }
        resolution.active.push(ActiveFunction {
            terminal,
            function: function.id,
            label: function.label.clone(),
            direction: function.direction,
            signal: function.signal,
            drive: function.drive,
            origin,
        });
    }
    let label = |key: FunctionKey| {
        resolution
            .active(key.terminal)
            .filter(|a| a.function == key.function)
            .map_or_else(|| key.function.to_string(), |a| a.label.clone())
    };
    let active = |key: FunctionKey| {
        resolution
            .active(key.terminal)
            .is_some_and(|a| a.function == key.function)
    };
    let mut conflicts = Vec::new();
    let mut incomplete = Vec::new();
    for resource in resources {
        let used: Vec<FunctionKey> = resource
            .roles
            .iter()
            .flat_map(|r| roles.get(&(resource.id, r.id)).into_iter().flatten())
            .copied()
            .collect();
        for role in &resource.roles {
            let holders = roles
                .get(&(resource.id, role.id))
                .cloned()
                .unwrap_or_default();
            let missing = role.required_when_used && holders.is_empty() && !used.is_empty();
            if holders.len() > usize::from(role.capacity) {
                let names: Vec<String> = holders
                    .iter()
                    .map(|k| {
                        format!(
                            "‘{}’ on terminal {}",
                            label(*k),
                            terminal_name(terminals, k.terminal)
                        )
                    })
                    .collect();
                conflicts.push(Conflict::Capacity {
                    resource: resource.id,
                    role: role.id,
                    capacity: role.capacity,
                    message: format!(
                        "Role ‘{}’ of ‘{}’ has capacity {} but {} active functions take it: {}.",
                        role.label,
                        resource.label,
                        role.capacity,
                        holders.len(),
                        names.join(", ")
                    ),
                    functions: holders,
                });
            }
            if missing {
                incomplete.push(Requirement::RoleMissing {
                    resource: resource.id,
                    role: role.id,
                    message: format!(
                        "‘{}’ is in use but its required role ‘{}’ is not active.",
                        resource.label, role.label
                    ),
                    used_by: used.clone(),
                });
            }
        }
    }
    for constraint in constraints {
        match constraint {
            FunctionConstraint::Excludes { a, b } if active(*a) && active(*b) => {
                conflicts.push(Conflict::Excluded {
                    a: *a,
                    b: *b,
                    message: format!(
                        "‘{}’ on terminal {} and ‘{}’ on terminal {} cannot be active together.",
                        label(*a),
                        terminal_name(terminals, a.terminal),
                        label(*b),
                        terminal_name(terminals, b.terminal)
                    ),
                });
            }
            FunctionConstraint::Requires { selected, requires }
                if active(*selected) && !active(*requires) =>
            {
                incomplete.push(Requirement::Requires {
                    selected: *selected,
                    requires: *requires,
                    message: format!(
                        "‘{}’ on terminal {} requires {} on terminal {}.",
                        label(*selected),
                        terminal_name(terminals, selected.terminal),
                        function_name(terminals, *requires),
                        terminal_name(terminals, requires.terminal)
                    ),
                });
            }
            _ => {}
        }
    }
    for key in unavailable.into_iter().flatten() {
        if active(*key) {
            conflicts.push(Conflict::Unavailable {
                function: *key,
                message: format!(
                    "‘{}’ on terminal {} is not available in the chosen implementation.",
                    label(*key),
                    terminal_name(terminals, key.terminal)
                ),
            });
        }
    }
    resolution.conflicts = conflicts;
    resolution.incomplete = incomplete;
    Ok(resolution)
}

/// [`resolve`] followed by the component's pin rules (founder decision
/// F2): every rule is evaluated over the resolved assignment and the
/// instance's pin configuration within the default [`rules::RuleBudget`],
/// and its verdict lands in the buckets (`forbid` → conflicts, `require`
/// → incomplete, `warn` → notes, failures → `rule_failures`). Rules
/// compile once per distinct source set ([`rules::compile`]).
pub fn resolve_with_rules(
    terminals: &[TerminalDeclaration<'_>],
    resources: &[Resource],
    constraints: &[FunctionConstraint],
    unavailable: Option<&[FunctionKey]>,
    selections: &FunctionSelections,
    pin_rules: &[PinRule],
    config: &PinConfigs,
) -> Result<Resolution> {
    let mut resolution = resolve(terminals, resources, constraints, unavailable, selections)?;
    if pin_rules.is_empty() {
        return Ok(resolution);
    }
    let compiled = rules::compile(pin_rules)?;
    let view = rules::view(terminals, resources, &resolution, config);
    let evaluation = rules::evaluate(&compiled, &view, rules::RuleBudget::default())?;
    rules::apply(&mut resolution, &compiled, &evaluation);
    Ok(resolution)
}

/// The explain/test panel: resolve a scratch `selections` with the rules
/// as [`resolve_with_rules`] does and report each rule's own verdict or
/// failure next to the resolution.
pub fn explain_rules(
    terminals: &[TerminalDeclaration<'_>],
    resources: &[Resource],
    constraints: &[FunctionConstraint],
    selections: &FunctionSelections,
    pin_rules: &[PinRule],
    config: &PinConfigs,
) -> Result<(Resolution, rules::RuleExplanations)> {
    let mut resolution = resolve(terminals, resources, constraints, None, selections)?;
    let compiled = rules::compile(pin_rules)?;
    let view = rules::view(terminals, resources, &resolution, config);
    let evaluation = rules::evaluate(&compiled, &view, rules::RuleBudget::default())?;
    rules::apply(&mut resolution, &compiled, &evaluation);
    Ok((resolution, rules::explanations(&compiled, &evaluation)))
}
