//! Integer lengths and angles: the units the document stores.
//!
//! Every stored length is a whole number of nanometres ([`Nm`]) within
//! ±[`MAX_COORD_NM`] (±2 m), and every stored rotation is whole millidegrees
//! ([`Angle`]). y grows upward. f64 millimetres are for computing (routing,
//! fills, rendering) and cross into stored geometry only through
//! [`Nm::from_mm_f64_rounded`]; the way out is [`Nm::to_mm_f64`]. Products of
//! lengths go through `i128` here ([`dot`], [`cross`], [`dist2`],
//! [`orient`]), so they cannot overflow.
//!
//! Length text — the one parser and formatter for UI fields, the CLI and
//! MCP — is [`text`]; the wire form of a length in command payloads is
//! [`Length`].
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod text;
mod wire;

pub use text::{LengthUnit, format_length, format_number, parse_length};
pub use wire::{Length, length_mm, option_length_mm, point_mm};

#[cfg(test)]
mod tests;

/// The bound of every stored coordinate and length, either sign: 2 m.
pub const MAX_COORD_NM: i64 = 2_000_000_000;
pub const NM_PER_MM: i64 = 1_000_000;
pub const NM_PER_UM: i64 = 1_000;
/// Exactly 25.4 µm.
pub const NM_PER_MIL: i64 = 25_400;
/// Exactly 25.4 mm.
pub const NM_PER_INCH: i64 = 25_400_000;
/// How far an arc's start and end radii may differ after both ends were
/// rounded to whole nanometres.
pub const ARC_RADIUS_TOLERANCE_NM: i64 = 2;
/// Millidegrees in one turn; an [`Angle`] is below this.
pub const MILLIDEGREES_PER_TURN: i32 = 360_000;

fn out_of_bounds(nm: impl std::fmt::Display) -> Error {
    Error::Limit(format!(
        "length {nm} nm is outside ±{MAX_COORD_NM} nm (±2 m)"
    ))
}

/// A length or coordinate in whole nanometres, within ±[`MAX_COORD_NM`].
/// Stored as a JSON integer; field names end in `_nm`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Nm(i64);

impl Nm {
    pub const ZERO: Self = Self(0);

    /// `nm` when it is within bounds; usable in constants.
    pub const fn try_new(nm: i64) -> Option<Self> {
        if nm >= -MAX_COORD_NM && nm <= MAX_COORD_NM {
            Some(Self(nm))
        } else {
            None
        }
    }

    pub fn new(nm: i64) -> Result<Self> {
        Self::try_new(nm).ok_or_else(|| out_of_bounds(nm))
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub fn checked_add(self, other: Self) -> Result<Self> {
        // Both are within ±2e9: the i64 sum cannot overflow.
        Self::new(self.0 + other.0)
    }

    pub fn checked_sub(self, other: Self) -> Result<Self> {
        Self::new(self.0 - other.0)
    }

    pub const fn abs(self) -> Self {
        Self(self.0.abs())
    }

    /// Millimetres for computation. Exact to the nearest `f64`: the
    /// shortest decimal form of the result is the exact millimetre value.
    pub fn to_mm_f64(self) -> f64 {
        // |self| ≤ 2e9 < 2^53: the conversion is exact, and one correctly
        // rounded division gives the nearest f64 to the decimal value.
        self.0 as f64 / NM_PER_MM as f64
    }

    /// The one crossing from computed millimetres into stored geometry:
    /// finite, within bounds, rounded half away from zero. Exact for every
    /// value with at most six decimals.
    pub fn from_mm_f64_rounded(mm: f64) -> Result<Self> {
        if !mm.is_finite() {
            return Err(Error::Invalid(format!("length {mm} mm is not finite")));
        }
        let nm = (mm * NM_PER_MM as f64).round();
        if nm.abs() > MAX_COORD_NM as f64 {
            return Err(out_of_bounds(nm));
        }
        // Integral and within ±2e9: the conversion is exact.
        Ok(Self(nm as i64))
    }
}

impl<'de> Deserialize<'de> for Nm {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::new(i64::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Nm {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Nm".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "integer",
            "description": "A length in whole nanometres.",
            "minimum": -MAX_COORD_NM,
            "maximum": MAX_COORD_NM
        })
    }
}

/// A stored position, y up.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct PointNm {
    pub x_nm: Nm,
    pub y_nm: Nm,
}

impl PointNm {
    pub const ORIGIN: Self = Self {
        x_nm: Nm::ZERO,
        y_nm: Nm::ZERO,
    };

    pub const fn new(x_nm: Nm, y_nm: Nm) -> Self {
        Self { x_nm, y_nm }
    }

    pub fn to_mm_f64(self) -> [f64; 2] {
        [self.x_nm.to_mm_f64(), self.y_nm.to_mm_f64()]
    }

    /// Both coordinates through [`Nm::from_mm_f64_rounded`].
    pub fn from_mm_f64_rounded([x, y]: [f64; 2]) -> Result<Self> {
        Ok(Self::new(
            Nm::from_mm_f64_rounded(x)?,
            Nm::from_mm_f64_rounded(y)?,
        ))
    }
}

/// `b − a` per axis, widened: at most 4e9, so products fit `i128`.
fn delta(a: PointNm, b: PointNm) -> (i128, i128) {
    (
        i128::from(b.x_nm.0) - i128::from(a.x_nm.0),
        i128::from(b.y_nm.0) - i128::from(a.y_nm.0),
    )
}

/// The dot product of the vectors `o→a` and `o→b`, in nm².
pub fn dot(o: PointNm, a: PointNm, b: PointNm) -> i128 {
    let (ax, ay) = delta(o, a);
    let (bx, by) = delta(o, b);
    ax * bx + ay * by
}

/// The cross product of the vectors `o→a` and `o→b`, in nm².
pub fn cross(o: PointNm, a: PointNm, b: PointNm) -> i128 {
    let (ax, ay) = delta(o, a);
    let (bx, by) = delta(o, b);
    ax * by - ay * bx
}

/// The squared distance between two points, in nm².
pub fn dist2(a: PointNm, b: PointNm) -> i128 {
    let (dx, dy) = delta(a, b);
    dx * dx + dy * dy
}

/// The turn `a → b → c` takes, exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    CounterClockwise,
    Clockwise,
    Collinear,
}

pub fn orient(a: PointNm, b: PointNm, c: PointNm) -> Orientation {
    match cross(a, b, c).signum() {
        1 => Orientation::CounterClockwise,
        -1 => Orientation::Clockwise,
        _ => Orientation::Collinear,
    }
}

/// A stored rotation in whole millidegrees, counter-clockwise, 0–359,999.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Angle(i32);

impl Angle {
    pub const ZERO: Self = Self(0);

    /// Refuses values outside 0–359,999.
    pub fn new(millidegrees: i32) -> Result<Self> {
        if (0..MILLIDEGREES_PER_TURN).contains(&millidegrees) {
            Ok(Self(millidegrees))
        } else {
            Err(Error::Invalid(format!(
                "angle {millidegrees} millidegrees is outside 0–359,999"
            )))
        }
    }

    /// Any number of millidegrees, reduced to one turn.
    pub fn normalized(millidegrees: i64) -> Self {
        // rem_euclid by 360,000 is within 0–359,999: the cast is lossless.
        Self(millidegrees.rem_euclid(i64::from(MILLIDEGREES_PER_TURN)) as i32)
    }

    pub const fn millidegrees(self) -> i32 {
        self.0
    }

    pub fn to_degrees_f64(self) -> f64 {
        f64::from(self.0) / 1_000.0
    }

    /// Computed degrees rounded half away from zero to whole millidegrees,
    /// then reduced to one turn.
    pub fn from_degrees_f64_rounded(degrees: f64) -> Result<Self> {
        if !degrees.is_finite() || degrees.abs() > 1e12 {
            return Err(Error::Invalid(format!(
                "angle {degrees}° is not a finite angle within ±1e12°"
            )));
        }
        // Within ±1e15 millidegrees: the cast is lossless.
        Ok(Self::normalized((degrees * 1_000.0).round() as i64))
    }

    /// `self + other`, reduced to one turn.
    pub fn rotated_by(self, other: Self) -> Self {
        Self::normalized(i64::from(self.0) + i64::from(other.0))
    }
}

impl<'de> Deserialize<'de> for Angle {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::new(i32::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Angle {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Angle".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "integer",
            "description": "A rotation in whole millidegrees, counter-clockwise.",
            "minimum": 0,
            "maximum": MILLIDEGREES_PER_TURN - 1
        })
    }
}

/// Which way an arc turns from its start to its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArcDirection {
    CounterClockwise,
    Clockwise,
}

/// An arc from the previous point of its path to `end` around `centre`
/// (the Gerber form). The radius is derived; [`ArcNm::check`] bounds how far
/// the two ends' radii may differ. `end` equal to the start is a full turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArcNm {
    pub end: PointNm,
    pub centre: PointNm,
    pub direction: ArcDirection,
}

impl ArcNm {
    /// The arc from `start` is well formed: a non-zero radius, and start and
    /// end radii within [`ARC_RADIUS_TOLERANCE_NM`] of each other. Exact:
    /// decided on integers, without square roots.
    pub fn check(&self, start: PointNm) -> Result<()> {
        let a = dist2(self.centre, start);
        let b = dist2(self.centre, self.end);
        if a == 0 || b == 0 {
            return Err(Error::Invalid(
                "an arc's start and end must not lie on its centre".into(),
            ));
        }
        if radii_within(a.max(b), a.min(b), ARC_RADIUS_TOLERANCE_NM) {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "an arc's start and end radii differ by more than {ARC_RADIUS_TOLERANCE_NM} nm"
            )))
        }
    }
}

/// `√big − √small ≤ t` for `big ≥ small ≥ 0`, exactly:
/// `√big ≤ √small + t` ⇔ `big − small − t² ≤ 2t√small`, and squaring a
/// positive left side gives `(big − small − t²)² ≤ 4t²·small`.
fn radii_within(big: i128, small: i128, t: i64) -> bool {
    let t = i128::from(t);
    let lhs = big - small - t * t;
    if lhs <= 0 {
        return true;
    }
    // An overflowing square exceeds 4t²·small (at most 5e20 here).
    match lhs.checked_mul(lhs) {
        Some(square) => square <= 4 * t * t * small,
        None => false,
    }
}
