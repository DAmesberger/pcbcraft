//! The board's looks (laminate, copper, soldermask, the display stand-ins of
//! package films), the pad finishes and legend ink with their thicknesses,
//! and the sizes of the display-only layers: shipped data
//! (`materials/board-looks.json`, `finishes.json`, `appearance.json`),
//! loaded by `sparksmith-io` and handed in. Values follow
//! `docs/materials.md`; each carries its source or estimate as a note.
//!
//! Per-colour and per-finish values are records with one field per choice,
//! so a file that leaves one out or names an unknown one is refused. The
//! integration crate maps the document's choices onto these fields.
use super::MIN_ROUGHNESS;
use crate::diagnostics::{Error, Result};
use serde::Deserialize;

fn unit(v: f64) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}

fn rough(v: f64) -> bool {
    v.is_finite() && (MIN_ROUGHNESS..=1.0).contains(&v)
}

fn ior(v: f64) -> bool {
    v.is_finite() && (1.0..=3.0).contains(&v)
}

fn check(ok: bool, what: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "material.invalid: {what} has a value outside the opaque-surface bounds"
        )))
    }
}

/// One physical look in Solidcraft's opaque-surface parameters: linear-sRGB
/// reflectance (F0 for a metal), metallic, perceptual roughness and IOR
/// (metals store 1.5, which Solidcraft requires and ignores).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Look {
    pub name: String,
    pub base_linear: [f64; 3],
    pub metallic: f64,
    pub roughness: f64,
    pub ior: f64,
    /// Where the values come from, or why they are estimated.
    pub note: String,
}

impl Look {
    fn validate(&self) -> Result<()> {
        check(
            self.base_linear.iter().all(|c| unit(*c))
                && (self.metallic == 0.0 || self.metallic == 1.0)
                && rough(self.roughness)
                && ior(self.ior)
                && !self.name.trim().is_empty()
                && !self.note.trim().is_empty(),
            &format!("look ‘{}’", self.name),
        )
    }
}

/// One value per soldermask colour.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PerMaskColour<T> {
    pub green: T,
    pub blue: T,
    pub red: T,
    pub purple: T,
    pub yellow: T,
    pub black: T,
    pub white: T,
    pub clear: T,
}

impl<T> PerMaskColour<T> {
    fn all(&self) -> [&T; 8] {
        [
            &self.green,
            &self.blue,
            &self.red,
            &self.purple,
            &self.yellow,
            &self.black,
            &self.white,
            &self.clear,
        ]
    }
}

/// One value per soldermask finish.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PerMaskFinish<T> {
    pub gloss: T,
    pub satin: T,
    pub matte: T,
}

/// One value per pad finish.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PerPadFinish<T> {
    pub enig: T,
    pub hasl: T,
    pub lead_free_hasl: T,
    pub osp: T,
    pub immersion_silver: T,
}

impl<T> PerPadFinish<T> {
    fn all(&self) -> [&T; 5] {
        [
            &self.enig,
            &self.hasl,
            &self.lead_free_hasl,
            &self.osp,
            &self.immersion_silver,
        ]
    }
}

/// One value per legend ink colour.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PerInk<T> {
    pub white: T,
    pub black: T,
    pub yellow: T,
}

/// A soldermask's pigmented resin: per-pass transmittance through 20 µm,
/// the light its own pigment scatters back over a black base, and how much
/// of the laminate's glass weave shows through it.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaskPigment {
    pub t20: [f64; 3],
    pub scatter: [f64; 3],
    pub weave_show_through: f64,
}

/// The soldermask layer model's constants and its colours.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Mask {
    /// The resin's index: the lacquer coat's IOR.
    pub ior: f64,
    /// Normal-incidence reflectance of the mask surface.
    pub surface_f0: f64,
    /// Internal diffuse reflectance of the mask/air interface (Saunderson's
    /// k₂).
    pub internal_reflectance: f64,
    /// The pigmented body under the lacquer: IOR and roughness.
    pub body_ior: f64,
    pub body_roughness: f64,
    /// The bare copper F0 a mask over copper covers.
    pub copper_under: [f64; 3],
    pub over_laminate_name: String,
    pub over_copper_name: String,
    pub colours: PerMaskColour<MaskPigment>,
    /// The lacquer coat's perceptual roughness per finish and colour.
    pub coat_roughness: PerMaskFinish<PerMaskColour<f64>>,
    pub note: String,
}

/// The board's looks: shipped `materials/board-looks.json`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoardLooks {
    pub about: String,
    pub laminate: Look,
    pub laminate_edge: Look,
    /// The board's exposed copper (tracks, pads, vias, fills, edge foils).
    pub copper: Look,
    pub via_bore: Look,
    /// Stand-ins for package films and bodies whose recorded material or
    /// finish does not decide their look.
    pub termination: Look,
    pub solder: Look,
    pub package_body: Look,
    pub chip_resistor: Look,
    pub chip_capacitor: Look,
    pub overcoat: Look,
    pub marking: Look,
    pub mask: Mask,
}

impl BoardLooks {
    /// The looks in `text`, refused when a value is out of bounds.
    pub fn from_json(text: &str) -> Result<Self> {
        let looks: Self = serde_json::from_str(text)?;
        for look in [
            &looks.laminate,
            &looks.laminate_edge,
            &looks.copper,
            &looks.via_bore,
            &looks.termination,
            &looks.solder,
            &looks.package_body,
            &looks.chip_resistor,
            &looks.chip_capacitor,
            &looks.overcoat,
            &looks.marking,
        ] {
            look.validate()?;
        }
        let m = &looks.mask;
        let finishes = [
            &m.coat_roughness.gloss,
            &m.coat_roughness.satin,
            &m.coat_roughness.matte,
        ];
        check(
            ior(m.ior)
                && unit(m.surface_f0)
                && m.surface_f0 < 1.0
                && unit(m.internal_reflectance)
                && m.internal_reflectance < 1.0
                && ior(m.body_ior)
                && rough(m.body_roughness)
                && m.copper_under.iter().all(|c| unit(*c))
                && m.colours.all().iter().all(|p| {
                    p.t20.iter().all(|t| unit(*t) && *t > 0.0)
                        && p.scatter.iter().all(|c| unit(*c))
                        && unit(p.weave_show_through)
                })
                && finishes
                    .iter()
                    .all(|f| f.all().iter().all(|r| rough(**r)))
                && !m.over_laminate_name.trim().is_empty()
                && !m.over_copper_name.trim().is_empty(),
            "the soldermask",
        )?;
        Ok(looks)
    }
}

/// One plated layer of a finish and the thickness range a source gives.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Plating {
    pub name: String,
    pub um: [f64; 2],
}

/// A finish's plated layers and where their ranges come from.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Thickness {
    pub layers: Vec<Plating>,
    pub source: String,
}

impl Thickness {
    /// The middle of the range: the layers' lows and highs summed.
    pub fn mid_um(&self) -> f64 {
        let low = self.layers.iter().fold(0.0, |sum, l| sum + l.um[0]);
        let high = self.layers.iter().fold(0.0, |sum, l| sum + l.um[1]);
        (low + high) / 2.0
    }
}

/// A pad finish: its look and its plated thickness.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadFinishDef {
    pub look: Look,
    pub thickness: Thickness,
}

/// The legend ink: one look per colour sharing a surface, its grain and
/// its film thickness.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LegendInk {
    pub name: String,
    pub colours: PerInk<[f64; 3]>,
    pub roughness: f64,
    pub ior: f64,
    /// How strongly the ink's grain shows (0..=1).
    pub grain: f64,
    pub thickness_mm: f64,
    pub note: String,
}

/// Pad finishes and legend ink: shipped `materials/finishes.json`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Finishes {
    pub about: String,
    pub pad: PerPadFinish<PadFinishDef>,
    pub legend_ink: LegendInk,
}

impl Finishes {
    /// The finishes in `text`, refused when a value is out of bounds.
    pub fn from_json(text: &str) -> Result<Self> {
        let finishes: Self = serde_json::from_str(text)?;
        for pad in finishes.pad.all() {
            pad.look.validate()?;
            check(
                !pad.thickness.layers.is_empty()
                    && !pad.thickness.source.trim().is_empty()
                    && pad.thickness.layers.iter().all(|l| {
                        l.um.iter().all(|t| t.is_finite() && *t > 0.0) && l.um[0] <= l.um[1]
                    }),
                &format!("the thickness of ‘{}’", pad.look.name),
            )?;
        }
        let ink = &finishes.legend_ink;
        let colours = [ink.colours.white, ink.colours.black, ink.colours.yellow];
        check(
            colours.iter().all(|c| c.iter().all(|v| unit(*v)))
                && rough(ink.roughness)
                && ior(ink.ior)
                && unit(ink.grain)
                && ink.thickness_mm.is_finite()
                && ink.thickness_mm > 0.0
                && !ink.name.trim().is_empty(),
            "the legend ink",
        )?;
        Ok(finishes)
    }
}

/// A size of a display-only layer and where it comes from.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Size {
    pub value: f64,
    pub note: String,
}

/// The sizes of the display-only layers over the native model:
/// terminations, overcoat, solder joints and via tents. Shipped
/// `materials/appearance.json`; lengths in mm, shares as fractions.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppearanceSizes {
    pub about: String,
    pub termination_growth_mm: Size,
    pub chip_band_share: Size,
    pub chip_band_max_mm: Size,
    pub lead_band_height_share: Size,
    pub overcoat_inset_mm: Size,
    pub overcoat_mm: Size,
    pub fillet_height_share: Size,
    pub fillet_max_mm: Size,
    pub fillet_width_share: Size,
    pub fillet_min_toe_mm: Size,
    pub foot_toe_share: Size,
    pub foot_heel_share: Size,
    pub pin_land_share: Size,
    pub pin_rise_share: Size,
    pub pin_rise_max_mm: Size,
    pub tent_sag_mm: Size,
    pub tent_skirt_mm: Size,
}

impl AppearanceSizes {
    /// The sizes in `text`; each must be positive and at most 10 (a share
    /// or a length in mm).
    pub fn from_json(text: &str) -> Result<Self> {
        let sizes: Self = serde_json::from_str(text)?;
        let all = [
            &sizes.termination_growth_mm,
            &sizes.chip_band_share,
            &sizes.chip_band_max_mm,
            &sizes.lead_band_height_share,
            &sizes.overcoat_inset_mm,
            &sizes.overcoat_mm,
            &sizes.fillet_height_share,
            &sizes.fillet_max_mm,
            &sizes.fillet_width_share,
            &sizes.fillet_min_toe_mm,
            &sizes.foot_toe_share,
            &sizes.foot_heel_share,
            &sizes.pin_land_share,
            &sizes.pin_rise_share,
            &sizes.pin_rise_max_mm,
            &sizes.tent_sag_mm,
            &sizes.tent_skirt_mm,
        ];
        if all
            .iter()
            .all(|s| s.value.is_finite() && s.value > 0.0 && s.value <= 10.0 && !s.note.trim().is_empty())
        {
            Ok(sizes)
        } else {
            Err(Error::Invalid(
                "appearance.invalid: every display-layer size is positive and at most 10".into(),
            ))
        }
    }
}
