//! Rhai feasibility slice, API version 1. This module owns the interpreter;
//! public requests/results contain only ordinary Rust/serde types.
//! No file, network, clock, module-loading or document-mutation APIs are exposed.
//! Native callbacks are bounded independently of the interpreter operation cap.
mod quantity;
#[cfg(test)]
mod tests;
pub use quantity::{Dimension, Quantity, UnitDefinition};

use crate::diagnostics::{Error, Result};
use crate::ids::PadId;
use crate::library::{self, Definition, DefinitionMeta, FootprintDefinition, Pad};
use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Scope};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

const MAX_SOURCE: usize = 16_384;
const MAX_DEFINITIONS: usize = 64;
const MAX_PADS: usize = 128;
const MAX_OPERATIONS: u64 = 50_000;
type ScriptResult<T> = std::result::Result<T, Box<EvalAltResult>>;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", deny_unknown_fields, rename_all = "snake_case")]
pub enum Input {
    Quantity { value: f64, unit: String },
    Integer { value: i64 },
}

/// Explicit, versioned inputs. Units and values can come from library fields;
/// no component-specific defaults or ambient project state are consulted.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub api_version: u32,
    pub source: String,
    pub units: Vec<UnitDefinition>,
    pub inputs: BTreeMap<String, Input>,
}

fn failure(message: &str) -> Box<EvalAltResult> {
    message.into()
}

fn callback_error(error: Error) -> Box<EvalAltResult> {
    error.to_string().into()
}

fn script_error(error: Box<EvalAltResult>) -> Error {
    if matches!(
        error.unwrap_inner(),
        EvalAltResult::ErrorTooManyOperations(..)
            | EvalAltResult::ErrorTooManyVariables(..)
            | EvalAltResult::ErrorStackOverflow(..)
            | EvalAltResult::ErrorDataTooLarge(..)
            | EvalAltResult::ErrorTerminated(..)
    ) {
        Error::Limit(format!("script execution limit: {error}"))
    } else {
        Error::Invalid(format!("script: {error}"))
    }
}

fn make_quantity(
    units: &BTreeMap<String, UnitDefinition>,
    value: f64,
    unit: &str,
) -> Result<Quantity> {
    if unit.len() > 32 {
        return Err(Error::Invalid("unit symbol exceeds 32 bytes".into()));
    }
    let definition = units
        .get(unit)
        .ok_or_else(|| Error::Invalid("unknown unit".into()))?;
    Quantity::new(value, definition.dimension)?.scale(definition.scale)
}

fn prepare(script: &Script) -> Result<(Engine, Scope<'static>)> {
    if script.api_version != 1 {
        return Err(Error::Unsupported("unsupported script API version".into()));
    }
    if script.source.len() > MAX_SOURCE
        || script.units.len() > MAX_DEFINITIONS
        || script.inputs.len() > MAX_DEFINITIONS
    {
        return Err(Error::Limit(
            "script source, units or input count exceeds limit".into(),
        ));
    }
    let mut units = BTreeMap::new();
    for unit in &script.units {
        if unit.symbol.is_empty()
            || unit.symbol.len() > 32
            || unit.symbol.chars().any(char::is_control)
            || !unit.scale.is_finite()
            || !(1e-30..=1e30).contains(&unit.scale)
        {
            return Err(Error::Invalid("invalid linear unit definition".into()));
        }
        Quantity::new(0.0, unit.dimension)?;
        if units.insert(unit.symbol.clone(), unit.clone()).is_some() {
            return Err(Error::Invalid("duplicate unit symbol".into()));
        }
    }
    let mut scope = Scope::new();
    for (name, input) in &script.inputs {
        if name.is_empty()
            || name.len() > 64
            || name == "footprint"
            || !name.starts_with(|c: char| c.is_ascii_alphabetic())
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(Error::Invalid("invalid or reserved input name".into()));
        }
        match input {
            Input::Quantity { value, unit } => {
                scope.push_constant(name.clone(), make_quantity(&units, *value, unit)?);
            }
            Input::Integer { value } => {
                scope.push_constant(name.clone(), *value);
            }
        }
    }
    let mut engine = Engine::new();
    engine
        .set_max_operations(MAX_OPERATIONS)
        .set_max_call_levels(16)
        .set_max_expr_depths(32, 32)
        .set_max_variables(128)
        .set_max_functions(16)
        .set_max_string_size(4096)
        .set_max_array_size(128)
        .set_max_map_size(128)
        .set_allow_anonymous_fn(false)
        .set_allow_shadowing(false)
        .disable_symbol("eval")
        .disable_symbol("print")
        .disable_symbol("debug");
    engine.register_type_with_name::<Quantity>("Quantity");
    let units = Arc::new(units);
    engine.register_fn("q", move |value: f64, unit: ImmutableString| {
        make_quantity(&units, value, &unit).map_err(callback_error)
    });
    engine.register_fn("+", |a: Quantity, b: Quantity| {
        a.add(b).map_err(callback_error)
    });
    engine.register_fn("-", |a: Quantity, b: Quantity| {
        a.sub(b).map_err(callback_error)
    });
    engine.register_fn("*", |a: Quantity, b: Quantity| {
        a.product(b, false).map_err(callback_error)
    });
    engine.register_fn("/", |a: Quantity, b: Quantity| {
        a.product(b, true).map_err(callback_error)
    });
    engine.register_fn("*", |a: Quantity, b: f64| {
        a.scale(b).map_err(callback_error)
    });
    engine.register_fn("*", |a: Quantity, b: i64| {
        a.scale_integer(b).map_err(callback_error)
    });
    engine.register_fn("*", |a: f64, b: Quantity| {
        b.scale(a).map_err(callback_error)
    });
    engine.register_fn("*", |a: i64, b: Quantity| {
        b.scale_integer(a).map_err(callback_error)
    });
    Ok((engine, scope))
}

/// Pure expression evaluation. No assignments/loops or document access.
pub fn evaluate_quantity(script: &Script) -> Result<Quantity> {
    let (engine, mut scope) = prepare(script)?;
    engine
        .eval_expression_with_scope::<Quantity>(&mut scope, &script.source)
        .map_err(script_error)
}

#[derive(Clone)]
struct PadArray {
    allowed: Arc<BTreeMap<PadId, Pad>>,
    generated: BTreeMap<PadId, Pad>,
}

impl PadArray {
    fn pad(
        &mut self,
        id: i64,
        x: Quantity,
        y: Quantity,
        width: Quantity,
        height: Quantity,
    ) -> ScriptResult<()> {
        let id = u32::try_from(id)
            .map(PadId)
            .map_err(|_| failure("pad ID must be positive"))?;
        if self.generated.contains_key(&id) {
            return Err(failure("pad ID generated more than once"));
        }
        if self.generated.len() >= MAX_PADS {
            return Err(failure("generated pad count exceeds limit"));
        }
        let mut pad = self
            .allowed
            .get(&id)
            .cloned()
            .ok_or_else(|| failure("unknown pad ID"))?;
        pad.position = [
            x.millimetres().map_err(callback_error)?,
            y.millimetres().map_err(callback_error)?,
        ];
        pad.size = [
            width.millimetres().map_err(callback_error)?,
            height.millimetres().map_err(callback_error)?,
        ];
        if pad.size.iter().any(|v| *v < 0.01) {
            return Err(failure("pad dimensions must be at least 0.01 mm"));
        }
        self.generated.insert(id, pad);
        Ok(())
    }
}

/// Produces a candidate only. Callers commit through `library.generate_pads`.
/// This proof emits each existing pad of a footprint draft exactly once,
/// preserving pad IDs and package-terminal associations and allocating
/// nothing.
pub fn generate_pads(script: &Script, draft: &FootprintDefinition) -> Result<FootprintDefinition> {
    // Check bounds before copying data into interpreter-owned values.
    let bounded = Definition::Footprint(draft.clone());
    library::check_bounds(&DefinitionMeta::default(), &bounded)?;
    if draft.pads.len() > MAX_PADS {
        return Err(Error::Limit(
            "generator proof supports at most 128 pads".into(),
        ));
    }
    let mut ids = std::collections::BTreeSet::new();
    if draft.pads.is_empty() || !draft.pads.iter().all(|p| p.id.0 > 0 && ids.insert(p.id)) {
        return Err(Error::Invalid(
            "pad generation needs existing pads with unique nonzero IDs".into(),
        ));
    }
    let (mut engine, mut scope) = prepare(script)?;
    engine.register_type_with_name::<PadArray>("PadArray");
    engine.register_fn("pad", PadArray::pad);
    scope.push(
        "footprint",
        PadArray {
            allowed: Arc::new(draft.pads.iter().map(|p| (p.id, p.clone())).collect()),
            generated: BTreeMap::new(),
        },
    );
    let _ = engine
        .eval_with_scope::<Dynamic>(&mut scope, &script.source)
        .map_err(script_error)?;
    let output = scope
        .get_value::<PadArray>("footprint")
        .ok_or_else(|| Error::Invalid("script replaced footprint output".into()))?;
    if output.generated.len() != draft.pads.len() {
        return Err(Error::Invalid(
            "generator must emit every existing pad exactly once".into(),
        ));
    }
    let mut candidate = draft.clone();
    for pad in &mut candidate.pads {
        *pad = output
            .generated
            .get(&pad.id)
            .cloned()
            .ok_or_else(|| Error::Invalid("missing generated pad".into()))?;
    }
    library::check_bounds(
        &DefinitionMeta::default(),
        &Definition::Footprint(candidate.clone()),
    )?;
    Ok(candidate)
}
