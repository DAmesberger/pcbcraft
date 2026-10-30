#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;

const RESISTANCE: Dimension = [2, 1, -3, -2, 0, 0, 0];

fn unit(symbol: &str, aliases: &[&str], scale: f64, si_prefixes: bool) -> UnitOfMeasure {
    UnitOfMeasure {
        symbol: symbol.into(),
        aliases: aliases.iter().map(|a| (*a).into()).collect(),
        scale,
        si_prefixes,
    }
}

fn resistance() -> UnitSet {
    UnitSet {
        dimension: RESISTANCE,
        units: vec![unit("Ω", &["ohm"], 1.0, true)],
    }
}

fn length() -> UnitSet {
    UnitSet {
        dimension: LENGTH,
        units: vec![unit("m", &[], 1.0, true), unit("in", &[], 0.0254, false)],
    }
}

fn si(text: &str, units: &UnitSet, default: &str) -> f64 {
    parse(text, units, default).unwrap().quantity.value()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * b.abs().max(1e-30)
}

#[test]
fn suffix_precedence_follows_the_four_rules() {
    let r = resistance();
    // Hand-derived: 10 × 10³ Ω.
    for text in ["10k", "10 kΩ", "10000 Ω", "10kohm", "1e4", " 10 k "] {
        assert!(close(si(text, &r, "Ω"), 10000.0), "{text}");
    }
    // A bare prefix applies to the default unit: 10 × 10⁻³ Ω.
    assert!(close(si("10m", &r, "Ω"), 0.01));
    // For a length kind `m` is the unit itself (rule 1 wins over rule 3).
    assert!(close(si("10m", &length(), "m"), 10.0));
    assert!(close(si("10mm", &length(), "m"), 0.01));
    assert!(close(si("2in", &length(), "m"), 0.0508));
    assert!(close(si("4.7 uΩ", &r, "Ω"), 4.7e-6));
    assert_eq!(parse("4.7u", &r, "Ω").unwrap().unit.as_deref(), Some("µΩ"));
    assert_eq!(parse("10", &r, "Ω").unwrap().unit, None);
    assert!(close(si("−5", &r, "Ω"), -5.0));
}

#[test]
fn other_suffixes_and_out_of_range_results_are_rejected() {
    let r = resistance();
    for text in [
        "10 µF", "10 kin", "4k7", "k", "", "nan", "inf", "10 Ω Ω", "1e",
    ] {
        let error = parse(text, &r, "Ω").unwrap_err();
        assert_eq!(error.code(), "invalid_input", "{text}");
    }
    assert!(
        parse("10 µF", &r, "Ω")
            .unwrap_err()
            .to_string()
            .contains("µF")
    );
    // A prefix on a unit that does not accept one.
    assert!(parse("1 kin", &length(), "m").is_err());
    assert!(parse("1e99 G", &r, "Ω").is_err());
    assert_eq!(
        parse(&"1".repeat(200), &r, "Ω").unwrap_err().code(),
        "limit_exceeded"
    );
}

#[test]
fn formatting_uses_the_recorded_unit_or_an_engineering_prefix() {
    let r = resistance();
    assert_eq!(format(10000.0, &r, "Ω", Some("kΩ")), "10 kΩ");
    assert_eq!(format(10000.0, &r, "Ω", None), "10 kΩ");
    assert_eq!(format(0.0047, &r, "Ω", None), "4.7 mΩ");
    assert_eq!(format(0.0, &r, "Ω", None), "0 Ω");
    assert_eq!(format(0.0254, &length(), "in", None), "1 in");
    let text = format(4700.0, &r, "Ω", Some("Ω"));
    assert_eq!(text, "4700 Ω");
    assert!(close(si(&text, &r, "Ω"), 4700.0));
}

#[test]
fn unit_sets_are_checked_once() {
    assert!(resistance().check().is_ok());
    let mut duplicate = resistance();
    duplicate.units.push(unit("ohm", &[], 1.0, false));
    assert!(duplicate.check().is_err());
    for bad in ["", "1x", "e3", "a b", "m/s", "-V", "a..b"] {
        let set = UnitSet {
            dimension: LENGTH,
            units: vec![unit(bad, &[], 1.0, false)],
        };
        assert!(set.check().is_err(), "{bad:?}");
    }
    let mut zero = length();
    zero.units[0].scale = 0.0;
    assert!(zero.check().is_err());
    assert!(UnitSet::default().check().is_err());
    // The scaling rule is the parsing rule.
    assert!(close(
        resistance().to_si(2.2, "kΩ").unwrap().value(),
        2200.0
    ));
    assert!(resistance().to_si(1.0, "F").is_err());
}

#[test]
fn arithmetic_is_checked() {
    let a = Quantity::new(2.0, LENGTH).unwrap();
    assert_eq!(a.checked_mul(a).unwrap().dimension(), [2, 0, 0, 0, 0, 0, 0]);
    assert_eq!(a.checked_div(a).unwrap().dimension(), DIMENSIONLESS);
    assert!(
        a.checked_add(Quantity::new(1.0, DIMENSIONLESS).unwrap())
            .is_err()
    );
    assert!(a.checked_div(Quantity::new(0.0, LENGTH).unwrap()).is_err());
    assert!(Quantity::new(f64::NAN, LENGTH).is_err());
}
