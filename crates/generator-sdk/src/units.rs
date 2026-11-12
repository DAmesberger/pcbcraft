//! Integer lengths and angles as the ABI carries them.

pub use pcbcraft_generator_abi::params::mm_text;

/// Nanometres per millimetre.
pub const MM: i64 = 1_000_000;
/// Nanometres per micrometre.
pub const UM: i64 = 1_000;
/// Millidegrees per degree.
pub const DEG: i64 = 1_000;

/// `n / d` rounded to the nearest integer, halves away from zero; `None` on
/// a zero divisor or overflow.
pub fn div_round(n: i64, d: i64) -> Option<i64> {
    if d == 0 {
        return None;
    }
    let q = n.checked_div(d)?;
    let r = n.checked_rem(d)?;
    let twice = r.checked_abs()?.checked_mul(2)?;
    if twice >= d.checked_abs()? {
        let away = if (n < 0) == (d < 0) { 1 } else { -1 };
        q.checked_add(away)
    } else {
        Some(q)
    }
}

/// Round `v` down (`up` false) or up to a multiple of `step` (> 0).
pub fn round_to(v: i64, step: i64, up: bool) -> Option<i64> {
    if step <= 0 {
        return None;
    }
    let floor = v.checked_div_euclid(step)?.checked_mul(step)?;
    if up && floor != v {
        floor.checked_add(step)
    } else {
        Some(floor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_halves_away_from_zero() {
        assert_eq!(div_round(5, 2), Some(3));
        assert_eq!(div_round(-5, 2), Some(-3));
        assert_eq!(div_round(4, 3), Some(1));
        assert_eq!(div_round(1, 0), None);
        assert_eq!(round_to(-123, 50, false), Some(-150));
        assert_eq!(round_to(-123, 50, true), Some(-100));
        assert_eq!(round_to(100, 50, true), Some(100));
    }
}
