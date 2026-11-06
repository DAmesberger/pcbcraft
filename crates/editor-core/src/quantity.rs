//! Checked dimensional arithmetic and the one unit-scaling and unit-text rule.
//!
//! Units are data: a [`UnitSet`] supplies a dimension and the linear units
//! that measure it, so nothing here is chosen by a component name or a unit
//! symbol. Quantity kinds (`fields`) and script requests (`scripting`) both
//! use this module; it knows nothing about either, and nothing about Rhai.
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod decimal;
pub use decimal::{Decimal, MAX_DIGITS};

/// SI exponents: length, mass, time, current, temperature, amount, luminosity.
pub type Dimension = [i8; 7];
pub const DIMENSIONLESS: Dimension = [0; 7];
pub const LENGTH: Dimension = [1, 0, 0, 0, 0, 0, 0];

/// The largest SI magnitude a quantity may hold.
pub const MAX_MAGNITUDE: f64 = 1e100;
/// Units in one set.
pub const MAX_UNITS: usize = 32;
/// Aliases of one unit.
pub const MAX_ALIASES: usize = 8;
/// Bytes of one unit symbol or alias.
pub const MAX_SYMBOL: usize = 32;
/// Bytes of one quantity text, checked before parsing.
pub const MAX_TEXT: usize = 128;

/// SI prefixes from pico to giga. `u` and the Greek small letter mu are
/// accepted spellings of the micro sign `µ`; parsed values are recorded with
/// the first spelling of each factor.
pub const SI_PREFIXES: &[(&str, f64)] = &[
    ("p", 1e-12),
    ("n", 1e-9),
    ("µ", 1e-6),
    ("u", 1e-6),
    ("\u{3bc}", 1e-6),
    ("m", 1e-3),
    ("c", 1e-2),
    ("d", 1e-1),
    ("da", 1e1),
    ("h", 1e2),
    ("k", 1e3),
    ("M", 1e6),
    ("G", 1e9),
];

/// Prefixes used when formatting without a recorded display unit, by
/// power of ten.
const ENGINEERING: &[(&str, i32)] = &[
    ("G", 9),
    ("M", 6),
    ("k", 3),
    ("", 0),
    ("m", -3),
    ("µ", -6),
    ("n", -9),
    ("p", -12),
];

fn failure(message: &str) -> Error {
    Error::Invalid(message.into())
}

/// A value with a dimension, independent of Rhai. Construction and every
/// operation are checked: results are finite and within ±1e100 SI units.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Quantity {
    value: f64,
    dimension: Dimension,
}

impl Quantity {
    pub fn new(value: f64, dimension: Dimension) -> Result<Self> {
        if !value.is_finite() || value.abs() > MAX_MAGNITUDE {
            return Err(failure(
                "quantity must be finite and within ±1e100 SI units",
            ));
        }
        if dimension.iter().any(|d| !(-16..=16).contains(d)) {
            return Err(failure("dimension exponent exceeds ±16"));
        }
        Ok(Self { value, dimension })
    }
    /// The SI value.
    pub fn value(&self) -> f64 {
        self.value
    }
    pub fn dimension(&self) -> Dimension {
        self.dimension
    }

    pub fn checked_add(self, other: Self) -> Result<Self> {
        if self.dimension != other.dimension {
            return Err(failure("cannot add quantities with different dimensions"));
        }
        Self::new(self.value + other.value, self.dimension)
    }

    pub fn checked_sub(self, other: Self) -> Result<Self> {
        self.checked_add(Self::new(-other.value, other.dimension)?)
    }

    pub fn checked_mul(self, other: Self) -> Result<Self> {
        self.product(other, false)
    }

    pub fn checked_div(self, other: Self) -> Result<Self> {
        self.product(other, true)
    }

    fn product(self, other: Self, divide: bool) -> Result<Self> {
        if divide && other.value == 0.0 {
            return Err(failure("quantity division by zero"));
        }
        let mut dimension = DIMENSIONLESS;
        for ((out, a), b) in dimension
            .iter_mut()
            .zip(self.dimension)
            .zip(other.dimension)
        {
            *out = if divide {
                a.checked_sub(b)
            } else {
                a.checked_add(b)
            }
            .ok_or_else(|| failure("dimension exponent overflow"))?;
        }
        let value = if divide {
            self.value / other.value
        } else {
            self.value * other.value
        };
        if value == 0.0 && self.value != 0.0 && other.value != 0.0 {
            return Err(failure("quantity arithmetic underflow"));
        }
        Self::new(value, dimension)
    }

    pub fn scale(self, factor: f64) -> Result<Self> {
        self.product(Self::new(factor, DIMENSIONLESS)?, false)
    }

    pub fn scale_integer(self, factor: i64) -> Result<Self> {
        if !(-9_007_199_254_740_992..=9_007_199_254_740_992).contains(&factor) {
            return Err(failure(
                "integer quantity multiplier exceeds exact floating-point range",
            ));
        }
        self.scale(factor as f64)
    }
}

/// One linear unit. It has no dimension of its own; its [`UnitSet`]
/// supplies it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UnitOfMeasure {
    pub symbol: String,
    /// Other spellings accepted in text, e.g. `ohm`. Never identity.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Multiply by this positive factor to obtain an SI value.
    pub scale: f64,
    /// Whether an SI prefix may precede the symbol or an alias.
    pub si_prefixes: bool,
}

/// A dimension and the units that measure it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UnitSet {
    pub dimension: Dimension,
    pub units: Vec<UnitOfMeasure>,
}

/// A unit text resolved through a [`UnitSet`].
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedUnit {
    /// Multiply by this factor to obtain an SI value.
    pub factor: f64,
    /// `factor` as the exact product of the prefix and the unit scale.
    pub exact: Decimal,
    /// The text in canonical spelling: prefix and unit symbol.
    pub canonical: String,
}

/// A symbol or alias the text grammar can recognise after a number.
fn symbol_valid(text: &str) -> bool {
    let mut chars = text.chars();
    let first = chars.next();
    let exponent_like = matches!(first, Some('e' | 'E'))
        && chars.next().is_some_and(|c| "+-0123456789".contains(c));
    !text.is_empty()
        && text.len() <= MAX_SYMBOL
        && !exponent_like
        && !first.is_some_and(|c| c.is_ascii_digit() || "+-.±\u{2212}".contains(c))
        && !text.contains("..")
        && !text.contains('/')
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

impl UnitSet {
    /// Counts and text lengths only: what an unfinished draft must respect.
    pub fn within_bounds(&self) -> bool {
        self.units.len() <= MAX_UNITS
            && self.units.iter().all(|u| {
                u.symbol.len() <= MAX_SYMBOL
                    && u.aliases.len() <= MAX_ALIASES
                    && u.aliases.iter().all(|a| a.len() <= MAX_SYMBOL)
            })
    }

    /// Every unit text, symbols and aliases.
    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.units.iter().flat_map(|u| {
            std::iter::once(u.symbol.as_str()).chain(u.aliases.iter().map(String::as_str))
        })
    }

    /// The validity rule for a set: 1–32 units with positive finite scales
    /// within 1e-30–1e30, unique symbols and aliases the text grammar can
    /// recognise, and a dimension within ±16.
    pub fn check(&self) -> Result<()> {
        Quantity::new(0.0, self.dimension)?;
        if self.units.is_empty() || !self.within_bounds() {
            return Err(failure(
                "a unit set needs 1–32 units with at most 8 aliases each",
            ));
        }
        let mut seen = BTreeSet::new();
        for unit in &self.units {
            if !unit.scale.is_finite() || !(1e-30..=1e30).contains(&unit.scale) {
                return Err(Error::Invalid(format!(
                    "unit {} needs a finite scale within 1e-30–1e30",
                    unit.symbol
                )));
            }
        }
        for text in self.texts() {
            if !symbol_valid(text) {
                return Err(Error::Invalid(format!(
                    "unit text ‘{text}’ must be 1–32 bytes without spaces, control characters, '/', '..' or a leading digit, sign or exponent"
                )));
            }
            if !seen.insert(text) {
                return Err(Error::Invalid(format!("unit text ‘{text}’ is not unique")));
            }
        }
        Ok(())
    }

    /// The unit with this exact symbol or alias.
    pub fn unit(&self, text: &str) -> Option<&UnitOfMeasure> {
        self.units
            .iter()
            .find(|u| u.symbol == text || u.aliases.iter().any(|a| a == text))
    }

    /// Resolve a unit text: an exact symbol or alias first, then an SI
    /// prefix followed by the symbol or an alias of a prefix-accepting unit.
    pub fn resolve(&self, text: &str) -> Option<ResolvedUnit> {
        if let Some(unit) = self.unit(text) {
            return Some(ResolvedUnit {
                factor: unit.scale,
                exact: Decimal::from_f64(unit.scale).ok()?,
                canonical: unit.symbol.clone(),
            });
        }
        let mut prefixes: Vec<&(&str, f64)> = SI_PREFIXES.iter().collect();
        prefixes.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
        for (prefix, factor) in prefixes {
            if let Some(rest) = text.strip_prefix(prefix)
                && let Some(unit) = self.unit(rest).filter(|u| u.si_prefixes)
            {
                return Some(ResolvedUnit {
                    factor: factor * unit.scale,
                    exact: prefixed(*factor, unit.scale)?,
                    canonical: format!("{}{}", canonical_prefix(*factor), unit.symbol),
                });
            }
        }
        None
    }

    /// The single scaling rule: `value` in the unit `text` as an SI quantity.
    pub fn to_si(&self, value: f64, text: &str) -> Result<Quantity> {
        if text.len() > MAX_SYMBOL {
            return Err(failure("unit symbol exceeds 32 bytes"));
        }
        let unit = self
            .resolve(text)
            .ok_or_else(|| Error::Invalid(format!("unknown unit ‘{text}’")))?;
        Quantity::new(value, self.dimension)?.scale(unit.factor)
    }

    fn listing(&self) -> String {
        self.texts().collect::<Vec<_>>().join(", ")
    }
}

/// The exact factor of an SI prefix applied to a unit scale.
fn prefixed(prefix: f64, scale: f64) -> Option<Decimal> {
    Decimal::from_f64(prefix)
        .and_then(|p| p.checked_mul(Decimal::from_f64(scale)?))
        .ok()
}

/// The recorded spelling of a prefix factor.
fn canonical_prefix(factor: f64) -> &'static str {
    SI_PREFIXES
        .iter()
        .find(|(_, f)| *f == factor)
        .map_or("", |(p, _)| p)
}

/// A parsed quantity text.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub quantity: Quantity,
    /// The exact SI value; `quantity` holds the nearest `f64`.
    pub si: Decimal,
    /// The unit written, in canonical spelling; `None` without a suffix.
    pub unit: Option<String>,
}

/// Split `number [whitespace] [suffix]`. The number is a decimal with
/// optional sign and exponent and `.` as the separator.
fn split_number(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut end = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        end += 1;
    }
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let integer = digits(end);
    end += integer;
    let mut fraction = 0;
    if bytes.get(end) == Some(&b'.') {
        fraction = digits(end + 1);
        end += 1 + fraction;
    }
    if integer + fraction == 0 {
        return None;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut at = end + 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let exponent = digits(at);
        if exponent > 0 {
            end = at + exponent;
        }
    }
    Some((&text[..end], text[end..].trim_start()))
}

/// Parse quantity text against a unit set. The suffix resolves, first match
/// winning: (1) a unit symbol or alias; (2) an SI prefix and a
/// prefix-accepting unit; (3) a bare SI prefix applied to `default_unit`;
/// (4) no suffix, so `default_unit` applies.
pub fn parse(text: &str, units: &UnitSet, default_unit: &str) -> Result<Parsed> {
    if text.len() > MAX_TEXT {
        return Err(Error::Limit("quantity text exceeds 128 bytes".into()));
    }
    let normalized = text.trim().replacen('\u{2212}', "-", 1);
    let (number, suffix) = split_number(&normalized).ok_or_else(|| {
        Error::Invalid(format!(
            "‘{}’ is not a number with an optional unit",
            text.trim()
        ))
    })?;
    let number: Decimal = number.parse()?;
    let default = units.unit(default_unit).ok_or_else(|| {
        Error::Invalid(format!(
            "default unit ‘{default_unit}’ is not in the unit set"
        ))
    })?;
    let (resolved, unit) = if suffix.is_empty() {
        (
            ResolvedUnit {
                factor: default.scale,
                exact: Decimal::from_f64(default.scale)?,
                canonical: default.symbol.clone(),
            },
            None,
        )
    } else if let Some(resolved) = units.resolve(suffix) {
        let canonical = resolved.canonical.clone();
        (resolved, Some(canonical))
    } else if let Some(factor) = SI_PREFIXES
        .iter()
        .find(|(p, _)| *p == suffix)
        .filter(|_| default.si_prefixes)
        .map(|(_, f)| f)
    {
        let canonical = format!("{}{}", canonical_prefix(*factor), default.symbol);
        (
            ResolvedUnit {
                factor: factor * default.scale,
                exact: prefixed(*factor, default.scale)
                    .ok_or_else(|| failure("unit scale has no decimal form"))?,
                canonical: canonical.clone(),
            },
            Some(canonical),
        )
    } else {
        return Err(Error::Invalid(format!(
            "‘{suffix}’ is not a unit of this quantity kind (units: {})",
            units.listing()
        )));
    };
    let si = number.checked_mul(resolved.exact)?;
    let quantity = Quantity::new(si.to_f64(), units.dimension)?;
    Ok(Parsed { quantity, si, unit })
}

/// A number rounded to 12 significant digits, in plain or exponent notation.
pub fn format_number(value: f64) -> String {
    Decimal::from_f64(value).map_or_else(|_| value.to_string(), |d| d.to_string())
}

/// `si` in units of `factor`: exact for a power-of-ten factor, otherwise
/// rounded to 12 significant digits.
fn in_units(si: Decimal, factor: Decimal) -> Decimal {
    factor
        .power_of_ten()
        .and_then(|n| si.shift(-n).ok())
        .or_else(|| Decimal::from_f64(si.to_f64() / factor.to_f64()).ok())
        .unwrap_or(si)
}

/// [`format_decimal`] for an `f64` SI value, rounded to 12 significant digits.
pub fn format(si: f64, units: &UnitSet, default_unit: &str, display_unit: Option<&str>) -> String {
    Decimal::from_f64(si).map_or_else(
        |_| si.to_string(),
        |si| format_decimal(si, units, default_unit, display_unit),
    )
}

/// Text for an SI value: in `display_unit` when it resolves, otherwise in
/// `default_unit`, with an engineering prefix when that unit accepts one.
/// The digits are exact when the unit's scale is a power of ten.
pub fn format_decimal(
    si: Decimal,
    units: &UnitSet,
    default_unit: &str,
    display_unit: Option<&str>,
) -> String {
    if let Some(resolved) = display_unit.and_then(|d| units.resolve(d)) {
        return format!("{} {}", in_units(si, resolved.exact), resolved.canonical);
    }
    let Some((unit, scale)) = units
        .unit(default_unit)
        .and_then(|u| Some((u, Decimal::from_f64(u.scale).ok()?)))
    else {
        return si.to_string();
    };
    let value = in_units(si, scale);
    let (prefix, power) = if unit.si_prefixes && !value.is_zero() {
        ENGINEERING
            .iter()
            .find(|(_, n)| value.leading_exponent() >= *n)
            .copied()
            .unwrap_or(("p", -12))
    } else {
        ("", 0)
    };
    let shown = value.shift(-power).unwrap_or(value);
    format!("{shown} {prefix}{}", unit.symbol)
}

#[cfg(test)]
mod tests;
