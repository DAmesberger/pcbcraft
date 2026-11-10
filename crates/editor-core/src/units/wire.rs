//! [`Length`]: a length as MCP and CLI payloads carry it. Commands hold
//! [`Nm`]; the wire takes millimetres because `0.25` is natural and short
//! where `250000` invites ×1000 mistakes. This is presentation, like the UI,
//! not a second stored unit.
use super::text::{LengthUnit, parse_length};
use super::{MAX_COORD_NM, Nm};
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
