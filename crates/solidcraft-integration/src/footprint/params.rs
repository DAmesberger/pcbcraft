//! Footprint parameters and injected package values, evaluated by
//! Solidcraft's expression language (`doc::expr`). Values are millimetres,
//! radians or plain numbers, as Solidcraft keeps them.

use std::collections::BTreeMap;

use pcbcraft_core::library::footprint_source::{
    FootprintParam, MAX_EXPR, MAX_PARAMETERS, PACKAGE_PREFIX, PackageValue, ParamUnit, ParamValue,
};
use solidcraft_engine::doc::DocError;
use solidcraft_engine::doc::expr::{self, Kind, ParamDef, Value};

use super::check::MAX_COORD_MM;
use super::{Outcome, problem};

const NAME: &str = "footprint.param_name";
const EXPR: &str = "footprint.param_expr";

/// Largest count an expression may give (a footprint has at most 1024 pads).
pub(crate) const MAX_COUNT: u16 = 1024;

/// Evaluated parameters and package values, by name.
#[derive(Clone, Debug, Default)]
pub(crate) struct Values(BTreeMap<String, Value>);

fn unit(unit: ParamUnit) -> &'static str {
    match unit {
        ParamUnit::Mm => "mm",
        ParamUnit::Deg => "deg",
        ParamUnit::Count => "",
    }
}

/// Identifier syntax, length and Solidcraft's reserved words.
pub(crate) fn check_name(name: &str) -> Outcome<()> {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !first_ok
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        || name.len() > pcbcraft_core::library::MAX_LABEL
    {
        return Err(problem(NAME, format!("`{name}` is not a parameter name")));
    }
    if let Some(kind) = expr::reserved_kind(name) {
        return Err(problem(
            NAME,
            format!("`{name}` is a {kind} in expressions"),
        ));
    }
    Ok(())
}

/// Evaluate the footprint parameters with the typed package values
/// injected (each already named `pkg_<name>`).
pub(crate) fn evaluate(params: &[FootprintParam], package: &[PackageValue]) -> Outcome<Values> {
    if params.len() > MAX_PARAMETERS || package.len() > MAX_PARAMETERS {
        return Err(problem("footprint.too_large", "too many parameters"));
    }
    let mut defs = Vec::with_capacity(params.len() + package.len());
    for p in params {
        check_name(&p.name.0)?;
        if p.name.is_package() {
            return Err(problem(
                NAME,
                format!(
                    "`{}`: the `{PACKAGE_PREFIX}` prefix is reserved for package values",
                    p.name.0
                ),
            ));
        }
        if p.expr.0.len() > MAX_EXPR {
            return Err(problem(EXPR, format!("`{}` is too long", p.name.0)));
        }
        defs.push(ParamDef {
            name: p.name.0.clone(),
            expr: p.expr.0.clone(),
            unit: unit(p.unit).into(),
        });
    }
    for v in package {
        check_name(&v.name.0)?;
        if !v.name.is_package() {
            return Err(problem(
                NAME,
                format!(
                    "package value `{}` must start with `{PACKAGE_PREFIX}`",
                    v.name.0
                ),
            ));
        }
        // `{:?}` keeps every bit of the f64 (shortest round-trip form).
        let (text, unit) = match v.value {
            ParamValue::Length(mm) if mm.is_finite() => (format!("{mm:?} mm"), "mm"),
            ParamValue::Angle(deg) if deg.is_finite() => (format!("{deg:?} deg"), "deg"),
            ParamValue::Count(n) => (n.to_string(), ""),
            _ => {
                return Err(problem(EXPR, format!("`{}` is not finite", v.name.0)));
            }
        };
        defs.push(ParamDef {
            name: v.name.0.clone(),
            expr: text,
            unit: unit.into(),
        });
    }
    let mut seen = std::collections::BTreeSet::new();
    for d in &defs {
        if !seen.insert(d.name.as_str()) {
            return Err(problem(NAME, format!("`{}` is defined twice", d.name)));
        }
    }
    let (values, errors) = expr::eval_params(&defs, 1.0);
    if let Some((name, error)) = errors.into_iter().next() {
        return Err(problem(EXPR, format!("`{name}`: {error}")));
    }
    for p in params {
        if p.unit == ParamUnit::Count {
            let v = values
                .get(&p.name.0)
                .copied()
                .unwrap_or(Value::num(f64::NAN));
            let n = v.to_kind(Kind::Unitless).unwrap_or(f64::NAN);
            if n.fract() != 0.0 || !(0.0..=f64::from(MAX_COUNT)).contains(&n) {
                return Err(problem(
                    EXPR,
                    format!(
                        "count `{}` = {n} is not a whole number in 0…{MAX_COUNT}",
                        p.name.0
                    ),
                ));
            }
        }
    }
    Ok(Values(values))
}

impl Values {
    fn raw(&self, text: &str, kind: Kind) -> Outcome<f64> {
        if text.len() > MAX_EXPR {
            return Err(problem(EXPR, "an expression is too long"));
        }
        let lookup = |n: &str| {
            self.0
                .get(n)
                .copied()
                .ok_or_else(|| DocError::Expr(format!("unknown parameter `{n}`")))
        };
        let v = expr::eval_with(text, &lookup)
            .and_then(|v| v.to_kind(kind))
            .map_err(|e| problem(EXPR, format!("`{text}`: {e}")))?;
        if !v.is_finite() {
            return Err(problem(EXPR, format!("`{text}` is not finite")));
        }
        Ok(v)
    }

    /// A signed length in mm within the coordinate range.
    pub(crate) fn signed_length(&self, text: &str) -> Outcome<f64> {
        let v = self.raw(text, Kind::Length)?;
        if v.abs() > MAX_COORD_MM {
            return Err(problem(EXPR, format!("`{text}` = {v} mm is out of range")));
        }
        Ok(v)
    }

    /// A length in mm, at least 0.
    pub(crate) fn length(&self, text: &str) -> Outcome<f64> {
        let v = self.signed_length(text)?;
        if v < 0.0 {
            return Err(problem(EXPR, format!("`{text}` = {v} mm is negative")));
        }
        Ok(v)
    }

    /// A strictly positive length in mm.
    pub(crate) fn positive(&self, text: &str) -> Outcome<f64> {
        let v = self.length(text)?;
        if v <= 0.0 {
            return Err(problem(EXPR, format!("`{text}` must be positive")));
        }
        Ok(v)
    }

    /// A plain number (ratios).
    pub(crate) fn number(&self, text: &str) -> Outcome<f64> {
        self.raw(text, Kind::Unitless)
    }

    /// A whole count in 0…[`MAX_COUNT`].
    pub(crate) fn count(&self, text: &str) -> Outcome<u16> {
        let n = self.raw(text, Kind::Unitless)?;
        if n.fract() != 0.0 || !(0.0..=f64::from(MAX_COUNT)).contains(&n) {
            return Err(problem(
                EXPR,
                format!("count `{text}` = {n} is not a whole number in 0…{MAX_COUNT}"),
            ));
        }
        // Whole and within 0…1024, so the conversion is exact.
        Ok(n as u16)
    }

    /// The value a bound sketch dimension takes (mm or radians).
    pub(crate) fn dimension(&self, text: &str, angle: bool) -> Outcome<f64> {
        let v = self.raw(text, if angle { Kind::Angle } else { Kind::Length })?;
        if v.abs() > MAX_COORD_MM {
            return Err(problem(EXPR, format!("`{text}` = {v} is out of range")));
        }
        Ok(v)
    }
}

/// Names an expression refers to (units, constants and functions excluded).
pub(crate) fn references(text: &str) -> Vec<String> {
    expr::references(text)
}

/// The expression with references to `from` renamed to `to`.
pub(crate) fn rename(text: &str, from: &str, to: &str) -> String {
    expr::rename_reference(text, from, to)
}
