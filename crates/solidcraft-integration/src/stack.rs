//! The board's physical layer stack: every film and foil the board has, in
//! the order it lies, with its height, built from the stackup alone. The 3D
//! view's layer toggles, which face lies under which body, the material
//! stream's draw order, the display films' heights and the contact shadows
//! all read it, so no other list of the layers exists.
//!
//! Heights: z = 0 at the bottom face of the lowest dielectric, as
//! [`Stackup::copper_span`] places the copper (the native model's
//! parameters follow the same rule). A height `h` on a side is measured
//! outward from that side's laminate face ([`Stack::z`]).
//!
//! The display films' own thicknesses (plated finish, legend ink) are
//! shipped data (`materials/finishes.json`, sources in
//! `docs/materials.md`), which the stack carries with the rest of the
//! loaded [`Looks`].
use crate::Result;
use crate::looks::Looks;
use crate::material::BodyKind;
use sparksmith_core::materials::MaterialLibrary;
use sparksmith_core::{CopperLayer, Layer, PadFinish, Stackup};

/// Plated finish over exposed copper, µm: the middle of the thickness
/// range a fabricator publishes for `finish` (the shipped finishes).
pub fn finish_um(looks: &MaterialLibrary, finish: PadFinish) -> f64 {
    crate::material::of_pad_finish(&looks.finishes.pad, finish)
        .thickness
        .mid_um()
}
/// The share of the ink sunk into the surface it is printed on, so it never
/// floats over it.
pub const INK_SINK_SHARE: f64 = 0.5;
/// How far a display face stands proud of the face it lies on (an edge
/// band off the routed edge, a fillet's foot off its land, a contact
/// shadow off the mask, laminate in a mask opening off the laminate), so
/// the two never fight in depth: far below what the eye resolves.
pub const PROUD_MM: f64 = 0.002;

/// One place in the stack. Films and foils lie on a side of the board;
/// component bodies and what is drawn on them stand outside the films; the
/// vias pass through all of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    /// Component bodies and leads.
    Bodies,
    /// Display films on a component body: terminations, overcoat and the
    /// solder fillets standing on the lands.
    BodyFilms,
    /// Marking decals on the bodies and their films.
    Marking,
    /// Legend ink on the mask (or the laminate where there is no mask).
    Silk(Layer),
    /// Soldermask: over the laminate and, as caps, over the copper.
    Mask(Layer),
    /// Plated finish over the copper a mask opening exposes.
    Finish(Layer),
    Copper(CopperLayer),
    /// The laminate, the copper foils embedded in it and its routed edge.
    Substrate,
    /// Plated holes and what lines and caps them, through the stack.
    Vias,
}

impl Slot {
    /// How far out from the laminate the slot lies: what the material
    /// stream draws first. A body that lies on another without covering
    /// all of its faces (a legend on the mask, a marking on a moulding)
    /// is drawn after it, so it shows wherever both pass the stream's
    /// depth test.
    pub fn rank(self) -> u8 {
        match self {
            Self::Substrate => 0,
            Self::Copper(_) | Self::Vias => 1,
            Self::Finish(_) => 2,
            Self::Mask(_) => 3,
            Self::Silk(_) => 4,
            Self::Bodies => 5,
            Self::BodyFilms => 6,
            Self::Marking => 7,
        }
    }

    /// A slot whose bodies lie on bodies of another slot and cover only
    /// part of their faces: those faces stay in the material stream, and
    /// these bodies are drawn after them ([`Slot::rank`]).
    pub fn overlays(self) -> bool {
        matches!(self, Self::Silk(_) | Self::BodyFilms | Self::Marking)
    }
}

impl BodyKind {
    /// The slot a body of this kind lies in. Films and copper read their
    /// side or layer from `layer` (a mask cap or legend without one lies
    /// on the top); copper on no single layer (a plated barrel, a via's
    /// finished ring) is the vias'.
    pub fn slot(self, layer: Option<CopperLayer>) -> Slot {
        let side = layer.and_then(CopperLayer::side).unwrap_or(Layer::Top);
        match self {
            BodyKind::Substrate
            | BodyKind::OpenLaminate
            | BodyKind::EdgeDielectric
            | BodyKind::EdgeCopper => Slot::Substrate,
            BodyKind::MaskTop => Slot::Mask(Layer::Top),
            BodyKind::MaskBottom => Slot::Mask(Layer::Bottom),
            BodyKind::MaskCap => Slot::Mask(side),
            BodyKind::Silkscreen => Slot::Silk(side),
            BodyKind::PadFinish => match layer {
                Some(layer) => layer.side().map_or(Slot::Copper(layer), Slot::Finish),
                None => Slot::Vias,
            },
            BodyKind::CopperTrack | BodyKind::PadCopper | BodyKind::FillCopper => {
                layer.map_or(Slot::Vias, Slot::Copper)
            }
            BodyKind::ViaCopper | BodyKind::ViaBore => Slot::Vias,
            BodyKind::PackageBody
            | BodyKind::ChipResistor
            | BodyKind::ChipCapacitor
            | BodyKind::PartBody
            | BodyKind::ModelBody
            | BodyKind::ModelLead => Slot::Bodies,
            BodyKind::Termination | BodyKind::Overcoat | BodyKind::Solder => Slot::BodyFilms,
            BodyKind::Marking => Slot::Marking,
        }
    }
}

/// A slot's z range in mm, low to high.
pub type Span = [f64; 2];

/// The films and foils a board has, top to bottom, each with its z span.
#[derive(Clone, Debug, PartialEq)]
pub struct Stack {
    slots: Vec<(Slot, Span)>,
    /// Each dielectric, top down: its z span and the inner foil at its top
    /// (`None` for the top dielectric, under the top foil).
    dielectrics: Vec<(Span, Option<CopperLayer>)>,
    /// The laminate's top face (its bottom face is z = 0).
    top: f64,
    /// The outer foils' thickness, top and bottom.
    copper: [f64; 2],
    /// Soldermask over copper, mm.
    mask: f64,
    /// Soldermask over bare laminate, mm.
    mask_laminate: f64,
    /// Plated finish over exposed copper, mm.
    finish: f64,
    /// The loaded material library the films' looks and thicknesses come
    /// from.
    looks: Looks,
}

/// A test fixture, never in the product: the stack of the core's fixture
/// [`Stackup::default`] with the default pad finish.
#[cfg(any(test, feature = "test-fixtures"))]
impl Stack {
    /// The stack of [`Stackup::default`] with the default pad finish.
    pub fn standard() -> Result<Self> {
        Self::of(&Stackup::default(), PadFinish::default())
    }
}

fn index(side: Layer) -> usize {
    match side {
        Layer::Top => 0,
        Layer::Bottom => 1,
    }
}

impl Stack {
    /// The stack of `stackup`: only the films it has (mask and legend per
    /// side), the `finish` on both outer foils, every copper layer at
    /// [`Stackup::copper_span`] and the laminate. The mask is the
    /// stackup's: [`Stackup::mask_laminate_nm`] over the laminate and
    /// [`Stackup::mask_um`] over copper.
    pub fn of(stackup: &Stackup, finish: PadFinish) -> Result<Self> {
        let looks = Looks::get()?;
        let foil = |layer: CopperLayer| stackup.copper_span(layer).map_or(0.0, |(a, b)| b - a);
        let layers = stackup.layers();
        let dielectrics = (0..stackup.dielectrics.len())
            .filter_map(|index| {
                let (low, high) = stackup.dielectric_span(index)?;
                let foil = layers
                    .iter()
                    .copied()
                    .find(|l| l.side().is_none() && stackup.index(*l) == Some(index));
                Some(([low, high], foil))
            })
            .collect();
        let mut stack = Self {
            slots: Vec::new(),
            dielectrics,
            top: stackup.dielectric_mm(),
            copper: [foil(CopperLayer::Top), foil(CopperLayer::Bottom)],
            mask: stackup.mask_um / 1000.0,
            mask_laminate: stackup.mask_laminate_nm.to_mm_f64(),
            finish: finish_um(&looks, finish) / 1000.0,
            looks,
        };
        let ink = stack.ink_mm();
        let films = |stack: &Stack, side: Layer| -> Vec<(Slot, Span)> {
            let (mask, silk) = match side {
                Layer::Top => (stackup.mask_top, stackup.silkscreen_top),
                Layer::Bottom => (stackup.mask_bottom, stackup.silkscreen_bottom),
            };
            let copper = stack.copper(side);
            let ground = if mask { stack.mask_laminate_mm() } else { 0.0 };
            let mut out = Vec::new();
            if silk {
                out.push((
                    Slot::Silk(side),
                    stack.span(side, ground - INK_SINK_SHARE * ink, ground + ink),
                ));
            }
            if mask {
                out.push((
                    Slot::Mask(side),
                    stack.span(
                        side,
                        0.0,
                        stack.mask_laminate_mm().max(copper + stack.mask_mm()),
                    ),
                ));
            }
            out.push((
                Slot::Finish(side),
                stack.span(side, stack.finish, copper + stack.finish),
            ));
            out
        };
        let mut slots = films(&stack, Layer::Top);
        let span = |l: &CopperLayer| {
            stackup
                .copper_span(*l)
                .map(|(a, b)| (Slot::Copper(*l), [a, b]))
        };
        slots.extend(
            layers
                .iter()
                .filter(|l| **l != CopperLayer::Bottom)
                .filter_map(span),
        );
        slots.push((Slot::Substrate, [0.0, stack.top]));
        slots.extend(
            layers
                .iter()
                .filter(|l| **l == CopperLayer::Bottom)
                .filter_map(span),
        );
        let mut below = films(&stack, Layer::Bottom);
        below.reverse();
        slots.extend(below);
        stack.slots = slots;
        Ok(stack)
    }

    /// No board: a package preview, whose bodies lie on nothing.
    pub fn none() -> Result<Self> {
        let looks = Looks::get()?;
        Ok(Self {
            slots: Vec::new(),
            dielectrics: Vec::new(),
            top: 0.0,
            copper: [0.0; 2],
            // No board, no soldermask.
            mask: 0.0,
            mask_laminate: 0.0,
            finish: finish_um(&looks, PadFinish::default()) / 1000.0,
            looks,
        })
    }

    /// The loaded material library this stack's films take their looks
    /// and thicknesses from.
    pub fn looks(&self) -> &'static MaterialLibrary {
        self.looks.0
    }

    /// Legend ink thickness, mm.
    pub fn ink_mm(&self) -> f64 {
        self.looks.finishes.legend_ink.thickness_mm
    }

    /// Every slot the board has, top to bottom, with its z span.
    pub fn slots(&self) -> &[(Slot, Span)] {
        &self.slots
    }

    /// Each dielectric of the laminate, top down: its z span and the inner
    /// copper foil lying at its top (`None` for the top dielectric). The
    /// laminate's bodies are divided by these ([`crate::scene::Geometry::dielectric`]).
    pub fn dielectrics(&self) -> &[(Span, Option<CopperLayer>)] {
        &self.dielectrics
    }

    /// The dielectric a copper foil lies in or on: an inner foil in the
    /// dielectric whose top it lies at, the top foil on the top dielectric,
    /// the bottom foil under the bottom one. `None` without a laminate.
    pub fn dielectric_of(&self, layer: CopperLayer) -> Option<usize> {
        let last = self.dielectrics.len().checked_sub(1)?;
        match layer {
            CopperLayer::Top => Some(0),
            CopperLayer::Bottom => Some(last),
            inner => self
                .dielectrics
                .iter()
                .position(|(_, foil)| *foil == Some(inner)),
        }
    }

    /// The dielectric on the outside of a side: the top one or the bottom
    /// one.
    pub fn outer_dielectric(&self, side: Layer) -> Option<usize> {
        self.dielectric_of(side.into())
    }

    /// Whether the board has `slot` (component bodies, their films and
    /// markings and the vias stand outside the stack: always).
    pub fn has(&self, slot: Slot) -> bool {
        match slot {
            Slot::Bodies | Slot::BodyFilms | Slot::Marking | Slot::Vias => true,
            slot => self.slots.iter().any(|(s, _)| *s == slot),
        }
    }

    /// A slot's z span, if the board has it.
    pub fn span_of(&self, slot: Slot) -> Option<Span> {
        self.slots
            .iter()
            .find_map(|(s, span)| (*s == slot).then_some(*span))
    }

    /// Whether a side carries soldermask.
    pub fn mask(&self, side: Layer) -> bool {
        self.has(Slot::Mask(side))
    }

    /// Soldermask over copper, mm (the stackup's [`Stackup::mask_um`]).
    pub fn mask_mm(&self) -> f64 {
        self.mask
    }

    /// Soldermask over bare laminate, mm (the stackup's
    /// [`Stackup::mask_laminate_nm`]).
    pub fn mask_laminate_mm(&self) -> f64 {
        self.mask_laminate
    }

    /// Plated finish over exposed copper, mm ([`finish_um`]).
    pub fn finish_mm(&self) -> f64 {
        self.finish
    }

    /// The outer copper foil's thickness on a side.
    pub fn copper(&self, side: Layer) -> f64 {
        self.copper[index(side)]
    }

    /// The laminate's thickness (every dielectric).
    pub fn laminate(&self) -> f64 {
        self.top
    }

    /// World z of a height outward from a side's laminate face.
    pub fn z(&self, side: Layer, height: f64) -> f64 {
        match side {
            Layer::Top => self.top + height,
            Layer::Bottom => -height,
        }
    }

    /// The z span between two heights outward from a side's laminate face.
    pub fn span(&self, side: Layer, low: f64, high: f64) -> Span {
        let (a, b) = (self.z(side, low), self.z(side, high));
        [a.min(b), a.max(b)]
    }

    /// The board's outer surface on a side, where a component's body
    /// stands over it: the mask over the laminate, or the laminate's face
    /// on a side without mask.
    pub fn surface(&self, side: Layer) -> f64 {
        let height = if self.mask(side) {
            self.mask_laminate
        } else {
            0.0
        };
        self.z(side, height)
    }

    /// The slot whose film lies on a face of a `kind` body on `layer`, if
    /// the board has it. `toward` is the side a flat face looks to; `None`
    /// is an upright face (a wall). The mask lies on the laminate; the
    /// finish on a land's copper, the mask on other outer copper (the
    /// finish where the side has no mask, which plates it all), both over
    /// its walls too; the laminate under the films and around the inner
    /// copper; the mask (or the laminate) under the legend. Laminate in a
    /// mask opening has nothing on it.
    pub fn cover(
        &self,
        kind: BodyKind,
        layer: Option<CopperLayer>,
        toward: Option<Layer>,
    ) -> Option<Slot> {
        let film = match (kind.slot(layer), toward) {
            (Slot::Substrate, Some(toward)) => match kind {
                BodyKind::Substrate => Slot::Mask(toward),
                BodyKind::OpenLaminate => {
                    let side = layer.and_then(CopperLayer::side).unwrap_or(Layer::Top);
                    if side == toward {
                        return None;
                    }
                    Slot::Substrate
                }
                _ => return None,
            },
            (Slot::Copper(copper), toward) => match copper.side() {
                Some(side) if toward.is_none_or(|t| t == side) => {
                    if kind == BodyKind::PadCopper || !self.mask(side) {
                        Slot::Finish(side)
                    } else {
                        Slot::Mask(side)
                    }
                }
                _ => Slot::Substrate,
            },
            (Slot::Mask(side), Some(toward)) if side != toward => Slot::Substrate,
            (Slot::Silk(side), Some(toward)) if side != toward => {
                if self.mask(side) {
                    Slot::Mask(side)
                } else {
                    Slot::Substrate
                }
            }
            _ => return None,
        };
        self.has(film).then_some(film)
    }

    /// Whether a `kind` body lies under another body's film on this stack
    /// ([`Stack::cover`]): a film on a face looking away from the laminate
    /// or on a wall (the laminate under the mask, copper under its mask or
    /// finish, inner copper in the laminate). A film's own face toward the
    /// board, which lies on what is under it, does not count: the film is
    /// drawn after that.
    pub fn buried(&self, kind: BodyKind, layer: Option<CopperLayer>) -> bool {
        let own = match kind.slot(layer) {
            Slot::Silk(side) | Slot::Mask(side) | Slot::Finish(side) => Some(side),
            Slot::Copper(copper) => copper.side(),
            _ => None,
        };
        [Some(Layer::Top), Some(Layer::Bottom), None]
            .into_iter()
            .filter(|face| own.is_none() || face.is_none() || *face == own)
            .any(|face| self.cover(kind, layer, face).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} against {b}");
    }

    /// Every copper slot is where the stackup puts that foil, and the films
    /// stack outward from the outer foils in physical order.
    #[test]
    fn the_stack_follows_the_stackup_in_physical_order() -> std::result::Result<(), String> {
        let _context = crate::looks::test_context().map(|c| c.enter());
        let mut stackup = Stackup::preset(6).map_err(|e| e.to_string())?;
        stackup.copper[0].thickness_um = 70.0;
        stackup.copper[2].thickness_um = 18.0;
        stackup.dielectrics[1].thickness_mm = 0.3;
        stackup.mask_um = 25.0;
        stackup.mask_laminate_nm = sparksmith_core::units::Nm::try_new(30_000).ok_or("nm")?;
        let stack = Stack::of(&stackup, PadFinish::Enig).map_err(|e| e.to_string())?;
        let order: Vec<Slot> = stack.slots().iter().map(|(s, _)| *s).collect();
        let mut expected = vec![
            Slot::Silk(Layer::Top),
            Slot::Mask(Layer::Top),
            Slot::Finish(Layer::Top),
        ];
        expected.extend(
            stackup
                .layers()
                .into_iter()
                .filter(|l| *l != CopperLayer::Bottom)
                .map(Slot::Copper),
        );
        expected.extend([
            Slot::Substrate,
            Slot::Copper(CopperLayer::Bottom),
            Slot::Finish(Layer::Bottom),
            Slot::Mask(Layer::Bottom),
            Slot::Silk(Layer::Bottom),
        ]);
        assert_eq!(order, expected);
        for layer in stackup.layers() {
            let (a, b) = stackup.copper_span(layer).ok_or("no span")?;
            let span = stack.span_of(Slot::Copper(layer)).ok_or("no slot")?;
            near(span[0], a);
            near(span[1], b);
        }
        let top = stackup.dielectric_mm();
        near(stack.span_of(Slot::Substrate).ok_or("no laminate")?[1], top);
        let mask = stack.span_of(Slot::Mask(Layer::Top)).ok_or("no mask")?;
        near(mask[1], top + 0.070 + 0.025);
        // Over the laminate the mask is the stackup's own thickness there.
        near(stack.surface(Layer::Top), top + 0.030);
        near(stack.mask_laminate_mm(), 0.030);
        let mask = stack.span_of(Slot::Mask(Layer::Bottom)).ok_or("no mask")?;
        near(mask[0], -(0.035 + 0.025));
        // Each film starts at or beyond the one under it (the legend sinks
        // into the mask by design, so it is compared by its top).
        let finish = stack.span_of(Slot::Finish(Layer::Top)).ok_or("no finish")?;
        let silk = stack.span_of(Slot::Silk(Layer::Top)).ok_or("no legend")?;
        // The default stackup's mask over copper is the stackup's own.
        near(
            Stack::standard().map_err(|e| e.to_string())?.mask_mm(),
            Stackup::default().mask_um / 1000.0,
        );
        assert!(finish[1] > top + 0.070 && silk[1] > top + 0.030);
        near(
            finish[1],
            top + 0.070 + finish_um(stack.looks(), PadFinish::Enig) / 1000.0,
        );
        // Each finish has its own thickness.
        let hasl = Stack::of(&stackup, PadFinish::Hasl).map_err(|e| e.to_string())?;
        near(hasl.finish_mm(), 0.021);
        Ok(())
    }

    /// A side without mask or legend has neither slot: its laminate and
    /// copper lie under nothing but the finish, and a legend there lies on
    /// the laminate.
    #[test]
    fn a_side_without_films_has_no_slots_for_them() -> Result<()> {
        let _context = crate::looks::test_context().map(|c| c.enter());
        let stackup = Stackup {
            mask_bottom: false,
            silkscreen_top: false,
            ..Stackup::default()
        };
        let stack = Stack::of(&stackup, PadFinish::default())?;
        assert!(stack.has(Slot::Mask(Layer::Top)));
        assert!(!stack.has(Slot::Mask(Layer::Bottom)));
        assert!(!stack.has(Slot::Silk(Layer::Top)));
        assert!(stack.has(Slot::Silk(Layer::Bottom)));
        let bottom = Some(CopperLayer::Bottom);
        let (top, under) = (Some(Layer::Top), Some(Layer::Bottom));
        assert_eq!(stack.cover(BodyKind::Substrate, None, under), None);
        assert_eq!(
            stack.cover(BodyKind::Substrate, None, top),
            Some(Slot::Mask(Layer::Top))
        );
        for face in [under, None] {
            assert_eq!(
                stack.cover(BodyKind::CopperTrack, bottom, face),
                Some(Slot::Finish(Layer::Bottom))
            );
        }
        assert_eq!(
            stack.cover(BodyKind::CopperTrack, Some(CopperLayer::Top), None),
            Some(Slot::Mask(Layer::Top))
        );
        assert_eq!(
            stack.cover(BodyKind::PadCopper, Some(CopperLayer::Top), top),
            Some(Slot::Finish(Layer::Top))
        );
        assert_eq!(
            stack.cover(BodyKind::Silkscreen, bottom, top),
            Some(Slot::Substrate)
        );
        near(stack.surface(Layer::Bottom), 0.0);
        near(
            stack.surface(Layer::Top),
            stackup.dielectric_mm() + stackup.mask_laminate_nm.to_mm_f64(),
        );
        Ok(())
    }
}
