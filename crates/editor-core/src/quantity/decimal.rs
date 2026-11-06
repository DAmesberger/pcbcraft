//! An exact decimal number: what a user types is what is stored.
//!
//! Typed field values keep their decimal digits exactly (`10 µF` is
//! `0.00001 F`, never `9.999999999999999e-6`). Values are saved as text so
//! no reader converts them through binary floating point. Checks and
//! geometry that compare values convert with [`Decimal::to_f64`].
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Significant digits a decimal may hold.
pub const MAX_DIGITS: usize = 18;
/// The largest power-of-ten exponent, either sign. Wide enough that every
/// finite `f64` (unit scales included) has a decimal form.
pub const MAX_EXPONENT: i32 = 400;
/// Bytes of decimal text, checked before parsing.
const MAX_TEXT: usize = 128;

/// `mantissa × 10^exponent`, normalised: the mantissa has no trailing zeros
/// and zero has exponent 0, so equal numbers are equal values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Decimal {
    mantissa: i64,
    exponent: i32,
}

fn limit(message: &str) -> Error {
    Error::Limit(message.into())
}

fn digit_count(mut value: u128) -> usize {
    let mut count = 1;
    while value >= 10 {
        value /= 10;
        count += 1;
    }
    count
}

impl Decimal {
    pub const ZERO: Self = Self {
        mantissa: 0,
        exponent: 0,
    };

    /// Normalise and bound `mantissa × 10^exponent`.
    fn new(mut mantissa: i128, mut exponent: i64) -> Result<Self> {
        if mantissa == 0 {
            return Ok(Self::ZERO);
        }
        while mantissa % 10 == 0 {
            mantissa /= 10;
            exponent += 1;
        }
        if digit_count(mantissa.unsigned_abs()) > MAX_DIGITS {
            return Err(limit("a decimal value holds at most 18 significant digits"));
        }
        let exponent = i32::try_from(exponent)
            .ok()
            .filter(|e| (-MAX_EXPONENT..=MAX_EXPONENT).contains(e))
            .ok_or_else(|| limit("a decimal exponent exceeds ±400"))?;
        let mantissa =
            i64::try_from(mantissa).map_err(|_| limit("a decimal mantissa exceeds 18 digits"))?;
        Ok(Self { mantissa, exponent })
    }

    /// `value` rounded to 12 significant digits, the precision quantity text
    /// has always shown. Fails for non-finite values.
    pub fn from_f64(value: f64) -> Result<Self> {
        if !value.is_finite() {
            return Err(Error::Invalid("a decimal value must be finite".into()));
        }
        format!("{value:.11e}").parse()
    }

    /// The nearest `f64`.
    pub fn to_f64(self) -> f64 {
        format!("{}e{}", self.mantissa, self.exponent)
            .parse()
            .unwrap_or(f64::NAN)
    }

    /// `self × 10^places`, exactly.
    pub fn shift(self, places: i32) -> Result<Self> {
        Self::new(
            i128::from(self.mantissa),
            i64::from(self.exponent) + i64::from(places),
        )
    }

    /// The exact product.
    pub fn checked_mul(self, other: Self) -> Result<Self> {
        Self::new(
            i128::from(self.mantissa) * i128::from(other.mantissa),
            i64::from(self.exponent) + i64::from(other.exponent),
        )
    }

    pub fn is_zero(self) -> bool {
        self.mantissa == 0
    }

    pub fn is_negative(self) -> bool {
        self.mantissa < 0
    }

    /// `Some(n)` when this is exactly `10^n`.
    pub fn power_of_ten(self) -> Option<i32> {
        (self.mantissa == 1).then_some(self.exponent)
    }

    /// The power of ten of the leading digit: 2 for 470, −6 for 0.0000047.
    pub fn leading_exponent(self) -> i32 {
        let digits = digit_count(u128::from(self.mantissa.unsigned_abs()));
        // At most 18 digits: the cast is lossless.
        self.exponent + digits as i32 - 1
    }
}

/// `[sign] digits [. digits] [e|E [sign] digits]`.
impl FromStr for Decimal {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        if text.len() > MAX_TEXT {
            return Err(limit("decimal text exceeds 128 bytes"));
        }
        let malformed = || Error::Invalid(format!("‘{text}’ is not a decimal number"));
        let (negative, rest) = match text.as_bytes().first() {
            Some(b'-') => (true, &text[1..]),
            Some(b'+') => (false, &text[1..]),
            _ => (false, text),
        };
        let (number, exponent) = match rest.find(['e', 'E']) {
            Some(at) => {
                let exponent = &rest[at + 1..];
                let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
                if digits.is_empty()
                    || digits.len() > 4
                    || !digits.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(malformed());
                }
                (
                    &rest[..at],
                    exponent.parse::<i64>().map_err(|_| malformed())?,
                )
            }
            None => (rest, 0),
        };
        let (integer, fraction) = number.split_once('.').unwrap_or((number, ""));
        let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        if integer.len() + fraction.len() == 0 || !all_digits(integer) || !all_digits(fraction) {
            return Err(malformed());
        }
        let digits = format!("{integer}{fraction}");
        let significant = digits.trim_start_matches('0');
        let kept = significant.trim_end_matches('0');
        if kept.len() > MAX_DIGITS {
            return Err(limit("a decimal value holds at most 18 significant digits"));
        }
        let mut mantissa: i128 = kept.parse().unwrap_or(0);
        if negative {
            mantissa = -mantissa;
        }
        // Both lengths are at most 128: the casts are lossless.
        let scale = (significant.len() - kept.len()) as i64 - fraction.len() as i64;
        Self::new(mantissa, exponent + scale)
    }
}

/// Plain notation when the leading digit is within 10^-6 – 10^14,
/// otherwise exponent notation (`1.5e-9`), as [`super::format_number`].
impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.is_negative() { "-" } else { "" };
        let digits = self.mantissa.unsigned_abs().to_string();
        let leading = self.leading_exponent();
        if self.is_zero() {
            f.write_str("0")
        } else if !(-6..15).contains(&leading) {
            let (first, rest) = digits.split_at(1);
            let point = if rest.is_empty() { "" } else { "." };
            write!(f, "{sign}{first}{point}{rest}e{leading}")
        } else if self.exponent >= 0 {
            // Within 10^14: at most 14 zeros.
            let zeros = "0".repeat(self.exponent.unsigned_abs() as usize);
            write!(f, "{sign}{digits}{zeros}")
        } else if leading >= 0 {
            let (whole, part) = digits.split_at(leading.unsigned_abs() as usize + 1);
            write!(f, "{sign}{whole}.{part}")
        } else {
            let zeros = "0".repeat(leading.unsigned_abs() as usize - 1);
            write!(f, "{sign}0.{zeros}{digits}")
        }
    }
}

impl Serialize for Decimal {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Decimal {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Decimal".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "An exact decimal number as text, e.g. \"0.00001\" or \"4.7e-9\".",
            "pattern": "^[+-]?([0-9]+\\.?[0-9]*|\\.[0-9]+)([eE][+-]?[0-9]{1,4})?$"
        })
    }
}
