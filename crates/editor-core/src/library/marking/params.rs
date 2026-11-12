//! The millimetre wire form of markings: what `instance.set_marking`,
//! `library.set_marking` and the `instance.marking` reply carry. Written by
//! hand, not derived, because the payloads keep their millimetre and degree
//! field names (`height_mm`, `size_mm`, `offset_mm`, `rotation_deg`,
//! `seam_deg`, `wrap_deg`, `along_mm`, `min_text_height_mm`), which the
//! derive would rename. Each converts both ways without loss: the wire
//! types hold nanometres and millidegrees.
use super::surface::{Placement, SurfaceRef, Wrap, one, unit_scale, yes};
use super::{
    Align, Anchor, AreaChoice, CodeScheme, Ecc, Finish, InstanceMarking, MarkingItem, MarkingSpec,
    PolarityMark, Symbology, Template,
};
use crate::ids::{ContentHash, FieldRef};
use crate::units::{Degrees, Length, MILLIDEGREES_PER_TURN, PointMm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The wire form of [`MarkingItem`]: sizes in millimetres.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "item", rename_all = "snake_case", deny_unknown_fields)]
pub enum MarkingItemParams {
    /// Auto-fit to the area when `height_mm` is `None`.
    Text {
        template: Template,
        #[serde(default)]
        height_mm: Option<Length>,
        #[serde(default)]
        align: Align,
    },
    Code {
        scheme: CodeScheme,
        field: FieldRef,
    },
    Artwork {
        svg: ContentHash,
        size_mm: [Length; 2],
        #[serde(default)]
        anchor: Anchor,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        licence: Option<String>,
    },
    Polarity {
        mark: PolarityMark,
        #[serde(default)]
        at: Anchor,
    },
    Bands {
        value: FieldRef,
        #[serde(default)]
        tolerance: Option<FieldRef>,
    },
    Code2d {
        #[serde(default)]
        symbology: Symbology,
        payload: Template,
        size_mm: Length,
        #[serde(default)]
        anchor: Anchor,
        #[serde(default)]
        ecc: Ecc,
    },
    Electrolytic {
        capacitance: FieldRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voltage: Option<FieldRef>,
        #[serde(default = "yes")]
        stripe: bool,
    },
}

impl From<MarkingItem> for MarkingItemParams {
    fn from(item: MarkingItem) -> Self {
        match item {
            MarkingItem::Text {
                template,
                height_nm,
                align,
            } => Self::Text {
                template,
                height_mm: height_nm.map(Length),
                align,
            },
            MarkingItem::Code { scheme, field } => Self::Code { scheme, field },
            MarkingItem::Artwork {
                svg,
                size_nm,
                anchor,
                licence,
            } => Self::Artwork {
                svg,
                size_mm: size_nm.map(Length),
                anchor,
                licence,
            },
            MarkingItem::Polarity { mark, at } => Self::Polarity { mark, at },
            MarkingItem::Bands { value, tolerance } => Self::Bands { value, tolerance },
            MarkingItem::Code2d {
                symbology,
                payload,
                size_nm,
                anchor,
                ecc,
            } => Self::Code2d {
                symbology,
                payload,
                size_mm: Length(size_nm),
                anchor,
                ecc,
            },
            MarkingItem::Electrolytic {
                capacitance,
                voltage,
                stripe,
            } => Self::Electrolytic {
                capacitance,
                voltage,
                stripe,
            },
        }
    }
}

impl From<MarkingItemParams> for MarkingItem {
    fn from(item: MarkingItemParams) -> Self {
        match item {
            MarkingItemParams::Text {
                template,
                height_mm,
                align,
            } => Self::Text {
                template,
                height_nm: height_mm.map(|l| l.0),
                align,
            },
            MarkingItemParams::Code { scheme, field } => Self::Code { scheme, field },
            MarkingItemParams::Artwork {
                svg,
                size_mm,
                anchor,
                licence,
            } => Self::Artwork {
                svg,
                size_nm: size_mm.map(|l| l.0),
                anchor,
                licence,
            },
            MarkingItemParams::Polarity { mark, at } => Self::Polarity { mark, at },
            MarkingItemParams::Bands { value, tolerance } => Self::Bands { value, tolerance },
            MarkingItemParams::Code2d {
                symbology,
                payload,
                size_mm,
                anchor,
                ecc,
            } => Self::Code2d {
                symbology,
                payload,
                size_nm: size_mm.0,
                anchor,
                ecc,
            },
            MarkingItemParams::Electrolytic {
                capacitance,
                voltage,
                stripe,
            } => Self::Electrolytic {
                capacitance,
                voltage,
                stripe,
            },
        }
    }
}

/// The wire form of [`AreaChoice`]: the explicit rectangle in millimetres.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "area", rename_all = "snake_case", deny_unknown_fields)]
pub enum AreaChoiceParams {
    #[default]
    FromModel,
    Explicit {
        centre: PointMm,
        size_mm: [Length; 2],
    },
}

impl From<AreaChoice> for AreaChoiceParams {
    fn from(area: AreaChoice) -> Self {
        match area {
            AreaChoice::FromModel => Self::FromModel,
            AreaChoice::Explicit { centre, size_nm } => Self::Explicit {
                centre: PointMm(centre),
                size_mm: size_nm.map(Length),
            },
        }
    }
}

impl From<AreaChoiceParams> for AreaChoice {
    fn from(area: AreaChoiceParams) -> Self {
        match area {
            AreaChoiceParams::FromModel => Self::FromModel,
            AreaChoiceParams::Explicit { centre, size_mm } => Self::Explicit {
                centre: centre.0,
                size_nm: size_mm.map(|l| l.0),
            },
        }
    }
}

/// The wire form of [`Wrap`]: degrees and millimetres. `wrap_deg` is a
/// sweep of up to a full turn, not a [`Degrees`] rotation (which reduces
/// 360° to 0).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WrapParams {
    /// Where the chart's start edge sits, degrees.
    #[serde(default)]
    pub seam_deg: Degrees,
    /// How far round the chart reaches, 1–360°.
    #[serde(default = "full_turn_deg")]
    pub wrap_deg: f64,
    /// The part of the side used, mm along the axis from its start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub along_mm: Option<[Length; 2]>,
    /// Copies of the layout side by side round the wrap (1–8).
    #[serde(default = "one")]
    pub repeat: u32,
}

fn full_turn_deg() -> f64 {
    360.0
}

/// A sweep in degrees as whole millidegrees. Out of range (or not finite)
/// becomes a value [`Placement::problem`] refuses, never a wrap that draws.
fn sweep_mdeg(degrees: f64) -> i32 {
    let mdeg = (degrees * 1_000.0).round();
    if mdeg.is_finite() && mdeg.abs() <= f64::from(MILLIDEGREES_PER_TURN) * 2.0 {
        // Integral and within ±720,000: the cast is exact.
        mdeg as i32
    } else {
        0
    }
}

impl From<Wrap> for WrapParams {
    fn from(w: Wrap) -> Self {
        Self {
            seam_deg: Degrees(w.seam_mdeg),
            wrap_deg: w.wrap_deg(),
            along_mm: w.along_nm.map(|a| a.map(Length)),
            repeat: w.repeat,
        }
    }
}

impl From<WrapParams> for Wrap {
    fn from(w: WrapParams) -> Self {
        Self {
            seam_mdeg: w.seam_deg.0,
            wrap_mdeg: sweep_mdeg(w.wrap_deg),
            along_nm: w.along_mm.map(|a| a.map(|l| l.0)),
            repeat: w.repeat,
        }
    }
}

/// The wire form of [`Placement`]: millimetres and degrees.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlacementParams {
    #[serde(default)]
    pub on: SurfaceRef,
    /// Shift of the layout's centre from the region's, chart mm `[u, v]`.
    #[serde(default)]
    pub offset_mm: PointMm,
    /// Counter-clockwise turn of the layout, seen from outside, degrees.
    #[serde(default)]
    pub rotation_deg: Degrees,
    #[serde(default = "unit_scale")]
    pub scale: f64,
    #[serde(default)]
    pub mirror: bool,
    #[serde(default = "yes")]
    pub clip: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap: Option<WrapParams>,
}

impl Default for PlacementParams {
    fn default() -> Self {
        Placement::default().into()
    }
}

impl PlacementParams {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl From<Placement> for PlacementParams {
    fn from(p: Placement) -> Self {
        Self {
            on: p.on,
            offset_mm: PointMm(p.offset),
            rotation_deg: Degrees(p.rotation_mdeg),
            scale: p.scale,
            mirror: p.mirror,
            clip: p.clip,
            wrap: p.wrap.map(WrapParams::from),
        }
    }
}

impl From<PlacementParams> for Placement {
    fn from(p: PlacementParams) -> Self {
        Self {
            on: p.on,
            offset: p.offset_mm.0,
            rotation_mdeg: p.rotation_deg.0,
            scale: p.scale,
            mirror: p.mirror,
            clip: p.clip,
            wrap: p.wrap.map(Wrap::from),
        }
    }
}

/// The wire form of [`MarkingSpec`]: millimetres and degrees.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MarkingSpecParams {
    pub items: Vec<MarkingItemParams>,
    #[serde(default)]
    pub area: AreaChoiceParams,
    #[serde(default)]
    pub finish: Finish,
    /// Smallest legible text height, mm.
    #[serde(default)]
    pub min_text_height_mm: Option<Length>,
    #[serde(default, skip_serializing_if = "PlacementParams::is_default")]
    pub placement: PlacementParams,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decals: Vec<MarkingSpecParams>,
}

impl From<MarkingSpec> for MarkingSpecParams {
    fn from(s: MarkingSpec) -> Self {
        Self {
            items: s.items.into_iter().map(MarkingItemParams::from).collect(),
            area: s.area.into(),
            finish: s.finish,
            min_text_height_mm: s.min_text_height_nm.map(Length),
            placement: s.placement.into(),
            decals: s.decals.into_iter().map(MarkingSpecParams::from).collect(),
        }
    }
}

impl From<MarkingSpecParams> for MarkingSpec {
    fn from(s: MarkingSpecParams) -> Self {
        Self {
            items: s.items.into_iter().map(MarkingItem::from).collect(),
            area: s.area.into(),
            finish: s.finish,
            min_text_height_nm: s.min_text_height_mm.map(|l| l.0),
            placement: s.placement.into(),
            decals: s.decals.into_iter().map(MarkingSpec::from).collect(),
        }
    }
}

/// The wire form of [`InstanceMarking`]: a replacing spec in millimetres.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InstanceMarkingParams {
    #[serde(default)]
    pub hide: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<MarkingSpecParams>,
}

impl From<InstanceMarking> for InstanceMarkingParams {
    fn from(m: InstanceMarking) -> Self {
        Self {
            hide: m.hide,
            vars: m.vars,
            replace: m.replace.map(MarkingSpecParams::from),
        }
    }
}

impl From<InstanceMarkingParams> for InstanceMarking {
    fn from(m: InstanceMarkingParams) -> Self {
        Self {
            hide: m.hide,
            vars: m.vars,
            replace: m.replace.map(MarkingSpec::from),
        }
    }
}

/// Serde adapter: an [`InstanceMarking`] in a command payload takes the
/// millimetre wire form ([`InstanceMarkingParams`]).
pub mod instance_mm {
    use super::{InstanceMarking, InstanceMarkingParams};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(m: &InstanceMarking, s: S) -> Result<S::Ok, S::Error> {
        InstanceMarkingParams::from(m.clone()).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<InstanceMarking, D::Error> {
        InstanceMarkingParams::deserialize(d).map(InstanceMarking::from)
    }
}

/// Serde adapter: a [`Placement`] in a reply takes the millimetre wire form
/// ([`PlacementParams`]).
pub mod placement_mm {
    use super::{Placement, PlacementParams};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(p: &Placement, s: S) -> Result<S::Ok, S::Error> {
        PlacementParams::from(p.clone()).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Placement, D::Error> {
        PlacementParams::deserialize(d).map(Placement::from)
    }
}
