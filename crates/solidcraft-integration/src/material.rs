//! Physical looks of the native PCB model's bodies.
//!
//! Every value is linear-sRGB reflectance with metallic/roughness/IOR in
//! Solidcraft's `OpaqueSurface` convention (glTF perceptual roughness). Metal
//! base colours are normal-incidence reflectance (F0) from measured optical
//! constants; dielectrics use F0 = ((n−1)/(n+1))². Sources, estimates and the
//! soldermask layer model are in `docs/materials.md`. No EDA tool, library or
//! material file was consulted.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use solidcraft_engine::geom::Vec3;
use solidcraft_engine::render::surface::OpaqueSurface;

/// What a native body represents, stored per body in the model metadata so a
/// reopened file renders and picks the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    /// FR-4 laminate core (its routed edge is what shows).
    Substrate,
    /// Soldermask over bare laminate, top side.
    MaskTop,
    /// Soldermask over bare laminate, bottom side.
    MaskBottom,
    /// Soldermask conforming over a copper track.
    MaskCap,
    /// Track copper (under the mask).
    CopperTrack,
    /// Pad copper (inside its finish).
    PadCopper,
    /// Surface finish plated over an exposed pad.
    PadFinish,
    /// A placed package's moulded envelope.
    PackageBody,
    /// A package's tinned termination.
    Termination,
    /// The envelope in the component authoring preview.
    PartBody,
}

impl BodyKind {
    pub const ALL: [BodyKind; 10] = [
        BodyKind::Substrate,
        BodyKind::MaskTop,
        BodyKind::MaskBottom,
        BodyKind::MaskCap,
        BodyKind::CopperTrack,
        BodyKind::PadCopper,
        BodyKind::PadFinish,
        BodyKind::PackageBody,
        BodyKind::Termination,
        BodyKind::PartBody,
    ];

    /// Board-level appearance layers: hidden with the substrate.
    pub fn is_board(self) -> bool {
        matches!(
            self,
            BodyKind::Substrate | BodyKind::MaskTop | BodyKind::MaskBottom | BodyKind::MaskCap
        )
    }
}

/// Exposed-copper surface finish.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadFinish {
    /// Electroless nickel, immersion gold (IPC-4552).
    Enig,
    /// Hot-air solder levelling (tin-based solder).
    Hasl,
}

/// The finish used until projects carry their own fabrication choice.
pub const DEFAULT_FINISH: PadFinish = PadFinish::Enig;

/// A physically based look in Solidcraft's opaque-surface parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    pub name: &'static str,
    /// Linear-sRGB reflectance (F0 for metals, albedo for dielectrics).
    pub base_linear: [f64; 3],
    pub metallic: f64,
    pub roughness: f64,
    pub ior: f64,
}

/// Bare copper F0, Johnson & Christy 1972 via Hoffman's table.
const COPPER: [f64; 3] = [0.955, 0.638, 0.538];
/// Natural FR-4 laminate albedo (est.).
const FR4: [f64; 3] = [0.36, 0.32, 0.17];
/// Soldermask transmittance through 20 µm of green LPI resin (est.).
const MASK_T20: [f64; 3] = [0.10, 0.45, 0.18];
/// Light scattered back by the mask's own pigment over a black base (est.).
const MASK_SCATTER: [f64; 3] = [0.010, 0.085, 0.030];
/// Internal diffuse reflectance of the mask/air interface for n ≈ 1.52
/// (first principles; Saunderson's k₂).
const MASK_INTERNAL: f64 = 0.60;
/// Normal-incidence reflectance of the mask surface, ((1.52−1)/(1.52+1))².
const MASK_F0: f64 = 0.043;
const MASK_IOR: f64 = 1.52;
/// Soldermask over bare laminate (20 µm) and over copper (15 µm), in µm.
pub const MASK_OVER_LAMINATE_UM: f64 = 20.0;
pub const MASK_OVER_COPPER_UM: f64 = 15.0;
/// Glossy LPI (gloss "L" texture code) as perceptual roughness (est.).
const MASK_ROUGHNESS: f64 = 0.20;
/// Silica-filled epoxy and glass-epoxy laminate index (handbook values).
const EPOXY_IOR: f64 = 1.55;
/// Metals ignore the dielectric IOR; Solidcraft still requires one in [1, 3].
const METAL_IOR: f64 = 1.5;

/// Reflectance of a soldermask layer `thickness_um` thick over a diffuse
/// substrate of reflectance `under` (two-flux layer with a Saunderson
/// interface correction; see `docs/materials.md`):
///
/// ρ = ρ_scatter + (1 − F0)(1 − ρ_i) · T²R / (1 − ρ_i R),  T = exp(−σd),
/// σ = −ln(T₂₀) / 20 µm.
pub fn mask_over(under: [f64; 3], thickness_um: f64) -> [f64; 3] {
    std::array::from_fn(|c| {
        let sigma = -MASK_T20[c].ln() / 20.0;
        let t = (-sigma * thickness_um).exp();
        let r = under[c];
        let body = (1.0 - MASK_F0) * (1.0 - MASK_INTERNAL) * t * t * r / (1.0 - MASK_INTERNAL * r);
        (MASK_SCATTER[c] + body).clamp(0.0, 1.0)
    })
}

/// The physical look of `kind` with exposed pads finished in `finish`.
pub fn material(kind: BodyKind, finish: PadFinish) -> Material {
    let metal = |name, base_linear, roughness| Material {
        name,
        base_linear,
        metallic: 1.0,
        roughness,
        ior: METAL_IOR,
    };
    let dielectric = |name, base_linear, roughness, ior| Material {
        name,
        base_linear,
        metallic: 0.0,
        roughness,
        ior,
    };
    match kind {
        BodyKind::Substrate => dielectric("FR-4 laminate", FR4, 0.60, EPOXY_IOR),
        BodyKind::MaskTop | BodyKind::MaskBottom => dielectric(
            "Soldermask over laminate",
            mask_over(FR4, MASK_OVER_LAMINATE_UM),
            MASK_ROUGHNESS,
            MASK_IOR,
        ),
        BodyKind::MaskCap => dielectric(
            "Soldermask over copper",
            mask_over(COPPER, MASK_OVER_COPPER_UM),
            MASK_ROUGHNESS,
            MASK_IOR,
        ),
        BodyKind::CopperTrack | BodyKind::PadCopper => metal("Copper", COPPER, 0.30),
        BodyKind::PadFinish => match finish {
            // 85 % Au F0 (1.000, 0.782, 0.344) over 15 % Ni F0 (0.660, 0.609,
            // 0.526): the immersion gold is only 1–2 penetration depths thick.
            PadFinish::Enig => metal("ENIG gold", [0.95, 0.76, 0.37], 0.25),
            PadFinish::Hasl => metal("HASL tin", [0.70, 0.70, 0.72], 0.25),
        },
        BodyKind::Termination => metal("Matte tin", [0.68, 0.68, 0.70], 0.40),
        BodyKind::PackageBody | BodyKind::PartBody => dielectric(
            "Epoxy mould compound",
            [0.022, 0.022, 0.024],
            0.45,
            EPOXY_IOR,
        ),
    }
}

impl Material {
    /// Solidcraft's validated surface for this look.
    pub fn surface(&self) -> Result<OpaqueSurface> {
        let [r, g, b] = self.base_linear;
        OpaqueSurface::new(Vec3::new(r, g, b), self.metallic, self.roughness, self.ior)
            .map_err(|error| Error::Invalid(format!("{}: {error}", self.name)))
    }

    /// The base colour as stored in a native look (sRGB, 8 bits).
    pub fn srgb8(&self) -> [u8; 3] {
        self.base_linear.map(linear_to_srgb8)
    }
}

/// The sRGB transfer function, rounded to 8 bits; out-of-range input clamps.
pub fn linear_to_srgb8(linear: f64) -> u8 {
    let c = if linear.is_finite() {
        linear.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let encoded = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    let scaled = (encoded * 255.0).round().clamp(0.0, 255.0);
    // In 0..=255 after the clamp, so the conversion is exact.
    scaled as u8
}

/// The inverse of [`linear_to_srgb8`].
pub fn srgb8_to_linear(value: u8) -> f64 {
    let c = f64::from(value) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_look_is_a_valid_opaque_surface() -> Result<()> {
        for kind in BodyKind::ALL {
            for finish in [PadFinish::Enig, PadFinish::Hasl] {
                let m = material(kind, finish);
                let surface = m.surface()?;
                assert!(surface.roughness() >= 0.045, "{kind:?}");
            }
        }
        Ok(())
    }

    #[test]
    fn metals_are_metallic_with_their_documented_f0() {
        let cu = material(BodyKind::CopperTrack, DEFAULT_FINISH);
        assert_eq!(cu.metallic, 1.0);
        assert_eq!(cu.base_linear, [0.955, 0.638, 0.538]);
        assert_eq!(material(BodyKind::PadCopper, DEFAULT_FINISH), cu);
        let enig = material(BodyKind::PadFinish, PadFinish::Enig);
        assert_eq!((enig.metallic, enig.base_linear), (1.0, [0.95, 0.76, 0.37]));
        let hasl = material(BodyKind::PadFinish, PadFinish::Hasl);
        assert_eq!((hasl.metallic, hasl.base_linear), (1.0, [0.70, 0.70, 0.72]));
        let tin = material(BodyKind::Termination, DEFAULT_FINISH);
        assert_eq!((tin.metallic, tin.base_linear), (1.0, [0.68, 0.68, 0.70]));
        for kind in [
            BodyKind::Substrate,
            BodyKind::MaskTop,
            BodyKind::MaskCap,
            BodyKind::PackageBody,
        ] {
            assert_eq!(material(kind, DEFAULT_FINISH).metallic, 0.0, "{kind:?}");
        }
    }

    #[test]
    fn srgb8_round_trips_within_one_step() {
        for kind in BodyKind::ALL {
            let m = material(kind, DEFAULT_FINISH);
            for (c, linear) in m.srgb8().into_iter().zip(m.base_linear) {
                let back = srgb8_to_linear(c);
                let step =
                    srgb8_to_linear(c.saturating_add(1)) - srgb8_to_linear(c.saturating_sub(1));
                assert!(
                    (back - linear).abs() <= step,
                    "{kind:?}: {linear} → {c} → {back}"
                );
                assert!((f64::from(linear_to_srgb8(back)) - f64::from(c)).abs() <= 1.0);
            }
        }
        assert_eq!(linear_to_srgb8(f64::NAN), 0);
        assert_eq!(linear_to_srgb8(2.0), 255);
    }

    #[test]
    fn soldermask_is_green_and_lighter_over_copper() {
        let laminate = mask_over(FR4, MASK_OVER_LAMINATE_UM);
        let copper = mask_over(COPPER, MASK_OVER_COPPER_UM);
        for albedo in [laminate, copper] {
            assert!(albedo.iter().all(|c| (0.0..=1.0).contains(c)), "{albedo:?}");
            assert!(
                albedo[1] > 2.0 * albedo[0] && albedo[1] > 2.0 * albedo[2],
                "{albedo:?}"
            );
        }
        for c in 0..3 {
            assert!(copper[c] > laminate[c], "{copper:?} vs {laminate:?}");
        }
        // Over a black base only the pigment scatter remains.
        assert_eq!(mask_over([0.0; 3], 20.0), MASK_SCATTER);
        // A perfect reflector under a vanishing layer stays bounded.
        assert!(mask_over([1.0; 3], 0.0).iter().all(|c| *c <= 1.0));
        assert_eq!(
            material(BodyKind::MaskTop, DEFAULT_FINISH).base_linear,
            laminate
        );
        assert_eq!(
            material(BodyKind::MaskCap, DEFAULT_FINISH).base_linear,
            copper
        );
    }
}
