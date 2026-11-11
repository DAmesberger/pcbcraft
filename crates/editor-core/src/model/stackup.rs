//! The board's layer stackup: copper foils and the dielectrics between them,
//! top to bottom, plus which sides carry soldermask and silkscreen. The
//! stackup sets the native board's thickness and copper heights; its rules
//! (`Stackup::check`) are pure data validation.
use super::Layer;
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Copper foil thickness of one ounce per square foot, in µm (IPC-4562
/// nominal 34.8 µm, rounded as fabricators quote it).
pub const OUNCE_UM: f64 = 35.0;
/// Fewest copper layers of a stackup: a double-sided board.
pub const MIN_COPPER_LAYERS: usize = 2;
/// Most copper layers of a stackup. Not a fabrication limit: production
/// boards above about 50 layers are rare and 64 stays above them. It bounds
/// the per-layer work a hostile or mistaken file can ask for (layer rows
/// and commands, artwork files, 3D foils and edge bands).
pub const MAX_COPPER_LAYERS: usize = 64;
/// Finished copper foil thicknesses a stackup accepts, in µm.
pub const COPPER_UM_RANGE: std::ops::RangeInclusive<f64> = 5.0..=210.0;
/// Dielectric thicknesses a stackup accepts, in mm.
pub const DIELECTRIC_MM_RANGE: std::ops::RangeInclusive<f64> = 0.025..=3.2;
/// Soldermask over copper of a new stackup, in µm: within the 10–20 µm
/// over copper that the LPI soldermask datasheets quote (Florida CirTech
/// LPI-950 LC-1, C-8GD and S-22; listed in `docs/materials.md`,
/// "Sources"), toward their thin end because LPI thins over a track's
/// crown and edges. The 3D view's mask over copper, its geometry and its
/// colour, is this thickness.
pub const MASK_UM: f64 = 12.0;
/// Soldermask thicknesses over copper a stackup accepts, in µm. An input
/// bound, not a fabrication limit: from below the thinnest LPI coat to
/// above dry-film masks.
pub const MASK_UM_RANGE: std::ops::RangeInclusive<f64> = 5.0..=100.0;

/// A named copper layer of the board, top to bottom. Its identity is its
/// name: `Inner(1)` is the first inner layer below the top, whatever the
/// stackup's count. Serialised as `"Top"`, `{"Inner": n}` and `"Bottom"`,
/// so outer layers read as the board sides they are.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum CopperLayer {
    Top,
    /// Inner layer `n`, counted from 1 below the top.
    Inner(u8),
    Bottom,
}

impl CopperLayer {
    /// The board side of an outer layer; `None` for an inner layer.
    pub fn side(self) -> Option<Layer> {
        match self {
            CopperLayer::Top => Some(Layer::Top),
            CopperLayer::Bottom => Some(Layer::Bottom),
            CopperLayer::Inner(_) => None,
        }
    }

    /// The user's name of the layer ("Top", "Inner 1", "Bottom").
    pub fn name(self) -> String {
        match self {
            CopperLayer::Top => "Top".into(),
            CopperLayer::Inner(n) => format!("Inner {n}"),
            CopperLayer::Bottom => "Bottom".into(),
        }
    }
}

impl From<Layer> for CopperLayer {
    fn from(side: Layer) -> Self {
        match side {
            Layer::Top => CopperLayer::Top,
            Layer::Bottom => CopperLayer::Bottom,
        }
    }
}

impl PartialEq<Layer> for CopperLayer {
    fn eq(&self, side: &Layer) -> bool {
        *self == CopperLayer::from(*side)
    }
}

/// One copper foil, as plated (outer) or laminated (inner).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CopperFoil {
    /// Finished copper thickness in µm (35 µm is 1 oz).
    pub thickness_um: f64,
    /// A reference plane (ground or power) for controlled impedance. When
    /// no foil of a stackup is marked, the impedance model takes the
    /// copper layers next to a signal layer as its references
    /// (`layout::impedance`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plane: bool,
}

impl CopperFoil {
    /// A foil of `ounces` per square foot.
    pub fn ounces(ounces: f64) -> Self {
        Self {
            thickness_um: ounces * OUNCE_UM,
            plane: false,
        }
    }
}

/// How a dielectric is built: a cured core laminate or prepreg bonding plies.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DielectricKind {
    Core,
    Prepreg,
}

/// E-glass fabric style (IPC-4412 designations), which sets the weave pitch
/// the routed board edge shows.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum GlassStyle {
    Style106,
    Style1080,
    Style2116,
    Style7628,
}

/// One dielectric between two consecutive copper foils.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Dielectric {
    pub kind: DielectricKind,
    pub thickness_mm: f64,
    pub glass: GlassStyle,
    /// Relative permittivity (Dk) the fabricator quotes for this layer.
    /// When absent, [`Dielectric::permittivity`] estimates it from the
    /// glass style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dk: Option<f64>,
}

/// The Dk values [`Dielectric::dk`] accepts.
pub const DK_RANGE: std::ops::RangeInclusive<f64> = 1.0..=20.0;

impl Dielectric {
    /// The relative permittivity the impedance model uses: the entered
    /// [`Dielectric::dk`], else a generic FR-4 estimate by glass style
    /// (resin-rich thin styles lower, glass-heavy 7628 higher; typical
    /// values near 1 GHz, not a fabricator's data). Checked against Isola's
    /// published 370HR core table (`docs/materials.md`, "Sources"): 106,
    /// 1080 and 2116 agree within 0.05 (3.81, 4.04, 4.26); 7628 does not
    /// (Isola 4.36 at 42 % resin), and stays uncited until decided.
    pub fn permittivity(&self) -> f64 {
        self.dk.unwrap_or(match self.glass {
            GlassStyle::Style106 => 3.8,
            GlassStyle::Style1080 => 4.0,
            GlassStyle::Style2116 => 4.3,
            GlassStyle::Style7628 => 4.6,
        })
    }

    /// Whether [`Dielectric::permittivity`] is an estimate.
    pub fn permittivity_estimated(&self) -> bool {
        self.dk.is_none()
    }
}

/// The board's stackup. `copper` lists foils top to bottom; `dielectrics[i]`
/// lies between `copper[i]` and `copper[i + 1]`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stackup {
    pub copper: Vec<CopperFoil>,
    pub dielectrics: Vec<Dielectric>,
    pub mask_top: bool,
    pub mask_bottom: bool,
    /// Soldermask thickness over copper in µm, on every side with mask
    /// ([`MASK_UM`] unless the fabricator quotes another).
    #[serde(default = "default_mask_um")]
    pub mask_um: f64,
    pub silkscreen_top: bool,
    pub silkscreen_bottom: bool,
}

fn default_mask_um() -> f64 {
    MASK_UM
}

impl Default for Stackup {
    /// Two layers of 1 oz copper on a 1.6 mm FR-4 core (7628 glass), mask
    /// and silkscreen on both sides.
    fn default() -> Self {
        Self {
            copper: vec![CopperFoil::ounces(1.0); 2],
            dielectrics: vec![Dielectric {
                kind: DielectricKind::Core,
                thickness_mm: 1.6,
                glass: GlassStyle::Style7628,
                dk: None,
            }],
            mask_top: true,
            mask_bottom: true,
            mask_um: MASK_UM,
            silkscreen_top: true,
            silkscreen_bottom: true,
        }
    }
}

impl Stackup {
    /// A symmetric preset of `count` copper layers of 1 oz copper, for any
    /// count the rules allow: prepreg outside and between the cores, which
    /// carry the inner layers in pairs (foil lamination). Up to ten layers
    /// it keeps the common 1.6 mm of dielectric; above that it grows by
    /// 0.16 mm a layer, as thick high-count boards do. Thin builds use the
    /// finer glass styles.
    pub fn preset(count: usize) -> Result<Self> {
        check_count(count)?;
        if count == MIN_COPPER_LAYERS {
            return Ok(Self::default());
        }
        let prepregs = count / 2;
        let cores = prepregs - 1;
        let lossy = |v: usize| f64::from(u32::try_from(v).unwrap_or(u32::MAX));
        let total = if count <= 10 {
            1.6
        } else {
            0.16 * lossy(count)
        };
        let prepreg_mm = if count <= 6 { 0.2 } else { 0.1 };
        let core_mm =
            ((total - prepreg_mm * lossy(prepregs)) / lossy(cores) * 1000.0).round() / 1000.0;
        let prepreg = Dielectric {
            kind: DielectricKind::Prepreg,
            thickness_mm: prepreg_mm,
            glass: if prepreg_mm >= 0.2 {
                GlassStyle::Style2116
            } else {
                GlassStyle::Style1080
            },
            dk: None,
        };
        let core = Dielectric {
            kind: DielectricKind::Core,
            thickness_mm: core_mm,
            glass: if core_mm >= 0.3 {
                GlassStyle::Style7628
            } else {
                GlassStyle::Style2116
            },
            dk: None,
        };
        let dielectrics = (0..count - 1)
            .map(|i| if i % 2 == 0 { prepreg } else { core })
            .collect();
        let stackup = Self {
            copper: vec![CopperFoil::ounces(1.0); count],
            dielectrics,
            ..Self::default()
        };
        stackup.check()?;
        Ok(stackup)
    }

    /// The copper layer counts [`Stackup::preset`] offers, in order.
    pub fn preset_counts() -> impl Iterator<Item = usize> {
        (MIN_COPPER_LAYERS..=MAX_COPPER_LAYERS).step_by(2)
    }

    /// The copper layers top to bottom.
    pub fn layers(&self) -> Vec<CopperLayer> {
        let count = self.copper.len();
        (0..count)
            .map(|i| match i {
                0 => CopperLayer::Top,
                i if i + 1 == count => CopperLayer::Bottom,
                // Bounded by `check`: at most `MAX_COPPER_LAYERS`.
                i => CopperLayer::Inner(u8::try_from(i).unwrap_or(u8::MAX)),
            })
            .collect()
    }

    /// Whether the stackup has this copper layer.
    pub fn has(&self, layer: CopperLayer) -> bool {
        match layer {
            CopperLayer::Top | CopperLayer::Bottom => true,
            CopperLayer::Inner(n) => n >= 1 && usize::from(n) + 1 < self.copper.len(),
        }
    }

    /// The index of a layer in `copper`.
    pub fn index(&self, layer: CopperLayer) -> Option<usize> {
        if !self.has(layer) {
            return None;
        }
        Some(match layer {
            CopperLayer::Top => 0,
            CopperLayer::Inner(n) => usize::from(n),
            CopperLayer::Bottom => self.copper.len().checked_sub(1)?,
        })
    }

    /// Total dielectric thickness in mm: the native substrate's height. The
    /// outer foils lie outside it (bottom copper below z = 0, top copper
    /// above this height).
    pub fn dielectric_mm(&self) -> f64 {
        self.dielectrics.iter().map(|d| d.thickness_mm).sum()
    }

    /// Finished board thickness over the outer copper, in mm (mask excluded).
    pub fn thickness_mm(&self) -> f64 {
        self.dielectric_mm()
            + self
                .copper
                .iter()
                .map(|c| c.thickness_um / 1000.0)
                .sum::<f64>()
    }

    /// A copper layer's z range in mm, with z = 0 at the bottom face of the
    /// lowest dielectric. Inner foils are embedded at the top of the
    /// dielectric below them (the bonding prepreg fills around them).
    pub fn copper_span(&self, layer: CopperLayer) -> Option<(f64, f64)> {
        let index = self.index(layer)?;
        let foil = self.copper.get(index)?.thickness_um / 1000.0;
        if layer == CopperLayer::Bottom {
            return Some((-foil, 0.0));
        }
        let top: f64 = self
            .dielectrics
            .iter()
            .skip(index)
            .map(|d| d.thickness_mm)
            .sum();
        if layer == CopperLayer::Top {
            return Some((top, top + foil));
        }
        Some((top - foil, top))
    }

    /// The z range in mm of `dielectrics[index]`.
    pub fn dielectric_span(&self, index: usize) -> Option<(f64, f64)> {
        let dielectric = self.dielectrics.get(index)?;
        let top: f64 = self
            .dielectrics
            .iter()
            .skip(index)
            .map(|d| d.thickness_mm)
            .sum();
        Some((top - dielectric.thickness_mm, top))
    }

    /// The stackup rules: an even number of copper layers from
    /// [`MIN_COPPER_LAYERS`] to [`MAX_COPPER_LAYERS`], exactly one
    /// dielectric between neighbouring foils, finite thicknesses within
    /// fabrication ranges, Dk within [`DK_RANGE`] and the mask thickness
    /// within [`MASK_UM_RANGE`].
    pub fn check(&self) -> Result<()> {
        check_count(self.copper.len())?;
        if self.dielectrics.len() + 1 != self.copper.len() {
            return Err(Error::Invalid(
                "a stackup has one dielectric between each pair of copper layers".into(),
            ));
        }
        if !self
            .copper
            .iter()
            .all(|c| c.thickness_um.is_finite() && COPPER_UM_RANGE.contains(&c.thickness_um))
        {
            return Err(Error::Invalid("copper thickness must be 5–210 µm".into()));
        }
        if !self
            .dielectrics
            .iter()
            .all(|d| d.thickness_mm.is_finite() && DIELECTRIC_MM_RANGE.contains(&d.thickness_mm))
        {
            return Err(Error::Invalid(
                "dielectric thickness must be 0.025–3.2 mm".into(),
            ));
        }
        if !self
            .dielectrics
            .iter()
            .filter_map(|d| d.dk)
            .all(|dk| dk.is_finite() && DK_RANGE.contains(&dk))
        {
            return Err(Error::Invalid("dielectric Dk must be 1–20".into()));
        }
        if !(self.mask_um.is_finite() && MASK_UM_RANGE.contains(&self.mask_um)) {
            return Err(Error::Invalid(
                "soldermask thickness must be 5–100 µm".into(),
            ));
        }
        Ok(())
    }
}

/// The copper count rules. Odd counts are refused: fabricators laminate
/// copper in pairs (double-sided cores and foil), so an odd board is the
/// next even build with one layer etched away, priced as that build and
/// prone to warp from the unbalanced copper.
fn check_count(count: usize) -> Result<()> {
    if count < MIN_COPPER_LAYERS {
        return Err(Error::Invalid(format!(
            "a stackup has at least {MIN_COPPER_LAYERS} copper layers"
        )));
    }
    if count > MAX_COPPER_LAYERS {
        return Err(Error::Invalid(format!(
            "a stackup has at most {MAX_COPPER_LAYERS} copper layers"
        )));
    }
    if !count.is_multiple_of(2) {
        return Err(Error::Invalid(
            "a stackup has an even number of copper layers".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "stackup_tests.rs"]
mod tests;
