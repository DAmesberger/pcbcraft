//! Physical looks of the native PCB model's bodies.
//!
//! Every value is linear-sRGB reflectance with metallic/roughness/IOR in
//! Solidcraft's `OpaqueSurface` convention (glTF perceptual roughness). Metal
//! base colours are normal-incidence reflectance (F0) from measured optical
//! constants; dielectrics use F0 = ((n−1)/(n+1))². Sources, estimates and the
//! soldermask layer model are in `docs/materials.md`. No EDA tool, library or
//! material file was consulted.
//!
//! The values are shipped data (`materials/board-looks.json` and
//! `finishes.json`), carried by the board's [`Stack`] from the loaded
//! [`crate::looks`]; this module maps body kinds and fabrication choices
//! onto them and computes the soldermask layer model.
use crate::geom::Vec3;
use crate::stack::Stack;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use solidcraft_viewport::clear_coat::ClearCoat;
use solidcraft_viewport::surface::OpaqueSurface;
use sparksmith_core::materials::{
    Look, Mask, MaskPigment, MaterialLibrary, PerMaskColour, PerPadFinish,
};
use sparksmith_core::{Fabrication, MaskColour, MaskFinish, PadFinish, SilkscreenColour};

/// What a native body represents, stored per body in the model metadata so a
/// reopened file renders and picks the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    /// FR-4 laminate core (its routed edge is what shows).
    Substrate,
    /// The laminate a soldermask opening leaves bare (display-only): the
    /// laminate's own face lies under the mask, so where the mask is open
    /// this film draws it.
    OpenLaminate,
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
    /// A body of a generated package model (moulding, case, ceramic).
    /// Its material is recorded per body (`models::MODEL_BODIES_KEY`).
    ModelBody,
    /// A lead, termination or exposed pad of a generated package model.
    ModelLead,
    /// A via's plated barrel and its annular rings (saved copper).
    ViaCopper,
    /// Legend ink film on the mask (display-only).
    Silkscreen,
    /// A dielectric band of the stackup at the routed board edge
    /// (display-only).
    EdgeDielectric,
    /// A copper foil's line at the routed board edge (display-only).
    EdgeCopper,
    /// The shadowed inside of an open via's drill (display-only). The 3D
    /// view computes no occlusion, so a narrow plated bore would otherwise
    /// light up like an open copper face; a real one reads as a dark hole.
    ViaBore,
    /// An instance marking decal on a package face: top mark, value code,
    /// polarity mark or colour bands (display-only, founder decision F7;
    /// never saved, never in fabrication output). Its look is the
    /// marking finish's, set where the decal is made.
    Marking,
    /// A copper fill's computed copper on one layer (saved copper, under
    /// the mask).
    FillCopper,
}

impl BodyKind {
    pub const ALL: [BodyKind; 24] = [
        BodyKind::Substrate,
        BodyKind::OpenLaminate,
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
        BodyKind::ViaCopper,
        BodyKind::Silkscreen,
        BodyKind::EdgeDielectric,
        BodyKind::EdgeCopper,
        BodyKind::ViaBore,
        BodyKind::Marking,
        BodyKind::FillCopper,
    ];

    /// Board-level layers: the laminate and the mask over it
    /// ([`crate::stack::Slot`]).
    pub fn is_board(self) -> bool {
        use crate::stack::Slot;
        matches!(self.slot(None), Slot::Substrate | Slot::Mask(_))
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
            BodyKind::OpenLaminate
                | BodyKind::MaskTop
                | BodyKind::MaskBottom
                | BodyKind::MaskCap
                | BodyKind::PadFinish
                | BodyKind::Termination
                | BodyKind::Overcoat
                | BodyKind::Solder
                | BodyKind::Silkscreen
                | BodyKind::EdgeDielectric
                | BodyKind::EdgeCopper
                | BodyKind::ViaBore
                | BodyKind::Marking
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

/// A soldermask's pigmented resin ([`MaskPigment`]): per-pass
/// transmittance through 20 µm, and light its own pigment scatters back
/// over a black base, from the shipped board looks.
pub type Pigment = MaskPigment;

/// One value of a per-colour record.
fn of_colour<T>(values: &PerMaskColour<T>, colour: MaskColour) -> &T {
    match colour {
        MaskColour::Green => &values.green,
        MaskColour::Blue => &values.blue,
        MaskColour::Red => &values.red,
        MaskColour::Purple => &values.purple,
        MaskColour::Yellow => &values.yellow,
        MaskColour::Black => &values.black,
        MaskColour::White => &values.white,
        MaskColour::Clear => &values.clear,
    }
}

/// One value of a per-finish record.
pub fn of_pad_finish<T>(values: &PerPadFinish<T>, finish: PadFinish) -> &T {
    match finish {
        PadFinish::Enig => &values.enig,
        PadFinish::Hasl => &values.hasl,
        PadFinish::LeadFreeHasl => &values.lead_free_hasl,
        PadFinish::Osp => &values.osp,
        PadFinish::ImmersionSilver => &values.immersion_silver,
    }
}

/// The resin of each mask colour.
pub fn pigment(looks: &MaterialLibrary, colour: MaskColour) -> Pigment {
    *of_colour(&looks.board.mask.colours, colour)
}

/// Perceptual roughness of a mask's lacquer surface, the clear coat that
/// carries its gloss (values and sources in the shipped board looks).
pub fn mask_coat_roughness(looks: &MaterialLibrary, colour: MaskColour, finish: MaskFinish) -> f64 {
    let by_finish = &looks.board.mask.coat_roughness;
    let by_colour = match finish {
        MaskFinish::Gloss => &by_finish.gloss,
        MaskFinish::Satin => &by_finish.satin,
        MaskFinish::Matte => &by_finish.matte,
    };
    *of_colour(by_colour, colour)
}

/// How much of the laminate's glass weave shows through a mask colour.
pub fn weave_show_through(looks: &MaterialLibrary, colour: MaskColour) -> f64 {
    pigment(looks, colour).weave_show_through
}

/// The procedural detail of `kind` on a board built to `fabrication`.
pub fn detail(kind: BodyKind, fabrication: &Fabrication, looks: &MaterialLibrary) -> Detail {
    match kind {
        BodyKind::Substrate | BodyKind::OpenLaminate | BodyKind::EdgeDielectric => Detail {
            pattern: Pattern::Weave,
            strength: 1.0,
        },
        BodyKind::MaskTop | BodyKind::MaskBottom => {
            match weave_show_through(looks, fabrication.mask_colour) {
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
        BodyKind::Silkscreen => Detail {
            pattern: Pattern::Silkscreen,
            strength: looks.finishes.legend_ink.grain,
        },
        _ => Detail::NONE,
    }
}

/// Reflectance of a `pigment` layer `thickness_um` thick over a diffuse
/// substrate of reflectance `under` (two-flux layer with a Saunderson
/// interface correction; see `docs/materials.md`):
///
/// ρ = ρ_scatter + (1 − F0)(1 − ρ_i) · T²R / (1 − ρ_i R),  T = exp(−σd),
/// σ = −ln(T₂₀) / 20 µm.
pub fn mask_over(mask: &Mask, pigment: Pigment, under: [f64; 3], thickness_um: f64) -> [f64; 3] {
    std::array::from_fn(|c| {
        let sigma = -pigment.t20[c].ln() / 20.0;
        let t = (-sigma * thickness_um).exp();
        let r = under[c];
        let internal = mask.internal_reflectance;
        let body = (1.0 - mask.surface_f0) * (1.0 - internal) * t * t * r / (1.0 - internal * r);
        (pigment.scatter[c] + body).clamp(0.0, 1.0)
    })
}

/// A mask layer's body under its coat: [`mask_over`] with the air-to-resin
/// Fresnel transmission taken out, since the coat applies it (counted once).
pub fn mask_body(mask: &Mask, pigment: Pigment, under: [f64; 3], thickness_um: f64) -> [f64; 3] {
    mask_over(mask, pigment, under, thickness_um)
        .map(|c| (c / (1.0 - mask.surface_f0)).clamp(0.0, 1.0))
}

/// The physical look of `kind` on a board built to `fabrication` with
/// `stack`'s films. Only the display-only layers (mask and plated finish)
/// depend on them; the bodies the native model saves look the same for
/// every fabrication. A mask's colour depends on its thickness: the
/// stack's mask over laminate, and the stackup's mask over copper.
pub fn material(kind: BodyKind, fabrication: &Fabrication, stack: &Stack) -> Material {
    let looks = stack.looks();
    let mut look = base_material(kind, fabrication, stack, looks);
    look.detail = detail(kind, fabrication, looks);
    look
}

/// Soldermask over bare laminate and over copper, in µm: the stackup's
/// two thicknesses, as the 3D films draw them.
pub fn mask_um(stack: &Stack) -> [f64; 2] {
    [stack.mask_laminate_mm() * 1000.0, stack.mask_mm() * 1000.0]
}

/// A shipped look in this crate's look type.
fn plain(look: &'static Look) -> Material {
    Material {
        name: &look.name,
        base_linear: look.base_linear,
        metallic: look.metallic,
        roughness: look.roughness,
        ior: look.ior,
        coat: None,
        detail: Detail::NONE,
    }
}

fn base_material(
    kind: BodyKind,
    fabrication: &Fabrication,
    stack: &Stack,
    looks: &'static MaterialLibrary,
) -> Material {
    let board = &looks.board;
    let mask_look = &board.mask;
    let [over_laminate, over_copper] = mask_um(stack);
    let pigment = pigment(looks, fabrication.mask_colour);
    let lacquer = Coat {
        weight: 1.0,
        roughness: mask_coat_roughness(looks, fabrication.mask_colour, fabrication.mask_finish),
        ior: mask_look.ior,
    };
    let mask = |name: &'static str, base_linear| Material {
        name,
        base_linear,
        metallic: 0.0,
        roughness: mask_look.body_roughness,
        ior: mask_look.body_ior,
        coat: Some(lacquer),
        detail: Detail::NONE,
    };
    match kind {
        BodyKind::Substrate | BodyKind::OpenLaminate => plain(&board.laminate),
        BodyKind::MaskTop | BodyKind::MaskBottom => mask(
            &mask_look.over_laminate_name,
            mask_body(
                mask_look,
                pigment,
                board.laminate.base_linear,
                over_laminate,
            ),
        ),
        BodyKind::MaskCap => mask(
            &mask_look.over_copper_name,
            mask_body(mask_look, pigment, mask_look.copper_under, over_copper),
        ),
        BodyKind::CopperTrack
        | BodyKind::PadCopper
        | BodyKind::ViaCopper
        | BodyKind::FillCopper
        | BodyKind::EdgeCopper => plain(&board.copper),
        BodyKind::EdgeDielectric => plain(&board.laminate_edge),
        BodyKind::ViaBore => plain(&board.via_bore),
        BodyKind::Silkscreen => {
            let ink = &looks.finishes.legend_ink;
            Material {
                name: &ink.name,
                base_linear: match fabrication.silkscreen_colour {
                    SilkscreenColour::White => ink.colours.white,
                    SilkscreenColour::Black => ink.colours.black,
                    SilkscreenColour::Yellow => ink.colours.yellow,
                },
                metallic: 0.0,
                roughness: ink.roughness,
                ior: ink.ior,
                coat: None,
                detail: Detail::NONE,
            }
        }
        BodyKind::PadFinish => {
            plain(&of_pad_finish(&looks.finishes.pad, fabrication.pad_finish).look)
        }
        // Generated bodies take their recorded material's look; these are
        // only the family defaults' stand-ins.
        BodyKind::Termination | BodyKind::ModelLead => plain(&board.termination),
        BodyKind::Solder => plain(&board.solder),
        BodyKind::PackageBody | BodyKind::PartBody | BodyKind::ModelBody => {
            plain(&board.package_body)
        }
        BodyKind::ChipResistor => plain(&board.chip_resistor),
        BodyKind::ChipCapacitor => plain(&board.chip_capacitor),
        BodyKind::Overcoat => plain(&board.overcoat),
        // A stand-in only: a decal takes its finish's cited look
        // (`marking::finish_surface`).
        BodyKind::Marking => plain(&board.marking),
    }
}

impl Material {
    /// Solidcraft's validated surface for this look.
    pub fn surface(&self) -> Result<OpaqueSurface> {
        let invalid = |error: solidcraft_viewport::surface::SurfaceError| {
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
    use sparksmith_core::SilkscreenColour;

    /// The shipped values these tests pin (`library/materials/`).
    const COPPER: [f64; 3] = [0.955, 0.638, 0.538];
    const FR4: [f64; 3] = [0.36, 0.32, 0.17];
    const TIN_PLATED: [f64; 3] = [0.78, 0.78, 0.80];
    const MASK_F0: f64 = 0.043;
    const MASK_IOR: f64 = 1.52;
    const SILK_GRAIN: f64 = 0.5;

    /// The repository's material library.
    #[allow(clippy::expect_used)]
    fn lib() -> &'static MaterialLibrary {
        crate::looks::get().expect("the repository's library/materials loads")
    }

    #[allow(clippy::expect_used)]
    fn stack() -> Stack {
        Stack::standard().expect("the repository's library/materials loads")
    }

    fn pigment(colour: MaskColour) -> Pigment {
        super::pigment(lib(), colour)
    }

    fn mask_over(pigment: Pigment, under: [f64; 3], thickness_um: f64) -> [f64; 3] {
        super::mask_over(&lib().board.mask, pigment, under, thickness_um)
    }

    fn weave_show_through(colour: MaskColour) -> f64 {
        super::weave_show_through(lib(), colour)
    }

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

    /// The default stack's mask over laminate and over copper, µm.
    fn over_laminate() -> f64 {
        mask_um(&stack())[0]
    }

    fn over_copper() -> f64 {
        mask_um(&stack())[1]
    }

    fn default_look(kind: BodyKind) -> Material {
        material(kind, &Fabrication::default(), &stack())
    }

    fn luminance(c: [f64; 3]) -> f64 {
        0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
    }

    /// Copper show-through: mask over copper against mask over laminate.
    fn show_through(colour: MaskColour) -> f64 {
        luminance(mask_over(pigment(colour), COPPER, over_copper()))
            / luminance(mask_over(pigment(colour), FR4, over_laminate()))
    }

    #[test]
    fn every_look_is_a_valid_opaque_surface() -> Result<()> {
        let all = fabrications();
        assert_eq!(all.len(), 8 * 3 * 5 * 3);
        for fabrication in &all {
            for kind in BodyKind::ALL {
                let m = material(kind, fabrication, &stack());
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
                    assert_eq!(
                        material(kind, &fabrication, &stack()),
                        default_look(kind),
                        "{kind:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn metals_are_metallic_with_their_documented_f0() {
        let cu = default_look(BodyKind::CopperTrack);
        assert_eq!(cu.metallic, 1.0);
        assert_eq!(cu.base_linear, [0.955, 0.61, 0.41]);
        assert_eq!(default_look(BodyKind::PadCopper), cu);
        let pad = |pad_finish| {
            let m = material(
                BodyKind::PadFinish,
                &Fabrication {
                    pad_finish,
                    ..Fabrication::default()
                },
                &stack(),
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
                let m = material(kind, &fabrication, &stack());
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
        let laminate = mask_over(green, FR4, over_laminate());
        let copper = mask_over(green, COPPER, over_copper());
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
        assert_eq!(coat.map(|c| c.roughness), Some(0.16));
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
                    mask_over(p, FR4, over_laminate()),
                ),
                (
                    BodyKind::MaskCap,
                    copper,
                    mask_over(p, COPPER, over_copper()),
                ),
            ] {
                let body = material(kind, &fabrication, &stack()).base_linear;
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
                material(BodyKind::MaskBottom, &fabrication, &stack()),
                material(BodyKind::MaskTop, &fabrication, &stack())
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
            let laminate = mask_over(p, FR4, over_laminate());
            let copper = mask_over(p, COPPER, over_copper());
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
                    &stack(),
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
                (0.10..=0.20).contains(&gloss) && matte >= 0.40,
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
                let m = material(kind, &fabrication, &stack());
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
                &stack(),
            )
            .detail
        };
        for colour in COLOURS {
            for kind in [BodyKind::Substrate, BodyKind::EdgeDielectric] {
                assert_eq!(
                    detail_of(kind, colour),
                    Detail {
                        pattern: Pattern::Weave,
                        strength: 1.0
                    },
                    "{kind:?}"
                );
            }
            assert_eq!(
                detail_of(BodyKind::Silkscreen, colour),
                Detail {
                    pattern: Pattern::Silkscreen,
                    strength: SILK_GRAIN
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
                        | BodyKind::OpenLaminate
                        | BodyKind::EdgeDielectric
                        | BodyKind::Silkscreen
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
