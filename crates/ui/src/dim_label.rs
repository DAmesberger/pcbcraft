//! A length dimension's label, from SolidCraft's `crates/ui-egui/src/dim_view.rs` (`label` for a
//! length) and `crates/doc/src/display_units.rs` (`length_number`).

/// The text shown for a length dimension of `mm` millimetres: the value with its unit, "fx: value"
/// when an expression drives it, in parentheses when driven.
pub fn length_label(mm: f64, expr: Option<&str>, driven: bool, units: &str, decimals: u8) -> String {
    // Retain deliberately authored trailing zeros in a four-place inch literal too.
    let authored_four = units == "in"
        && expr.is_some_and(|e| {
            e.trim().strip_suffix("in").map(str::trim).is_some_and(|number| {
                number.parse::<f64>().is_ok()
                    && number.split_once('.').is_some_and(|(_, fraction)| fraction.len() >= 4 && fraction.bytes().all(|b| b.is_ascii_digit()))
            })
        });
    let decimals = if authored_four { decimals.max(4) } else { decimals };
    // A length says its unit, as the value inputs and the pointer's coordinates do (#1155).
    let num = format!("{} {units}", length_number(mm, units, decimals));
    let plain = expr.is_none_or(|e| {
        let e = e.trim();
        let e = ["mm", "cm", "in", "ft", "m", "deg", "rad", "°"].iter().find_map(|unit| e.strip_suffix(unit)).unwrap_or(e);
        crate::kit::parse_number(e).is_some()
    });
    let text = if plain { num } else { format!("fx: {num}") };
    if driven { format!("({text})") } else { text }
}

/// Units per millimetre. Unknown names use millimetres.
fn unit_scale(units: &str) -> f64 {
    match units {
        "cm" => 0.1,
        "m" => 0.001,
        "in" => 1.0 / 25.4,
        "ft" => 1.0 / 304.8,
        _ => 1.0,
    }
}

/// Inch labels retain a meaningful fourth digit without promoting floating-point noise.
/// `value` is already converted to the requested display unit, not a millimetre value.
fn length_precision(value: f64, units: &str, decimals: u8) -> usize {
    let base = usize::from(decimals.min(8));
    if units != "in" {
        return base;
    }
    let needs_four = ((value * 10_000.0).round() - (value * 1000.0).round() * 10.0).abs() >= 0.5;
    base.max(if needs_four { 4 } else { 3 })
}

/// A millimetre length in the requested display unit, without a unit suffix.
fn length_number(mm: f64, units: &str, decimals: u8) -> String {
    let value = mm * unit_scale(units);
    format!("{:.*}", length_precision(value, units, decimals), value)
}
