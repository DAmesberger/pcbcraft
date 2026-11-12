//! The board plan: every flat body of the board (laminate slabs, pad,
//! track, via and fill copper) as pure data, each a prism of 2D loops
//! between two heights: its kind, the document object it comes from, its
//! layer, its z span, its loops and its look.
//!
//! The plan is the one description of the board's flat bodies. The exact
//! native build ([`super::build`]) makes each body from it, adding the
//! parameters and joints that keep the saved model editable without
//! Sparksmith; nothing in the build adds, drops or reshapes a flat body.
//! `cargo xtask board-parity` checks the build against the plan on every
//! example board.
//!
//! The plan reads only the document: no native session, no files, so it
//! builds for the web as the rest of this crate does.
use super::fill::{self, MODEL_CHORD_MM, TILE_MM, Tile};
use super::{
    MAX_DRILLS, PlacedPad, mm_slug, other, placed_pads, polygon, track_outline, without_nested,
};
use crate::material::{self, BodyKind, Material};
use crate::pad_outline::{self, Contour, FLATTEN_TOLERANCE_MM};
use crate::{Error, Result};
use pcbcraft_core::library::PadCopper;
use pcbcraft_core::model::fill::FilledRegion;
use pcbcraft_core::{CopperLayer, Fabrication, Id, Layer, PadId, Point, Project, ViaSpan};

/// How far a built body's box may lie from its plan body's, mm: the
/// kernel bounds a curved face by its tessellation and the plan an arc by
/// its points at a tenth of [`FLATTEN_TOLERANCE_MM`]. Measured on the
/// ESP32-S31 example: 0.2 µm at most (a pad's rounded land).
pub const PARITY_MM: f64 = 1e-3;

/// How far, as a fraction, a built body's volume may lie from its plan
/// body's (exact areas, [`Contour::area`]): the kernel measures curved
/// faces on refined tessellations. Measured on the ESP32-S31 example:
/// 7e-8 at most (a fill tile).
pub const PARITY_VOLUME: f64 = 1e-6;

/// Where a body's loops lie on the board: turned by `angle_deg` about
/// its origin and moved to `origin`, mirrored across its x axis first when
/// `flipped` (a body held on the bottom face, turned over).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: Point,
    pub angle_deg: f64,
    pub flipped: bool,
}

impl Frame {
    /// The board's own frame.
    pub const BOARD: Self = Self {
        origin: [0.0, 0.0],
        angle_deg: 0.0,
        flipped: false,
    };

    /// A point of the body's frame on the board.
    pub fn apply(&self, p: Point) -> Point {
        let q = if self.flipped { [p[0], -p[1]] } else { p };
        let (c, s) = pcbcraft_core::geometry::cos_sin(self.angle_deg);
        [
            self.origin[0] + c * q[0] - s * q[1],
            self.origin[1] + s * q[0] + c * q[1],
        ]
    }

    /// A contour of the body's frame on the board.
    pub fn contour(&self, contour: &Contour) -> Contour {
        contour.mapped(|p| self.apply(p), self.flipped)
    }
}

/// Which part of a placed pad a body is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadPart {
    /// The land on the pad's own side.
    Land,
    /// A through-hole pad's land on the other side.
    Opposite,
    /// The plated barrel of the pad's hole (0) or of each of its vias.
    Barrel(usize),
}

/// Which part of a board via a body is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViaPart {
    Barrel,
    Ring(CopperLayer),
}

/// Which slab of the laminate a body is, counted from the top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// `dielectrics[index]`, whole.
    Dielectric(usize),
    /// The level of an inner foil at the top of `dielectrics[index]`:
    /// laminate where the layer has no copper.
    Foil { index: usize, layer: CopperLayer },
    /// `dielectrics[index]` below the foil at its top.
    UnderFoil(usize),
}

/// The document object a flat body comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Laminate(Level),
    Pad {
        instance: Id,
        pad: PadId,
        part: PadPart,
    },
    Track(Id),
    Via {
        via: Id,
        part: ViaPart,
    },
    Fill {
        fill: Id,
        column: i64,
        row: i64,
        piece: usize,
    },
}

/// One flat body: `loops` (in `frame`, the first the outer boundary, the
/// rest holes and the islands in them, nested even-odd as a sketch takes
/// them) extruded from `z[0]` to `z[1]` (board heights, mm, z = 0 at the
/// laminate's bottom face).
#[derive(Clone, Debug, PartialEq)]
pub struct PlanBody {
    /// The native body's name (a via's bodies are named by its size's
    /// definition, which every via of that size places).
    pub name: String,
    pub kind: BodyKind,
    pub source: Origin,
    pub net: Option<Id>,
    /// The copper layer the body lies on; `None` for one spanning the
    /// board (a slab, a barrel).
    pub layer: Option<CopperLayer>,
    pub z: [f64; 2],
    pub frame: Frame,
    pub loops: Vec<Contour>,
    /// The physical look the saved body carries.
    pub look: Material,
}

impl PlanBody {
    /// A body of every field but its look, which its kind gives.
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        kind: BodyKind,
        source: Origin,
        net: Option<Id>,
        layer: Option<CopperLayer>,
        z: (f64, f64),
        frame: Frame,
        loops: Vec<Contour>,
    ) -> Self {
        Self {
            name,
            kind,
            source,
            net,
            layer,
            z: [z.0, z.1],
            frame,
            loops,
            look: look(kind),
        }
    }

    /// The loops on the board.
    pub fn board_loops(&self) -> Vec<Contour> {
        self.loops.iter().map(|c| self.frame.contour(c)).collect()
    }

    /// The box round the body, low and high corner.
    pub fn bounds(&self) -> Result<([f64; 3], [f64; 3])> {
        let outer = self
            .loops
            .first()
            .ok_or_else(|| Error::Verification(format!("plan body {} has no loop", self.name)))?;
        let (lo, hi) = self
            .frame
            .contour(outer)
            .bounds(FLATTEN_TOLERANCE_MM / 10.0)?;
        Ok(([lo[0], lo[1], self.z[0]], [hi[0], hi[1], self.z[1]]))
    }

    /// The body's volume, mm³, where its loops say it without a Boolean:
    /// an outer boundary round disjoint holes (every copper body). A
    /// laminate slab's holes may cross (a via ring through a track's cut),
    /// so it states none.
    pub fn volume(&self) -> Option<f64> {
        if self.kind == BodyKind::Substrate {
            return None;
        }
        let (outer, holes) = self.loops.split_first()?;
        let area = outer.area() - holes.iter().map(Contour::area).sum::<f64>();
        Some(area * (self.z[1] - self.z[0]))
    }
}

/// The look a saved body of `kind` carries: the same for every
/// fabrication (only display-only layers take the project's mask and
/// finish).
fn look(kind: BodyKind) -> Material {
    material::material(
        kind,
        &Fabrication::default(),
        &crate::stack::Stack::default(),
    )
}

/// A copper fill's tiles.
#[derive(Clone, Debug)]
pub struct FillPlan {
    pub fill: Id,
    pub net: Option<Id>,
    pub layer: CopperLayer,
    /// The fill's regions simplified for the model, as the display layers
    /// draw over them.
    pub regions: Vec<FilledRegion>,
    /// Each tile's body and its region.
    pub tiles: Vec<(PlanBody, FilledRegion)>,
}

/// One placed pad's copper: none for a pad without copper.
#[derive(Clone, Debug)]
pub struct PadPlan {
    pub instance: Id,
    pub pad: PadId,
    pub bodies: Vec<PlanBody>,
}

#[derive(Clone, Debug)]
pub struct TrackPlan {
    pub track: Id,
    pub body: PlanBody,
}

/// A board via's barrel and rings, in the via's frame (its centre at the
/// origin).
#[derive(Clone, Debug)]
pub struct ViaPlan {
    pub via: Id,
    pub bodies: Vec<PlanBody>,
}

/// The board's flat bodies, grouped by the objects they come from in the
/// order the native build makes them.
#[derive(Clone, Debug)]
pub struct BoardPlan {
    pub fills: Vec<FillPlan>,
    /// One per placed pad, in [`pcbcraft_core::layout::placed_pads`] order.
    pub pads: Vec<PadPlan>,
    pub tracks: Vec<TrackPlan>,
    pub vias: Vec<ViaPlan>,
    /// The slabs, top down.
    pub laminate: Vec<PlanBody>,
}

impl BoardPlan {
    /// Every body: each fill tile, pad, track and via body, then the slabs.
    pub fn bodies(&self) -> impl Iterator<Item = &PlanBody> {
        self.fills
            .iter()
            .flat_map(|f| f.tiles.iter().map(|(body, _)| body))
            .chain(self.pads.iter().flat_map(|p| &p.bodies))
            .chain(self.tracks.iter().map(|t| &t.body))
            .chain(self.vias.iter().flat_map(|v| &v.bodies))
            .chain(&self.laminate)
    }

    /// The laminate as the live view draws it: one body from the bottom
    /// slab's bottom face to the top slab's top, its loops those of every
    /// slab that is not a foil level (the board outline less every drill
    /// and every cut through the board; those slabs hold the same loops).
    /// It differs from the slabs only in each inner foil level, where the
    /// slabs leave room for that layer's copper; drawn whole, that copper
    /// lies inside it. The exact build makes the slabs ([`Self::laminate`]),
    /// so its saved bodies stay disjoint. `None` for a plan without
    /// laminate.
    pub fn whole_laminate(&self) -> Result<Option<PlanBody>> {
        let mut through = self
            .laminate
            .iter()
            .filter(|s| !matches!(s.source, Origin::Laminate(Level::Foil { .. })));
        let Some(first) = through.next() else {
            return if self.laminate.is_empty() {
                Ok(None)
            } else {
                Err(Error::Verification(
                    "the laminate has only foil levels".into(),
                ))
            };
        };
        if let Some(other) = through.find(|s| s.loops != first.loops) {
            return Err(Error::Verification(format!(
                "laminate slabs {} and {} are drilled differently",
                first.name, other.name
            )));
        }
        let low = self
            .laminate
            .iter()
            .map(|s| s.z[0])
            .fold(f64::MAX, f64::min);
        let high = self
            .laminate
            .iter()
            .map(|s| s.z[1])
            .fold(f64::MIN, f64::max);
        Ok(Some(PlanBody {
            name: "Substrate".to_owned(),
            z: [low, high],
            ..first.clone()
        }))
    }
}

/// The plan of `project`'s board. Refused (as the build refuses it) when
/// the document does not validate, a via is blind or buried, a pad with
/// copper has no outline, a track has no length or the board holds more
/// drills than [`MAX_DRILLS`].
pub fn of(project: &Project) -> Result<BoardPlan> {
    pcbcraft_core::validate::project(project).map_err(|e| Error::Invalid(e.to_string()))?;
    project
        .stackup
        .check()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let pads = placed_pads(project)?;
    of_pads(project, &pads)
}

/// [`of`] for the board's placed pads.
pub(super) fn of_pads(project: &Project, pads: &[PlacedPad<'_>]) -> Result<BoardPlan> {
    if project
        .vias
        .iter()
        .any(|v| !matches!(v.span, ViaSpan::Through))
    {
        return Err(Error::Invalid(
            "blind and buried vias are not built in the native model yet".into(),
        ));
    }
    let fills = fills(project)?;
    let laminate = laminate(project, pads, &fills)?;
    Ok(BoardPlan {
        pads: pads
            .iter()
            .map(|pad| pad_plan(project, pad))
            .collect::<Result<_>>()?,
        tracks: tracks(project)?,
        vias: vias(project)?,
        fills,
        laminate,
    })
}

/// A copper layer's z span.
fn span(project: &Project, layer: CopperLayer) -> Result<(f64, f64)> {
    project
        .stackup
        .copper_span(layer)
        .ok_or_else(|| Error::Invalid(format!("the stackup has no {} layer", layer.name())))
}

/// The laminate's thickness: every dielectric.
fn board_thickness(project: &Project) -> f64 {
    project.stackup.dielectric_mm()
}

/// Each fill with copper, one body per tile of its regions
/// ([`fill::tiles`]).
fn fills(project: &Project) -> Result<Vec<FillPlan>> {
    let mut out = Vec::new();
    for (fill, regions) in fill::built(project) {
        if regions.iter().all(|r| fill::loops(r).is_empty()) {
            continue;
        }
        let z = span(project, fill.layer)?;
        let name = fill::body_name(fill);
        let raw = fill
            .computed
            .as_ref()
            .map(|c| c.regions.as_slice())
            .unwrap_or_default();
        let mut tiles = Vec::new();
        for Tile {
            column,
            row,
            piece,
            region,
        } in fill::tiles(raw, TILE_MM, MODEL_CHORD_MM)
        {
            let contours: Vec<Contour> = fill::loops(&region)
                .into_iter()
                .filter_map(polygon)
                .collect();
            if contours.is_empty() {
                continue;
            }
            let body = PlanBody::new(
                format!("{name}_{column}_{row}_{piece}"),
                BodyKind::FillCopper,
                Origin::Fill {
                    fill: fill.id,
                    column,
                    row,
                    piece,
                },
                fill.net,
                Some(fill.layer),
                z,
                Frame::BOARD,
                contours,
            );
            tiles.push((body, region));
        }
        out.push(FillPlan {
            fill: fill.id,
            net: fill.net,
            layer: fill.layer,
            regions,
            tiles,
        });
    }
    Ok(out)
}

/// A placed pad's copper: its land, a through pad's opposite land and
/// its plated barrels, in the frame of the pad's component (at the pad
/// origin, turned with the placement, turned over on the bottom).
fn pad_plan(project: &Project, pad: &PlacedPad<'_>) -> Result<PadPlan> {
    let local = pad.pad();
    let copper = local.kind.copper();
    let instance = pad.placed.instance;
    let mut plan = PadPlan {
        instance,
        pad: local.id,
        bodies: Vec::new(),
    };
    if copper == PadCopper::None {
        return Ok(plan);
    }
    let Some(shape) = pad_outline::shape_contour(&local.shape, 0.0) else {
        return Err(Error::Invalid(format!(
            "{} pad {} has no copper outline",
            pad.designator, local.id
        )));
    };
    let near: CopperLayer = pad.placed.layer.into();
    let far: CopperLayer = other(pad.placed.layer).into();
    let frame = Frame {
        origin: pad.origin,
        angle_deg: pad.placement.rotation_deg,
        flipped: pad.placed.layer == Layer::Bottom,
    };
    let pp = format!("pad_{}_{}", instance, local.id);
    let net = pad.placed.net;
    let source = |part| Origin::Pad {
        instance,
        pad: local.id,
        part,
    };
    let mut land = vec![pad.local(&shape)];
    land.extend(pad.bores().iter().map(|c| pad.local(c)));
    plan.bodies.push(PlanBody::new(
        pp.clone(),
        BodyKind::PadCopper,
        source(PadPart::Land),
        net,
        Some(near),
        span(project, near)?,
        frame,
        land.clone(),
    ));
    if copper == PadCopper::Through {
        plan.bodies.push(PlanBody::new(
            format!("{pp}_opposite"),
            BodyKind::PadCopper,
            source(PadPart::Opposite),
            net,
            Some(far),
            span(project, far)?,
            frame,
            land,
        ));
    }
    for (index, [drill, bore]) in pad.barrels().iter().enumerate() {
        plan.bodies.push(PlanBody::new(
            format!("{pp}_barrel_{index}"),
            BodyKind::PadCopper,
            source(PadPart::Barrel(index)),
            net,
            None,
            (0.0, board_thickness(project)),
            frame,
            vec![pad.local(drill), pad.local(bore)],
        ));
    }
    Ok(plan)
}

/// Each track's copper: its stadium in the track's frame (from its start,
/// along +x).
fn tracks(project: &Project) -> Result<Vec<TrackPlan>> {
    project
        .tracks
        .iter()
        .map(|track| {
            let d = [track.end[0] - track.start[0], track.end[1] - track.start[1]];
            let length = d[0].hypot(d[1]);
            let contour = polygon(&track_outline([0.0, 0.0], [length, 0.0], track.width))
                .ok_or_else(|| Error::Invalid(format!("track {} has no length", track.id)))?;
            let frame = Frame {
                origin: track.start,
                angle_deg: d[1].atan2(d[0]).to_degrees(),
                flipped: track.layer == CopperLayer::Bottom,
            };
            Ok(TrackPlan {
                track: track.id,
                body: PlanBody::new(
                    format!("trace_{}", track.id),
                    BodyKind::CopperTrack,
                    Origin::Track(track.id),
                    Some(track.net),
                    Some(track.layer),
                    span(project, track.layer)?,
                    frame,
                    vec![contour],
                ),
            })
        })
        .collect()
}

/// Each via's plated barrel and its annular ring on every layer: outer
/// rings cap the barrel; inner rings stand round it, in the circle the
/// laminate's foil level leaves for them. A via without a bore (no drill)
/// has neither barrel nor outer rings.
fn vias(project: &Project) -> Result<Vec<ViaPlan>> {
    let layers = project.stackup.layers();
    let circle = |radius| Contour::Circle {
        center: [0.0, 0.0],
        radius,
    };
    project
        .vias
        .iter()
        .map(|via| {
            let name = format!("via_{}_{}", mm_slug(via.drill), mm_slug(via.diameter));
            let drill = pad_outline::plated_drill_radius(via.drill);
            let bore = via.drill / 2.0;
            let ring = via.diameter / 2.0;
            let frame = Frame {
                origin: via.position,
                ..Frame::BOARD
            };
            let body = |name, part, layer, z, loops| {
                PlanBody::new(
                    name,
                    BodyKind::ViaCopper,
                    Origin::Via { via: via.id, part },
                    Some(via.net),
                    layer,
                    z,
                    frame,
                    loops,
                )
            };
            let mut bodies = Vec::new();
            if bore > 0.0 {
                bodies.push(body(
                    format!("{name}_barrel"),
                    ViaPart::Barrel,
                    None,
                    (0.0, board_thickness(project)),
                    vec![circle(drill), circle(bore)],
                ));
            }
            for layer in layers.iter().filter(|l| via.on(**l, &layers)) {
                if bore <= 0.0 && layer.side().is_some() {
                    continue;
                }
                let hole = if layer.side().is_some() { bore } else { drill };
                bodies.push(body(
                    format!("{name}_ring_{}", super::layer_slug(*layer)),
                    ViaPart::Ring(*layer),
                    Some(*layer),
                    span(project, *layer)?,
                    vec![circle(ring), circle(hole)],
                ));
            }
            Ok(ViaPlan {
                via: via.id,
                bodies,
            })
        })
        .collect()
}

/// The laminate as the stack it is made of, top down: each dielectric a
/// slab of the board outline with every drill as a hole, and where an
/// inner foil lies at the top of a dielectric, the foil's level a slab of
/// its own whose holes are also that layer's copper (tracks and via rings;
/// inner fills and the copper they touch as one union,
/// [`fill::foil_level`]). A rectangular board's outline is the rectangle
/// of its size; a sketched board's is its solved outline, with its
/// cutouts, slots and mounting holes as holes of every slab
/// ([`crate::board::laminate`]).
fn laminate(
    project: &Project,
    pads: &[PlacedPad<'_>],
    fills: &[FillPlan],
) -> Result<Vec<PlanBody>> {
    let stackup = &project.stackup;
    let pad_drills: Vec<Contour> = pads
        .iter()
        .flat_map(|pad| pad.drills().into_iter().map(|d| pad.board(&d)))
        .collect();
    if pad_drills.len().saturating_add(project.vias.len()) > MAX_DRILLS {
        return Err(Error::Invalid(format!(
            "the board has more than {MAX_DRILLS} drilled holes"
        )));
    }
    let via_drills = project.vias.iter().map(|via| Contour::Circle {
        center: via.position,
        radius: pad_outline::plated_drill_radius(via.drill),
    });
    let shaped = crate::board::laminate(project);
    let through: Vec<Contour> = shaped
        .as_ref()
        .map(|l| l.through.clone())
        .unwrap_or_default();
    let outline = match shaped {
        Some(shaped) => shaped.outline,
        None => polygon(&[
            [0.0, 0.0],
            [project.board_size[0], 0.0],
            [project.board_size[0], project.board_size[1]],
            [0.0, project.board_size[1]],
        ])
        .ok_or_else(|| Error::Verification("the board rectangle has no corners".into()))?,
    };
    let mut dielectric = pad_drills.clone();
    dielectric.extend(via_drills);
    dielectric.extend(through.iter().cloned());
    let dielectric = without_nested(dielectric);
    let layers = stackup.layers();
    let mut slabs = Vec::new();
    let mut slab = |level: Level, z: (f64, f64), holes: &[Contour]| {
        let name = if slabs.is_empty() {
            "Substrate".to_owned()
        } else {
            format!("Substrate {}", slabs.len() + 1)
        };
        let mut loops = Vec::with_capacity(holes.len() + 1);
        loops.push(outline.clone());
        loops.extend(holes.iter().cloned());
        slabs.push(PlanBody::new(
            name,
            BodyKind::Substrate,
            Origin::Laminate(level),
            None,
            None,
            z,
            Frame::BOARD,
            loops,
        ));
    };
    for index in 0..stackup.dielectrics.len() {
        let (low, high) = stackup
            .dielectric_span(index)
            .ok_or_else(|| Error::Verification(format!("the stackup has no dielectric {index}")))?;
        let foil = layers
            .iter()
            .copied()
            .find(|l| l.side().is_none() && stackup.index(*l) == Some(index));
        let Some(layer) = foil else {
            slab(Level::Dielectric(index), (low, high), &dielectric);
            continue;
        };
        let mut holes = pad_drills.clone();
        holes.extend(through.iter().cloned());
        let mut level = holes.clone();
        level.extend(
            project
                .tracks
                .iter()
                .filter(|t| t.layer == layer)
                .filter_map(|t| polygon(&track_outline(t.start, t.end, t.width))),
        );
        level.extend(
            project
                .vias
                .iter()
                .filter(|v| v.on(layer, &layers))
                .map(|v| Contour::Circle {
                    center: v.position,
                    radius: v.diameter / 2.0,
                }),
        );
        // A via ring or drill inside a wide track's copper is already cut
        // with it; drawn too, it would be an island of laminate.
        let level = match fill::foil_level(project, layer, &holes, fills) {
            Some(level) => level,
            None => without_nested(level),
        };
        let (foil_low, _) = span(project, layer)?;
        slab(Level::Foil { index, layer }, (foil_low, high), &level);
        slab(Level::UnderFoil(index), (low, foil_low), &dielectric);
    }
    Ok(slabs)
}

/// The document object of a flat body, as the build's sources name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Object {
    Board,
    Pad(Id, PadId),
    Track(Id),
    Via(Id),
    Fill(Id),
}

impl Origin {
    fn object(&self) -> Object {
        match *self {
            Self::Laminate(_) => Object::Board,
            Self::Pad { instance, pad, .. } => Object::Pad(instance, pad),
            Self::Track(track) => Object::Track(track),
            Self::Via { via, .. } => Object::Via(via),
            Self::Fill { fill, .. } => Object::Fill(fill),
        }
    }
}

/// A flat body as compared: kind, object, layer, box and volume (`None`:
/// not compared).
#[derive(Clone, Debug)]
struct Compared {
    kind: BodyKind,
    object: Object,
    layer: Option<CopperLayer>,
    min: [f64; 3],
    max: [f64; 3],
    volume: Option<f64>,
}

impl Compared {
    fn key(&self) -> (BodyKind, Object, Option<CopperLayer>) {
        (self.kind, self.object, self.layer)
    }

    /// Whether built body `built` is this plan body.
    fn holds(&self, built: &Self) -> bool {
        let corners = self
            .min
            .iter()
            .chain(&self.max)
            .zip(built.min.iter().chain(&built.max))
            .all(|(a, b)| (a - b).abs() <= PARITY_MM);
        let volume = match (self.volume, built.volume) {
            (Some(a), Some(b)) => (a - b).abs() <= PARITY_VOLUME * a.abs().max(b.abs()),
            _ => true,
        };
        corners && volume
    }
}

/// The kinds of the board's flat bodies.
const FLAT: [BodyKind; 5] = [
    BodyKind::Substrate,
    BodyKind::PadCopper,
    BodyKind::CopperTrack,
    BodyKind::ViaCopper,
    BodyKind::FillCopper,
];

/// Where `model`'s flat bodies differ from `plan`'s: each plan body must be
/// built once, of its kind, object and layer, its box within
/// [`PARITY_MM`] and its volume (where the plan states one) within
/// [`PARITY_VOLUME`], and no flat body may be built that the plan lacks.
/// A slab the kernel cut into pieces (laminate left standing in a fill's
/// clearance) counts as the one body it was planned as. The build files
/// a via's bodies under no layer (a via shows with every layer), so a
/// via ring's layer is not compared. Empty when they agree.
pub fn parity(plan: &BoardPlan, model: &super::NativeModel) -> Result<Vec<String>> {
    let count = model.world.bodies.len();
    let natives = model
        .geometry
        .get(..count)
        .ok_or_else(|| Error::Verification("the model has fewer bodies than its world".into()))?;
    let object = |item: Option<u64>| -> Object {
        let Some(source) = item.and_then(|pick| model.sources.iter().find(|s| s.pick_id == pick))
        else {
            return Object::Board;
        };
        match (
            source.instance,
            source.pad,
            source.track,
            source.via,
            source.fill,
        ) {
            (Some(instance), Some(pad), ..) => Object::Pad(instance, pad),
            (_, _, Some(track), ..) => Object::Track(track),
            (_, _, _, Some(via), _) => Object::Via(via),
            (_, _, _, _, Some(fill)) => Object::Fill(fill),
            _ => Object::Board,
        }
    };
    let mut built: Vec<Compared> = Vec::new();
    for g in natives.iter().filter(|g| FLAT.contains(&g.kind)) {
        let body = Compared {
            kind: g.kind,
            object: object(g.item),
            layer: g.layer,
            min: g.min,
            max: g.max,
            volume: Some(g.volume.get()),
        };
        // The pieces of one slab share its z span; slabs differ in it.
        let piece_of = built.iter_mut().find(|s| {
            body.kind == BodyKind::Substrate
                && s.kind == BodyKind::Substrate
                && (s.min[2] - body.min[2]).abs() <= PARITY_MM
                && (s.max[2] - body.max[2]).abs() <= PARITY_MM
        });
        match piece_of {
            Some(slab) => {
                for i in 0..3 {
                    slab.min[i] = slab.min[i].min(body.min[i]);
                    slab.max[i] = slab.max[i].max(body.max[i]);
                }
                slab.volume = slab.volume.zip(body.volume).map(|(a, b)| a + b);
            }
            None => built.push(body),
        }
    }
    let mut groups: std::collections::BTreeMap<_, Vec<Compared>> = Default::default();
    for body in built {
        groups.entry(body.key()).or_default().push(body);
    }
    let mut out = Vec::new();
    for body in plan.bodies() {
        let (min, max) = body.bounds()?;
        let planned = Compared {
            kind: body.kind,
            object: body.source.object(),
            layer: match body.source {
                Origin::Via { .. } => None,
                _ => body.layer,
            },
            min,
            max,
            volume: body.volume(),
        };
        let group = groups.entry(planned.key()).or_default();
        match group.iter().position(|b| planned.holds(b)) {
            Some(at) => {
                group.swap_remove(at);
            }
            None => out.push(format!(
                "planned, not built: {} {:?} {:?} {:?} box {:?}–{:?} volume {:?}; built of that object: {:?}",
                body.name,
                body.kind,
                body.source,
                body.layer,
                min,
                max,
                planned.volume,
                group
                    .iter()
                    .map(|b| (b.min, b.max, b.volume))
                    .collect::<Vec<_>>()
            )),
        }
    }
    for body in groups.into_values().flatten() {
        out.push(format!("built, not planned: {body:?}"));
    }
    Ok(out)
}
