//! The one-line catalog query: free text, reserved filters and unit-aware
//! field predicates.
//!
//! Field keys are data. A key resolves against the names of the field
//! definitions in the indexed library and against the unit texts of their
//! quantity kinds; the code has no field names, units or categories of its
//! own. Values are read by [`quantity::parse`] and [`fields::parse_value`],
//! after an RKM pre-pass ([`rkm`]) whose letters come from the field's own
//! unit set.
use crate::diagnostics::{Error, Result};
use crate::fields::{self, FieldValue, QuantityKind, ValueType};
use crate::ids::{ChoiceId, DefinitionId};
use crate::library::Library;
use crate::quantity::{self, SI_PREFIXES, UnitSet};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Bytes of one query, checked before parsing.
pub const MAX_QUERY: usize = 512;
/// Terms of one query.
pub const MAX_TERMS: usize = 16;
/// Field names listed in an unknown-key message.
const MAX_LISTED: usize = 8;

/// Relative tolerance of an equality test between two SI values, so that
/// `4k7` and `4.7 kΩ` compare equal after floating-point scaling.
const EQUAL_RELATIVE: f64 = 1e-9;

/// A failed parse: the offending term and why. Displayed as
/// `term: message`, e.g. `V>=10k: 10 k has no unit of voltage`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct QueryError {
    pub term: String,
    pub message: String,
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.term, self.message)
    }
}

impl std::error::Error for QueryError {}

impl From<QueryError> for Error {
    fn from(error: QueryError) -> Self {
        Error::Invalid(error.to_string())
    }
}

/// A quantity kind as the query sees it: its unit set and display name.
#[derive(Clone, Debug)]
pub struct VocabularyKind {
    pub name: String,
    pub quantity: QuantityKind,
}

/// One field lineage the query can name, at its latest revision.
#[derive(Clone, Debug)]
pub struct VocabularyField {
    pub field: DefinitionId,
    pub name: String,
    pub value_type: ValueType,
    pub kind: Option<VocabularyKind>,
}

/// The field definitions a query resolves keys against.
#[derive(Clone, Debug, Default)]
pub struct Vocabulary {
    pub fields: Vec<VocabularyField>,
}

impl Vocabulary {
    /// Every field lineage of a library at its latest revision, with the
    /// quantity kind that revision names. Fields whose kind is missing are
    /// left out; they cannot be compared.
    pub fn of(library: &Library) -> Self {
        let mut latest: BTreeMap<DefinitionId, &crate::library::Published<_>> = BTreeMap::new();
        for field in &library.fields {
            let newer = latest
                .get(&field.id)
                .is_none_or(|known| known.revision < field.revision);
            if newer {
                latest.insert(field.id, field);
            }
        }
        let fields = latest
            .into_values()
            .filter_map(|field| {
                let kind = match field.definition.value_type.kind() {
                    Some(r) => {
                        let kind = library.quantity_kind(r)?;
                        Some(VocabularyKind {
                            name: kind.meta.name.clone(),
                            quantity: kind.definition.clone(),
                        })
                    }
                    None => None,
                };
                Some(VocabularyField {
                    field: field.id,
                    name: field.meta.name.clone(),
                    value_type: field.definition.value_type.clone(),
                    kind,
                })
            })
            .collect();
        Self { fields }
    }

    pub fn field(&self, id: DefinitionId) -> Option<&VocabularyField> {
        self.fields.iter().find(|f| f.field == id)
    }
}

/// A comparison operator of a field predicate.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Op {
    fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Lt => "<",
            Self::Le => "≤",
            Self::Gt => ">",
            Self::Ge => "≥",
        }
    }
    /// `value op bound`.
    pub fn holds(self, value: f64, bound: f64) -> bool {
        match self {
            Self::Eq => approx_equal(value, bound),
            Self::Lt => value < bound && !approx_equal(value, bound),
            Self::Le => value <= bound || approx_equal(value, bound),
            Self::Gt => value > bound && !approx_equal(value, bound),
            Self::Ge => value >= bound || approx_equal(value, bound),
        }
    }
}

pub(crate) fn approx_equal(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= EQUAL_RELATIVE * a.abs().max(b.abs())
}

/// What a predicate checks on one field value. Quantities and bounds are SI.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "test", rename_all = "snake_case")]
pub enum Test {
    /// A quantity or integer compared with a bound; a range field's
    /// interval reaches the bound (`≥` its maximum, `≤` its minimum).
    Compare {
        op: Op,
        value: f64,
    },
    /// A quantity or integer inside `min ..= max`; a range field's
    /// interval covers it.
    Between {
        min: f64,
        max: f64,
    },
    /// Tolerance magnitudes compared side by side: `tol≤1%` is met by ±0.5 %.
    Tolerance {
        op: Op,
        minus: f64,
        plus: f64,
    },
    Choice {
        choice: ChoiceId,
    },
    Boolean {
        value: bool,
    },
    /// Case-folded substring of a text field.
    Contains {
        text: String,
    },
}

impl Test {
    /// Whether `value` meets this test.
    pub fn accepts(&self, value: &FieldValue) -> bool {
        match (self, value) {
            (Self::Compare { op, value: bound }, FieldValue::Quantity { si, .. }) => {
                op.holds(*si, *bound)
            }
            (Self::Compare { op, value: bound }, FieldValue::Integer(v)) => {
                op.holds(*v as f64, *bound)
            }
            (Self::Compare { op, value: bound }, FieldValue::Range { min, max }) => match op {
                Op::Eq => min <= bound && max >= bound,
                Op::Gt | Op::Ge => op.holds(*max, *bound),
                Op::Lt | Op::Le => op.holds(*min, *bound),
            },
            (Self::Between { min, max }, FieldValue::Quantity { si, .. }) => {
                Op::Ge.holds(*si, *min) && Op::Le.holds(*si, *max)
            }
            (Self::Between { min, max }, FieldValue::Integer(v)) => {
                Op::Ge.holds(*v as f64, *min) && Op::Le.holds(*v as f64, *max)
            }
            (
                Self::Between { min, max },
                FieldValue::Range {
                    min: low,
                    max: high,
                },
            ) => Op::Le.holds(*low, *min) && Op::Ge.holds(*high, *max),
            (Self::Tolerance { op, minus, plus }, FieldValue::Tolerance { minus: m, plus: p }) => {
                op.holds(*m, *minus) && op.holds(*p, *plus)
            }
            (Self::Choice { choice }, FieldValue::Enum(id)) => choice == id,
            (Self::Boolean { value }, FieldValue::Boolean(v)) => value == v,
            (Self::Contains { text }, FieldValue::Text(v)) => fold(v).contains(text.as_str()),
            _ => false,
        }
    }
}

/// One field a predicate or bare value applies to.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FieldTest {
    pub field: DefinitionId,
    pub test: Test,
    /// How the chip shows this reading, e.g. `Resistance ≥ 10 kΩ`.
    pub display: String,
}

/// The reserved completeness badges `has:` and `missing:` name.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Badge {
    Symbol,
    Footprint,
    Model,
    Mpn,
}

impl Badge {
    pub const ALL: [Self; 4] = [Self::Symbol, Self::Footprint, Self::Model, Self::Mpn];
    pub fn label(self) -> &'static str {
        match self {
            Self::Symbol => "sym",
            Self::Footprint => "fp",
            Self::Model => "3D",
            Self::Mpn => "MPN",
        }
    }
    fn parse(text: &str) -> Option<Self> {
        match text {
            "symbol" | "sym" => Some(Self::Symbol),
            "footprint" | "fp" => Some(Self::Footprint),
            "3d" | "model" => Some(Self::Model),
            "mpn" => Some(Self::Mpn),
            _ => None,
        }
    }
}

/// One parsed term. Terms are ANDed.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "term", rename_all = "snake_case")]
pub enum Term {
    /// Case-folded text matched against names, descriptions, package labels
    /// and field texts.
    Text { text: String },
    /// A bare value such as `10k`: its text, or any field reading it fits.
    Value {
        text: String,
        readings: Vec<FieldTest>,
    },
    /// `key op value`: any of the fields the key resolves to meets its test.
    Field { key: String, tests: Vec<FieldTest> },
    /// `pkg:` — a text match on package names only; it implies nothing
    /// about compatibility.
    Package { text: String },
    /// `has:` and `missing:`.
    Has { badge: Badge, present: bool },
    /// `src:project` — the only source before library folders exist.
    Project,
    /// `kind:component` — the only entry kind before manufacturer parts
    /// exist; it keeps every entry.
    Components,
}

impl Term {
    /// How the palette shows this term as a chip.
    pub fn chip(&self) -> String {
        match self {
            Self::Text { text } => format!("“{text}”"),
            Self::Value { text, readings } => {
                let mut shown: Vec<&str> = readings.iter().map(|r| r.display.as_str()).collect();
                shown.dedup();
                shown.push(text);
                shown.join(" or ")
            }
            Self::Field { tests, .. } => {
                let mut shown: Vec<&str> = tests.iter().map(|t| t.display.as_str()).collect();
                shown.dedup();
                shown.join(" or ")
            }
            Self::Package { text } => format!("pkg: {text}"),
            Self::Has { badge, present } => {
                format!(
                    "{} {}",
                    if *present { "has" } else { "missing" },
                    badge.label()
                )
            }
            Self::Project => "in this project".into(),
            Self::Components => "components".into(),
        }
    }
}

/// A parsed query.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CatalogQuery {
    pub terms: Vec<Term>,
}

impl CatalogQuery {
    pub fn chips(&self) -> Vec<String> {
        self.terms.iter().map(Term::chip).collect()
    }
    /// Whether any term ranks by text.
    pub fn has_text(&self) -> bool {
        self.terms
            .iter()
            .any(|t| matches!(t, Term::Text { .. } | Term::Value { .. }))
    }
}

/// Case folding for matching: lower case, with the micro sign and the Greek
/// small letter mu treated as one letter.
pub fn fold(text: &str) -> String {
    text.to_lowercase().replace('\u{b5}', "\u{3bc}")
}

/// Reserved filter keys, checked before field names and units: a field
/// named like one of them is reached through a shorter prefix (`kin:`).
pub const RESERVED: &[&str] = &[
    "pkg", "has", "missing", "src", "kind", "tag", "shelf", "mpn", "life",
];
/// Reserved keys that name data arriving with library labels and
/// manufacturer parts.
const LATER: &[&str] = &["tag", "shelf", "mpn", "life"];

/// Parse one query line against a vocabulary.
pub fn parse_query(
    text: &str,
    vocabulary: &Vocabulary,
) -> std::result::Result<CatalogQuery, QueryError> {
    if text.len() > MAX_QUERY {
        return Err(QueryError {
            term: String::new(),
            message: format!("a query is at most {MAX_QUERY} bytes"),
        });
    }
    let tokens = tokens(text).map_err(|message| QueryError {
        term: text.trim().into(),
        message,
    })?;
    if tokens.len() > MAX_TERMS {
        return Err(QueryError {
            term: String::new(),
            message: format!("a query has at most {MAX_TERMS} terms"),
        });
    }
    let mut terms = Vec::new();
    for (token, quoted) in tokens {
        let fail = |message: String| QueryError {
            term: token.clone(),
            message,
        };
        let term = if quoted {
            Term::Text { text: fold(&token) }
        } else if let Some((key, op, value)) = split_predicate(&token) {
            predicate(key, op, value, vocabulary).map_err(fail)?
        } else {
            bare(&token, vocabulary)
        };
        terms.push(term);
    }
    Ok(CatalogQuery { terms })
}

/// Whitespace-separated tokens; `"…"` keeps spaces and is always text.
fn tokens(text: &str) -> std::result::Result<Vec<(String, bool)>, String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut quoted = String::new();
            let mut closed = false;
            for c in chars.by_ref() {
                if c == '"' {
                    closed = true;
                    break;
                }
                quoted.push(c);
            }
            if !closed {
                return Err("a quoted text needs its closing \"".into());
            }
            if !quoted.trim().is_empty() {
                out.push((quoted.trim().to_owned(), true));
            }
        } else {
            let mut token = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                token.push(c);
                chars.next();
            }
            out.push((token, false));
        }
    }
    Ok(out)
}

/// `key op value`, where op is the first of `>= <= > < = :` after a
/// non-empty key.
fn split_predicate(token: &str) -> Option<(&str, Op, &str)> {
    let at = token.find(['>', '<', '=', ':']).filter(|&i| i > 0)?;
    let (key, rest) = token.split_at(at);
    let (op, value) = if let Some(v) = rest.strip_prefix(">=") {
        (Op::Ge, v)
    } else if let Some(v) = rest.strip_prefix("<=") {
        (Op::Le, v)
    } else if let Some(v) = rest.strip_prefix('>') {
        (Op::Gt, v)
    } else if let Some(v) = rest.strip_prefix('<') {
        (Op::Lt, v)
    } else {
        (Op::Eq, rest.get(1..).unwrap_or_default())
    };
    Some((key, op, value))
}

fn predicate(
    key: &str,
    op: Op,
    value: &str,
    vocabulary: &Vocabulary,
) -> std::result::Result<Term, String> {
    if value.is_empty() {
        return Err("needs a value after the operator".into());
    }
    let folded = fold(key);
    let filter_op = || {
        if op == Op::Eq {
            Ok(())
        } else {
            Err(format!("{key}: takes a value after ‘:’"))
        }
    };
    match folded.as_str() {
        "pkg" => {
            filter_op()?;
            return Ok(Term::Package { text: fold(value) });
        }
        "has" | "missing" => {
            filter_op()?;
            let badge = Badge::parse(&fold(value))
                .ok_or_else(|| format!("‘{value}’ is not symbol, footprint, 3d or mpn"))?;
            return Ok(Term::Has {
                badge,
                present: folded == "has",
            });
        }
        "src" => {
            filter_op()?;
            return if fold(value) == "project" {
                Ok(Term::Project)
            } else {
                Err("the only source is ‘project’ until library folders exist".into())
            };
        }
        "kind" => {
            filter_op()?;
            return if fold(value) == "component" {
                Ok(Term::Components)
            } else {
                Err("the catalog lists components only until manufacturer parts exist".into())
            };
        }
        reserved if LATER.contains(&reserved) => {
            return Err(format!(
                "‘{reserved}:’ is reserved for library labels and manufacturer parts, which this project has none of yet"
            ));
        }
        _ => {}
    }
    let (fields, by_unit) = resolve_key(key, vocabulary)?;
    let mut tests = Vec::new();
    let mut first_error = None;
    for field in fields {
        match field_test(field, op, value, by_unit.then_some(key)) {
            Ok(test) => tests.push(test),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    if tests.is_empty() {
        return Err(first_error.unwrap_or_else(|| format!("‘{key}’ names no comparable field")));
    }
    Ok(Term::Field {
        key: key.into(),
        tests,
    })
}

/// The fields a key names. A key that spells a unit of some field's
/// quantity kind (`V`, `kV`, `Ω`, an alias such as `R`) is always a unit key
/// and names every field measured in that unit; only otherwise is it matched
/// against field names, exactly and then as a unique prefix. The order never
/// depends on which other fields exist, so adding a field cannot turn a unit
/// key into a name key or back. The flag is set for a unit key.
fn resolve_key<'a>(
    key: &str,
    vocabulary: &'a Vocabulary,
) -> std::result::Result<(Vec<&'a VocabularyField>, bool), String> {
    let measured: Vec<_> = vocabulary
        .fields
        .iter()
        .filter(|f| {
            f.kind
                .as_ref()
                .is_some_and(|k| k.quantity.units.resolve(key).is_some())
        })
        .collect();
    if !measured.is_empty() {
        return Ok((measured, true));
    }
    let folded = fold(key);
    let exact: Vec<_> = vocabulary
        .fields
        .iter()
        .filter(|f| fold(f.name.trim()) == folded)
        .collect();
    if !exact.is_empty() {
        return Ok((exact, false));
    }
    let prefixed: Vec<_> = vocabulary
        .fields
        .iter()
        .filter(|f| fold(f.name.trim()).starts_with(&folded))
        .collect();
    if prefixed.len() == 1 {
        return Ok((prefixed, false));
    }
    let names = |fields: &[&VocabularyField]| {
        let mut names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names.truncate(MAX_LISTED);
        names.join(", ")
    };
    if prefixed.len() > 1 {
        return Err(format!(
            "‘{key}’ matches several fields: {}",
            names(&prefixed)
        ));
    }
    let all: Vec<&VocabularyField> = vocabulary.fields.iter().collect();
    if all.is_empty() {
        Err(format!(
            "‘{key}’ is not a field; this library has no fields"
        ))
    } else {
        Err(format!(
            "‘{key}’ is not a field or filter (fields: {}; filters: {})",
            names(&all),
            RESERVED.join(", ")
        ))
    }
}

/// The test one field gives a predicate value. `unit_key` is the key when it
/// spelled a unit rather than a field name.
fn field_test(
    field: &VocabularyField,
    op: Op,
    value: &str,
    unit_key: Option<&str>,
) -> std::result::Result<FieldTest, String> {
    let kind = field.kind.as_ref();
    let label = field.name.as_str();
    let make = |test: Test, shown: String| FieldTest {
        field: field.field,
        test,
        display: format!("{label} {shown}"),
    };
    match &field.value_type {
        ValueType::Quantity { .. } | ValueType::Range { .. } => {
            let kind = kind.ok_or_else(|| format!("{label} has no quantity kind"))?;
            let format = |si: f64| {
                quantity::format(si, &kind.quantity.units, &kind.quantity.default_unit, None)
            };
            if let Some((low, high)) = value.split_once("..") {
                if op != Op::Eq {
                    return Err("a range takes ‘:’, as in key:min..max".into());
                }
                let min = quantity_value(low.trim(), kind, unit_key)?;
                let max = quantity_value(high.trim(), kind, unit_key)?;
                if min > max {
                    return Err("a range needs minimum ≤ maximum".into());
                }
                return Ok(make(
                    Test::Between { min, max },
                    format!("{} .. {}", format(min), format(max)),
                ));
            }
            let si = quantity_value(value, kind, unit_key)?;
            Ok(make(
                Test::Compare { op, value: si },
                format!("{} {}", op.symbol(), format(si)),
            ))
        }
        ValueType::Tolerance { .. } => {
            if value.contains("..") {
                return Err("tolerances compare with < ≤ ≥ > or ‘:’, not ranges".into());
            }
            let parsed = fields::parse_value(value, &field.value_type, kind.map(|k| &k.quantity))
                .map_err(|e| e.to_string())?;
            let FieldValue::Tolerance { minus, plus } = parsed else {
                return Err("this field expects a tolerance".into());
            };
            let shown = fields::display(&parsed, &field.value_type, kind.map(|k| &k.quantity));
            Ok(make(
                Test::Tolerance { op, minus, plus },
                format!("{} {shown}", op.symbol()),
            ))
        }
        ValueType::Integer { .. } => {
            let parse = |text: &str| {
                text.trim()
                    .parse::<i64>()
                    .map(|v| v as f64)
                    .map_err(|_| format!("‘{}’ is not an integer", text.trim()))
            };
            if let Some((low, high)) = value.split_once("..") {
                if op != Op::Eq {
                    return Err("a range takes ‘:’, as in key:min..max".into());
                }
                let (min, max) = (parse(low)?, parse(high)?);
                return Ok(make(Test::Between { min, max }, format!("{low} .. {high}")));
            }
            let v = parse(value)?;
            Ok(make(
                Test::Compare { op, value: v },
                format!("{} {value}", op.symbol()),
            ))
        }
        ValueType::Enum { .. } | ValueType::Boolean | ValueType::Text { .. } => {
            if op != Op::Eq {
                return Err(format!(
                    "{label} is not ordered; use ‘:’ as in {label}:value"
                ));
            }
            let test = match fields::parse_value(value, &field.value_type, None)
                .map_err(|e| e.to_string())?
            {
                FieldValue::Enum(choice) => Test::Choice { choice },
                FieldValue::Boolean(value) => Test::Boolean { value },
                _ => Test::Contains { text: fold(value) },
            };
            Ok(make(test, format!("= {value}")))
        }
    }
}

/// A predicate value as an SI quantity of `kind`.
///
/// After a field-name key, the value reads like a property entry: a plain
/// number or a bare multiplier is in the kind's default unit. A unit key
/// (`V`, `kV`, `R`) filters every field measured in that unit, so its value
/// is a plain number in the key's unit or states a unit of the same kind; a
/// bare multiplier (`V>=10k`, `R>=10k`) is always refused rather than
/// guessed.
fn quantity_value(
    text: &str,
    kind: &VocabularyKind,
    unit_key: Option<&str>,
) -> std::result::Result<f64, String> {
    let units = &kind.quantity.units;
    let text = rkm(text, units).unwrap_or_else(|| text.to_owned());
    let kind_name = if kind.name.trim().is_empty() {
        "this quantity kind".to_owned()
    } else {
        fold(kind.name.trim())
    };
    let Some(key) = unit_key else {
        return quantity::parse(&text, units, &kind.quantity.default_unit)
            .map(|p| p.quantity.value())
            .map_err(|e| e.to_string());
    };
    let (number, suffix) = split_number(&text)
        .ok_or_else(|| format!("‘{text}’ is not a number with an optional unit"))?;
    let unit = if suffix.is_empty() { key } else { suffix };
    if units.resolve(unit).is_none() {
        return Err(if SI_PREFIXES.iter().any(|(p, _)| *p == suffix) {
            format!(
                "{} {suffix} has no unit of {kind_name}",
                quantity::format_number(number)
            )
        } else {
            format!(
                "‘{suffix}’ is not a unit of {kind_name} (units: {})",
                units.texts().collect::<Vec<_>>().join(", ")
            )
        });
    }
    units
        .to_si(number, unit)
        .map(|q| q.value())
        .map_err(|e| e.to_string())
}

/// `number [whitespace] suffix`, with the number grammar of
/// [`quantity::parse`]: optional sign, digits with `.` and an exponent.
fn split_number(text: &str) -> Option<(f64, &str)> {
    let bytes = text.as_bytes();
    let digits = |from: usize| {
        bytes
            .get(from..)
            .map_or(0, |b| b.iter().take_while(|b| b.is_ascii_digit()).count())
    };
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
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
    let number = text.get(..end)?.parse::<f64>().ok()?;
    Some((number, text.get(end..)?.trim_start()))
}

/// The RKM pre-pass: a letter written as the decimal point becomes a
/// decimal number followed by that letter (`4k7` → `4.7k`, `2u2` → `2.2u`,
/// `4R7` → `4.7R`, `R47` → `0.47R`). Multiplier letters are the one-letter
/// SI prefixes; unit letters are the one-letter symbols and aliases of
/// `units`, so `R` reads as a decimal point only where the unit set itself
/// declares `R`. A leading letter needs to be a unit letter. Returns `None`
/// when the text is not in this form.
pub fn rkm(text: &str, units: &UnitSet) -> Option<String> {
    let unit_letter = |c: char| {
        c.is_ascii_alphabetic()
            && !matches!(c, 'e' | 'E')
            && units.texts().any(|t| t.chars().eq(std::iter::once(c)))
    };
    let prefix_letter = |c: char| {
        SI_PREFIXES
            .iter()
            .any(|(p, _)| p.chars().eq(std::iter::once(c)))
    };
    let text = text.trim();
    let lead: String = text.chars().take_while(char::is_ascii_digit).collect();
    let mut rest = text.get(lead.len()..)?.chars();
    let letter = rest.next()?;
    let unit = unit_letter(letter);
    if !(unit || (prefix_letter(letter) && !lead.is_empty())) {
        return None;
    }
    let tail = rest.as_str();
    let fraction: String = tail.chars().take_while(char::is_ascii_digit).collect();
    if fraction.is_empty() {
        return None;
    }
    let after = tail.get(fraction.len()..)?;
    if after.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let whole = if lead.is_empty() { "0" } else { lead.as_str() };
    Some(format!("{whole}.{fraction}{letter}{after}"))
}

/// A token without an operator: always text, and also a value of every
/// quantity field it reads as when it carries a letter (`10k`, `4k7`,
/// `100nF`). Plain numbers such as `0603` stay text.
fn bare(token: &str, vocabulary: &Vocabulary) -> Term {
    let text = fold(token);
    let starts = token
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == '.' || c.is_ascii_alphabetic());
    let lettered = token
        .chars()
        .any(|c| c.is_alphabetic() || c == 'Ω' || c == 'µ');
    let mut readings = Vec::new();
    if starts && lettered {
        for field in &vocabulary.fields {
            let (ValueType::Quantity { .. }, Some(kind)) = (&field.value_type, &field.kind) else {
                continue;
            };
            let numeric = token.starts_with(|c: char| c.is_ascii_digit() || c == '.')
                || rkm(token, &kind.quantity.units).is_some();
            if !numeric {
                continue;
            }
            if let Ok(si) = quantity_value(token, kind, None) {
                readings.push(FieldTest {
                    field: field.field,
                    test: Test::Compare {
                        op: Op::Eq,
                        value: si,
                    },
                    display: format!(
                        "{} = {}",
                        field.name,
                        quantity::format(
                            si,
                            &kind.quantity.units,
                            &kind.quantity.default_unit,
                            None
                        )
                    ),
                });
            }
        }
    }
    if readings.is_empty() {
        Term::Text { text }
    } else {
        Term::Value { text, readings }
    }
}

/// Parse a full query and report a structured core error.
pub fn parse(text: &str, vocabulary: &Vocabulary) -> Result<CatalogQuery> {
    parse_query(text, vocabulary).map_err(Error::from)
}
