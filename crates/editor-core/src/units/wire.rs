//! [`Length`]: a length as MCP and CLI payloads carry it. Commands hold
//! [`Nm`]; the wire takes millimetres because `0.25` is natural and short
//! where `250000` invites ×1000 mistakes. This is presentation, like the UI,
//! not a second stored unit.
use super::text::{LengthUnit, parse_length};
use super::{Angle, ArcDirection, ArcNm, MAX_COORD_NM, Nm, PointNm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Accepts a JSON number in millimetres or a string with a unit (`"0.25mm"`,
/// `"10mil"`); a bare string number is millimetres. Serializes as a
/// millimetre number formatted exactly from nanometres.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Length(pub Nm);

impl Serialize for Length {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        // The shortest decimal form of `to_mm_f64` is the exact mm value.
        s.serialize_f64(self.0.to_mm_f64())
    }
}

struct LengthVisitor;

impl serde::de::Visitor<'_> for LengthVisitor {
    type Value = Length;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a length: a number in mm, or text with mm, µm, mil or in")
    }

    fn visit_f64<E: serde::de::Error>(self, mm: f64) -> Result<Length, E> {
        Nm::from_mm_f64_rounded(mm).map(Length).map_err(E::custom)
    }

    fn visit_i64<E: serde::de::Error>(self, mm: i64) -> Result<Length, E> {
        mm.checked_mul(super::NM_PER_MM)
            .and_then(Nm::try_new)
            .map(Length)
            .ok_or_else(|| E::custom(format!("length {mm} mm is outside ±{MAX_COORD_NM} nm")))
    }

    fn visit_u64<E: serde::de::Error>(self, mm: u64) -> Result<Length, E> {
        let mm = i64::try_from(mm)
            .map_err(|_| E::custom(format!("length {mm} mm is outside ±{MAX_COORD_NM} nm")))?;
        self.visit_i64(mm)
    }

    fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<Length, E> {
        parse_length(text, LengthUnit::Mm)
            .map(Length)
            .map_err(E::custom)
    }
}

impl<'de> Deserialize<'de> for Length {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(LengthVisitor)
    }
}

impl JsonSchema for Length {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Length".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "A length: a number in millimetres, or text with a unit (mm, µm or um, mil, in), such as \"0.25mm\" or \"10mil\". Replies give millimetres.",
            "oneOf": [
                { "type": "number" },
                { "type": "string", "maxLength": super::text::MAX_TEXT }
            ]
        })
    }
}

/// `#[serde(with = "units::length_mm")]` with `#[schemars(with = "Length")]`:
/// a command's [`Nm`] field, carried on the wire as a [`Length`].
pub mod length_mm {
    use super::{Length, Nm};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(nm: &Nm, s: S) -> Result<S::Ok, S::Error> {
        Length(*nm).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Nm, D::Error> {
        Length::deserialize(d).map(|l| l.0)
    }
}

/// `#[serde(with = "units::option_length_mm")]` with
/// `#[schemars(with = "Option<Length>")]`: an optional [`Nm`] as a [`Length`].
pub mod option_length_mm {
    use super::{Length, Nm};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(nm: &Option<Nm>, s: S) -> Result<S::Ok, S::Error> {
        nm.map(Length).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Nm>, D::Error> {
        Ok(Option::<Length>::deserialize(d)?.map(|l| l.0))
    }
}

/// `#[serde(with = "units::point_mm")]` with `#[schemars(with = "[Length; 2]")]`:
/// a command's [`PointNm`] field, carried on the wire as `[x, y]` lengths.
pub mod point_mm {
    use super::Length;
    use crate::units::PointNm;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(p: &PointNm, s: S) -> Result<S::Ok, S::Error> {
        [Length(p.x_nm), Length(p.y_nm)].serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PointNm, D::Error> {
        let [x, y] = <[Length; 2]>::deserialize(d)?;
        Ok(PointNm::new(x.0, y.0))
    }
}

impl From<Nm> for Length {
    fn from(nm: Nm) -> Self {
        Self(nm)
    }
}

impl From<Length> for Nm {
    fn from(length: Length) -> Self {
        length.0
    }
}

/// A position as MCP and CLI payloads carry it: `[x, y]`, each a
/// [`Length`]. The wire form of [`PointNm`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PointMm(pub PointNm);

impl Serialize for PointMm {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        point_mm::serialize(&self.0, s)
    }
}

impl<'de> Deserialize<'de> for PointMm {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        point_mm::deserialize(d).map(Self)
    }
}

impl JsonSchema for PointMm {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PointMm".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <[Length; 2]>::json_schema(generator)
    }
}

impl From<PointNm> for PointMm {
    fn from(p: PointNm) -> Self {
        Self(p)
    }
}

impl From<PointMm> for PointNm {
    fn from(p: PointMm) -> Self {
        p.0
    }
}

/// A rotation as MCP and CLI payloads carry it: a number of degrees,
/// counter-clockwise, kept to whole millidegrees and reduced to one turn.
/// The wire form of [`Angle`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Degrees(pub Angle);

impl Serialize for Degrees {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Millidegrees / 1000 is the nearest f64 to the exact decimal.
        s.serialize_f64(self.0.to_degrees_f64())
    }
}

impl<'de> Deserialize<'de> for Degrees {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let degrees = f64::deserialize(d)?;
        Angle::from_degrees_f64_rounded(degrees)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Degrees {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Degrees".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "number",
            "description": "An angle in degrees, counter-clockwise; kept to 0.001°."
        })
    }
}

impl From<Angle> for Degrees {
    fn from(a: Angle) -> Self {
        Self(a)
    }
}

impl From<Degrees> for Angle {
    fn from(d: Degrees) -> Self {
        d.0
    }
}

/// An arc as MCP and CLI payloads carry it: [`ArcNm`] with its points in
/// millimetres.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArcMm {
    pub end: PointMm,
    pub centre: PointMm,
    pub direction: ArcDirection,
}

impl From<ArcNm> for ArcMm {
    fn from(a: ArcNm) -> Self {
        Self {
            end: a.end.into(),
            centre: a.centre.into(),
            direction: a.direction,
        }
    }
}

impl From<ArcMm> for ArcNm {
    fn from(a: ArcMm) -> Self {
        Self {
            end: a.end.into(),
            centre: a.centre.into(),
            direction: a.direction,
        }
    }
}
