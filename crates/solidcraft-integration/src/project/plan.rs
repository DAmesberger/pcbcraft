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
use super::fit;
use super::{
    PlacedPad, mm_slug, other, placed_pads, polygon, track_outline, without_nested,
};
use crate::material::{self, BodyKind, Material};
use crate::pad_outline::{self, Contour, FLATTEN_TOLERANCE_MM};
use crate::{Error, Result};
use sparksmith_core::library::PadCopper;
use sparksmith_core::model::Fill;
use sparksmith_core::model::fill::FilledRegion;
use sparksmith_core::{CopperLayer, Fabrication, Id, Layer, PadId, Point, Project, ViaSpan};

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
        let (c, s) = sparksmith_core::geometry::cos_sin(self.angle_deg);
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

impl Level {
    /// The dielectric the slab is (part of), counted from the top.
    pub fn dielectric(self) -> usize {
        match self {
            Self::Dielectric(index) | Self::Foil { index, .. } | Self::UnderFoil(index) => index,
        }
    }
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
    ) -> Result<Self> {
        Ok(Self {
            name,
            kind,
            source,
            net,
            layer,
            z: [z.0, z.1],
            frame,
            loops,
            look: look(kind)?,
        })
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
fn look(kind: BodyKind) -> Result<Material> {
    Ok(material::material(
        kind,
        &Fabrication::default(),
        &crate::stack::Stack::none()?,
    ))
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
    /// One per placed pad, in [`sparksmith_core::layout::placed_pads`] order.
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
}

/// The plan of `project`'s board. Refused (as the build refuses it) when
/// the document does not validate, a via is blind or buried, a pad with
/// copper has no outline or a track has no length.
pub fn of(project: &Project) -> Result<BoardPlan> {
    sparksmith_core::validate::project(project).map_err(|e| Error::Invalid(e.to_string()))?;
    project
        .stackup
        .check()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let pads = placed_pads(project)?;
    of_pads(project, &pads, &mut Memo::default())
}

/// The fills' plans made before, by a digest of each fill's stored copper
/// and heights: a fill's tiles depend on nothing else, so [`of_pads`]
/// takes an unchanged fill's from here instead of simplifying and tiling
/// its copper again, and leaves here the ones it used.
#[derive(Clone, Default)]
pub struct Memo {
    fills: std::collections::HashMap<u64, FillPlan>,
}

/// [`of`] for the board's placed pads, taking unchanged fills from `memo`.
pub(super) fn of_pads(
    project: &Project,
    pads: &[PlacedPad<'_>],
    memo: &mut Memo,
) -> Result<BoardPlan> {
    if project
        .vias
        .iter()
        .any(|v| !matches!(v.span, ViaSpan::Through))
    {
        return Err(Error::Invalid(
            "blind and buried vias are not built in the native model yet".into(),
        ));
    }
    let fills = fills(project, memo)?;
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

/// What a fill's plan is made from, digested: the fill, its stored
/// copper (every number exactly) and its layer's heights.
fn fill_key(fill: &Fill, z: (f64, f64)) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!(
        "{:?} {:?} {:?} {:?}",
        fill.id, fill.layer, fill.net, fill.name
    )
    .hash(&mut h);
    z.0.to_bits().hash(&mut h);
    z.1.to_bits().hash(&mut h);
    let regions = fill
        .computed
        .as_ref()
        .map(|c| c.regions.as_slice())
        .unwrap_or_default();
    regions.len().hash(&mut h);
    for region in regions {
        for points in std::iter::once(&region.outer).chain(&region.holes) {
            points.len().hash(&mut h);
            for p in points {
                p[0].to_bits().hash(&mut h);
                p[1].to_bits().hash(&mut h);
            }
        }
    }
    h.finish()
}

/// Each fill with copper, one body per tile of its regions
/// ([`fill::tiles`]): an unchanged fill's from `memo`, the others made on
/// every core.
fn fills(project: &Project, memo: &mut Memo) -> Result<Vec<FillPlan>> {
    let built: Vec<(&Fill, u64, (f64, f64))> = fill::with_copper(project)
        .map(|f| {
            let z = span(project, f.layer)?;
            Ok((f, fill_key(f, z), z))
        })
        .collect::<Result<_>>()?;
    let wanted: Vec<&(&Fill, u64, (f64, f64))> = built
        .iter()
        .filter(|(_, key, _)| !memo.fills.contains_key(key))
        .collect();
    let slots: Vec<std::sync::OnceLock<Result<Option<FillPlan>>>> =
        wanted.iter().map(|_| std::sync::OnceLock::new()).collect();
    let indexed: Vec<usize> = (0..wanted.len()).collect();
    super::on_threads(&indexed, |&i| {
        if let (Some((fill, _, z)), Some(slot)) = (wanted.get(i), slots.get(i)) {
            let _ = slot.set(fill_plan(fill, *z));
        }
    });
    let mut made = std::collections::HashMap::new();
    for ((_, key, _), slot) in wanted.iter().zip(slots) {
        let plan = slot
            .into_inner()
            .ok_or_else(|| Error::Verification("a fill was not planned".into()))??;
        made.insert(*key, plan);
    }
    let mut next = Memo::default();
    let mut out = Vec::new();
    for (_, key, _) in &built {
        let plan = match made.remove(key) {
            Some(plan) => plan,
            None => memo.fills.get(key).or_else(|| next.fills.get(key)).cloned(),
        };
        if let Some(plan) = plan {
            next.fills.insert(*key, plan.clone());
            out.push(plan);
        }
    }
    *memo = next;
    Ok(out)
}

/// A fill's plan: its copper simplified for the model and cut in tiles;
/// `None` when no region has a loop.
fn fill_plan(fill: &Fill, z: (f64, f64)) -> Result<Option<FillPlan>> {
    let regions = fill::simplified_regions(fill);
    if regions.iter().all(|r| fill::loops(r).is_empty()) {
        return Ok(None);
    }
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
        )?;
        tiles.push((body, region));
    }
    Ok(Some(FillPlan {
        fill: fill.id,
        net: fill.net,
        layer: fill.layer,
        regions,
        tiles,
    }))
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
    )?);
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
        )?);
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
        )?);
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
                )?,
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
                )?);
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
                )?);
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
        let mut body = PlanBody::new(
            name,
            BodyKind::Substrate,
            Origin::Laminate(level),
            None,
            None,
            z,
            Frame::BOARD,
            loops,
        )?;
        // Each slab shows its own dielectric's weave, as the edge band of
        // that dielectric does.
        if let Some(d) = stackup.dielectrics.get(level.dielectric()) {
            body.look.detail = crate::stackup_edge::laminate_detail(d.kind, d.glass);
        }
        slabs.push(body);
        Ok::<(), Error>(())
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
            slab(Level::Dielectric(index), (low, high), &dielectric)?;
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
        slab(Level::Foil { index, layer }, (foil_low, high), &level)?;
        slab(Level::UnderFoil(index), (low, foil_low), &dielectric)?;
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

    fn measure(&self) -> Measure {
        Measure {
            min: self.min,
            max: self.max,
            volume: self.volume,
        }
    }

    /// How far `built`'s box lies from this one's: its farthest corner
    /// coordinate, mm.
    fn box_off(&self, built: &Self) -> f64 {
        self.min
            .iter()
            .chain(&self.max)
            .zip(built.min.iter().chain(&built.max))
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max)
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

/// Whether `kind` is one of the board's flat bodies, which the plan
/// describes.
pub fn is_flat(kind: BodyKind) -> bool {
    FLAT.contains(&kind)
}

/// The document object a compared body comes from, as a check reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The laminate (or a flat body the build ties to no object).
    Board,
    Pad {
        instance: Id,
        pad: PadId,
    },
    Track {
        track: Id,
    },
    Via {
        via: Id,
    },
    Fill {
        fill: Id,
    },
}

impl From<Object> for Source {
    fn from(object: Object) -> Self {
        match object {
            Object::Board => Self::Board,
            Object::Pad(instance, pad) => Self::Pad { instance, pad },
            Object::Track(track) => Self::Track { track },
            Object::Via(via) => Self::Via { via },
            Object::Fill(fill) => Self::Fill { fill },
        }
    }
}

/// A body's box (mm) and volume, as compared.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Measure {
    pub min: [f64; 3],
    pub max: [f64; 3],
    /// mm³; `None` where it is not compared (a laminate slab, whose
    /// outline is).
    pub volume: Option<f64>,
}

/// How a built body fits a plan body ([`check`]): its top and bottom
/// faces against the plan's loops, its heights against the plan's, its
/// holes, and its volume as one more sign.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Fit {
    /// The area inside exactly one of the plan's outline and the built
    /// face (the larger of the top and bottom faces'), mm².
    pub area_mm2: f64,
    /// What `area_mm2` may be for the same exact curves, each outline
    /// sampled to [`fit::CHORD_MM`]: their length times both tolerances.
    pub allowed_area_mm2: f64,
    /// The farthest a point of either outline lies from the other (of
    /// both faces), mm; at most [`fit::SEARCH_MM`].
    pub deviation_mm: f64,
    /// Holes in the plan's outline.
    pub planned_holes: usize,
    /// Holes in the built top and bottom faces.
    pub built_holes: [usize; 2],
    /// Bottom and top heights, mm.
    pub planned_z: [f64; 2],
    pub built_z: [f64; 2],
    /// The built volume's change from the plan's, as a fraction, where
    /// both are known (not a laminate slab's).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_change: Option<f64>,
}

impl Fit {
    /// The faces lie on the plan's outline: within [`PARITY_MM`] at every
    /// point, the area between them what sampling the same curves leaves.
    pub fn outline_fits(&self) -> bool {
        self.deviation_mm <= PARITY_MM && self.area_mm2 <= self.allowed_area_mm2
    }

    pub fn holes_fit(&self) -> bool {
        self.built_holes.iter().all(|h| *h == self.planned_holes)
    }

    /// Thickness and position within [`PARITY_MM`].
    pub fn heights_fit(&self) -> bool {
        self.planned_z
            .iter()
            .zip(&self.built_z)
            .all(|(a, b)| (a - b).abs() <= PARITY_MM)
    }

    /// The volume within [`PARITY_VOLUME`], where it is compared.
    pub fn volume_fits(&self) -> bool {
        self.volume_change.is_none_or(|c| c.abs() <= PARITY_VOLUME)
    }

    pub fn fits(&self) -> bool {
        self.outline_fits() && self.holes_fit() && self.heights_fit() && self.volume_fits()
    }
}

/// How a built body and the plan disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    /// The plan holds the body; the build made none that fits it.
    PlannedNotBuilt,
    /// The build made a flat body the plan does not hold.
    BuiltNotPlanned,
}

/// One body where a build differs from its plan.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Difference {
    pub problem: Problem,
    pub kind: BodyKind,
    pub source: Source,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<CopperLayer>,
    /// The plan body's name (planned, not built).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The planned body (planned, not built), or the built one.
    pub measure: Measure,
    /// Bodies built of the same object, kind and layer that no planned
    /// body took (planned, not built): what was built instead.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub instead: Vec<Measure>,
    /// How the nearest body built of that object, kind and layer fits the
    /// planned one (planned, not built; `None` when none was built).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fit: Option<Fit>,
}

impl Difference {
    /// The difference as one line of text, starting `planned, not built`
    /// or `built, not planned`.
    pub fn text(&self) -> String {
        let what = match self.problem {
            Problem::PlannedNotBuilt => "planned, not built",
            Problem::BuiltNotPlanned => "built, not planned",
        };
        format!(
            "{what}: {} {:?} {:?} {:?} box {:?}–{:?} volume {:?}; nearest built fit: {:?}; built of that object instead: {:?}",
            self.body.as_deref().unwrap_or("-"),
            self.kind,
            self.source,
            self.layer,
            self.measure.min,
            self.measure.max,
            self.measure.volume,
            self.fit,
            self.instead
                .iter()
                .map(|m| (m.min, m.max, m.volume))
                .collect::<Vec<_>>()
        )
    }
}

/// A build checked against its plan ([`check`]).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Check {
    /// Flat bodies the plan holds.
    pub planned: usize,
    /// Flat bodies the build made (a slab cut into pieces counted once).
    pub built: usize,
    /// Of the bodies that fit, the farthest any outline lay from its
    /// plan, mm, and the largest area between them, mm².
    pub max_deviation_mm: f64,
    pub max_area_mm2: f64,
    /// Where they differ; empty when they agree.
    pub differences: Vec<Difference>,
}

/// [`check`]'s differences, one line of text each. Empty when the build
/// and the plan agree.
pub fn parity(plan: &BoardPlan, model: &super::NativeModel) -> Result<Vec<String>> {
    Ok(check(plan, model, &mut || Ok(()))?
        .differences
        .iter()
        .map(Difference::text)
        .collect())
}

/// A built flat body: as compared, and its top and bottom faces' outlines
/// (prepared when first compared).
struct Built {
    compared: Compared,
    top: fit::Rings,
    bottom: fit::Rings,
    prepared: Option<(fit::Prepared, fit::Prepared)>,
}

impl Built {
    fn faces(&mut self) -> &(fit::Prepared, fit::Prepared) {
        let (top, bottom) = (&self.top, &self.bottom);
        self.prepared
            .get_or_insert_with(|| (top.prepare(), bottom.prepare()))
    }
}

/// How `built` fits `planned`, whose outline is `outline`.
fn fit_of(planned: &Compared, z: [f64; 2], outline: &fit::Prepared, built: &mut Built) -> Fit {
    let volume_change = match (planned.volume, built.compared.volume) {
        (Some(a), Some(b)) if a.abs() > 0.0 => Some((b - a) / a),
        _ => None,
    };
    let built_z = [built.compared.min[2], built.compared.max[2]];
    let (top, bottom) = built.faces();
    let (up, down) = (outline.apart(top), outline.apart(bottom));
    Fit {
        area_mm2: up.area.max(down.area),
        allowed_area_mm2: up.allowed_area.min(down.allowed_area),
        deviation_mm: up.deviation.max(down.deviation),
        planned_holes: outline.holes,
        built_holes: [top.holes, bottom.holes],
        planned_z: z,
        built_z,
        volume_change,
    }
}

/// Where `model`'s flat bodies differ from `plan`'s, by geometry fit:
/// each plan body must be built once, of its kind, object and layer, its
/// top and bottom faces on the plan's loops (every point within
/// [`PARITY_MM`], the area between them no more than sampling the same
/// curves leaves: [`fit`]), with the plan's holes, between the plan's
/// heights (within [`PARITY_MM`]) and its volume, where both say one,
/// within [`PARITY_VOLUME`]; and no flat body may be built that the plan
/// lacks. Laminate slabs are compared by outline, not volume. A slab the
/// kernel cut into pieces (laminate left standing in a fill's clearance)
/// counts as the one body it was planned as, its pieces' faces together.
/// The build files a via's bodies under no layer (a via shows with every
/// layer), so a via ring's layer is not compared.
///
/// `step` runs before each built body is measured (its faces read here,
/// and its volume on first use), so a caller can count and stop a long
/// check; its error ends the check.
pub fn check(
    plan: &BoardPlan,
    model: &super::NativeModel,
    step: &mut dyn FnMut() -> Result<()>,
) -> Result<Check> {
    let natives = model.native_geometry()?;
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
    let mut built: Vec<Built> = Vec::new();
    for (index, g) in natives.iter().enumerate() {
        if !FLAT.contains(&g.kind) {
            continue;
        }
        step()?;
        let exact = model.native_body(index).ok_or_else(|| {
            Error::Verification(format!("flat body {index} has no evaluated body"))
        })?;
        let top = fit::face_rings(exact, g.max[2])?;
        let bottom = fit::face_rings(exact, g.min[2])?;
        let compared = Compared {
            kind: g.kind,
            object: object(g.item),
            layer: g.layer,
            min: g.min,
            max: g.max,
            // A slab's outline is compared, not its volume (measuring it
            // costs more than building it).
            volume: (g.kind != BodyKind::Substrate).then(|| g.volume.get()),
        };
        // The pieces of one slab share its z span; slabs differ in it.
        let piece_of = built.iter_mut().find(|s| {
            compared.kind == BodyKind::Substrate
                && s.compared.kind == BodyKind::Substrate
                && (s.compared.min[2] - compared.min[2]).abs() <= PARITY_MM
                && (s.compared.max[2] - compared.max[2]).abs() <= PARITY_MM
        });
        match piece_of {
            Some(slab) => {
                for i in 0..3 {
                    slab.compared.min[i] = slab.compared.min[i].min(compared.min[i]);
                    slab.compared.max[i] = slab.compared.max[i].max(compared.max[i]);
                }
                slab.top.extend(top);
                slab.bottom.extend(bottom);
            }
            None => built.push(Built {
                compared,
                top,
                bottom,
                prepared: None,
            }),
        }
    }
    let built_count = built.len();
    let mut groups: std::collections::BTreeMap<_, Vec<Built>> = Default::default();
    for body in built {
        groups.entry(body.compared.key()).or_default().push(body);
    }
    let mut planned_count = 0;
    let mut missing = Vec::new();
    let (mut max_deviation, mut max_area) = (0.0_f64, 0.0_f64);
    for body in plan.bodies() {
        planned_count += 1;
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
        let mut outline = None;
        let mut taken = None;
        // A fit is worked out only for a body whose box is the plan's: a
        // body that fits has it.
        for (at, candidate) in group.iter_mut().enumerate() {
            if planned.box_off(&candidate.compared) > PARITY_MM {
                continue;
            }
            let outline = match &outline {
                Some(o) => o,
                None => outline.insert(fit::plan_rings(&body.board_loops())?.prepare()),
            };
            let fit = fit_of(&planned, body.z, outline, candidate);
            if fit.fits() {
                taken = Some((at, fit));
                break;
            }
        }
        match taken {
            Some((at, fit)) => {
                max_deviation = max_deviation.max(fit.deviation_mm);
                max_area = max_area.max(fit.area_mm2);
                group.swap_remove(at);
            }
            None => {
                // How the nearest body of the object was built instead.
                let nearest = group.iter_mut().min_by(|a, b| {
                    planned
                        .box_off(&a.compared)
                        .total_cmp(&planned.box_off(&b.compared))
                });
                let fit = match nearest {
                    Some(candidate) => {
                        let outline = match &outline {
                            Some(o) => o,
                            None => outline.insert(fit::plan_rings(&body.board_loops())?.prepare()),
                        };
                        Some(fit_of(&planned, body.z, outline, candidate))
                    }
                    None => None,
                };
                missing.push((body.name.clone(), planned, fit));
            }
        }
    }
    let mut differences = Vec::new();
    for (name, planned, fit) in missing {
        let instead = groups
            .get(&planned.key())
            .into_iter()
            .flatten()
            .map(|b| b.compared.measure())
            .collect();
        differences.push(Difference {
            problem: Problem::PlannedNotBuilt,
            kind: planned.kind,
            source: planned.object.into(),
            layer: planned.layer,
            body: Some(name),
            measure: planned.measure(),
            instead,
            fit,
        });
    }
    for body in groups.into_values().flatten() {
        differences.push(Difference {
            problem: Problem::BuiltNotPlanned,
            kind: body.compared.kind,
            source: body.compared.object.into(),
            layer: body.compared.layer,
            body: None,
            measure: body.compared.measure(),
            instead: Vec::new(),
            fit: None,
        });
    }
    Ok(Check {
        planned: planned_count,
        built: built_count,
        max_deviation_mm: max_deviation,
        max_area_mm2: max_area,
        differences,
    })
}
