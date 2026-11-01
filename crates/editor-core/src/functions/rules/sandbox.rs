//! The Rhai sandbox of pin rules: one engine configuration for compiling
//! and evaluating, the read-only view index the helpers read, and the
//! helper functions themselves. Rhai values never leave this module.
//!
//! The engine starts raw (no packages) and gets only Rhai's standard
//! package (arithmetic, logic, strings, arrays, maps), shared once per
//! process. `eval`, `print` and `debug` are disabled; the crate builds
//! Rhai without modules or time, and nothing here registers I/O or
//! randomness, so a rule is a deterministic function of its view.
use super::{
    MAX_RULE_ARRAY, MAX_RULE_CALL_DEPTH, MAX_RULE_MAP, MAX_RULE_OPERATIONS, MAX_RULE_STRING,
    MAX_VERDICT_MESSAGE, RuleView, ViewResource, ViewTerminal,
};
use crate::functions::pin_config::{Level, Pull};
use crate::ids::TerminalId;
use rhai::packages::{Package, StandardPackage};
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Position, Shared};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

type ScriptResult<T> = std::result::Result<T, Box<EvalAltResult>>;

/// Script-defined functions one rule may declare.
const MAX_RULE_FUNCTIONS: usize = 16;
/// Variables one rule may hold at once.
const MAX_RULE_VARIABLES: usize = 256;
/// Expression nesting depth (top level, inside functions).
const MAX_EXPR_DEPTH: usize = 64;
const MAX_FN_EXPR_DEPTH: usize = 32;

/// What a verdict helper builds. The script's last expression must be one.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ScriptVerdict {
    Ok,
    Forbid(String, Vec<TerminalId>),
    Require(String, Vec<TerminalId>),
    Warn(String, Vec<TerminalId>),
}

/// A helper's argument names nothing in the view: a reference error, which
/// publishing reports as `rule.reference`.
#[derive(Clone, Debug)]
pub(super) struct ReferenceError(pub String);

fn reference(message: String) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(Dynamic::from(ReferenceError(message)), Position::NONE).into()
}

fn failure(message: &str) -> Box<EvalAltResult> {
    message.into()
}

fn standard() -> Shared<rhai::Module> {
    static PACKAGE: OnceLock<Shared<rhai::Module>> = OnceLock::new();
    PACKAGE
        .get_or_init(|| StandardPackage::new().as_shared_module())
        .clone()
}

/// The sandboxed engine without helpers: what compiling needs.
pub(super) fn engine() -> Engine {
    let mut engine = Engine::new_raw();
    engine.register_global_module(standard());
    engine
        .set_max_operations(MAX_RULE_OPERATIONS)
        .set_max_call_levels(MAX_RULE_CALL_DEPTH)
        .set_max_expr_depths(MAX_EXPR_DEPTH, MAX_FN_EXPR_DEPTH)
        .set_max_variables(MAX_RULE_VARIABLES)
        .set_max_functions(MAX_RULE_FUNCTIONS)
        .set_max_string_size(MAX_RULE_STRING)
        .set_max_array_size(MAX_RULE_ARRAY)
        .set_max_map_size(MAX_RULE_MAP)
        .set_strict_variables(true)
        .set_allow_anonymous_fn(false)
        .disable_symbol("eval")
        .disable_symbol("print")
        .disable_symbol("debug");
    engine
}

/// The view, indexed for the helpers.
pub(super) struct ViewIndex {
    view: RuleView,
    /// Trimmed terminal name → terminal positions.
    names: BTreeMap<String, Vec<usize>>,
    /// Lower-case peripheral label → resource position.
    resources: BTreeMap<String, usize>,
}

fn fold(text: &str) -> String {
    text.trim().to_lowercase()
}

impl ViewIndex {
    pub(super) fn new(view: RuleView) -> Self {
        let mut names: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, t) in view.terminals.iter().enumerate() {
            names.entry(t.name.trim().to_owned()).or_default().push(i);
        }
        let resources = view
            .resources
            .iter()
            .enumerate()
            .map(|(i, r)| (fold(&r.label), i))
            .collect();
        Self {
            view,
            names,
            resources,
        }
    }

    /// A terminal by its exact (trimmed) name, or by number as `#12`.
    pub(super) fn terminal(&self, name: &str) -> Result<&ViewTerminal, String> {
        let name = name.trim();
        if let Some(number) = name.strip_prefix('#') {
            return number
                .parse::<u32>()
                .ok()
                .and_then(|n| self.view.terminals.iter().find(|t| t.terminal.0 == n))
                .ok_or_else(|| format!("No terminal has number ‘{name}’."));
        }
        match self.names.get(name).map(Vec::as_slice) {
            Some([only]) => self
                .view
                .terminals
                .get(*only)
                .ok_or_else(|| format!("No terminal is named ‘{name}’.")),
            Some([_, _, ..]) => Err(format!(
                "Several terminals are named ‘{name}’; name one by number, as ‘#12’."
            )),
            _ => Err(format!("No terminal is named ‘{name}’.")),
        }
    }

    /// A function a terminal declares, matched without case.
    pub(super) fn declares(&self, terminal: &ViewTerminal, function: &str) -> Result<(), String> {
        let wanted = fold(function);
        if terminal.functions.iter().any(|f| fold(f) == wanted) {
            Ok(())
        } else {
            Err(format!(
                "Terminal ‘{}’ has no function ‘{}’.",
                terminal.name.trim(),
                function.trim()
            ))
        }
    }

    /// `"TERM.function"`: tries each dot from the right, so terminal names
    /// with dots (`P1.2.uart_tx`) resolve when the function label has none.
    pub(super) fn function_ref(&self, text: &str) -> Result<(&ViewTerminal, String), String> {
        let mut first = None;
        for (at, _) in text.rmatch_indices('.') {
            let name = text.get(..at).unwrap_or_default();
            let function = text.get(at + 1..).unwrap_or_default();
            match self
                .terminal(name)
                .and_then(|t| self.declares(t, function).map(|()| t))
            {
                Ok(terminal) => return Ok((terminal, function.to_owned())),
                Err(error) => {
                    first.get_or_insert(error);
                }
            }
        }
        Err(first
            .unwrap_or_else(|| format!("‘{}’ is not written as TERMINAL.function.", text.trim())))
    }

    pub(super) fn is_active(terminal: &ViewTerminal, function: &str) -> bool {
        let wanted = fold(function);
        terminal
            .active
            .as_ref()
            .is_some_and(|a| fold(&a.label) == wanted)
    }

    pub(super) fn resource(&self, label: &str) -> Result<&ViewResource, String> {
        self.resources
            .get(&fold(label))
            .and_then(|i| self.view.resources.get(*i))
            .ok_or_else(|| format!("No peripheral is named ‘{}’.", label.trim()))
    }

    pub(super) fn role(&self, resource: &str, role: &str) -> Result<bool, String> {
        let r = self.resource(resource)?;
        let wanted = fold(role);
        if !r.roles.iter().any(|x| fold(x) == wanted) {
            return Err(format!(
                "Peripheral ‘{}’ has no role ‘{}’.",
                r.label.trim(),
                role.trim()
            ));
        }
        Ok(r.active_roles.iter().any(|x| fold(x) == wanted))
    }

    /// One configuration value as text; "" when unset.
    pub(super) fn option(terminal: &ViewTerminal, name: &str) -> Result<String, String> {
        let config = terminal.config.clone().unwrap_or_default();
        Ok(match fold(name).as_str() {
            "pull" => config
                .pull
                .map(|p| match p {
                    Pull::None => "none",
                    Pull::Up => "up",
                    Pull::Down => "down",
                })
                .unwrap_or_default()
                .into(),
            "speed" => config.speed.unwrap_or_default(),
            "initial" => config
                .initial
                .map(|l| match l {
                    Level::Low => "low",
                    Level::High => "high",
                })
                .unwrap_or_default()
                .into(),
            "schmitt" => config.schmitt.map(|s| s.to_string()).unwrap_or_default(),
            "name" => config.name.unwrap_or_default(),
            _ => {
                return Err(format!(
                    "‘{}’ is not a pin option; use pull, speed, initial, schmitt or name.",
                    name.trim()
                ));
            }
        })
    }
}

fn message(text: &str) -> ScriptResult<String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(failure("A verdict needs a message."));
    }
    if text.len() > MAX_VERDICT_MESSAGE {
        return Err(failure("A verdict message exceeds 512 bytes."));
    }
    Ok(text.to_owned())
}

fn terminals(index: &ViewIndex, names: &Array) -> ScriptResult<Vec<TerminalId>> {
    let mut out = Vec::new();
    for name in names {
        let name = name
            .clone()
            .into_immutable_string()
            .map_err(|_| failure("A verdict names terminals as strings."))?;
        let id = index.terminal(&name).map_err(reference)?.terminal;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

fn label_of(terminal: &ViewTerminal, function: &str) -> String {
    terminal
        .functions
        .iter()
        .find(|f| fold(f) == fold(function))
        .map_or_else(|| function.trim().to_owned(), |f| f.trim().to_owned())
}

fn pair(a: TerminalId, b: TerminalId) -> Vec<TerminalId> {
    if a == b { vec![a] } else { vec![a, b] }
}

/// The view the helpers of one engine read: swapped per evaluation, so a
/// search builds its engine once and evaluates many views with it.
pub(super) type ViewSlot = Arc<std::sync::RwLock<Arc<ViewIndex>>>;

/// The view in `slot`; a poisoned lock (a panic elsewhere) is a runtime
/// failure, never a panic here.
fn load(slot: &ViewSlot) -> ScriptResult<Arc<ViewIndex>> {
    slot.read()
        .map(|view| view.clone())
        .map_err(|_| failure("The rule view is unavailable."))
}

/// The engine with every helper registered over the view in `slot`, and the counter
/// its progress callback updates with the operations of the current run.
pub(super) fn evaluator(slot: ViewSlot) -> (Engine, Arc<AtomicU64>) {
    let mut engine = engine();
    let spent = Arc::new(AtomicU64::new(0));
    let counter = spent.clone();
    engine.on_progress(move |ops| {
        counter.store(ops, Ordering::Relaxed);
        None
    });
    engine.register_type_with_name::<ScriptVerdict>("Verdict");
    engine.register_fn("ok", || ScriptVerdict::Ok);
    type Make = fn(String, Vec<TerminalId>) -> ScriptVerdict;
    let kinds: [(&str, Make); 3] = [
        ("forbid", ScriptVerdict::Forbid),
        ("require", ScriptVerdict::Require),
        ("warn", ScriptVerdict::Warn),
    ];
    for (name, make) in kinds {
        engine.register_fn(name, move |text: ImmutableString| -> ScriptResult<_> {
            Ok(make(message(&text)?, Vec::new()))
        });
        let s = slot.clone();
        engine.register_fn(
            name,
            move |text: ImmutableString, names: Array| -> ScriptResult<_> {
                let i = load(&s)?;
                Ok(make(message(&text)?, terminals(&i, &names)?))
            },
        );
    }
    let s = slot.clone();
    engine.register_fn(
        "active",
        move |terminal: ImmutableString, function: ImmutableString| -> ScriptResult<bool> {
            let i = load(&s)?;
            let t = i.terminal(&terminal).map_err(reference)?;
            i.declares(t, &function).map_err(reference)?;
            Ok(ViewIndex::is_active(t, &function))
        },
    );
    let s = slot.clone();
    engine.register_fn(
        "active",
        move |fref: ImmutableString| -> ScriptResult<bool> {
            let i = load(&s)?;
            let (t, f) = i.function_ref(&fref).map_err(reference)?;
            Ok(ViewIndex::is_active(t, &f))
        },
    );
    let s = slot.clone();
    engine.register_fn("active_count", move |frefs: Array| -> ScriptResult<i64> {
        let i = load(&s)?;
        let mut count = 0_i64;
        for fref in frefs {
            let fref = fref
                .into_immutable_string()
                .map_err(|_| failure("active_count takes an array of strings."))?;
            let (t, f) = i.function_ref(&fref).map_err(reference)?;
            if ViewIndex::is_active(t, &f) {
                count = count.saturating_add(1);
            }
        }
        Ok(count)
    });
    let s = slot.clone();
    engine.register_fn(
        "function",
        move |terminal: ImmutableString| -> ScriptResult<String> {
            let i = load(&s)?;
            let t = i.terminal(&terminal).map_err(reference)?;
            Ok(t.active
                .as_ref()
                .map(|a| a.label.clone())
                .unwrap_or_default())
        },
    );
    let s = slot.clone();
    engine.register_fn(
        "used",
        move |resource: ImmutableString| -> ScriptResult<bool> {
            let i = load(&s)?;
            Ok(!i
                .resource(&resource)
                .map_err(reference)?
                .active_roles
                .is_empty())
        },
    );
    let s = slot.clone();
    engine.register_fn(
        "kind",
        move |resource: ImmutableString| -> ScriptResult<String> {
            let i = load(&s)?;
            Ok(i.resource(&resource)
                .map_err(reference)?
                .kind
                .clone()
                .unwrap_or_default())
        },
    );
    let s = slot.clone();
    engine.register_fn(
        "role",
        move |resource: ImmutableString, role: ImmutableString| -> ScriptResult<bool> {
            let i = load(&s)?;
            i.role(&resource, &role).map_err(reference)
        },
    );
    let s = slot.clone();
    engine.register_fn(
        "option",
        move |terminal: ImmutableString, name: ImmutableString| -> ScriptResult<String> {
            let i = load(&s)?;
            let t = i.terminal(&terminal).map_err(reference)?;
            ViewIndex::option(t, &name).map_err(reference)
        },
    );
    let s = slot.clone();
    engine.register_fn(
        "excludes",
        move |a: ImmutableString, b: ImmutableString| -> ScriptResult<ScriptVerdict> {
            let i = load(&s)?;
            let (ta, fa) = i.function_ref(&a).map_err(reference)?;
            let (tb, fb) = i.function_ref(&b).map_err(reference)?;
            if !(ViewIndex::is_active(ta, &fa) && ViewIndex::is_active(tb, &fb)) {
                return Ok(ScriptVerdict::Ok);
            }
            let text = format!(
                "‘{}’ on terminal ‘{}’ and ‘{}’ on terminal ‘{}’ cannot be active together.",
                label_of(ta, &fa),
                ta.name.trim(),
                label_of(tb, &fb),
                tb.name.trim()
            );
            Ok(ScriptVerdict::Forbid(
                message(&text)?,
                pair(ta.terminal, tb.terminal),
            ))
        },
    );
    let s = slot;
    engine.register_fn(
        "requires",
        move |a: ImmutableString, b: ImmutableString| -> ScriptResult<ScriptVerdict> {
            let i = load(&s)?;
            let (ta, fa) = i.function_ref(&a).map_err(reference)?;
            let (tb, fb) = i.function_ref(&b).map_err(reference)?;
            if !ViewIndex::is_active(ta, &fa) || ViewIndex::is_active(tb, &fb) {
                return Ok(ScriptVerdict::Ok);
            }
            let text = format!(
                "‘{}’ on terminal ‘{}’ requires ‘{}’ on terminal ‘{}’.",
                label_of(ta, &fa),
                ta.name.trim(),
                label_of(tb, &fb),
                tb.name.trim()
            );
            Ok(ScriptVerdict::Require(
                message(&text)?,
                pair(ta.terminal, tb.terminal),
            ))
        },
    );
    (engine, spent)
}

/// How an evaluation error is reported.
pub(super) enum ErrorClass {
    Budget(String),
    Reference(String),
    Runtime(String),
}

pub(super) fn classify(error: &EvalAltResult) -> ErrorClass {
    let position = error.position();
    let at = |text: String| {
        if position.is_none() {
            text
        } else {
            format!("{text} ({position})")
        }
    };
    match error.unwrap_inner() {
        EvalAltResult::ErrorTooManyOperations(..) | EvalAltResult::ErrorTerminated(..) => {
            ErrorClass::Budget(at("The rule ran out of operations.".into()))
        }
        EvalAltResult::ErrorStackOverflow(..) => {
            ErrorClass::Budget(at("The rule calls functions too deeply.".into()))
        }
        EvalAltResult::ErrorDataTooLarge(what, ..) => {
            ErrorClass::Budget(at(format!("The rule exceeds a size limit ({what}).")))
        }
        EvalAltResult::ErrorTooManyVariables(..) => {
            ErrorClass::Budget(at("The rule holds too many variables.".into()))
        }
        EvalAltResult::ErrorRuntime(value, ..) if value.is::<ReferenceError>() => {
            let text = value
                .clone()
                .try_cast::<ReferenceError>()
                .map(|r| r.0)
                .unwrap_or_default();
            ErrorClass::Reference(at(text))
        }
        EvalAltResult::ErrorRuntime(value, ..) if value.is_string() => {
            ErrorClass::Runtime(at(value.to_string()))
        }
        _ => ErrorClass::Runtime(error.to_string()),
    }
}
