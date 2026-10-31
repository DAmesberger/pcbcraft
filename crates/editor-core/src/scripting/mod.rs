//! Rhai feasibility slice, API version 1. This module owns the interpreter;
//! public requests/results contain only ordinary Rust/serde types.
//! No file, network, clock, module-loading or document-mutation APIs are exposed.
//! Native callbacks are bounded independently of the interpreter operation cap.
//! Quantity arithmetic and unit scaling are `crate::quantity`'s; this module
//! only registers them with the interpreter, and Rhai values never leave it.
#[cfg(test)]
mod tests;

use crate::diagnostics::{Error, Result};
use crate::ids::PadId;
use crate::library::{self, Definition, DefinitionMeta, FootprintDefinition, Pad};
use crate::quantity::{LENGTH, Quantity, UnitSet};
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
    /// Unit sets; every symbol and alias is unique across the sets.
    pub units: Vec<UnitSet>,
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

/// Scale through the one set that names `unit`: an exact symbol or alias
/// first, then a prefixed unit, which must not be ambiguous across sets.
fn make_quantity(sets: &[UnitSet], value: f64, unit: &str) -> Result<Quantity> {
    if unit.len() > 32 {
        return Err(Error::Invalid("unit symbol exceeds 32 bytes".into()));
    }
    let set = match sets.iter().find(|s| s.unit(unit).is_some()) {
        Some(set) => set,
        None => {
            let mut prefixed = sets.iter().filter(|s| s.resolve(unit).is_some());
            match (prefixed.next(), prefixed.next()) {
                (Some(set), None) => set,
                (Some(_), Some(_)) => {
                    return Err(Error::Invalid(format!("ambiguous unit ‘{unit}’")));
                }
                (None, _) => return Err(Error::Invalid("unknown unit".into())),
            }
        }
    };
    set.to_si(value, unit)
}

/// Pad geometry is a length in millimetres within ±10000 mm.
fn millimetres(quantity: &Quantity) -> Result<f64> {
    if quantity.dimension() != LENGTH {
        return Err(Error::Invalid(
            "pad geometry requires a length quantity".into(),
        ));
    }
    let mm = quantity.value() * 1000.0;
    if !mm.is_finite() || mm.abs() > 10000.0 {
        return Err(Error::Invalid("pad geometry exceeds ±10000 mm".into()));
    }
    Ok(mm)
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
    let mut texts = std::collections::BTreeSet::new();
    for set in &script.units {
        set.check()?;
        for text in set.texts() {
            if !texts.insert(text) {
                return Err(Error::Invalid("duplicate unit symbol".into()));
            }
        }
    }
    let units = &script.units;
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
                scope.push_constant(name.clone(), make_quantity(units, *value, unit)?);
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
    let units = Arc::new(script.units.clone());
    engine.register_fn("q", move |value: f64, unit: ImmutableString| {
        make_quantity(&units, value, &unit).map_err(callback_error)
    });
    engine.register_fn("+", |a: Quantity, b: Quantity| {
        a.checked_add(b).map_err(callback_error)
    });
    engine.register_fn("-", |a: Quantity, b: Quantity| {
        a.checked_sub(b).map_err(callback_error)
    });
    engine.register_fn("*", |a: Quantity, b: Quantity| {
        a.checked_mul(b).map_err(callback_error)
    });
    engine.register_fn("/", |a: Quantity, b: Quantity| {
        a.checked_div(b).map_err(callback_error)
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
            millimetres(&x).map_err(callback_error)?,
            millimetres(&y).map_err(callback_error)?,
        ];
        let size = [
            millimetres(&width).map_err(callback_error)?,
            millimetres(&height).map_err(callback_error)?,
        ];
        if size.iter().any(|v| *v < 0.01) {
            return Err(failure("pad dimensions must be at least 0.01 mm"));
        }
        if !pad.shape.set_size(size) {
            return Err(failure("scripts size only rectangular and oblong pads"));
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
