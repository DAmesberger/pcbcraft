//! The one length parser and formatter, for UI fields, the CLI and MCP.
//!
//! Text is a decimal number (the [`Decimal`] grammar: sign, digits, point,
//! exponent) and an optional unit: `mm`, `µm` (`um`), `mil` (`mils`,
//! `thou`) or `in` (`"`), in any ASCII case, with optional space between.
//! A bare number takes the field's default unit. Conversion to nanometres is
//! exact integer arithmetic, rounded half away from zero to whole nm.
use super::{MAX_COORD_NM, NM_PER_INCH, NM_PER_MIL, NM_PER_MM, NM_PER_UM, Nm};
use crate::diagnostics::{Error, Result};
use crate::quantity::Decimal;

/// Bytes of one length text, checked before parsing.
pub const MAX_TEXT: usize = 64;

/// A unit lengths are typed and shown in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LengthUnit {
    Mm,
    Um,
    Mil,
    Inch,
}

impl LengthUnit {
    pub const ALL: [Self; 4] = [Self::Mm, Self::Um, Self::Mil, Self::Inch];

    /// Nanometres in one unit, exactly.
    pub const fn nm(self) -> i64 {
        match self {
            Self::Mm => NM_PER_MM,
            Self::Um => NM_PER_UM,
            Self::Mil => NM_PER_MIL,
            Self::Inch => NM_PER_INCH,
        }
    }

    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Mm => "mm",
            Self::Um => "µm",
            Self::Mil => "mil",
            Self::Inch => "in",
        }
    }

    /// Decimals shown: enough that every whole-nm value reads back exactly
    /// (mm and µm are exact; one step of mil and in is 0.254 nm).
    const fn decimals(self) -> u32 {
        match self {
            Self::Mm => 6,
            Self::Um => 3,
            Self::Mil => 5,
            Self::Inch => 8,
        }
    }

    /// The unit a suffix names, in any ASCII case.
    pub fn from_suffix(suffix: &str) -> Option<Self> {
        Some(match suffix.to_ascii_lowercase().as_str() {
            "mm" => Self::Mm,
            "um" | "µm" | "\u{3bc}m" => Self::Um,
            "mil" | "mils" | "thou" => Self::Mil,
            "in" | "\"" => Self::Inch,
            _ => return None,
        })
    }
}

fn invalid(text: &str) -> Error {
    Error::Invalid(format!(
        "‘{text}’ is not a length (a number with mm, µm, mil or in)"
    ))
}

/// `[sign] digits [. digits] [e|E [sign] digits]`, then the rest.
fn split_number(text: &str) -> (&str, &str) {
    let bytes = text.as_bytes();
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    while bytes
        .get(end)
        .is_some_and(|b| b.is_ascii_digit() || *b == b'.')
    {
        end += 1;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut at = end + 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        if bytes.get(at).is_some_and(u8::is_ascii_digit) {
            while bytes.get(at).is_some_and(u8::is_ascii_digit) {
                at += 1;
            }
            end = at;
        }
    }
    // Every byte before `end` is ASCII: `end` is a char boundary.
    text.split_at(end)
}

/// `n × 10^exponent`, rounded half away from zero; `None` on overflow.
fn scaled(n: i128, exponent: i32) -> Option<i128> {
    if exponent >= 0 {
        return n.checked_mul(10i128.checked_pow(exponent.unsigned_abs())?);
    }
    let Some(divisor) = 10i128.checked_pow(exponent.unsigned_abs()) else {
        // |n| < 1e26 is far below half of a divisor beyond i128.
        return Some(0);
    };
    // |n| < 1e26: the rounding step cannot overflow.
    Some(rounded_div(n, divisor))
}

/// Length text to whole nanometres; a bare number is in `default_unit`.
pub fn parse_length(text: &str, default_unit: LengthUnit) -> Result<Nm> {
    if text.len() > MAX_TEXT {
        return Err(Error::Limit(format!(
            "length text exceeds {MAX_TEXT} bytes"
        )));
    }
    let trimmed = text.trim();
    let (number, suffix) = split_number(trimmed);
    let suffix = suffix.trim_start();
    let unit = if suffix.is_empty() {
        default_unit
    } else {
        LengthUnit::from_suffix(suffix).ok_or_else(|| invalid(trimmed))?
    };
    let decimal: Decimal = number.parse().map_err(|_| invalid(trimmed))?;
    let (mantissa, exponent) = decimal.parts();
    scaled(i128::from(mantissa) * i128::from(unit.nm()), exponent)
        .and_then(|nm| i64::try_from(nm).ok())
        .and_then(Nm::try_new)
        .ok_or_else(|| {
            Error::Limit(format!(
                "length ‘{trimmed}’ is outside ±{MAX_COORD_NM} nm (±2 m)"
            ))
        })
}

/// The number of `unit`s in `nm`, without trailing zeros: `0.25`, `-10`,
/// `3.93701`. Reads back through [`parse_length`] to the same `nm`.
pub fn format_number(nm: Nm, unit: LengthUnit) -> String {
    let decimals = unit.decimals();
    let scale = 10i128.pow(decimals);
    // nm × 10^8 ≤ 2e17 and the unit is at least 1 nm: no overflow, and the
    // quotient by `scale` below is the whole part.
    let steps = rounded_div(i128::from(nm.get()) * scale, i128::from(unit.nm()));
    let sign = if steps < 0 { "-" } else { "" };
    let steps = steps.unsigned_abs();
    let scale = scale.unsigned_abs();
    let whole = steps / scale;
    let fraction = format!("{:0width$}", steps % scale, width = decimals as usize);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction}")
    }
}

/// [`format_number`] and the unit symbol: `0.25 mm`, `10 mil`.
pub fn format_length(nm: Nm, unit: LengthUnit) -> String {
    format!("{} {}", format_number(nm, unit), unit.symbol())
}

/// `n / d` for `d > 0`, rounded half away from zero.
fn rounded_div(n: i128, d: i128) -> i128 {
    let (quotient, remainder) = (n / d, n % d);
    if 2 * remainder.abs() >= d {
        quotient + n.signum()
    } else {
        quotient
    }
}
