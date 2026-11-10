//! The wire form of schematic command parameters that carry the document's
//! geometry: each length a [`Length`] (a number in mm, or text with a unit)
//! and each point `[x, y]` lengths. Commands hold the document's types, in
//! whole nanometres; these forms are presentation only, as in the UI.
//! Apply them with `#[serde(with = "…")]` and `#[schemars(with = "…")]`.
use crate::library::Rotation;
use crate::model::{Corner, FreeEnd, SheetItem};
use crate::units::{Length, Nm, PointNm, point_mm};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

fn is_zero(length: &Length) -> bool {
    length.0 == Nm::ZERO
}

/// A wire corner: a vertex and an optional fillet radius (0, sharp, when
/// omitted).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CornerParams {
    #[serde(with = "point_mm")]
    #[schemars(with = "[Length; 2]")]
    pub at: PointNm,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub radius: Length,
}

impl From<Corner> for CornerParams {
    fn from(c: Corner) -> Self {
        Self {
            at: c.at,
            radius: Length(c.radius_nm),
        }
    }
}

impl From<CornerParams> for Corner {
    fn from(c: CornerParams) -> Self {
        Self {
            at: c.at,
            radius_nm: c.radius.0,
        }
    }
}

/// `Vec<Corner>` as [`CornerParams`].
pub mod corners {
    use super::*;

    pub fn serialize<S: Serializer>(corners: &[Corner], s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(corners.iter().map(|c| CornerParams::from(*c)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Corner>, D::Error> {
        Ok(Vec::<CornerParams>::deserialize(d)?
            .into_iter()
            .map(Corner::from)
            .collect())
    }
}

/// A free wire end: `{"free": [x, y]}`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FreeParams {
    #[serde(with = "point_mm")]
    #[schemars(with = "[Length; 2]")]
    pub free: PointNm,
}

/// [`FreeEnd`] as [`FreeParams`].
pub mod free_end {
    use super::*;

    pub fn serialize<S: Serializer>(end: &FreeEnd, s: S) -> Result<S::Ok, S::Error> {
        FreeParams { free: end.free }.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<FreeEnd, D::Error> {
        FreeParams::deserialize(d).map(|p| FreeEnd { free: p.free })
    }
}

/// Sheet furniture: a note, line or rectangle.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "item", rename_all = "snake_case", deny_unknown_fields)]
pub enum SheetItemParams {
    /// A text note; lines split at `\n`.
    Text {
        #[serde(with = "point_mm")]
        #[schemars(with = "[Length; 2]")]
        at: PointNm,
        text: String,
        /// Cap height.
        height: Length,
        #[serde(default)]
        rotation: Rotation,
    },
    Line {
        #[serde(with = "point_mm")]
        #[schemars(with = "[Length; 2]")]
        from: PointNm,
        #[serde(with = "point_mm")]
        #[schemars(with = "[Length; 2]")]
        to: PointNm,
    },
    Rect {
        #[serde(with = "point_mm")]
        #[schemars(with = "[Length; 2]")]
        min: PointNm,
        #[serde(with = "point_mm")]
        #[schemars(with = "[Length; 2]")]
        max: PointNm,
    },
}

impl From<SheetItem> for SheetItemParams {
    fn from(item: SheetItem) -> Self {
        match item {
            SheetItem::Text {
                at,
                text,
                height_nm,
                rotation,
            } => Self::Text {
                at,
                text,
                height: Length(height_nm),
                rotation,
            },
            SheetItem::Line { from, to } => Self::Line { from, to },
            SheetItem::Rect { min, max } => Self::Rect { min, max },
        }
    }
}

impl From<SheetItemParams> for SheetItem {
    fn from(item: SheetItemParams) -> Self {
        match item {
            SheetItemParams::Text {
                at,
                text,
                height,
                rotation,
            } => Self::Text {
                at,
                text,
                height_nm: height.0,
                rotation,
            },
            SheetItemParams::Line { from, to } => Self::Line { from, to },
            SheetItemParams::Rect { min, max } => Self::Rect { min, max },
        }
    }
}

/// [`SheetItem`] as [`SheetItemParams`].
pub mod sheet_item {
    use super::*;

    pub fn serialize<S: Serializer>(item: &SheetItem, s: S) -> Result<S::Ok, S::Error> {
        SheetItemParams::from(item.clone()).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SheetItem, D::Error> {
        SheetItemParams::deserialize(d).map(SheetItem::from)
    }
}
