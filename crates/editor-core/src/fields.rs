//! Typed reusable fields: value types and the meaning of values.
//!
//! Field definitions and quantity kinds are library data with their own
//! revisions; `library` stores them and calls this module to check them.
//! Instances assign values by field lineage; `instances` calls this module to
//! check, apply and resolve them. Parsing, checking, defaults versus
//! overrides and formatting are written here once. Inputs are plain data:
//! this module never sees `library` or `model` types.
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::ids::{ChoiceId, DefinitionId, FieldRef, MAX_LABEL, QuantityKindRef, printable};
use crate::quantity::{self, MAX_MAGNITUDE, MAX_SYMBOL, UnitSet};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod spec;
pub use spec::{MAX_RATED_SPECS, RatedSpec, Requirement, SpecMeaning, satisfies};

/// Fields one component revision selects; overrides one instance holds.
pub const MAX_FIELDS: usize = 64;
/// Edits one `instance.set_properties` applies.
pub const MAX_PROPERTY_EDITS: usize = 64;
pub const MAX_CHOICES: usize = 128;
/// The largest `Text { max_bytes }`.
pub const MAX_TEXT_BYTES: u16 = 256;
/// Bytes of one property text input, checked before parsing.
pub const MAX_INPUT: usize = 256;

/// A kind of quantity: a unit set with its own identity. Two kinds with the
/// same dimension are different kinds; a unit symbol never establishes kind
/// identity.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QuantityKind {
    pub units: UnitSet,
    /// The unit bare numbers and bare SI prefixes are read in.
    pub default_unit: String,
}

/// A reusable typed field.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FieldDefinition {
    /// Next unused local ID (enumeration choices). Retired IDs are never
    /// reused.
    pub next_local: u32,
    pub value_type: ValueType,
    pub default: Option<FieldValue>,
    /// The instance stage a missing value blocks, if the field is required.
    pub required_at: Option<Stage>,
}

impl Default for FieldDefinition {
    fn default() -> Self {
        Self {
            next_local: 1,
            value_type: ValueType::Boolean,
            default: None,
            required_at: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueType {
    /// An SI value of one quantity kind, optionally bounded (SI).
    Quantity {
        kind: QuantityKindRef,
        min: Option<f64>,
        max: Option<f64>,
    },
    Integer {
        min: i64,
        max: i64,
    },
    Boolean,
    Enum {
        choices: Vec<Choice>,
    },
    Text {
        /// At most 256.
        max_bytes: u16,
    },
    Tolerance {
        basis: ToleranceBasis,
    },
    /// An SI interval of one quantity kind.
    Range {
        kind: QuantityKindRef,
    },
}

/// One enumeration choice. The label is display text, never identity.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub id: ChoiceId,
    pub label: String,
}

/// What a tolerance is relative to.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "basis", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToleranceBasis {
    /// Fractions of the nominal value: `5%` is 0.05.
    Relative,
    /// SI amounts of a quantity kind.
    Absolute { kind: QuantityKindRef },
}

/// A typed value. Quantities are stored in SI; `display_unit` is
/// presentation and never changes `si`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FieldValue {
    Quantity {
        si: f64,
        display_unit: Option<String>,
    },
    Integer(i64),
    Boolean(bool),
    Enum(ChoiceId),
    Text(String),
    /// Magnitudes below and above the nominal value, both non-negative.
    Tolerance {
        minus: f64,
        plus: f64,
    },
    Range {
        min: f64,
        max: f64,
    },
}

/// A field a component revision selects, with its component default.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FieldSelection {
    pub field: FieldRef,
    /// Overrides the field default; must satisfy the field.
    pub default: Option<FieldValue>,
    pub show_on_symbol: bool,
}

/// An instance's decision for one field. A missing entry means inherited.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(
    tag = "state",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FieldOverride {
    Assigned(FieldValue),
    /// Explicitly no value, even where a default exists.
    Unset,
}

/// Overrides keyed by field lineage, so a field revision that only renames
/// its label keeps every assignment.
pub type InstanceFields = BTreeMap<DefinitionId, FieldOverride>;

/// One requested change of one instance field.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PropertyEdit {
    /// The field lineage.
    pub field: DefinitionId,
    pub input: PropertyInput,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(
    tag = "input",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PropertyInput {
    /// Text parsed in core against the field's value type.
    Text(String),
    Value(FieldValue),
    /// Remove the override; the component or field default applies.
    Inherit,
    Unset,
}

/// Where an effective value comes from.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Assigned,
    ComponentDefault,
    FieldDefault,
    ExplicitlyUnset,
    NoValue,
}

/// A field's effective value on one instance. Derived, never stored.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EffectiveField {
    pub field: FieldRef,
    /// The field's name, for presentation.
    pub label: String,
    pub value: Option<FieldValue>,
    pub origin: Origin,
    /// The value formatted in core, for presentation.
    pub text: Option<String>,
    pub show_on_symbol: bool,
    pub required_at: Option<Stage>,
}

fn invalid(message: String) -> Error {
    Error::Invalid(message)
}

fn finite(value: f64) -> bool {
    value.is_finite() && value.abs() <= MAX_MAGNITUDE
}

impl ValueType {
    /// The quantity kind this value type is measured in, if any.
    pub fn kind(&self) -> Option<QuantityKindRef> {
        match self {
            Self::Quantity { kind, .. }
            | Self::Range { kind }
            | Self::Tolerance {
                basis: ToleranceBasis::Absolute { kind },
            } => Some(*kind),
            _ => None,
        }
    }
    fn name(&self) -> &'static str {
        match self {
            Self::Quantity { .. } => "quantity",
            Self::Integer { .. } => "integer",
            Self::Boolean => "boolean",
            Self::Enum { .. } => "enumeration",
            Self::Text { .. } => "text",
            Self::Tolerance { .. } => "tolerance",
            Self::Range { .. } => "range",
        }
    }
}

impl FieldValue {
    /// Text lengths: what a draft must respect to persist.
    pub fn within_bounds(&self) -> bool {
        match self {
            Self::Quantity { display_unit, .. } => {
                display_unit.as_ref().is_none_or(|u| u.len() <= MAX_SYMBOL)
            }
            Self::Text(text) => text.len() <= usize::from(MAX_TEXT_BYTES),
            _ => true,
        }
    }
    /// Finite numbers: what a draft must respect to persist meaningfully.
    pub fn values_finite(&self) -> bool {
        match self {
            Self::Quantity { si, .. } => si.is_finite(),
            Self::Tolerance { minus, plus } => minus.is_finite() && plus.is_finite(),
            Self::Range { min, max } => min.is_finite() && max.is_finite(),
            Self::Integer(_) | Self::Boolean(_) | Self::Enum(_) | Self::Text(_) => true,
        }
    }
}

impl QuantityKind {
    pub fn within_bounds(&self) -> bool {
        self.units.within_bounds() && self.default_unit.len() <= MAX_SYMBOL
    }
    pub fn values_finite(&self) -> bool {
        self.units.units.iter().all(|u| u.scale.is_finite())
    }
}

impl FieldDefinition {
    pub fn within_bounds(&self) -> bool {
        let choices_bounded = match &self.value_type {
            ValueType::Enum { choices } => {
                choices.len() <= MAX_CHOICES && choices.iter().all(|c| c.label.len() <= MAX_LABEL)
            }
            _ => true,
        };
        choices_bounded && self.default.as_ref().is_none_or(FieldValue::within_bounds)
    }
    pub fn values_finite(&self) -> bool {
        let bounds_finite = match &self.value_type {
            ValueType::Quantity { min, max, .. } => {
                min.is_none_or(f64::is_finite) && max.is_none_or(f64::is_finite)
            }
            _ => true,
        };
        bounds_finite && self.default.as_ref().is_none_or(FieldValue::values_finite)
    }
    /// Local IDs this definition allocates: its enumeration choices.
    pub fn local_ids(&self) -> Vec<u32> {
        match &self.value_type {
            ValueType::Enum { choices } => choices.iter().map(|c| c.id.0).collect(),
            _ => vec![],
        }
    }
}

fn kind_for<'a>(
    value_type: &ValueType,
    kind: Option<&'a QuantityKind>,
) -> Result<Option<&'a QuantityKind>> {
    match (value_type.kind(), kind) {
        (Some(r), None) => Err(invalid(format!(
            "quantity kind {r} is not in the project library"
        ))),
        (_, kind) => Ok(kind),
    }
}

/// Whether `value` satisfies `value_type`. `kind` is the quantity kind the
/// value type names, if it names one.
pub fn check_value(
    value: &FieldValue,
    value_type: &ValueType,
    kind: Option<&QuantityKind>,
) -> Result<()> {
    let kind = kind_for(value_type, kind)?;
    let mismatch = || invalid(format!("this field expects a {} value", value_type.name()));
    match (value_type, value) {
        (ValueType::Quantity { min, max, .. }, FieldValue::Quantity { si, display_unit }) => {
            if !finite(*si) {
                return Err(invalid(
                    "quantities must be finite and within ±1e100".into(),
                ));
            }
            if min.is_some_and(|m| *si < m) || max.is_some_and(|m| *si > m) {
                return Err(invalid(format!(
                    "{} is outside the allowed range{}{}",
                    display(value, value_type, kind),
                    min.map(|m| format!(", minimum {}", quantity_text(m, kind)))
                        .unwrap_or_default(),
                    max.map(|m| format!(", maximum {}", quantity_text(m, kind)))
                        .unwrap_or_default(),
                )));
            }
            check_display_unit(display_unit.as_deref(), kind)
        }
        (ValueType::Integer { min, max }, FieldValue::Integer(v)) => {
            if (*min..=*max).contains(v) {
                Ok(())
            } else {
                Err(invalid(format!("{v} is outside {min}–{max}")))
            }
        }
        (ValueType::Boolean, FieldValue::Boolean(_)) => Ok(()),
        (ValueType::Enum { choices }, FieldValue::Enum(id)) => {
            if choices.iter().any(|c| c.id == *id) {
                Ok(())
            } else {
                Err(invalid(format!("choice {id} is not offered by this field")))
            }
        }
        (ValueType::Text { max_bytes }, FieldValue::Text(text)) => {
            if text.len() > usize::from(*max_bytes) || text.chars().any(char::is_control) {
                Err(invalid(format!(
                    "text must be at most {max_bytes} bytes without control characters"
                )))
            } else {
                Ok(())
            }
        }
        (ValueType::Tolerance { .. }, FieldValue::Tolerance { minus, plus }) => {
            if finite(*minus) && finite(*plus) && *minus >= 0.0 && *plus >= 0.0 {
                Ok(())
            } else {
                Err(invalid(
                    "tolerance magnitudes must be finite and non-negative".into(),
                ))
            }
        }
        (ValueType::Range { .. }, FieldValue::Range { min, max }) => {
            if finite(*min) && finite(*max) && min <= max {
                Ok(())
            } else {
                Err(invalid(
                    "a range needs finite bounds with minimum ≤ maximum".into(),
                ))
            }
        }
        _ => Err(mismatch()),
    }
}

fn check_display_unit(unit: Option<&str>, kind: Option<&QuantityKind>) -> Result<()> {
    match (unit, kind) {
        (None, _) => Ok(()),
        (Some(unit), Some(kind))
            if unit.len() <= MAX_SYMBOL && kind.units.resolve(unit).is_some() =>
        {
            Ok(())
        }
        (Some(unit), _) => Err(invalid(format!(
            "display unit ‘{unit}’ is not a unit of this quantity kind"
        ))),
    }
}

/// Parse property text against a value type. Quantities follow
/// [`quantity::parse`]; booleans accept true/false/yes/no; enumerations
/// accept a choice label; tolerances accept `±5%`, `-20%/+80%` (relative,
/// also `ppm`) or quantities (absolute); ranges accept `min .. max`.
pub fn parse_value(
    text: &str,
    value_type: &ValueType,
    kind: Option<&QuantityKind>,
) -> Result<FieldValue> {
    if text.len() > MAX_INPUT {
        return Err(Error::Limit("property text exceeds 256 bytes".into()));
    }
    let kind = kind_for(value_type, kind)?;
    let trimmed = text.trim();
    let value = match value_type {
        ValueType::Quantity { .. } => {
            let parsed = parse_quantity(trimmed, kind)?;
            FieldValue::Quantity {
                si: parsed.quantity.value(),
                display_unit: parsed.unit,
            }
        }
        ValueType::Integer { .. } => FieldValue::Integer(
            trimmed
                .parse()
                .map_err(|_| invalid(format!("‘{trimmed}’ is not an integer")))?,
        ),
        ValueType::Boolean => FieldValue::Boolean(match trimmed.to_lowercase().as_str() {
            "true" | "yes" => true,
            "false" | "no" => false,
            _ => return Err(invalid(format!("‘{trimmed}’ is not true or false"))),
        }),
        ValueType::Enum { choices } => FieldValue::Enum(
            choices
                .iter()
                .find(|c| c.label.trim().eq_ignore_ascii_case(trimmed))
                .map(|c| c.id)
                .ok_or_else(|| invalid(format!("‘{trimmed}’ is not a choice of this field")))?,
        ),
        ValueType::Text { .. } => FieldValue::Text(text.into()),
        ValueType::Tolerance { basis } => parse_tolerance(trimmed, basis, kind)?,
        ValueType::Range { .. } => {
            let mut sides = trimmed.split("..");
            let (Some(low), Some(high), None) = (sides.next(), sides.next(), sides.next()) else {
                return Err(invalid(format!("‘{trimmed}’ is not a range ‘min .. max’")));
            };
            FieldValue::Range {
                min: parse_quantity(low.trim(), kind)?.quantity.value(),
                max: parse_quantity(high.trim(), kind)?.quantity.value(),
            }
        }
    };
    check_value(&value, value_type, kind)?;
    Ok(value)
}

fn parse_quantity(text: &str, kind: Option<&QuantityKind>) -> Result<quantity::Parsed> {
    let kind = kind.ok_or_else(|| invalid("this field has no quantity kind".into()))?;
    quantity::parse(text, &kind.units, &kind.default_unit)
}

fn parse_tolerance(
    text: &str,
    basis: &ToleranceBasis,
    kind: Option<&QuantityKind>,
) -> Result<FieldValue> {
    let magnitude = |part: &str| -> Result<f64> {
        let part = part.trim();
        let value = match basis {
            ToleranceBasis::Relative => {
                let (number, divisor) = if let Some(n) = part.strip_suffix('%') {
                    (n, 100.0)
                } else if let Some(n) = part.strip_suffix("ppm") {
                    (n, 1e6)
                } else {
                    return Err(invalid(format!(
                        "‘{part}’ needs a % or ppm suffix for a relative tolerance"
                    )));
                };
                let number = number.trim();
                if !number.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
                    return Err(invalid(format!("‘{part}’ is not a tolerance")));
                }
                number
                    .parse::<f64>()
                    .map_err(|_| invalid(format!("‘{part}’ is not a tolerance")))?
                    / divisor
            }
            ToleranceBasis::Absolute { .. } => {
                if part.starts_with(['+', '-', '\u{2212}']) {
                    return Err(invalid(format!("‘{part}’ is not a tolerance")));
                }
                parse_quantity(part, kind)?.quantity.value()
            }
        };
        if finite(value) && value >= 0.0 {
            Ok(value)
        } else {
            Err(invalid(format!("‘{part}’ is not a non-negative tolerance")))
        }
    };
    let symmetric = text
        .strip_prefix('±')
        .or_else(|| text.strip_prefix("+/-"))
        .or_else(|| (!text.contains('/')).then_some(text));
    if let Some(rest) = symmetric {
        let m = magnitude(rest)?;
        return Ok(FieldValue::Tolerance { minus: m, plus: m });
    }
    let malformed = || {
        invalid(format!(
            "‘{text}’ is not a tolerance such as ±5% or -20%/+80%"
        ))
    };
    let parts: Vec<&str> = text.split('/').map(str::trim).collect();
    if parts.len() != 2 {
        return Err(malformed());
    }
    let (mut minus, mut plus) = (None, None);
    for part in parts {
        if let Some(rest) = part.strip_prefix(['-', '\u{2212}'])
            && minus.is_none()
        {
            minus = Some(magnitude(rest)?);
        } else if let Some(rest) = part.strip_prefix('+')
            && plus.is_none()
        {
            plus = Some(magnitude(rest)?);
        } else {
            return Err(malformed());
        }
    }
    match (minus, plus) {
        (Some(minus), Some(plus)) => Ok(FieldValue::Tolerance { minus, plus }),
        _ => Err(malformed()),
    }
}

/// Apply one input to an instance's overrides. `Text` is parsed and `Value`
/// checked against the field; `Inherit` removes the override.
pub fn apply_input(
    overrides: &mut InstanceFields,
    field: DefinitionId,
    input: &PropertyInput,
    definition: &FieldDefinition,
    kind: Option<&QuantityKind>,
) -> Result<()> {
    match input {
        PropertyInput::Text(text) => {
            let value = parse_value(text, &definition.value_type, kind)?;
            overrides.insert(field, FieldOverride::Assigned(value));
        }
        PropertyInput::Value(value) => {
            check_value(value, &definition.value_type, kind)?;
            overrides.insert(field, FieldOverride::Assigned(value.clone()));
        }
        PropertyInput::Inherit => {
            overrides.remove(&field);
        }
        PropertyInput::Unset => {
            overrides.insert(field, FieldOverride::Unset);
        }
    }
    Ok(())
}

/// Whether values of `old` keep their meaning under `new`: the same value
/// type measured in the same quantity-kind lineage. Bounds and choices are
/// checked separately with [`check_value`].
pub fn compatible(old: &ValueType, new: &ValueType) -> bool {
    use std::mem::discriminant;
    let same_basis = match (old, new) {
        (ValueType::Tolerance { basis: a }, ValueType::Tolerance { basis: b }) => {
            discriminant(a) == discriminant(b)
        }
        _ => true,
    };
    discriminant(old) == discriminant(new)
        && same_basis
        && old.kind().map(|k| k.id) == new.kind().map(|k| k.id)
}

/// The effective value of a selected field on one instance: an assignment,
/// then the component default, then the field default.
pub fn effective(
    label: &str,
    definition: &FieldDefinition,
    kind: Option<&QuantityKind>,
    selection: &FieldSelection,
    assigned: Option<&FieldOverride>,
) -> EffectiveField {
    let (value, origin) = match (assigned, &selection.default, &definition.default) {
        (Some(FieldOverride::Assigned(v)), _, _) => (Some(v.clone()), Origin::Assigned),
        (Some(FieldOverride::Unset), _, _) => (None, Origin::ExplicitlyUnset),
        (None, Some(v), _) => (Some(v.clone()), Origin::ComponentDefault),
        (None, None, Some(v)) => (Some(v.clone()), Origin::FieldDefault),
        (None, None, None) => (None, Origin::NoValue),
    };
    EffectiveField {
        field: selection.field,
        label: label.into(),
        text: value
            .as_ref()
            .map(|v| display(v, &definition.value_type, kind)),
        value,
        origin,
        show_on_symbol: selection.show_on_symbol,
        required_at: definition.required_at,
    }
}

/// The stage an effective field blocks: a required field with no value.
pub fn unmet(field: &EffectiveField) -> Option<Stage> {
    field.required_at.filter(|_| field.value.is_none())
}

/// The interval a tolerance gives around a nominal value: relative
/// magnitudes scale the nominal value, absolute ones are SI amounts.
pub fn tolerance_interval(
    nominal: f64,
    minus: f64,
    plus: f64,
    basis: &ToleranceBasis,
) -> Result<(f64, f64)> {
    let (low, high) = match basis {
        ToleranceBasis::Relative => (
            nominal - nominal.abs() * minus,
            nominal + nominal.abs() * plus,
        ),
        ToleranceBasis::Absolute { .. } => (nominal - minus, nominal + plus),
    };
    if finite(low) && finite(high) {
        Ok((low, high))
    } else {
        Err(invalid("tolerance interval is not finite".into()))
    }
}

fn quantity_text(si: f64, kind: Option<&QuantityKind>) -> String {
    match kind {
        Some(kind) => quantity::format(si, &kind.units, &kind.default_unit, None),
        None => quantity::format_number(si),
    }
}

/// A value as text, in a form [`parse_value`] reads back.
pub fn display(value: &FieldValue, value_type: &ValueType, kind: Option<&QuantityKind>) -> String {
    let percent = |m: f64| match value_type {
        ValueType::Tolerance {
            basis: ToleranceBasis::Absolute { .. },
        } => quantity_text(m, kind),
        _ => format!("{}%", quantity::format_number(m * 100.0)),
    };
    match value {
        FieldValue::Quantity { si, display_unit } => match kind {
            Some(kind) => quantity::format(
                *si,
                &kind.units,
                &kind.default_unit,
                display_unit.as_deref(),
            ),
            None => quantity::format_number(*si),
        },
        FieldValue::Integer(v) => v.to_string(),
        FieldValue::Boolean(v) => v.to_string(),
        FieldValue::Enum(id) => match value_type {
            ValueType::Enum { choices } => choices
                .iter()
                .find(|c| c.id == *id)
                .map_or_else(|| format!("choice {id}"), |c| c.label.clone()),
            _ => format!("choice {id}"),
        },
        FieldValue::Text(text) => text.clone(),
        FieldValue::Tolerance { minus, plus } if minus == plus => format!("±{}", percent(*minus)),
        FieldValue::Tolerance { minus, plus } => {
            format!("-{}/+{}", percent(*minus), percent(*plus))
        }
        FieldValue::Range { min, max } => {
            format!(
                "{} .. {}",
                quantity_text(*min, kind),
                quantity_text(*max, kind)
            )
        }
    }
}

fn issue(code: &str, message: String, objects: Vec<ObjectRef>) -> Finding {
    Finding::blocking(code, message, objects, Stage::Publish)
}

/// Publication findings of a quantity kind.
pub fn kind_findings(id: DefinitionId, kind: &QuantityKind) -> Vec<Finding> {
    let this = vec![ObjectRef::Definition { id }];
    let mut out = Vec::new();
    if let Err(error) = kind.units.check() {
        out.push(issue(
            "quantity_kind.units",
            error.to_string(),
            this.clone(),
        ));
    }
    if kind
        .units
        .units
        .iter()
        .all(|u| u.symbol != kind.default_unit)
    {
        out.push(issue(
            "quantity_kind.default_unit",
            format!(
                "Default unit ‘{}’ must be the symbol of one of the kind's units.",
                kind.default_unit
            ),
            this,
        ));
    }
    out
}

/// Publication findings of a field. `kind` is the quantity kind its value
/// type names; the caller reports a missing one.
pub fn definition_findings(
    id: DefinitionId,
    field: &FieldDefinition,
    kind: Option<&QuantityKind>,
) -> Vec<Finding> {
    let this = vec![ObjectRef::Definition { id }];
    let mut out = Vec::new();
    if field.required_at == Some(Stage::Publish) {
        out.push(issue(
            "field.required_stage",
            "A field can be required for board placement or electrical resolution, not publication."
                .into(),
            this.clone(),
        ));
    }
    match &field.value_type {
        ValueType::Quantity { min, max, .. } => {
            let bounded = |v: &Option<f64>| v.is_none_or(finite);
            if !bounded(min) || !bounded(max) || matches!((min, max), (Some(a), Some(b)) if a > b) {
                out.push(issue(
                    "field.value_type",
                    "Quantity bounds must be finite with minimum ≤ maximum.".into(),
                    this.clone(),
                ));
            }
        }
        ValueType::Integer { min, max } if min > max => out.push(issue(
            "field.value_type",
            "Integer minimum must not exceed the maximum.".into(),
            this.clone(),
        )),
        ValueType::Text { max_bytes } if !(1..=MAX_TEXT_BYTES).contains(max_bytes) => {
            out.push(issue(
                "field.value_type",
                "Text fields allow 1–256 bytes.".into(),
                this.clone(),
            ))
        }
        ValueType::Enum { choices } => {
            if !(1..=MAX_CHOICES).contains(&choices.len()) {
                out.push(issue(
                    "field.value_type",
                    "An enumeration needs 1–128 choices.".into(),
                    this.clone(),
                ));
            }
            let mut labels = BTreeSet::new();
            for choice in choices {
                if !printable(&choice.label) || !labels.insert(choice.label.trim().to_lowercase()) {
                    out.push(issue(
                        "field.choice_label",
                        "A choice needs a unique printable label.".into(),
                        vec![ObjectRef::LibraryObject {
                            definition: id,
                            local: choice.id.0,
                        }],
                    ));
                }
            }
        }
        _ => {}
    }
    let kind_missing = field.value_type.kind().is_some() && kind.is_none();
    if let Some(default) = &field.default
        && !kind_missing
        && let Err(error) = check_value(default, &field.value_type, kind)
    {
        out.push(issue(
            "field.default_invalid",
            format!("The field default is invalid: {error}."),
            this,
        ));
    }
    out
}
