//! Rated specifications and the one comparison between an instance
//! requirement and a manufacturer part's rated value (S2).
//!
//! `satisfies` is shared by catalog search, `instance.assign_part` and
//! purchasing completeness. It compares by the spec's declared meaning,
//! never by matching names or text.
use super::{FieldValue, ToleranceBasis, ValueType, tolerance_interval};
use crate::diagnostics::{Finding, ObjectRef};
use crate::ids::FieldRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Rated specs one Part revision carries.
pub const MAX_RATED_SPECS: usize = 64;
/// Relative tolerance of quantity equality, as in catalog search.
pub const QUANTITY_EPSILON: f64 = 1e-9;

/// How a rated value meets a requirement of the same field.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum SpecMeaning {
    /// The part's value is the required value (resistance 10 kΩ, an
    /// enumeration choice, an integer, text). Quantities compare within a
    /// relative [`QUANTITY_EPSILON`].
    ExactNominal,
    /// The part is rated at least the required value (a 25 V part meets a
    /// 16 V requirement). Quantities and integers only.
    MinimumRating,
    /// The part's tolerance is no wider than the required one, each side
    /// compared (±0.5 % meets ±1 %). Tolerances only.
    MaximumTolerance,
}

/// One rated value of a Part: a typed field value with its meaning.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RatedSpec {
    /// The field revision this value is typed by. It is matched to a
    /// requirement by field lineage.
    pub field: FieldRef,
    pub value: FieldValue,
    pub meaning: SpecMeaning,
}

/// An instance's effective value of one field, with what comparing it needs.
/// `instances` and `completeness` build it from the effective field; here it
/// is plain data.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub field: FieldRef,
    /// The field's name, for messages.
    pub label: String,
    pub value: FieldValue,
    /// The requirement's value type; the spec is read in it.
    pub value_type: ValueType,
    /// The nominal SI value a tolerance applies to, when the instance has
    /// one. Only comparing tolerances of different bases needs it, through
    /// [`super::tolerance_interval`].
    pub nominal: Option<f64>,
}

fn close(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= QUANTITY_EPSILON * a.abs().max(b.abs())
}

/// `a ≥ b`, or equal within [`QUANTITY_EPSILON`].
fn at_least(a: f64, b: f64) -> bool {
    a >= b || close(a, b)
}

impl SpecMeaning {
    /// Whether this meaning can compare values of `value_type`: exact
    /// nominal for every type but tolerances, a minimum rating for
    /// quantities, integers and ranges (the rated range covers the required
    /// one), a maximum tolerance for tolerances only.
    pub fn fits(self, value_type: &ValueType) -> bool {
        match self {
            Self::ExactNominal => !matches!(value_type, ValueType::Tolerance { .. }),
            Self::MinimumRating => matches!(
                value_type,
                ValueType::Quantity { .. } | ValueType::Integer { .. } | ValueType::Range { .. }
            ),
            Self::MaximumTolerance => matches!(value_type, ValueType::Tolerance { .. }),
        }
    }

    /// How the meaning reads in messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::ExactNominal => "exact nominal",
            Self::MinimumRating => "minimum rating",
            Self::MaximumTolerance => "maximum tolerance",
        }
    }
}

/// Whether two value types of one field lineage read values the same way:
/// the same value type in the same quantity-kind lineage. Tolerance bases
/// are compared separately, through the nominal value.
fn same_reading(a: &ValueType, b: &ValueType) -> bool {
    use std::mem::discriminant;
    match (a, b) {
        (ValueType::Tolerance { .. }, ValueType::Tolerance { .. }) => true,
        _ => discriminant(a) == discriminant(b) && a.kind().map(|k| k.id) == b.kind().map(|k| k.id),
    }
}

/// Whether the rated tolerance is no wider than the required one. Equal
/// bases compare magnitudes side by side; different bases compare the
/// intervals both give around `nominal`, which absolute bases of different
/// quantity-kind lineages never share.
fn tolerance_met(
    required: (f64, f64, &ToleranceBasis),
    rated: (f64, f64, &ToleranceBasis),
    nominal: Option<f64>,
) -> Option<bool> {
    let (rm, rp, rb) = required;
    let (sm, sp, sb) = rated;
    match (rb, sb) {
        (ToleranceBasis::Relative, ToleranceBasis::Relative) => {
            return Some(at_least(rm, sm) && at_least(rp, sp));
        }
        (ToleranceBasis::Absolute { kind: a }, ToleranceBasis::Absolute { kind: b }) => {
            return (a.id == b.id).then(|| at_least(rm, sm) && at_least(rp, sp));
        }
        _ => {}
    }
    let nominal = nominal?;
    let (r_low, r_high) = tolerance_interval(nominal, rm, rp, rb).ok()?;
    let (s_low, s_high) = tolerance_interval(nominal, sm, sp, sb).ok()?;
    Some(at_least(s_low, r_low) && at_least(r_high, s_high))
}

impl Requirement {
    /// [`satisfies`], with the rated value read in `spec_type`: the value
    /// type of the spec's own field revision, which may be another revision
    /// of the requirement's field lineage (a tolerance basis may change
    /// between revisions; the comparison then goes through `nominal`).
    pub fn satisfied_by(&self, spec: &RatedSpec, spec_type: &ValueType) -> Vec<Finding> {
        let objects = vec![ObjectRef::Definition { id: self.field.id }];
        let label = &self.label;
        if spec.field.id != self.field.id {
            return vec![Finding::new(
                "part.spec_field_mismatch",
                format!("The rated value is not a value of {label}."),
                objects,
            )];
        }
        let met = if same_reading(&self.value_type, spec_type) && spec.meaning.fits(spec_type) {
            compare(self, spec, spec_type)
        } else {
            None
        };
        let meaning = spec.meaning.label();
        match met {
            Some(true) => Vec::new(),
            Some(false) => vec![Finding::new(
                "part.spec_mismatch",
                format!("The rated {label} ({meaning}) does not meet the requirement."),
                objects,
            )],
            None => vec![Finding::new(
                "part.spec_uncomparable",
                format!("The rated {label} ({meaning}) cannot be compared with the requirement."),
                objects,
            )],
        }
    }
}

fn compare(requirement: &Requirement, spec: &RatedSpec, spec_type: &ValueType) -> Option<bool> {
    use FieldValue as V;
    use SpecMeaning as M;
    match (spec.meaning, &requirement.value, &spec.value) {
        (M::ExactNominal, V::Quantity { si: r, .. }, V::Quantity { si: s, .. }) => {
            Some(close(r.to_f64(), s.to_f64()))
        }
        (M::ExactNominal, V::Integer(r), V::Integer(s)) => Some(r == s),
        (M::ExactNominal, V::Boolean(r), V::Boolean(s)) => Some(r == s),
        (M::ExactNominal, V::Enum(r), V::Enum(s)) => Some(r == s),
        (M::ExactNominal, V::Text(r), V::Text(s)) => Some(r == s),
        (
            M::ExactNominal,
            V::Range {
                min: r_min,
                max: r_max,
            },
            V::Range {
                min: s_min,
                max: s_max,
            },
        ) => Some(close(r_min.to_f64(), s_min.to_f64()) && close(r_max.to_f64(), s_max.to_f64())),
        (M::MinimumRating, V::Quantity { si: r, .. }, V::Quantity { si: s, .. }) => {
            Some(at_least(s.to_f64(), r.to_f64()))
        }
        (M::MinimumRating, V::Integer(r), V::Integer(s)) => Some(s >= r),
        // A rated range covers the required one: an operating range of
        // −40 … 125 °C meets a requirement of −20 … 85 °C.
        (
            M::MinimumRating,
            V::Range {
                min: r_min,
                max: r_max,
            },
            V::Range {
                min: s_min,
                max: s_max,
            },
        ) => Some(
            at_least(r_min.to_f64(), s_min.to_f64()) && at_least(s_max.to_f64(), r_max.to_f64()),
        ),
        (
            M::MaximumTolerance,
            V::Tolerance {
                minus: rm,
                plus: rp,
            },
            V::Tolerance {
                minus: sm,
                plus: sp,
            },
        ) => {
            let (ValueType::Tolerance { basis: rb }, ValueType::Tolerance { basis: sb }) =
                (&requirement.value_type, spec_type)
            else {
                return None;
            };
            tolerance_met(
                (rm.to_f64(), rp.to_f64(), rb),
                (sm.to_f64(), sp.to_f64(), sb),
                requirement.nominal,
            )
        }
        _ => None,
    }
}

/// Why `spec` does not meet `requirement`; empty when it does. Findings are
/// advisory here: purchasing completeness and `instance.assign_part` decide
/// what they block. Codes: `part.spec_mismatch` (the comparison fails),
/// `part.spec_field_mismatch` (different field lineages) and
/// `part.spec_uncomparable` (the meaning does not apply to the value, or the
/// comparison needs data it does not have). The spec is read in the
/// requirement's value type; [`Requirement::satisfied_by`] takes the spec's
/// own.
pub fn satisfies(requirement: &Requirement, spec: &RatedSpec) -> Vec<Finding> {
    requirement.satisfied_by(spec, &requirement.value_type)
}

#[cfg(test)]
mod tests;
