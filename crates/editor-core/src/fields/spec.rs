//! Rated specifications and the one comparison between an instance
//! requirement and a manufacturer part's rated value (S2).
//!
//! `satisfies` is shared by catalog search, `instance.assign_part` and
//! purchasing completeness. It compares by the spec's declared meaning,
//! never by matching names or text.
use super::{FieldValue, ValueType};
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
    (a - b).abs() <= QUANTITY_EPSILON * a.abs().max(b.abs())
}

/// Why `spec` does not meet `requirement`; empty when it does. Findings are
/// advisory here: purchasing completeness and `instance.assign_part` decide
/// what they block. Codes: `part.spec_mismatch` (the comparison fails),
/// `part.spec_field_mismatch` (different field lineages) and
/// `part.spec_uncomparable` (the meaning does not apply to the value, or the
/// comparison needs data it does not have).
pub fn satisfies(requirement: &Requirement, spec: &RatedSpec) -> Vec<Finding> {
    let objects = vec![ObjectRef::Definition {
        id: requirement.field.id,
    }];
    let label = &requirement.label;
    if spec.field.id != requirement.field.id {
        return vec![Finding::new(
            "part.spec_field_mismatch",
            format!("The rated value is not a value of {label}."),
            objects,
        )];
    }
    use FieldValue as V;
    let met = match (spec.meaning, &requirement.value, &spec.value) {
        (SpecMeaning::ExactNominal, V::Quantity { si: r, .. }, V::Quantity { si: s, .. }) => {
            Some(close(*r, *s))
        }
        (SpecMeaning::ExactNominal, V::Integer(r), V::Integer(s)) => Some(r == s),
        (SpecMeaning::ExactNominal, V::Boolean(r), V::Boolean(s)) => Some(r == s),
        (SpecMeaning::ExactNominal, V::Enum(r), V::Enum(s)) => Some(r == s),
        (SpecMeaning::ExactNominal, V::Text(r), V::Text(s)) => Some(r == s),
        (SpecMeaning::MinimumRating, V::Quantity { si: r, .. }, V::Quantity { si: s, .. }) => {
            Some(*s >= *r || close(*r, *s))
        }
        (SpecMeaning::MinimumRating, V::Integer(r), V::Integer(s)) => Some(s >= r),
        (
            SpecMeaning::MaximumTolerance,
            V::Tolerance {
                minus: rm,
                plus: rp,
            },
            V::Tolerance {
                minus: sm,
                plus: sp,
            },
        ) if matches!(requirement.value_type, ValueType::Tolerance { .. }) => {
            // One field lineage read in one value type: both sides share the
            // requirement's basis.
            Some((*sm <= *rm || close(*sm, *rm)) && (*sp <= *rp || close(*sp, *rp)))
        }
        _ => None,
    };
    match met {
        Some(true) => Vec::new(),
        Some(false) => vec![Finding::new(
            "part.spec_mismatch",
            format!("The part's rated {label} does not meet the requirement."),
            objects,
        )],
        None => vec![Finding::new(
            "part.spec_uncomparable",
            format!("The part's rated {label} cannot be compared with the requirement."),
            objects,
        )],
    }
}
