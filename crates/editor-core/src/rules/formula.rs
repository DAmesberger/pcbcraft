//! Rule formulas: a profile value written as a Rhai expression of other
//! rules (`via_drill_default + 2.0 * via_annular_ring_min`). Formulas run
//! sandboxed: expressions only (no statements, loops or functions of
//! their own), no I/O, printing, `eval` or modules, and a bounded number
//! of operations and nesting depth. Every other rule is a variable named
//! by [`RuleKey::name`]; a formula naming itself, directly or through
//! others, is refused with the chain spelled out.
use super::{DesignRules, RuleKey, RuleValue};
use crate::diagnostics::{Error, Result};
use rhai::{Dynamic, Engine, EvalAltResult, Scope};
use std::collections::{BTreeMap, BTreeSet};

/// Bytes of one formula.
pub const MAX_FORMULA: usize = 512;
/// Interpreter operations one formula may run.
const MAX_OPERATIONS: u64 = 10_000;

/// The sandboxed interpreter formulas run in.
fn engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .set_max_operations(MAX_OPERATIONS)
        .set_max_call_levels(8)
        .set_max_expr_depths(32, 32)
        .set_max_variables(0)
        .set_max_functions(0)
        .set_max_string_size(64)
        .set_max_array_size(0)
        .set_max_map_size(0)
        .set_allow_anonymous_fn(false)
        .set_allow_looping(false)
        .set_allow_shadowing(false)
        .disable_symbol("eval")
        .disable_symbol("print")
        .disable_symbol("debug");
    engine
}

/// The identifiers a formula's text names (outside string literals).
fn identifiers(text: &str) -> BTreeSet<&str> {
    let mut out = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' || b == b'\'' {
            i += 1;
            while i < bytes.len() && bytes[i] != b {
                i += 1;
            }
            i += 1;
        } else if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            out.insert(&text[start..i]);
        } else if b.is_ascii_digit() {
            // A number, with any exponent or suffix letters it carries.
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Every rule the formulas name.
pub fn inputs<'a>(formulas: impl IntoIterator<Item = &'a str>) -> BTreeSet<RuleKey> {
    let names: BTreeMap<String, RuleKey> =
        RuleKey::all().into_iter().map(|k| (k.name(), k)).collect();
    formulas
        .into_iter()
        .flat_map(identifiers)
        .filter_map(|id| names.get(id).copied())
        .collect()
}

/// A rule value as a Rhai value: lengths, areas, angles, fractions and set
/// ratios are floats, an unchecked ratio is `()`, counts are integers,
/// flags booleans and pad connections their names.
fn to_dynamic(value: RuleValue) -> Dynamic {
    match value {
        RuleValue::Length(v)
        | RuleValue::Area(v)
        | RuleValue::Angle(v)
        | RuleValue::Fraction(v)
        | RuleValue::Ratio(Some(v)) => Dynamic::from_float(v),
        RuleValue::Ratio(None) => Dynamic::UNIT,
        RuleValue::Count(n) => Dynamic::from_int(i64::from(n)),
        RuleValue::Flag(f) => Dynamic::from_bool(f),
        RuleValue::Connection(c) => Dynamic::from(super::connection_name(c).to_string()),
    }
}

/// A formula's result as a value of `key`'s kind.
fn from_dynamic(key: RuleKey, result: Dynamic) -> Option<RuleValue> {
    let number = || {
        result
            .as_float()
            .ok()
            .or_else(|| result.as_int().ok().map(|n| n as f64))
    };
    Some(match key.kind() {
        RuleValue::Length(_) => RuleValue::Length(number()?),
        RuleValue::Area(_) => RuleValue::Area(number()?),
        RuleValue::Angle(_) => RuleValue::Angle(number()?),
        RuleValue::Fraction(_) => RuleValue::Fraction(number()?),
        RuleValue::Ratio(_) if result.is_unit() => RuleValue::Ratio(None),
        RuleValue::Ratio(_) => RuleValue::Ratio(Some(number()?)),
        RuleValue::Count(_) => RuleValue::Count(u32::try_from(result.as_int().ok()?).ok()?),
        RuleValue::Flag(_) => RuleValue::Flag(result.as_bool().ok()?),
        RuleValue::Connection(_) => {
            let name = result.into_immutable_string().ok()?;
            RuleValue::Connection(super::connection_from_name(&name)?)
        }
    })
}

fn plain(error: &EvalAltResult) -> String {
    match error.unwrap_inner() {
        EvalAltResult::ErrorVariableNotFound(name, _) => {
            format!("{name} is not a rule name")
        }
        EvalAltResult::ErrorTooManyOperations(_) => "it runs too long".into(),
        other => other.to_string(),
    }
}

/// Check one formula's text before it is stored: bounded, printable and
/// naming only rules.
pub fn check_text(key: RuleKey, text: &str) -> Result<()> {
    if text.trim().is_empty() || text.len() > MAX_FORMULA || text.chars().any(char::is_control) {
        return Err(Error::Invalid(format!(
            "rules.formula: the formula for {} must be one line of at most {MAX_FORMULA} bytes",
            key.name()
        )));
    }
    Ok(())
}

/// Write every formula's value into `rules`, in dependency order. `rules`
/// already holds every literal value. A formula that names itself through
/// any chain, names something that is not a rule, fails or gives a value
/// of the wrong kind is refused, naming the rule.
pub fn evaluate(rules: &mut DesignRules, formulas: &[(RuleKey, &str)]) -> Result<()> {
    if formulas.is_empty() {
        return Ok(());
    }
    let pending: BTreeMap<RuleKey, &str> = formulas.iter().copied().collect();
    let names: BTreeMap<String, RuleKey> =
        RuleKey::all().into_iter().map(|k| (k.name(), k)).collect();
    let deps: BTreeMap<RuleKey, Vec<RuleKey>> = pending
        .iter()
        .map(|(key, text)| {
            let uses = identifiers(text)
                .into_iter()
                .filter_map(|id| names.get(id).copied())
                .filter(|k| pending.contains_key(k))
                .collect();
            (*key, uses)
        })
        .collect();
    let order = order(&deps)?;
    let engine = engine();
    for key in order {
        let Some(text) = pending.get(&key) else {
            continue;
        };
        check_text(key, text)?;
        let mut scope = Scope::new();
        for other in RuleKey::all() {
            if other != key {
                scope.push_constant_dynamic(other.name(), to_dynamic(other.get(rules)));
            }
        }
        let result = engine
            .eval_expression_with_scope::<Dynamic>(&mut scope, text)
            .map_err(|e| {
                Error::Invalid(format!(
                    "rules.formula: the formula for {} ({text}) fails: {}",
                    key.name(),
                    plain(&e)
                ))
            })?;
        let value = from_dynamic(key, result).ok_or_else(|| {
            Error::Invalid(format!(
                "rules.formula: the formula for {} ({text}) gives a value of another kind",
                key.name()
            ))
        })?;
        key.set(rules, value)?;
    }
    Ok(())
}

/// The formulas in an order where each comes after those it names; a
/// cycle is refused with its chain ("a → b → a").
fn order(deps: &BTreeMap<RuleKey, Vec<RuleKey>>) -> Result<Vec<RuleKey>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Visiting,
        Done,
    }
    fn visit(
        key: RuleKey,
        deps: &BTreeMap<RuleKey, Vec<RuleKey>>,
        marks: &mut BTreeMap<RuleKey, Mark>,
        path: &mut Vec<RuleKey>,
        out: &mut Vec<RuleKey>,
    ) -> Result<()> {
        match marks.get(&key) {
            Some(Mark::Done) => return Ok(()),
            Some(Mark::Visiting) => {
                let start = path.iter().position(|k| *k == key).unwrap_or(0);
                let chain: Vec<String> = path[start..]
                    .iter()
                    .chain(std::iter::once(&key))
                    .map(|k| k.name())
                    .collect();
                return Err(Error::Invalid(format!(
                    "rules.formula_cycle: these formulas depend on each other: {}",
                    chain.join(" → ")
                )));
            }
            None => {}
        }
        marks.insert(key, Mark::Visiting);
        path.push(key);
        for dep in deps.get(&key).into_iter().flatten() {
            visit(*dep, deps, marks, path, out)?;
        }
        path.pop();
        marks.insert(key, Mark::Done);
        out.push(key);
        Ok(())
    }
    let mut marks = BTreeMap::new();
    let mut out = Vec::new();
    for key in deps.keys() {
        visit(*key, deps, &mut marks, &mut Vec::new(), &mut out)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::PadConnection;

    #[test]
    fn identifiers_skip_numbers_and_strings() {
        let ids = identifiers("max(drill_min, 1.5e-3) + \"thermal\" * x2");
        assert_eq!(
            ids.into_iter().collect::<Vec<_>>(),
            ["drill_min", "max", "x2"]
        );
    }

    #[test]
    fn the_sandbox_refuses_loops_and_runaways() {
        let engine = engine();
        let mut scope = Scope::new();
        assert!(
            engine
                .eval_expression_with_scope::<Dynamic>(&mut scope, "loop { }")
                .is_err()
        );
        assert!(
            engine
                .eval_expression_with_scope::<Dynamic>(&mut scope, "print(1)")
                .is_err()
        );
        assert_eq!(
            super::super::connection_from_name("solid"),
            Some(PadConnection::Solid)
        );
    }
}
