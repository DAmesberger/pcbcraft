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

fn exact(text: &str, units: &UnitSet, default: &str) -> String {
    let parsed = parse(text, units, default).unwrap();
    format_decimal(parsed.si, units, default, parsed.unit.as_deref())
}

fn farads() -> UnitSet {
    UnitSet {
        dimension: [-2, -1, 4, 2, 0, 0, 0],
        units: vec![unit("F", &[], 1.0, true)],
    }
}

#[test]
fn typed_values_are_stored_and_shown_exactly() {
    let f = farads();
    // 10 × 10⁻⁶ in binary floating point is 9.999999999999999e-6.
    let ten = parse("10 µF", &f, "F").unwrap();
    assert_eq!(ten.si.to_string(), "0.00001");
    assert_eq!(ten.si, "1e-5".parse().unwrap());
    for (typed, shown) in [
        ("10 µF", "10 µF"),
        ("100nF", "100 nF"),
        ("0.1 µF", "0.1 µF"),
        ("0.3 µF", "0.3 µF"),
        ("2.2 nF", "2.2 nF"),
        ("4.7u", "4.7 µF"),
        ("0.00001", "10 µF"),
        ("1e-5", "10 µF"),
    ] {
        assert_eq!(exact(typed, &f, "F"), shown, "{typed}");
    }
    assert_eq!(exact("4.7k", &resistance(), "Ω"), "4.7 kΩ");
    assert_eq!(exact("10.10 kΩ", &resistance(), "Ω"), "10.1 kΩ");
    // Without a recorded unit: an exact engineering prefix.
    let si = parse("0.0000047", &f, "F").unwrap().si;
    assert_eq!(format_decimal(si, &f, "F", None), "4.7 µF");
    // A unit scale that is not a power of ten rounds to 12 digits.
    let inches = parse("2in", &length(), "m").unwrap();
    assert_eq!(inches.si.to_string(), "0.0508");
    assert_eq!(
        format_decimal(inches.si, &length(), "m", Some("in")),
        "2 in"
    );
}

#[test]
fn decimals_read_and_write_text_exactly_within_bounds() {
    let d = |text: &str| text.parse::<Decimal>().unwrap();
    for (text, shown) in [
        ("0.3", "0.3"),
        ("0.30", "0.3"),
        ("-0", "0"),
        ("+12.50", "12.5"),
        (".5", "0.5"),
        ("1000", "1000"),
        ("1e-7", "1e-7"),
        ("1.5E20", "1.5e20"),
        ("123456789012345678", "1.23456789012345678e17"),
        ("0.000001", "0.000001"),
    ] {
        assert_eq!(d(text).to_string(), shown, "{text}");
        assert_eq!(d(shown), d(text));
    }
    // 0.1 + 0.2 is 0.30000000000000004 in f64; an f64 boundary rounds to
    // 12 significant digits, and typed text never goes through f64.
    assert_eq!(Decimal::from_f64(0.1 + 0.2).unwrap(), d("0.3"));
    assert_eq!(
        Decimal::from_f64(10.0 * 1e-6).unwrap().to_string(),
        "0.00001"
    );
    assert_eq!(d("0.1").checked_mul(d("3")).unwrap(), d("0.3"));
    for (bad, code) in [
        ("1234567890123456789", "limit_exceeded"),
        ("1e401", "limit_exceeded"),
        ("1e-401", "limit_exceeded"),
        ("1e12345", "invalid_input"),
        ("", "invalid_input"),
        (".", "invalid_input"),
        ("1.2.3", "invalid_input"),
        ("1e", "invalid_input"),
        ("nan", "invalid_input"),
        ("inf", "invalid_input"),
        ("0x10", "invalid_input"),
    ] {
        assert_eq!(bad.parse::<Decimal>().unwrap_err().code(), code, "{bad}");
    }
    assert_eq!(
        "1".repeat(129).parse::<Decimal>().unwrap_err().code(),
        "limit_exceeded"
    );
    // Trailing zeros are not significant digits.
    assert_eq!(d(&format!("1{}", "0".repeat(30))).to_string(), "1e30");
    assert!(Decimal::from_f64(f64::NAN).is_err());
    // Too many digits in a product is refused, not rounded.
    let big = d("123456789012345678");
    assert!(big.checked_mul(big).is_err());
    let json = serde_json::to_string(&d("0.00001")).unwrap();
    assert_eq!(json, "\"0.00001\"");
    assert!(serde_json::from_str::<Decimal>("0.00001").is_err());
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
