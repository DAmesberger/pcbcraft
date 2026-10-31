//! Physical looks of the native PCB model's bodies.
//!
//! Every value is linear-sRGB reflectance with metallic/roughness/IOR in
//! Solidcraft's `OpaqueSurface` convention (glTF perceptual roughness). Metal
//! base colours are normal-incidence reflectance (F0) from measured optical
//! constants; dielectrics use F0 = ((n−1)/(n+1))². Sources, estimates and the
//! soldermask layer model are in `docs/materials.md`. No EDA tool, library or
//! material file was consulted.
use crate::{Error, Result};
use pcbcraft_core::{Fabrication, MaskColour, MaskFinish, PadFinish};
use serde::{Deserialize, Serialize};
use solidcraft_engine::geom::Vec3;
use solidcraft_engine::render::clear_coat::ClearCoat;
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
    /// A chip resistor's alumina body.
    ChipResistor,
    /// A multilayer ceramic chip capacitor's body.
    ChipCapacitor,
    /// A package's tinned termination.
    Termination,
    /// The black overcoat over a chip resistor's top.
    Overcoat,
    /// A reflowed solder fillet between a termination and its land.
    Solder,
    /// The envelope in the component authoring preview.
    PartBody,
    /// A body of a generated package model (moulding, housing, ceramic).
    /// Its material is recorded per body (`models::MODEL_BODIES_KEY`).
    ModelBody,
    /// A lead, termination or exposed pad of a generated package model.
    ModelLead,
}

impl BodyKind {
    pub const ALL: [BodyKind; 16] = [
        BodyKind::Substrate,
        BodyKind::MaskTop,
        BodyKind::MaskBottom,
        BodyKind::MaskCap,
        BodyKind::CopperTrack,
        BodyKind::PadCopper,
        BodyKind::PadFinish,
        BodyKind::PackageBody,
        BodyKind::ChipResistor,
        BodyKind::ChipCapacitor,
        BodyKind::Termination,
        BodyKind::Overcoat,
        BodyKind::Solder,
        BodyKind::PartBody,
        BodyKind::ModelBody,
        BodyKind::ModelLead,
    ];

    /// Board-level layers: hidden with the substrate.
    pub fn is_board(self) -> bool {
        matches!(
            self,
            BodyKind::Substrate | BodyKind::MaskTop | BodyKind::MaskBottom | BodyKind::MaskCap
        )
    }

    /// A package's envelope body (placed or previewed).
    pub fn is_envelope(self) -> bool {
        matches!(
            self,
            BodyKind::PackageBody
                | BodyKind::ChipResistor
                | BodyKind::ChipCapacitor
                | BodyKind::PartBody
                | BodyKind::ModelBody
        )
    }

    /// A body of a generated package model, whose look comes from its
    /// recorded material rather than from its kind.
    pub fn is_generated(self) -> bool {
        matches!(self, BodyKind::ModelBody | BodyKind::ModelLead)
    }

    /// Display-only appearance layers ([`crate::appearance`]): drawn and
    /// picked, never native bodies, so the saved model stays physically
    /// valid.
    pub fn is_appearance(self) -> bool {
        matches!(
            self,
            BodyKind::MaskTop
                | BodyKind::MaskBottom
                | BodyKind::MaskCap
                | BodyKind::PadFinish
                | BodyKind::Termination
                | BodyKind::Overcoat
                | BodyKind::Solder
        )
    }
}

/// A physically based look in Solidcraft's opaque-surface parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    pub name: &'static str,
    /// Linear-sRGB reflectance (F0 for metals, albedo for dielectrics; for a
    /// coated layer, the body under the coat).
    pub base_linear: [f64; 3],
    pub metallic: f64,
    pub roughness: f64,
    pub ior: f64,
    /// A clear lacquer over the base (soldermask), carrying the gloss.
    pub coat: Option<Coat>,
    /// Procedural detail the 3D view draws over the look (display only).
    pub detail: Detail,
}

/// A clear coat: weight, perceptual roughness and IOR, in Solidcraft's
/// `ClearCoat` convention.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coat {
    pub weight: f64,
    pub roughness: f64,
    pub ior: f64,
}

/// Procedural surface detail, drawn by the 3D view's product look from the
/// world position in millimetres. Display only: never saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Pattern {
    #[default]
    None,
    /// The laminate's glass weave (bare laminate and routed edges).
    Weave,
    /// A soldermask's orange peel (relief only).
    MaskPeel,
    /// Orange peel over a weave showing through the mask (colour only).
    MaskPeelOverWeave,
    /// Silkscreen ink grain.
    Silkscreen,
}

/// A pattern and how strongly it shows (0..=1). For
/// [`Pattern::MaskPeelOverWeave`] the strength is the weave's show-through;
/// the peel stays at full strength.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Detail {
    pub pattern: Pattern,
    pub strength: f64,
}

impl Detail {
    pub const NONE: Detail = Detail {
        pattern: Pattern::None,
        strength: 0.0,
    };
}

/// Bare copper F0, Johnson & Christy 1972 via Hoffman's table.
const COPPER: [f64; 3] = [0.955, 0.638, 0.538];
/// Natural FR-4 laminate albedo (est.).
const FR4: [f64; 3] = [0.36, 0.32, 0.17];
/// A soldermask's pigmented resin: per-pass transmittance through 20 µm,
/// and light its own pigment scatters back over a black base. Every value
/// is **est.** (see `docs/materials.md`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pigment {
    pub t20: [f64; 3],
    pub scatter: [f64; 3],
}

/// The resin of each mask colour.
pub fn pigment(colour: MaskColour) -> Pigment {
    let (t20, scatter) = match colour {
        MaskColour::Green => ([0.10, 0.45, 0.18], [0.010, 0.085, 0.030]),
        MaskColour::Blue => ([0.03, 0.10, 0.35], [0.006, 0.025, 0.110]),
        // Red and purple scatter more than green, so copper shows through
        // less (about 1.6× and 1.8× against green's 2.1×). Red is a
        // scarlet, not a maroon: most of its colour is pigment scatter.
        MaskColour::Red => ([0.30, 0.03, 0.03], [0.280, 0.014, 0.012]),
        MaskColour::Purple => ([0.14, 0.035, 0.22], [0.074, 0.008, 0.102]),
        // Lemon-ochre yellow: mostly scatter, so traces barely lighten it.
        MaskColour::Yellow => ([0.30, 0.26, 0.03], [0.600, 0.440, 0.022]),
        // Black pigment in a resin still scatters a little: a dark grey
        // body (about 2.4 % luminance), never a crushed black.
        MaskColour::Black => ([0.011, 0.012, 0.015], [0.024, 0.024, 0.026]),
        MaskColour::White => ([0.08, 0.087, 0.06], [0.70, 0.70, 0.66]),
        MaskColour::Clear => ([0.92, 0.88, 0.87], [0.005, 0.011, 0.002]),
    };
    Pigment { t20, scatter }
}

/// Perceptual roughness of a mask's lacquer surface, the clear coat that
/// carries its gloss (est. from glossmeter readings: gloss masks 80–94 GU,
/// matte 3–43). Clear resin has no pigment to break the surface; the yellow
/// fabricators sell is semi-gloss at best; black matte is the flattest.
pub fn mask_coat_roughness(colour: MaskColour, finish: MaskFinish) -> f64 {
    match (finish, colour) {
        (MaskFinish::Gloss, MaskColour::Clear) => 0.05,
        (MaskFinish::Gloss, MaskColour::Yellow) => 0.12,
        (MaskFinish::Gloss, _) => 0.06,
        (MaskFinish::Satin, _) => 0.20,
        (MaskFinish::Matte, MaskColour::Black) => 0.45,
        (MaskFinish::Matte, _) => 0.40,
    }
}

/// How much of the laminate's glass weave shows through a mask colour
/// (est.): fully under clear resin, faintly under the translucent
/// pigments, not at all under the opaque ones (black, white, yellow: the
/// pigments needing the most exposure energy, see `docs/materials.md`).
pub fn weave_show_through(colour: MaskColour) -> f64 {
    match colour {
        MaskColour::Clear => 0.6,
        MaskColour::Green => 0.12,
        MaskColour::Purple => 0.10,
        MaskColour::Blue | MaskColour::Red => 0.08,
        MaskColour::Yellow | MaskColour::Black | MaskColour::White => 0.0,
    }
}

/// The procedural detail of `kind` on a board built to `fabrication`.
pub fn detail(kind: BodyKind, fabrication: &Fabrication) -> Detail {
    match kind {
        BodyKind::Substrate => Detail {
            pattern: Pattern::Weave,
            strength: 1.0,
        },
        BodyKind::MaskTop | BodyKind::MaskBottom => {
            match weave_show_through(fabrication.mask_colour) {
                0.0 => Detail {
                    pattern: Pattern::MaskPeel,
                    strength: 1.0,
                },
                strength => Detail {
                    pattern: Pattern::MaskPeelOverWeave,
                    strength,
                },
            }
        }
        BodyKind::MaskCap => Detail {
            pattern: Pattern::MaskPeel,
            strength: 1.0,
        },
        _ => Detail::NONE,
    }
}

/// Internal diffuse reflectance of the mask/air interface for n ≈ 1.52
/// (first principles; Saunderson's k₂).
const MASK_INTERNAL: f64 = 0.60;
/// Normal-incidence reflectance of the mask surface, ((1.52−1)/(1.52+1))².
const MASK_F0: f64 = 0.043;
const MASK_IOR: f64 = 1.52;
/// The pigmented body under a mask's lacquer has no interface of its own:
/// the coat reflects at the air boundary, so the body takes IOR 1 and a
/// fully rough, diffuse surface.
const MASK_BODY_IOR: f64 = 1.0;
const MASK_BODY_ROUGHNESS: f64 = 1.0;
/// Soldermask over bare laminate (20 µm) and over a track's crown (12 µm;
/// LPI thins over copper edges and crowns to 10–15 µm), in µm.
pub const MASK_OVER_LAMINATE_UM: f64 = 20.0;
pub const MASK_OVER_COPPER_UM: f64 = 12.0;
/// Silica-filled epoxy and glass-epoxy laminate index (handbook values).
const EPOXY_IOR: f64 = 1.55;
/// Metals ignore the dielectric IOR; Solidcraft still requires one in [1, 3].
const METAL_IOR: f64 = 1.5;
/// Tin F0 of a hot-air levelled pad, flat across the visible (est.; below
/// polished tin, for its oxide film; Palik's Sn n, k to confirm).
const TIN: [f64; 3] = [0.64, 0.64, 0.66];
/// Tin F0 of plated terminations and reflowed solder joints (est.; the
/// core's package tin, `materials::MaterialId::TinMatte`).
const TIN_PLATED: [f64; 3] = [0.78, 0.78, 0.80];
/// ENIG pad F0 (est.): the gold F0 (1.000, 0.782, 0.344; J&C/Hoffman)
/// with a little nickel showing through the 0.04–0.10 µm immersion gold,
/// kept saturated so the finish reads gold, not khaki.
const ENIG: [f64; 3] = [1.0, 0.73, 0.30];
/// Eutectic tin-lead (Sn63Pb37) F0: about 30 % lead by volume, which
/// reflects less than tin (est.).
const TIN_LEAD: [f64; 3] = [0.62, 0.62, 0.63];
/// Silver F0 (Hoffman's table, from measured optical constants).
const SILVER: [f64; 3] = [0.972, 0.960, 0.915];
/// 96 % alumina substrate diffuse reflectance (est. from published
/// 400–700 nm reflectance of thick-film substrates, 0.75–0.85).
const ALUMINA: [f64; 3] = [0.78, 0.77, 0.73];
/// Alumina refractive index (sapphire, n_o = 1.77; handbook value).
const ALUMINA_IOR: f64 = 1.77;
/// Sintered barium-titanate MLCC body, tan (est.).
const MLCC: [f64; 3] = [0.32, 0.25, 0.15];
/// Barium titanate is n ≈ 2.4; Solidcraft's dielectric IOR is at most 3.
const MLCC_IOR: f64 = 2.4;

/// Reflectance of a `pigment` layer `thickness_um` thick over a diffuse
/// substrate of reflectance `under` (two-flux layer with a Saunderson
/// interface correction; see `docs/materials.md`):
///
/// ρ = ρ_scatter + (1 − F0)(1 − ρ_i) · T²R / (1 − ρ_i R),  T = exp(−σd),
/// σ = −ln(T₂₀) / 20 µm.
pub fn mask_over(pigment: Pigment, under: [f64; 3], thickness_um: f64) -> [f64; 3] {
    std::array::from_fn(|c| {
        let sigma = -pigment.t20[c].ln() / 20.0;
        let t = (-sigma * thickness_um).exp();
        let r = under[c];
        let body = (1.0 - MASK_F0) * (1.0 - MASK_INTERNAL) * t * t * r / (1.0 - MASK_INTERNAL * r);
        (pigment.scatter[c] + body).clamp(0.0, 1.0)
    })
}

/// A mask layer's body under its coat: [`mask_over`] with the air-to-resin
/// Fresnel transmission taken out, since the coat applies it (counted once).
pub fn mask_body(pigment: Pigment, under: [f64; 3], thickness_um: f64) -> [f64; 3] {
    mask_over(pigment, under, thickness_um).map(|c| (c / (1.0 - MASK_F0)).clamp(0.0, 1.0))
}

/// The physical look of `kind` on a board built to `fabrication`. Only the
/// display-only layers (mask and plated finish) depend on it; the bodies the
/// native model saves look the same for every fabrication.
pub fn material(kind: BodyKind, fabrication: &Fabrication) -> Material {
    let mut look = base_material(kind, fabrication);
    look.detail = detail(kind, fabrication);
    look
}

fn base_material(kind: BodyKind, fabrication: &Fabrication) -> Material {
    let pigment = pigment(fabrication.mask_colour);
    let lacquer = Coat {
        weight: 1.0,
        roughness: mask_coat_roughness(fabrication.mask_colour, fabrication.mask_finish),
        ior: MASK_IOR,
    };
    let metal = |name, base_linear, roughness| Material {
        name,
        base_linear,
        metallic: 1.0,
        roughness,
        ior: METAL_IOR,
        coat: None,
        detail: Detail::NONE,
    };
    let dielectric = |name, base_linear, roughness, ior| Material {
        name,
        base_linear,
        metallic: 0.0,
        roughness,
        ior,
        coat: None,
        detail: Detail::NONE,
    };
    let mask = |name, base_linear| Material {
        coat: Some(lacquer),
        ..dielectric(name, base_linear, MASK_BODY_ROUGHNESS, MASK_BODY_IOR)
    };
    match kind {
        BodyKind::Substrate => dielectric("FR-4 laminate", FR4, 0.60, EPOXY_IOR),
        BodyKind::MaskTop | BodyKind::MaskBottom => mask(
            "Soldermask over laminate",
            mask_body(pigment, FR4, MASK_OVER_LAMINATE_UM),
        ),
        BodyKind::MaskCap => mask(
            "Soldermask over copper",
            mask_body(pigment, COPPER, MASK_OVER_COPPER_UM),
        ),
        BodyKind::CopperTrack | BodyKind::PadCopper => metal("Copper", COPPER, 0.30),
        BodyKind::PadFinish => match fabrication.pad_finish {
            // 85 % Au F0 (1.000, 0.782, 0.344) over 15 % Ni F0 (0.660, 0.609,
            // 0.526): the immersion gold is only 1–2 penetration depths thick.
            PadFinish::Enig => metal("ENIG gold", ENIG, 0.28),
            // Eutectic tin-lead freezes bright; lead-free tin alloys freeze
            // duller and grainier.
            PadFinish::Hasl => metal("HASL tin-lead", TIN_LEAD, 0.16),
            PadFinish::LeadFreeHasl => metal("Lead-free HASL tin", TIN, 0.20),
            // A sub-micrometre transparent film over micro-etched copper.
            PadFinish::Osp => metal("OSP copper", COPPER, 0.35),
            // Immersion silver is many optical penetration depths thick.
            PadFinish::ImmersionSilver => metal("Immersion silver", SILVER, 0.20),
        },
        // Generated bodies take their recorded material's look; these are
        // only the family defaults' stand-ins.
        BodyKind::Termination | BodyKind::ModelLead => metal("Matte tin", TIN_PLATED, 0.40),
        // Reflowed lead-free SAC solder freezes satin, not mirror-bright.
        BodyKind::Solder => metal("Reflowed solder", TIN_PLATED, 0.38),
        BodyKind::PackageBody | BodyKind::PartBody | BodyKind::ModelBody => dielectric(
            "Epoxy mould compound",
            [0.022, 0.022, 0.024],
            0.45,
            EPOXY_IOR,
        ),
        BodyKind::ChipResistor => dielectric("Alumina ceramic", ALUMINA, 0.55, ALUMINA_IOR),
        BodyKind::ChipCapacitor => dielectric("MLCC ceramic", MLCC, 0.55, MLCC_IOR),
        // Glass or epoxy overcoat with a black marking layer.
        BodyKind::Overcoat => {
            dielectric("Resistor overcoat", [0.025, 0.025, 0.027], 0.30, EPOXY_IOR)
        }
    }
}

impl Material {
    /// Solidcraft's validated surface for this look.
    pub fn surface(&self) -> Result<OpaqueSurface> {
        let invalid = |error: solidcraft_engine::render::surface::SurfaceError| {
            Error::Invalid(format!("{}: {error}", self.name))
        };
        let [r, g, b] = self.base_linear;
        let surface =
            OpaqueSurface::new(Vec3::new(r, g, b), self.metallic, self.roughness, self.ior)
                .map_err(invalid)?;
        match self.coat {
            Some(c) => Ok(surface
                .with_clear_coat(ClearCoat::new(c.weight, c.roughness, c.ior).map_err(invalid)?)),
            None => Ok(surface),
        }
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
    use pcbcraft_core::SilkscreenColour;

    const COLOURS: [MaskColour; 8] = [
        MaskColour::Green,
        MaskColour::Blue,
        MaskColour::Red,
        MaskColour::Purple,
        MaskColour::Yellow,
        MaskColour::Black,
        MaskColour::White,
        MaskColour::Clear,
    ];
    const FINISHES: [MaskFinish; 3] = [MaskFinish::Gloss, MaskFinish::Satin, MaskFinish::Matte];
    const PADS: [PadFinish; 5] = [
        PadFinish::Enig,
        PadFinish::Hasl,
        PadFinish::LeadFreeHasl,
        PadFinish::Osp,
        PadFinish::ImmersionSilver,
    ];

    /// Every fabrication combination the document can hold.
    fn fabrications() -> Vec<Fabrication> {
        let mut out = Vec::new();
        for mask_colour in COLOURS {
            for mask_finish in FINISHES {
                for pad_finish in PADS {
                    for silkscreen_colour in [
                        SilkscreenColour::White,
                        SilkscreenColour::Black,
                        SilkscreenColour::Yellow,
                    ] {
                        out.push(Fabrication {
                            mask_colour,
                            mask_finish,
                            pad_finish,
                            silkscreen_colour,
                        });
                    }
                }
            }
        }
        out
    }

    fn default_look(kind: BodyKind) -> Material {
        material(kind, &Fabrication::default())
    }

    fn luminance(c: [f64; 3]) -> f64 {
        0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
    }

    /// Copper show-through: mask over copper against mask over laminate.
    fn show_through(colour: MaskColour) -> f64 {
        luminance(mask_over(pigment(colour), COPPER, MASK_OVER_COPPER_UM))
            / luminance(mask_over(pigment(colour), FR4, MASK_OVER_LAMINATE_UM))
    }

    #[test]
    fn every_look_is_a_valid_opaque_surface() -> Result<()> {
        let all = fabrications();
        assert_eq!(all.len(), 8 * 3 * 5 * 3);
        for fabrication in &all {
            for kind in BodyKind::ALL {
                let m = material(kind, fabrication);
                let surface = m.surface()?;
                assert!(surface.roughness() >= 0.045, "{kind:?} {fabrication:?}");
                assert!(
                    m.base_linear.iter().all(|c| (0.0..=1.0).contains(c)),
                    "{kind:?} {fabrication:?}"
                );
            }
        }
        Ok(())
    }

    /// The saved native model is physical: only display-only layers change
    /// with the fabrication.
    #[test]
    fn saved_bodies_look_the_same_for_every_fabrication() {
        for fabrication in fabrications() {
            for kind in BodyKind::ALL {
                if !kind.is_appearance() {
                    assert_eq!(material(kind, &fabrication), default_look(kind), "{kind:?}");
                }
            }
        }
    }

    #[test]
    fn metals_are_metallic_with_their_documented_f0() {
        let cu = default_look(BodyKind::CopperTrack);
        assert_eq!(cu.metallic, 1.0);
        assert_eq!(cu.base_linear, [0.955, 0.638, 0.538]);
        assert_eq!(default_look(BodyKind::PadCopper), cu);
        let pad = |pad_finish| {
            let m = material(
                BodyKind::PadFinish,
                &Fabrication {
                    pad_finish,
                    ..Fabrication::default()
                },
            );
            (m.metallic, m.base_linear, m.roughness)
        };
        assert_eq!(pad(PadFinish::Enig), (1.0, [1.0, 0.73, 0.30], 0.28));
        assert_eq!(pad(PadFinish::Hasl), (1.0, [0.62, 0.62, 0.63], 0.16));
        assert_eq!(
            pad(PadFinish::LeadFreeHasl),
            (1.0, [0.64, 0.64, 0.66], 0.20)
        );
        assert_eq!(pad(PadFinish::Osp), (1.0, COPPER, 0.35));
        assert_eq!(
            pad(PadFinish::ImmersionSilver),
            (1.0, [0.972, 0.960, 0.915], 0.20)
        );
        for kind in [BodyKind::Termination, BodyKind::Solder] {
            let tin = default_look(kind);
            assert_eq!(
                (tin.metallic, tin.base_linear),
                (1.0, TIN_PLATED),
                "{kind:?}"
            );
            // Satin, never a mirror.
            assert!(tin.roughness >= 0.3, "{kind:?}");
        }
        for kind in [
            BodyKind::Substrate,
            BodyKind::MaskTop,
            BodyKind::MaskCap,
            BodyKind::PackageBody,
            BodyKind::ChipResistor,
            BodyKind::ChipCapacitor,
            BodyKind::Overcoat,
        ] {
            assert_eq!(default_look(kind).metallic, 0.0, "{kind:?}");
        }
    }

    #[test]
    fn srgb8_round_trips_within_one_step() {
        for fabrication in fabrications() {
            for kind in BodyKind::ALL {
                let m = material(kind, &fabrication);
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
        }
        assert_eq!(linear_to_srgb8(f64::NAN), 0);
        assert_eq!(linear_to_srgb8(2.0), 255);
    }

    #[test]
    fn soldermask_is_green_and_lighter_over_copper() {
        let green = pigment(MaskColour::Green);
        let laminate = mask_over(green, FR4, MASK_OVER_LAMINATE_UM);
        let copper = mask_over(green, COPPER, MASK_OVER_COPPER_UM);
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
        assert_eq!(mask_over(green, [0.0; 3], 20.0), [0.010, 0.085, 0.030]);
        // A perfect reflector under a vanishing layer stays bounded.
        assert!(mask_over(green, [1.0; 3], 0.0).iter().all(|c| *c <= 1.0));
        // The coat applies the air-to-resin Fresnel, so the body under it
        // is the layer's reflectance with that taken out once.
        let body = |albedo: [f64; 3]| albedo.map(|c| c / (1.0 - MASK_F0));
        assert_eq!(default_look(BodyKind::MaskTop).base_linear, body(laminate));
        assert_eq!(default_look(BodyKind::MaskCap).base_linear, body(copper));
        let coat = default_look(BodyKind::MaskTop).coat;
        assert_eq!(coat.map(|c| c.roughness), Some(0.06));
    }

    /// The documented colour table (`docs/materials.md`), to three decimals.
    #[test]
    fn mask_colours_match_the_documented_table() {
        let table = [
            (
                MaskColour::Green,
                [0.012, 0.116, 0.032],
                [0.064, 0.237, 0.069],
            ),
            (
                MaskColour::Blue,
                [0.006, 0.027, 0.119],
                [0.019, 0.050, 0.196],
            ),
            (
                MaskColour::Red,
                [0.296, 0.014, 0.012],
                [0.482, 0.020, 0.017],
            ),
            (
                MaskColour::Purple,
                [0.077, 0.008, 0.106],
                [0.155, 0.015, 0.151],
            ),
            (
                MaskColour::Yellow,
                [0.616, 0.450, 0.022],
                [0.802, 0.519, 0.027],
            ),
            (
                MaskColour::Black,
                [0.024, 0.024, 0.026],
                [0.028, 0.026, 0.028],
            ),
            (
                MaskColour::White,
                [0.701, 0.701, 0.660],
                [0.741, 0.721, 0.670],
            ),
            (
                MaskColour::Clear,
                [0.154, 0.128, 0.057],
                [0.780, 0.350, 0.259],
            ),
        ];
        for (colour, laminate, copper) in table {
            let fabrication = Fabrication {
                mask_colour: colour,
                ..Fabrication::default()
            };
            let p = pigment(colour);
            for (kind, expected, got) in [
                (
                    BodyKind::MaskTop,
                    laminate,
                    mask_over(p, FR4, MASK_OVER_LAMINATE_UM),
                ),
                (
                    BodyKind::MaskCap,
                    copper,
                    mask_over(p, COPPER, MASK_OVER_COPPER_UM),
                ),
            ] {
                let body = material(kind, &fabrication).base_linear;
                for c in 0..3 {
                    assert!(
                        (got[c] - expected[c]).abs() <= 0.0006,
                        "{colour:?} {kind:?}: {got:?} vs {expected:?}"
                    );
                    assert!(
                        (body[c] * (1.0 - MASK_F0) - got[c]).abs() < 1e-12,
                        "{colour:?} {kind:?}: the body under the coat"
                    );
                }
            }
            assert_eq!(
                material(BodyKind::MaskBottom, &fabrication),
                material(BodyKind::MaskTop, &fabrication)
            );
        }
    }

    /// Each colour reads as its name, copper always lightens it, and the
    /// show-through order follows the cited observations: clear shows the
    /// most, red, purple and blue less than green, and yellow, black and
    /// white least.
    #[test]
    fn mask_colours_keep_their_hue_and_show_through_order() {
        for colour in COLOURS {
            let p = pigment(colour);
            let laminate = mask_over(p, FR4, MASK_OVER_LAMINATE_UM);
            let copper = mask_over(p, COPPER, MASK_OVER_COPPER_UM);
            for c in 0..3 {
                assert!(copper[c] > laminate[c], "{colour:?}");
            }
            let [r, g, b] = laminate;
            let hue = match colour {
                MaskColour::Green => g > 2.0 * r && g > 2.0 * b,
                MaskColour::Blue => b > 2.0 * g && g > r,
                MaskColour::Red => r > 5.0 * g && r > 5.0 * b,
                MaskColour::Purple => r > 5.0 * g && b > 5.0 * g,
                MaskColour::Yellow => r > 10.0 * b && g > 10.0 * b,
                // Dark grey, neither crushed to black nor washed out.
                MaskColour::Black => (0.015..0.035).contains(&luminance(laminate)),
                MaskColour::White => luminance(laminate) > 0.6,
                MaskColour::Clear => r > g && g > b,
            };
            assert!(hue, "{colour:?}: {laminate:?}");
        }
        let green = show_through(MaskColour::Green);
        assert!(show_through(MaskColour::Clear) > 3.0);
        // Red is cited as showing traces less than green; purple and blue
        // are estimates (docs/materials.md, Trace contrast).
        for colour in [MaskColour::Red, MaskColour::Purple, MaskColour::Blue] {
            let ratio = show_through(colour);
            assert!(ratio < green - 0.2 && ratio > 1.5, "{colour:?}: {ratio}");
        }
        for colour in [MaskColour::Yellow, MaskColour::Black] {
            assert!(show_through(colour) < 1.4, "{colour:?}");
        }
        assert!(show_through(MaskColour::White) < 1.1);
    }

    /// The mask's gloss is its lacquer coat's roughness; the body under it
    /// is diffuse whatever the finish.
    #[test]
    fn mask_finish_sets_the_coat_roughness() -> Result<()> {
        for colour in COLOURS {
            let look = |mask_finish| {
                material(
                    BodyKind::MaskTop,
                    &Fabrication {
                        mask_colour: colour,
                        mask_finish,
                        ..Fabrication::default()
                    },
                )
            };
            let rough = |finish| look(finish).coat.map_or(f64::NAN, |c| c.roughness);
            let (gloss, satin, matte) = (
                rough(MaskFinish::Gloss),
                rough(MaskFinish::Satin),
                rough(MaskFinish::Matte),
            );
            assert!(gloss < satin && satin < matte, "{colour:?}");
            assert!(
                (0.045..=0.12).contains(&gloss) && matte >= 0.40,
                "{colour:?}"
            );
            for finish in FINISHES {
                let m = look(finish);
                assert_eq!((m.roughness, m.ior), (1.0, 1.0), "{colour:?} {finish:?}");
            }
        }
        Ok(())
    }

    /// Every mask layer is a pigmented body under a full lacquer coat of
    /// the resin's index, which Solidcraft's surface carries; nothing else
    /// is coated, so the saved bodies stay uncoated.
    #[test]
    fn mask_layers_are_coated_and_nothing_else_is() -> Result<()> {
        for fabrication in fabrications() {
            for kind in BodyKind::ALL {
                let m = material(kind, &fabrication);
                let coat = m.surface()?.clear_coat();
                let is_mask = matches!(
                    kind,
                    BodyKind::MaskTop | BodyKind::MaskBottom | BodyKind::MaskCap
                );
                assert_eq!(coat.is_some(), is_mask, "{kind:?}");
                if let Some(coat) = coat {
                    assert_eq!((coat.weight(), coat.ior()), (1.0, MASK_IOR));
                    assert_eq!(m.metallic, 0.0);
                }
            }
        }
        Ok(())
    }

    /// The weave shows on bare laminate and edges, fully under clear mask,
    /// faintly under the translucent colours and not at all under black,
    /// white or yellow; every mask shows orange peel; nothing else has a
    /// pattern.
    #[test]
    fn patterns_follow_the_mask_colour() {
        let detail_of = |kind, colour| {
            material(
                kind,
                &Fabrication {
                    mask_colour: colour,
                    ..Fabrication::default()
                },
            )
            .detail
        };
        for colour in COLOURS {
            assert_eq!(
                detail_of(BodyKind::Substrate, colour),
                Detail {
                    pattern: Pattern::Weave,
                    strength: 1.0
                }
            );
            assert_eq!(
                detail_of(BodyKind::MaskCap, colour),
                Detail {
                    pattern: Pattern::MaskPeel,
                    strength: 1.0
                }
            );
            let mask = detail_of(BodyKind::MaskTop, colour);
            assert_eq!(mask, detail_of(BodyKind::MaskBottom, colour));
            match colour {
                MaskColour::Clear => assert_eq!(
                    mask,
                    Detail {
                        pattern: Pattern::MaskPeelOverWeave,
                        strength: 0.6
                    }
                ),
                MaskColour::Black | MaskColour::White | MaskColour::Yellow => assert_eq!(
                    mask,
                    Detail {
                        pattern: Pattern::MaskPeel,
                        strength: 1.0
                    }
                ),
                _ => {
                    assert_eq!(mask.pattern, Pattern::MaskPeelOverWeave, "{colour:?}");
                    assert!(
                        (0.05..=0.2).contains(&mask.strength),
                        "{colour:?}: faint, {}",
                        mask.strength
                    );
                }
            }
            for kind in BodyKind::ALL {
                if !matches!(
                    kind,
                    BodyKind::Substrate
                        | BodyKind::MaskTop
                        | BodyKind::MaskBottom
                        | BodyKind::MaskCap
                ) {
                    assert_eq!(detail_of(kind, colour), Detail::NONE, "{kind:?}");
                }
            }
        }
        // Clear shows the weave most; green, the most transparent pigment,
        // next.
        for colour in COLOURS {
            assert!(weave_show_through(MaskColour::Clear) >= weave_show_through(colour));
            if colour != MaskColour::Clear {
                assert!(weave_show_through(MaskColour::Green) >= weave_show_through(colour));
            }
        }
    }
}
