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
//! The rules are code; the tables are data in the shipped library's
//! `standards/` folder, each file citing its source ([`MarkingCodes`]):
//!
//! - **3- and 4-digit resistor codes:** IEC 60062 *Marking codes for
//!   resistors and capacitors*: significant digits followed by a
//!   power-of-ten multiplier digit, and the letter `R` as the decimal marker
//!   for values below the multiplier range.
//! - **Colour code:** IEC 60062's colours of digits, multipliers and
//!   tolerances (`colour-code.json`).
//! - **E96 series:** IEC 60063 *Preferred number series for resistors and
//!   capacitors* (`e96.json`).
//! - **EIA-96:** the two-digit index into the E96 series followed by a
//!   multiplier letter (`eia-96.json`), as published in chip-resistor
//!   manufacturer data sheets.
//! - **Capacitor 3-digit code:** two significant digits in picofarads and
//!   a multiplier digit 0–6, with 9 for ×0.1 and 8 for ×0.01 (IEC 60062).
//! - **Voltage letters:** named, cited tables (`voltage-letters.json`).
use super::super::standards::{self, check_format};
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
/// names are [`MarkingCodes::voltage_table_names`].
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
            Self::VoltageLetter { table } => match standards::get() {
                Ok(s) if s.marking.voltage_table(&table.0).is_some() => None,
                Ok(_) => Some(format!("unknown voltage-letter table \"{}\"", table.0)),
                Err(error) => Some(error.to_string()),
            },
            _ => None,
        }
    }
}

/// A preferred number series of one decade (`standards/e96.json`), as
/// three significant digits in increasing order.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PreferredSeries {
    pub format: u32,
    pub name: String,
    pub source: String,
    pub values: Vec<u16>,
}

/// One EIA-96 multiplier letter with its power of ten.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Eia96Multiplier {
    pub letter: char,
    pub exponent: i32,
}

/// The EIA-96 code (`standards/eia-96.json`): a two-digit index into the
/// E96 series (01 = its first value) followed by a multiplier letter.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Eia96Code {
    pub format: u32,
    pub name: String,
    pub source: String,
    pub multipliers: Vec<Eia96Multiplier>,
}

/// One rated-voltage letter.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VoltageLetter {
    pub letter: String,
    pub volts: f64,
}

/// A cited voltage-letter table, by name.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VoltageLetterTable {
    pub name: String,
    pub source: String,
    pub letters: Vec<VoltageLetter>,
}

/// The voltage-letter tables (`standards/voltage-letters.json`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VoltageLetterTables {
    pub format: u32,
    #[serde(default)]
    pub about: String,
    pub tables: Vec<VoltageLetterTable>,
}

/// One colour of the resistor colour code: the digit, multiplier (a power
/// of ten) and relative tolerance it stands for, where it stands for one.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BandColour {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digit: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiplier: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
}

/// The resistor colour code (`standards/colour-code.json`, IEC 60062). A
/// colour's position in [`ColourCode::colours`] is its index in
/// [`LaidElement::Bands`].
///
/// [`LaidElement::Bands`]: super::LaidElement::Bands
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ColourCode {
    pub format: u32,
    pub name: String,
    pub source: String,
    pub colours: Vec<BandColour>,
    /// The tolerance of a value printed without a tolerance band.
    pub unbanded_tolerance: f64,
}

/// Entries one marking table may list.
const MAX_TABLE: usize = 99;

/// The marking-code standards, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkingCodes {
    pub e96: PreferredSeries,
    pub eia96: Eia96Code,
    pub voltage_letters: VoltageLetterTables,
    pub colour_code: ColourCode,
}

fn bad(what: &str) -> Error {
    Error::Invalid(format!("the marking standards: {what}"))
}

fn finite_positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

/// Whether `values` holds no value twice.
fn unique<T: Ord + Clone>(values: impl Iterator<Item = T>) -> bool {
    let mut all: Vec<T> = values.collect();
    let count = all.len();
    all.sort_unstable();
    all.dedup();
    all.len() == count
}

impl PreferredSeries {
    /// 1–99 increasing three-digit values.
    pub fn check(&self) -> Result<()> {
        check_format(self.format, "the E96 series")?;
        let values = &self.values;
        if values.is_empty()
            || values.len() > MAX_TABLE
            || values.iter().any(|v| !(100..=999).contains(v))
            || values.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(bad(
                "the E96 series needs 1–99 increasing three-digit values",
            ));
        }
        Ok(())
    }
}

impl Eia96Code {
    /// Unique upper-case letters with unique exponents within ±9.
    pub fn check(&self) -> Result<()> {
        check_format(self.format, "the EIA-96 code")?;
        let letters = &self.multipliers;
        if letters.is_empty()
            || letters.len() > MAX_TABLE
            || !unique(letters.iter().map(|m| m.letter))
            || !unique(letters.iter().map(|m| m.exponent))
            || letters
                .iter()
                .any(|m| !m.letter.is_ascii_uppercase() || !(-9..=9).contains(&m.exponent))
        {
            return Err(bad(
                "EIA-96 needs unique upper-case multiplier letters with unique exponents within ±9",
            ));
        }
        Ok(())
    }
}

impl VoltageLetterTables {
    /// Uniquely named tables of 1–99 short letters of positive voltages.
    pub fn check(&self) -> Result<()> {
        check_format(self.format, "the voltage-letter tables")?;
        let tables = &self.tables;
        if tables.len() > MAX_TABLE
            || !unique(tables.iter().map(|t| t.name.as_str()))
            || tables.iter().any(|t| t.name.is_empty())
        {
            return Err(bad("voltage-letter tables need unique names"));
        }
        for table in tables {
            if table.letters.is_empty()
                || table.letters.len() > MAX_TABLE
                || table
                    .letters
                    .iter()
                    .any(|l| l.letter.is_empty() || l.letter.len() > 4 || !finite_positive(l.volts))
            {
                return Err(bad(&format!(
                    "the {} voltage-letter table needs 1–99 letters of positive voltages",
                    table.name
                )));
            }
        }
        Ok(())
    }
}

impl ColourCode {
    /// Each digit 0–9 once, unique multipliers within ±9 and positive
    /// tolerances.
    pub fn check(&self) -> Result<()> {
        check_format(self.format, "the colour code")?;
        let colours = &self.colours;
        let digits_once =
            (0..10u8).all(|d| colours.iter().filter(|c| c.digit == Some(d)).count() == 1);
        if colours.len() > usize::from(u8::MAX)
            || !digits_once
            || colours.iter().any(|c| c.digit.is_some_and(|d| d > 9))
            || !unique(colours.iter().filter_map(|c| c.multiplier))
            || colours
                .iter()
                .filter_map(|c| c.multiplier)
                .any(|m| !(-9..=9).contains(&m))
            || colours
                .iter()
                .any(|c| c.tolerance.is_some_and(|t| !finite_positive(t)))
            || !finite_positive(self.unbanded_tolerance)
        {
            return Err(bad(
                "the colour code needs each digit 0–9 once, unique multipliers within ±9 and positive tolerances",
            ));
        }
        Ok(())
    }
}

impl MarkingCodes {
    /// The tables, after each table's own check.
    pub fn new(
        e96: PreferredSeries,
        eia96: Eia96Code,
        voltage_letters: VoltageLetterTables,
        colour_code: ColourCode,
    ) -> Result<Self> {
        e96.check()?;
        eia96.check()?;
        voltage_letters.check()?;
        colour_code.check()?;
        Ok(Self {
            e96,
            eia96,
            voltage_letters,
            colour_code,
        })
    }

    /// The letters of a named voltage table.
    pub fn voltage_table(&self, name: &str) -> Option<&[VoltageLetter]> {
        self.voltage_letters
            .tables
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.letters.as_slice())
    }

    /// The names of the voltage-letter tables.
    pub fn voltage_table_names(&self) -> impl Iterator<Item = &str> {
        self.voltage_letters.tables.iter().map(|t| t.name.as_str())
    }

    /// The band index of the colour named `name`.
    pub fn colour_index(&self, name: &str) -> Option<u8> {
        self.colour_where(|c| c.name == name)
    }

    fn colour_where(&self, test: impl Fn(&BandColour) -> bool) -> Option<u8> {
        self.colour_code
            .colours
            .iter()
            .position(test)
            .and_then(|k| u8::try_from(k).ok())
    }

    fn eia96(&self, value: f64) -> Option<String> {
        self.eia96.multipliers.iter().find_map(|m| {
            let mantissa = value / 10f64.powi(m.exponent);
            let index = self
                .e96
                .values
                .iter()
                .position(|&e| same(mantissa, f64::from(e)))?;
            Some(format!("{:02}{}", index + 1, m.letter))
        })
    }

    fn voltage_letter(&self, table: &TableRef, volts: f64) -> Result<Option<String>> {
        let rows = self.voltage_table(&table.0).ok_or_else(|| {
            Error::Invalid(format!(
                "marking.template: unknown voltage-letter table \"{}\"",
                table.0
            ))
        })?;
        Ok(rows
            .iter()
            .find(|row| same(row.volts, volts))
            .map(|row| row.letter.clone()))
    }

    /// [`bands`] under these tables.
    pub fn bands(&self, value: f64, tolerance: Option<f64>) -> Result<Option<Vec<u8>>> {
        if !value.is_finite() || tolerance.is_some_and(|t| !t.is_finite()) {
            return Err(Error::Invalid(
                "marking.template: colour bands need finite values".into(),
            ));
        }
        if value <= 0.0 {
            return Ok(None);
        }
        let code = &self.colour_code;
        let tolerance_colour = match tolerance {
            None => None,
            Some(t) if same(t, code.unbanded_tolerance) => None,
            Some(t) => match self.colour_where(|c| c.tolerance.is_some_and(|v| same(v, t))) {
                Some(colour) => Some(colour),
                None => return Ok(None),
            },
        };
        let exponents = code.colours.iter().filter_map(|c| c.multiplier);
        let (Some(min_exp), Some(max_exp)) = (exponents.clone().min(), exponents.max()) else {
            return Ok(None);
        };
        let digits = significant(value, 2, min_exp, max_exp)
            .map(|(m, e)| (m, e, 2))
            .or_else(|| significant(value, 3, min_exp, max_exp).map(|(m, e)| (m, e, 3)));
        let Some((mantissa, exp, count)) = digits else {
            return Ok(None);
        };
        let mut colours = Vec::with_capacity(count + 2);
        for b in mantissa.to_string().bytes() {
            let digit = b.saturating_sub(b'0');
            match self.colour_where(|c| c.digit == Some(digit)) {
                Some(colour) => colours.push(colour),
                None => return Ok(None),
            }
        }
        if colours.len() != count {
            return Ok(None);
        }
        match self.colour_where(|c| c.multiplier == Some(exp)) {
            Some(colour) => colours.push(colour),
            None => return Ok(None),
        }
        colours.extend(tolerance_colour);
        Ok(Some(colours))
    }
}

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

/// The code of `value` (SI units: ohm, farad, volt) in `scheme`, or `None`
/// when the scheme cannot represent it exactly. `Auto` returns the first
/// representable scheme's code (fitting is [`super::layout`]'s job). A
/// non-finite value, a nested or oversized `Auto` list and an unknown table
/// are errors. The tables are the installed standards'.
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
        CodeScheme::Eia96 => standards::get()?.marking.eia96(value),
        CodeScheme::CapacitorPf3Digit => capacitor_code(value),
        CodeScheme::VoltageLetter { table } => {
            standards::get()?.marking.voltage_letter(table, value)?
        }
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
/// tolerance, from the −x end: two digits and a multiplier, or three digits
/// when the value needs them; then the tolerance colour. Each band is an
/// index into the installed colour code ([`ColourCode`]). `None` when the
/// value or tolerance has no code; no tolerance band means the code's
/// unbanded tolerance (±20 %).
pub fn bands(value: f64, tolerance: Option<f64>) -> Result<Option<Vec<u8>>> {
    standards::get()?.marking.bands(value, tolerance)
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
        let _context = crate::library::standards::Context::fixture().map(|c| c.enter());
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
    fn eia96_covers_the_series_and_multipliers() -> Result<()> {
        let _context = crate::library::standards::Context::fixture().map(|c| c.enter());
        use CodeScheme::Eia96;
        assert_eq!(code(Eia96, 976e5).as_deref(), Some("96F"));
        assert_eq!(code(Eia96, 0.1).as_deref(), Some("01Z"));
        assert_eq!(code(Eia96, 1.0).as_deref(), Some("01Y"));
        assert_eq!(code(Eia96, 12.4).as_deref(), Some("10X"));
        assert_eq!(code(Eia96, 249.0).as_deref(), Some("39A"));
        assert_eq!(code(Eia96, 10.3e3), None, "103 is not an E96 value");
        assert_eq!(code(Eia96, 1e8), None, "above the F multiplier");
        // The series is strictly increasing within one decade.
        let standards = standards::get()?;
        let e96 = &standards.marking.e96.values;
        assert_eq!(e96.len(), 96);
        assert!(e96.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(e96[67], 499);
        Ok(())
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
        let _context = crate::library::standards::Context::fixture().map(|c| c.enter());
        let tantalum = CodeScheme::VoltageLetter {
            table: TableRef("tantalum".into()),
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
        let _context = crate::library::standards::Context::fixture().map(|c| c.enter());
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
        let _context = crate::library::standards::Context::fixture().map(|c| c.enter());
        // 4.7 kΩ ±5 %: yellow violet red gold (index 10).
        assert_eq!(bands(4.7e3, Some(0.05))?, Some(vec![4, 7, 2, 10]));
        // 10 kΩ ±1 %: brown black orange brown.
        assert_eq!(bands(10e3, Some(0.01))?, Some(vec![1, 0, 3, 1]));
        // 4.99 kΩ ±1 %: yellow white white brown brown (three digits).
        assert_eq!(bands(4.99e3, Some(0.01))?, Some(vec![4, 9, 9, 1, 1]));
        // 0.47 Ω, no tolerance band: yellow violet silver (index 11).
        assert_eq!(bands(0.47, None)?, Some(vec![4, 7, 11]));
        // ±20 % has no band.
        assert_eq!(bands(1e3, Some(0.20))?, Some(vec![1, 0, 2]));
        assert_eq!(bands(1e3, Some(0.03))?, None, "no ±3 % colour");
        assert_eq!(bands(4.991e3, None)?, None);
        assert_eq!(bands(0.0, None)?, None);
        assert!(bands(f64::INFINITY, None).is_err());
        Ok(())
    }

    #[test]
    fn broken_tables_are_refused() -> Result<()> {
        let _context = crate::library::standards::Context::fixture().map(|c| c.enter());
        let standards = standards::get()?;
        let shipped = &standards.marking;
        let rebuilt = |edit: &dyn Fn(&mut MarkingCodes)| {
            let mut codes = shipped.clone();
            edit(&mut codes);
            MarkingCodes::new(
                codes.e96,
                codes.eia96,
                codes.voltage_letters,
                codes.colour_code,
            )
        };
        assert!(rebuilt(&|_| {}).is_ok());
        assert!(rebuilt(&|c| c.e96.values.swap(0, 1)).is_err(), "decreasing");
        assert!(rebuilt(&|c| c.e96.format = 2).is_err(), "another format");
        assert!(
            rebuilt(&|c| c.eia96.multipliers[1].letter = 'Z').is_err(),
            "a letter twice"
        );
        assert!(
            rebuilt(&|c| c.colour_code.colours[3].digit = Some(2)).is_err(),
            "a digit twice"
        );
        assert!(
            rebuilt(&|c| c.voltage_letters.tables[0].letters[0].volts = f64::NAN).is_err(),
            "a voltage that is not a number"
        );
        Ok(())
    }
}
