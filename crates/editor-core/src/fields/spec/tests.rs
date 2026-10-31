//! `satisfies` against hand-derived comparisons. Values are invented.
use super::*;
use crate::ids::{DefinitionId, QuantityKindRef, Revision};

const FIELD: DefinitionId = DefinitionId(7);
const VOLTS: QuantityKindRef = QuantityKindRef {
    id: DefinitionId(3),
    revision: Revision::FIRST,
};
const OHMS: QuantityKindRef = QuantityKindRef {
    id: DefinitionId(4),
    revision: Revision::FIRST,
};

fn quantity(si: f64) -> FieldValue {
    FieldValue::Quantity {
        si,
        display_unit: None,
    }
}

fn tolerance(minus: f64, plus: f64) -> FieldValue {
    FieldValue::Tolerance { minus, plus }
}

fn requirement(value: FieldValue, value_type: ValueType) -> Requirement {
    Requirement {
        field: FieldRef::new(FIELD, Revision::FIRST),
        label: "Rated voltage".into(),
        value,
        value_type,
        nominal: None,
    }
}

fn spec(value: FieldValue, meaning: SpecMeaning) -> RatedSpec {
    RatedSpec {
        field: FieldRef::new(FIELD, Revision::FIRST),
        value,
        meaning,
    }
}

fn codes(findings: Vec<Finding>) -> Vec<String> {
    findings.into_iter().map(|f| f.code).collect()
}

fn volts() -> ValueType {
    ValueType::Quantity {
        kind: VOLTS,
        min: None,
        max: None,
    }
}

const RELATIVE: ValueType = ValueType::Tolerance {
    basis: ToleranceBasis::Relative,
};

fn absolute_ohms() -> ValueType {
    ValueType::Tolerance {
        basis: ToleranceBasis::Absolute { kind: OHMS },
    }
}

#[test]
fn ce02_a_higher_rating_meets_a_lower_requirement_and_not_the_reverse() {
    let need_16 = requirement(quantity(16.0), volts());
    let need_25 = requirement(quantity(25.0), volts());
    let rated_25 = spec(quantity(25.0), SpecMeaning::MinimumRating);
    let rated_16 = spec(quantity(16.0), SpecMeaning::MinimumRating);
    assert!(satisfies(&need_16, &rated_25).is_empty());
    assert!(satisfies(&need_16, &rated_16).is_empty(), "equal meets");
    assert_eq!(
        codes(satisfies(&need_25, &rated_16)),
        ["part.spec_mismatch"]
    );
    // An exact nominal never accepts a higher value.
    let exact_25 = spec(quantity(25.0), SpecMeaning::ExactNominal);
    assert_eq!(
        codes(satisfies(&need_16, &exact_25)),
        ["part.spec_mismatch"]
    );
}

#[test]
fn exact_nominals_compare_within_the_relative_epsilon_and_by_identity() {
    let ohms = ValueType::Quantity {
        kind: OHMS,
        min: None,
        max: None,
    };
    // 4.7 kΩ written as 4.7e3 and as 47 × 100 are one value.
    let need = requirement(quantity(4.7e3), ohms.clone());
    assert!(
        satisfies(
            &need,
            &spec(quantity(47.0 * 100.0), SpecMeaning::ExactNominal)
        )
        .is_empty()
    );
    let off = spec(quantity(4.7e3 * (1.0 + 1e-6)), SpecMeaning::ExactNominal);
    assert_eq!(codes(satisfies(&need, &off)), ["part.spec_mismatch"]);
    let enumeration = ValueType::Enum { choices: vec![] };
    let need = requirement(FieldValue::Enum(crate::ids::ChoiceId(2)), enumeration);
    let same = spec(
        FieldValue::Enum(crate::ids::ChoiceId(2)),
        SpecMeaning::ExactNominal,
    );
    let other = spec(
        FieldValue::Enum(crate::ids::ChoiceId(1)),
        SpecMeaning::ExactNominal,
    );
    assert!(satisfies(&need, &same).is_empty());
    assert_eq!(codes(satisfies(&need, &other)), ["part.spec_mismatch"]);
}

#[test]
fn tolerances_compare_each_side_and_across_bases_through_the_nominal() {
    let need = Requirement {
        nominal: Some(10e3),
        ..requirement(tolerance(0.01, 0.01), RELATIVE)
    };
    let tighter = spec(tolerance(0.005, 0.005), SpecMeaning::MaximumTolerance);
    let lopsided = spec(tolerance(0.005, 0.02), SpecMeaning::MaximumTolerance);
    assert!(satisfies(&need, &tighter).is_empty());
    assert_eq!(codes(satisfies(&need, &lopsided)), ["part.spec_mismatch"]);
    // ±1 % of 10 kΩ is 9 900 … 10 100 Ω. ±50 Ω gives 9 950 … 10 050 Ω,
    // inside it; ±150 Ω gives 9 850 … 10 150 Ω, outside; −100/+100 Ω is
    // exactly the boundary and meets.
    let absolute = absolute_ohms();
    let inside = spec(tolerance(50.0, 50.0), SpecMeaning::MaximumTolerance);
    let outside = spec(tolerance(150.0, 150.0), SpecMeaning::MaximumTolerance);
    let boundary = spec(tolerance(100.0, 100.0), SpecMeaning::MaximumTolerance);
    assert!(need.satisfied_by(&inside, &absolute).is_empty());
    assert!(need.satisfied_by(&boundary, &absolute).is_empty());
    assert_eq!(
        codes(need.satisfied_by(&outside, &absolute)),
        ["part.spec_mismatch"]
    );
    // Without a nominal, bases cannot be compared.
    let bare = Requirement {
        nominal: None,
        ..need.clone()
    };
    assert_eq!(
        codes(bare.satisfied_by(&inside, &absolute)),
        ["part.spec_uncomparable"]
    );
    // Absolute against relative: ±80 Ω of 10 kΩ is wider than ±0.5 %
    // (±50 Ω), so ±0.5 % meets it and ±1 % (±100 Ω) does not.
    let need_absolute = Requirement {
        nominal: Some(10e3),
        ..requirement(tolerance(80.0, 80.0), absolute_ohms())
    };
    assert!(need_absolute.satisfied_by(&tighter, &RELATIVE).is_empty());
    let one_percent = spec(tolerance(0.01, 0.01), SpecMeaning::MaximumTolerance);
    assert_eq!(
        codes(need_absolute.satisfied_by(&one_percent, &RELATIVE)),
        ["part.spec_mismatch"]
    );
    // Absolute amounts of different quantity-kind lineages never compare.
    let volts_tolerance = ValueType::Tolerance {
        basis: ToleranceBasis::Absolute { kind: VOLTS },
    };
    assert_eq!(
        codes(need_absolute.satisfied_by(&inside, &volts_tolerance)),
        ["part.spec_uncomparable"]
    );
}

#[test]
fn a_rated_range_meets_a_requirement_it_covers() {
    let range = ValueType::Range { kind: VOLTS };
    let need = requirement(
        FieldValue::Range {
            min: -20.0,
            max: 85.0,
        },
        range,
    );
    let wide = spec(
        FieldValue::Range {
            min: -40.0,
            max: 125.0,
        },
        SpecMeaning::MinimumRating,
    );
    let narrow = spec(
        FieldValue::Range {
            min: -10.0,
            max: 125.0,
        },
        SpecMeaning::MinimumRating,
    );
    assert!(satisfies(&need, &wide).is_empty());
    assert_eq!(codes(satisfies(&need, &narrow)), ["part.spec_mismatch"]);
}

#[test]
fn other_lineages_unfit_meanings_and_other_readings_are_reported() {
    let need = requirement(quantity(16.0), volts());
    let other = RatedSpec {
        field: FieldRef::new(DefinitionId(8), Revision::FIRST),
        ..spec(quantity(25.0), SpecMeaning::MinimumRating)
    };
    assert_eq!(
        codes(satisfies(&need, &other)),
        ["part.spec_field_mismatch"]
    );
    let unfit = spec(quantity(25.0), SpecMeaning::MaximumTolerance);
    assert_eq!(codes(satisfies(&need, &unfit)), ["part.spec_uncomparable"]);
    // A spec read in another quantity-kind lineage is not comparable.
    let ohms = ValueType::Quantity {
        kind: OHMS,
        min: None,
        max: None,
    };
    let rated = spec(quantity(25.0), SpecMeaning::MinimumRating);
    assert_eq!(
        codes(need.satisfied_by(&rated, &ohms)),
        ["part.spec_uncomparable"]
    );
    // Meanings fit the value types they can compare.
    assert!(SpecMeaning::MinimumRating.fits(&volts()));
    assert!(!SpecMeaning::MinimumRating.fits(&ValueType::Boolean));
    assert!(!SpecMeaning::ExactNominal.fits(&RELATIVE));
    assert!(SpecMeaning::MaximumTolerance.fits(&RELATIVE));
}
