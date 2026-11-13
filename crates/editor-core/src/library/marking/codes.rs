//! Slice D4: value marking codes as pure functions over SI values, with
//! hand-derived tests and cited tables. `None` means the scheme cannot
//! represent the value exactly; nothing is ever rounded into a wrong code.
//!
//! | Value | 3-digit | 4-digit | EIA-96 | Capacitor 3-digit |
//! | --- | --- | --- | --- | --- |
//! | 10 kΩ | `103` | `1002` | `01C` | — |
//! | 4.7 kΩ | `472` | `4701` | — | — |
//! | 4.99 kΩ | — | `4991` | `68B` | — |
//! | 4.7 Ω | `4R7` | `4R70` | — | — |
//! | 100 nF | — | — | — | `104` |
//!
//! Sources (public specifications and manufacturer documentation, no EDA
//! data):
//!
//! - **3- and 4-digit resistor codes and the colour code:** IEC 60062
//!   *Marking codes for resistors and capacitors*: significant digits
//!   followed by a power-of-ten multiplier digit, and the letter `R` as the
//!   decimal marker for values below the multiplier range.
//! - **E96 series:** IEC 60063 *Preferred number series for resistors and
//!   capacitors*, the 96 values per decade of [`E96`].
//! - **EIA-96:** the two-digit index into the E96 series (01 = 100 …
//!   96 = 976) followed by a multiplier letter ([`EIA96_MULTIPLIERS`]), as
//!   published in chip-resistor manufacturer data sheets.
//! - **Capacitor 3-digit code:** two significant digits in picofarads and
//!   a multiplier digit 0–6, with 9 for ×0.1 and 8 for ×0.01 (IEC 60062).
//! - **Tantalum voltage letters:** [`TANTALUM_VOLTAGE_LETTERS`], the rated
//!   voltage codes printed by Vishay 293D, TDK/EPCOS B45196E and AVX TAJ
//!   data sheets (identical for the voltages listed). Letters vary by
//!   series and case for some makers, so the table is named explicitly.
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Schemes one `Auto` list may try.
pub const MAX_AUTO_SCHEMES: usize = 8;

/// Relative tolerance of "exactly representable": absorbs the binary
/// rounding of a parsed decimal (4.7e3 → 4700.000000000001), never a
/// different value.
const EXACT: f64 = 1e-9;

/// A cited voltage-letter table (tantalum capacitors), by name. The known
/// names are in [`voltage_tables`].
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
pub struct TableRef(pub String);

/// A value marking scheme.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "scheme", rename_all = "snake_case")]
pub enum CodeScheme {
    Resistor3Digit,
    Resistor4Digit,
    Eia96,
    CapacitorPf3Digit,
    VoltageLetter {
        table: TableRef,
    },
    /// The first scheme that represents the value and fits the area at the
    /// minimum legible height. Nested `Auto` lists are refused.
    Auto {
        schemes: Vec<CodeScheme>,
    },
}

impl CodeScheme {
    /// A short name for messages.
    pub fn label(&self) -> String {
        match self {
            Self::Resistor3Digit => "3-digit".into(),
            Self::Resistor4Digit => "4-digit".into(),
            Self::Eia96 => "EIA-96".into(),
            Self::CapacitorPf3Digit => "capacitor 3-digit".into(),
            Self::VoltageLetter { table } => format!("voltage letter ({})", table.0),
            Self::Auto { schemes } => {
                let names: Vec<String> = schemes.iter().map(Self::label).collect();
                format!("first of {}", names.join(", "))
            }
        }
    }

    /// The schemes an `Auto` list tries in order, or this one alone.
    pub fn candidates(&self) -> Vec<&CodeScheme> {
        match self {
            Self::Auto { schemes } => schemes.iter().collect(),
            other => vec![other],
        }
    }

    /// Why the scheme cannot be used, if it cannot: an empty, long or
    /// nested `Auto` list or an unknown voltage table.
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::Auto { schemes } if schemes.is_empty() => {
                Some("an automatic code lists no schemes".into())
            }
            Self::Auto { schemes } if schemes.len() > MAX_AUTO_SCHEMES => Some(format!(
                "an automatic code lists more than {MAX_AUTO_SCHEMES} schemes"
            )),
            Self::Auto { schemes } => schemes.iter().find_map(|s| match s {
                Self::Auto { .. } => Some("automatic codes cannot nest".into()),
                other => other.problem(),
            }),
            Self::VoltageLetter { table } if voltage_table(&table.0).is_none() => {
                Some(format!("unknown voltage-letter table \"{}\"", table.0))
            }
            _ => None,
        }
    }
}

/// The E96 series per decade (IEC 60063), index 0 = EIA-96 code `01`.
pub const E96: [u16; 96] = [
    100, 102, 105, 107, 110, 113, 115, 118, 121, 124, 127, 130, 133, 137, 140, 143, 147, 150, 154,
    158, 162, 165, 169, 174, 178, 182, 187, 191, 196, 200, 205, 210, 215, 221, 226, 232, 237, 243,
    249, 255, 261, 267, 274, 280, 287, 294, 301, 309, 316, 324, 332, 340, 348, 357, 365, 374, 383,
    392, 402, 412, 422, 432, 442, 453, 464, 475, 487, 499, 511, 523, 536, 549, 562, 576, 590, 604,
    619, 634, 649, 665, 681, 698, 715, 732, 750, 768, 787, 806, 825, 845, 866, 887, 909, 931, 953,
    976,
];

/// EIA-96 multiplier letters with their power of ten. The alternative
/// letters some makers print (`R` for Y, `S` for X, `H` for B) decode to
/// the same values; Sparksmith writes the letters below.
pub const EIA96_MULTIPLIERS: [(char, i32); 9] = [
    ('Z', -3),
    ('Y', -2),
    ('X', -1),
    ('A', 0),
    ('B', 1),
    ('C', 2),
    ('D', 3),
    ('E', 4),
    ('F', 5),
];

/// Name of the tantalum voltage-letter table.
pub const TANTALUM_TABLE: &str = "tantalum";

/// Tantalum rated-voltage letters: (letter, volts). Cited in the module
/// documentation.
pub const TANTALUM_VOLTAGE_LETTERS: [(&str, f64); 9] = [
    ("e", 2.5),
    ("G", 4.0),
    ("J", 6.3),
    ("A", 10.0),
    ("C", 16.0),
    ("D", 20.0),
    ("E", 25.0),
    ("V", 35.0),
    ("T", 50.0),
];

/// The named voltage-letter tables.
pub fn voltage_tables() -> [&'static str; 1] {
    [TANTALUM_TABLE]
}

fn voltage_table(name: &str) -> Option<&'static [(&'static str, f64)]> {
    (name == TANTALUM_TABLE).then_some(&TANTALUM_VOLTAGE_LETTERS[..])
}

/// IEC 60062 colour indices used by [`LaidElement::Bands`]: 0–9 are the
/// digit colours black, brown, red, orange, yellow, green, blue, violet,
/// grey and white; then gold and silver.
///
/// [`LaidElement::Bands`]: super::LaidElement::Bands
pub const BAND_GOLD: u8 = 10;
pub const BAND_SILVER: u8 = 11;
/// Colour names by index, for presentation.
pub const BAND_COLOURS: [&str; 12] = [
    "black", "brown", "red", "orange", "yellow", "green", "blue", "violet", "grey", "white",
    "gold", "silver",
];

/// IEC 60062 tolerance colours: (relative tolerance, colour index).
/// Without a tolerance band the tolerance is ±20 %.
const TOLERANCE_COLOURS: [(f64, u8); 8] = [
    (0.01, 1),
    (0.02, 2),
    (0.005, 5),
    (0.0025, 6),
    (0.001, 7),
    (0.0005, 8),
    (0.05, BAND_GOLD),
    (0.10, BAND_SILVER),
];

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= EXACT * a.abs().max(b.abs())
}

/// `value` as `mantissa × 10^exp` with exactly `digits` significant digits
/// (the mantissa in `10^(digits−1) ..= 10^digits − 1`) and `exp` in
/// `min_exp ..= max_exp`.
fn significant(value: f64, digits: u32, min_exp: i32, max_exp: i32) -> Option<(u32, i32)> {
    let low = 10f64.powi(i32::try_from(digits).ok()? - 1);
    for exp in min_exp..=max_exp {
        let mantissa = value / 10f64.powi(exp);
        let rounded = mantissa.round();
        if rounded >= low && rounded < low * 10.0 && same(mantissa, rounded) {
            // In range of u32: below 10^digits with digits ≤ 3.
            return Some((rounded as u32, exp));
        }
    }
    None
}

/// The resistor code with `sig` significant digits (2 for 3-digit, 3 for
/// 4-digit): digits then a multiplier digit 0–9, or `R` as the decimal
/// marker below `10^(sig−1)` Ω.
fn resistor_code(value: f64, sig: u32) -> Option<String> {
    let width = usize::try_from(sig).ok()?;
    if value == 0.0 {
        // A zero-ohm jumper is printed as zeros.
        return Some("0".repeat(width + 1));
    }
    let threshold = 10f64.powi(i32::try_from(sig).ok()? - 1);
    if value >= threshold {
        let (mantissa, exp) = significant(value, sig, 0, 9)?;
        return Some(format!("{mantissa:0width$}{exp}"));
    }
    // Below the multiplier range: `sig` digits with an R in place of the
    // decimal point (4R7, R47, 49R9).
    let int_digits = if value >= 10.0 {
        2
    } else if value >= 1.0 {
        1
    } else {
        0
    };
    if int_digits >= sig {
        return None;
    }
    let decimals = sig - int_digits;
    let scaled = value * 10f64.powi(i32::try_from(decimals).ok()?);
    let rounded = scaled.round();
    if !same(scaled, rounded) || rounded < 1.0 || rounded >= threshold * 10.0 {
        return None;
    }
    let digits = format!("{:0width$}", rounded as u32);
    let at = usize::try_from(int_digits).ok()?;
    let (head, tail) = digits.split_at(at.min(digits.len()));
    Some(format!("{head}R{tail}"))
}

fn eia96(value: f64) -> Option<String> {
    EIA96_MULTIPLIERS.iter().find_map(|&(letter, exp)| {
        let mantissa = value / 10f64.powi(exp);
        let index = E96.iter().position(|&e| same(mantissa, f64::from(e)))?;
        Some(format!("{:02}{letter}", index + 1))
    })
}

fn capacitor_code(farads: f64) -> Option<String> {
    let pf = farads * 1e12;
    let (mantissa, exp) = significant(pf, 2, -2, 6)?;
    let digit = match exp {
        -2 => 8,
        -1 => 9,
        e => e,
    };
    Some(format!("{mantissa:02}{digit}"))
}

fn voltage_letter(table: &TableRef, volts: f64) -> Result<Option<String>> {
    let rows = voltage_table(&table.0).ok_or_else(|| {
        Error::Invalid(format!(
            "marking.template: unknown voltage-letter table \"{}\"",
            table.0
        ))
    })?;
    Ok(rows
        .iter()
        .find(|(_, v)| same(*v, volts))
        .map(|(letter, _)| (*letter).to_string()))
}

/// The code of `value` (SI units: ohm, farad, volt) in `scheme`, or `None`
/// when the scheme cannot represent it exactly. `Auto` returns the first
/// representable scheme's code (fitting is [`super::layout`]'s job). A
/// non-finite value, a nested or oversized `Auto` list and an unknown table
/// are errors.
pub fn encode(scheme: &CodeScheme, value: f64) -> Result<Option<String>> {
    if !value.is_finite() {
        return Err(Error::Invalid(
            "marking.template: a marking code needs a finite value".into(),
        ));
    }
    if let Some(problem) = scheme.problem() {
        return Err(Error::Invalid(format!("marking.template: {problem}")));
    }
    if value < 0.0 {
        return Ok(None);
    }
    Ok(match scheme {
        CodeScheme::Resistor3Digit => resistor_code(value, 2),
        CodeScheme::Resistor4Digit => resistor_code(value, 3),
        CodeScheme::Eia96 => eia96(value),
        CodeScheme::CapacitorPf3Digit => capacitor_code(value),
        CodeScheme::VoltageLetter { table } => voltage_letter(table, value)?,
        CodeScheme::Auto { schemes } => {
            for inner in schemes {
                if let Some(code) = encode(inner, value)? {
                    return Ok(Some(code));
                }
            }
            None
        }
    })
}

/// IEC 60062 colour bands of a resistance (ohm) with an optional relative
/// tolerance, from the −x end: two digits and a multiplier (silver ×0.01
/// to white ×10⁹), or three digits when the value needs them; then the
/// tolerance colour. `None` when the value or tolerance has no code; no
/// tolerance band means ±20 %.
pub fn bands(value: f64, tolerance: Option<f64>) -> Result<Option<Vec<u8>>> {
    if !value.is_finite() || tolerance.is_some_and(|t| !t.is_finite()) {
        return Err(Error::Invalid(
            "marking.template: colour bands need finite values".into(),
        ));
    }
    if value <= 0.0 {
        return Ok(None);
    }
    let tolerance_colour = match tolerance {
        None => None,
        Some(t) if same(t, 0.20) => None,
        Some(t) => match TOLERANCE_COLOURS.iter().find(|(v, _)| same(*v, t)) {
            Some(&(_, colour)) => Some(colour),
            None => return Ok(None),
        },
    };
    let digits = significant(value, 2, -2, 9)
        .map(|(m, e)| (m, e, 2))
        .or_else(|| significant(value, 3, -2, 9).map(|(m, e)| (m, e, 3)));
    let Some((mantissa, exp, count)) = digits else {
        return Ok(None);
    };
    let mut colours: Vec<u8> = mantissa
        .to_string()
        .bytes()
        .map(|b| b.saturating_sub(b'0'))
        .collect();
    if colours.len() != count {
        return Ok(None);
    }
    colours.push(match exp {
        -2 => BAND_SILVER,
        -1 => BAND_GOLD,
        e => u8::try_from(e).map_err(|_| Error::Invalid("band multiplier".into()))?,
    });
    colours.extend(tolerance_colour);
    Ok(Some(colours))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(scheme: CodeScheme, value: f64) -> Option<String> {
        encode(&scheme, value).ok().flatten()
    }

    // Every expected code below is derived by hand from the cited rules.

    #[test]
    fn the_contract_table() {
        use CodeScheme::*;
        assert_eq!(code(Resistor3Digit, 10e3).as_deref(), Some("103"));
        assert_eq!(code(Resistor4Digit, 10e3).as_deref(), Some("1002"));
        assert_eq!(code(Eia96, 10e3).as_deref(), Some("01C"));
        assert_eq!(code(Resistor3Digit, 4.7e3).as_deref(), Some("472"));
        assert_eq!(code(Resistor4Digit, 4.7e3).as_deref(), Some("4701"));
        assert_eq!(code(Eia96, 4.7e3), None, "470 is not an E96 value");
        assert_eq!(
            code(Resistor3Digit, 4.99e3),
            None,
            "three significant digits"
        );
        assert_eq!(code(Resistor4Digit, 4.99e3).as_deref(), Some("4991"));
        assert_eq!(code(Eia96, 4.99e3).as_deref(), Some("68B"));
        assert_eq!(code(Resistor3Digit, 4.7).as_deref(), Some("4R7"));
        assert_eq!(code(Resistor4Digit, 4.7).as_deref(), Some("4R70"));
        assert_eq!(code(CapacitorPf3Digit, 100e-9).as_deref(), Some("104"));
    }

    #[test]
    fn resistor_codes_at_the_edges() {
        use CodeScheme::*;
        assert_eq!(code(Resistor3Digit, 10.0).as_deref(), Some("100"));
        assert_eq!(code(Resistor3Digit, 47.0).as_deref(), Some("470"));
        assert_eq!(code(Resistor3Digit, 0.47).as_deref(), Some("R47"));
        assert_eq!(code(Resistor3Digit, 0.05).as_deref(), Some("R05"));
        assert_eq!(code(Resistor3Digit, 1.0).as_deref(), Some("1R0"));
        assert_eq!(code(Resistor3Digit, 0.0).as_deref(), Some("000"));
        assert_eq!(code(Resistor3Digit, 1e6).as_deref(), Some("105"));
        assert_eq!(code(Resistor3Digit, 99e9).as_deref(), Some("999"));
        assert_eq!(code(Resistor3Digit, 1e11), None, "multiplier above 9");
        assert_eq!(code(Resistor3Digit, 4.75), None);
        assert_eq!(code(Resistor3Digit, 0.001), None);
        assert_eq!(code(Resistor4Digit, 100.0).as_deref(), Some("1000"));
        assert_eq!(code(Resistor4Digit, 49.9).as_deref(), Some("49R9"));
        assert_eq!(code(Resistor4Digit, 10.0).as_deref(), Some("10R0"));
        assert_eq!(code(Resistor4Digit, 0.1).as_deref(), Some("R100"));
        assert_eq!(code(Resistor4Digit, 0.0).as_deref(), Some("0000"));
        assert_eq!(code(Resistor4Digit, 10.3e3).as_deref(), Some("1032"));
        assert_eq!(
            code(Resistor4Digit, 4.991e3),
            None,
            "four significant digits"
        );
        assert_eq!(code(Resistor3Digit, -1.0), None);
    }

    #[test]
    fn eia96_covers_the_series_and_multipliers() {
        use CodeScheme::Eia96;
        assert_eq!(code(Eia96, 976e5).as_deref(), Some("96F"));
        assert_eq!(code(Eia96, 0.1).as_deref(), Some("01Z"));
        assert_eq!(code(Eia96, 1.0).as_deref(), Some("01Y"));
        assert_eq!(code(Eia96, 12.4).as_deref(), Some("10X"));
        assert_eq!(code(Eia96, 249.0).as_deref(), Some("39A"));
        assert_eq!(code(Eia96, 10.3e3), None, "103 is not an E96 value");
        assert_eq!(code(Eia96, 1e8), None, "above the F multiplier");
        // The series is strictly increasing within one decade.
        assert!(E96.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(E96[67], 499);
    }

    #[test]
    fn capacitor_codes() {
        use CodeScheme::CapacitorPf3Digit as Cap;
        assert_eq!(code(Cap, 10e-12).as_deref(), Some("100"));
        assert_eq!(code(Cap, 4.7e-12).as_deref(), Some("479"));
        assert_eq!(code(Cap, 0.47e-12).as_deref(), Some("478"));
        assert_eq!(code(Cap, 1e-6).as_deref(), Some("105"));
        assert_eq!(code(Cap, 22e-6).as_deref(), Some("226"));
        assert_eq!(code(Cap, 100e-6), None, "multiplier above 6");
        assert_eq!(code(Cap, 4.75e-9), None);
    }

    #[test]
    fn voltage_letters_come_from_the_named_table() -> Result<()> {
        let tantalum = CodeScheme::VoltageLetter {
            table: TableRef(TANTALUM_TABLE.into()),
        };
        assert_eq!(code(tantalum.clone(), 6.3).as_deref(), Some("J"));
        assert_eq!(code(tantalum.clone(), 16.0).as_deref(), Some("C"));
        assert_eq!(code(tantalum.clone(), 2.5).as_deref(), Some("e"));
        assert_eq!(code(tantalum, 12.0), None);
        let unknown = CodeScheme::VoltageLetter {
            table: TableRef("vendor".into()),
        };
        assert!(encode(&unknown, 6.3).is_err());
        Ok(())
    }

    #[test]
    fn auto_picks_the_first_representable_scheme() -> Result<()> {
        use CodeScheme::*;
        let auto = Auto {
            schemes: vec![Eia96, Resistor4Digit, Resistor3Digit],
        };
        assert_eq!(encode(&auto, 10e3)?.as_deref(), Some("01C"));
        assert_eq!(encode(&auto, 4.7e3)?.as_deref(), Some("4701"));
        assert_eq!(encode(&auto, 4.991e3)?, None);
        let nested = Auto {
            schemes: vec![auto.clone()],
        };
        assert!(encode(&nested, 1.0).is_err());
        assert!(encode(&Auto { schemes: vec![] }, 1.0).is_err());
        assert!(encode(&Eia96, f64::NAN).is_err());
        Ok(())
    }

    #[test]
    fn colour_bands() -> Result<()> {
        // 4.7 kΩ ±5 %: yellow violet red gold.
        assert_eq!(bands(4.7e3, Some(0.05))?, Some(vec![4, 7, 2, BAND_GOLD]));
        // 10 kΩ ±1 %: brown black orange brown.
        assert_eq!(bands(10e3, Some(0.01))?, Some(vec![1, 0, 3, 1]));
        // 4.99 kΩ ±1 %: yellow white white brown brown (three digits).
        assert_eq!(bands(4.99e3, Some(0.01))?, Some(vec![4, 9, 9, 1, 1]));
        // 0.47 Ω, no tolerance band: yellow violet silver.
        assert_eq!(bands(0.47, None)?, Some(vec![4, 7, BAND_SILVER]));
        // ±20 % has no band.
        assert_eq!(bands(1e3, Some(0.20))?, Some(vec![1, 0, 2]));
        assert_eq!(bands(1e3, Some(0.03))?, None, "no ±3 % colour");
        assert_eq!(bands(4.991e3, None)?, None);
        assert_eq!(bands(0.0, None)?, None);
        assert!(bands(f64::INFINITY, None).is_err());
        Ok(())
    }
}
