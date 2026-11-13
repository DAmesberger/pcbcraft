//! Display-only appearance layers over the native model: soldermask, finish
//! over exposed copper, package terminations, resistor overcoat and solder
//! fillets. They are thin films and decorations derived from the project, in
//! Solidcraft's mesh type, built on the model worker. They are drawn and
//! picked with the native bodies but never stored in the `.solidcraft` file,
//! so the saved model's bodies stay disjoint (except copper joining copper)
//! and its mass properties stay physical. Dimensions and sources are in
//! `docs/materials.md`.
use crate::geom::prelude::*;
use crate::geom::{Mesh, Vec3};
use crate::material::{self, BodyKind};
use crate::scene::{Geometry, Place};
use crate::stack::{PROUD_MM, Stack};
use crate::{Error, Result};
use sparksmith_core::geometry::{cos_sin, is_quarter_turn};
use sparksmith_core::library::offset_polygon;
use sparksmith_core::library::{
    ChipKind, FootprintDefinition, PackageDefinition, PadKind, PadSide,
};
use sparksmith_core::{CopperLayer, Fabrication, Layer, Placement, Point, Project, layout};
use std::sync::Arc;

mod outline;
mod region;
mod via;
pub use outline::union;
pub use region::{Region, slab};
use via::via_layers;

/// Soldermask tiles: the slab's faces are triangulated per square of this
/// size, so a dense board stays local.
const MASK_TILE_MM: f64 = 4.0;
/// How far a track's mask cap overhangs its copper on each side.
pub(crate) const CAP_MARGIN_MM: f64 = 0.01;
/// Shorter cap pieces are left out.
const CAP_MIN_MM: f64 = 0.01;
/// Finish over a track inside an opening sits this share of the finish
/// thickness up, just under the land's own finish where they overlap.
const STRIP_SHARE: f64 = 0.8;
/// Termination plating beyond the body faces (real terminations are 10–30 µm).
const TERMINATION_GROWTH_MM: f64 = 0.01;
/// A chip termination's band on the top face: a fifth of the length, at
/// most 0.5 mm (EIA chip-resistor bands are 0.3–0.5 mm from 0603 to 1206).
const BAND_SHARE: f64 = 0.2;
const BAND_MAX_MM: f64 = 0.5;
/// Height share of a non-chip lead band.
const LEAD_HEIGHT_SHARE: f64 = 0.15;
/// The resistor overcoat stops this far short of the body's long edges,
/// leaving a white alumina rim, and stands this far above the body.
const OVERCOAT_INSET_MM: f64 = 0.04;
const OVERCOAT_MM: f64 = 0.005;
/// Solder fillet height as a share of the body height, at most 0.4 mm
/// (J-STD-001 asks for at least a quarter of the termination height).
const FILLET_HEIGHT_SHARE: f64 = 0.4;
const FILLET_MAX_MM: f64 = 0.4;
/// The fillet is narrower than the termination it wets.
const FILLET_WIDTH_SHARE: f64 = 0.92;
/// Pads reaching less than this beyond the termination get no fillet.
const FILLET_MIN_TOE_MM: f64 = 0.02;
/// Segments of a round track end and of a fillet profile.
const ARC_STEPS: usize = 8;
const FILLET_STEPS: usize = 24;
const FILLET_ACROSS_STEPS: usize = 24;
/// Adjacent prism walls turning less than this (radians) share a smoothed
/// normal, and the plan mesher draws no edge between them.
pub(crate) const SMOOTH_TURN: f64 = 0.55;

/// A tented via's mask sags this far into the drill it bridges (est.; LPI
/// tents over a 0.3 mm hole dip into it as a visible dimple). 8 µm read as
/// no dimple in the look review. The dimple stays [`PROUD_MM`] above the
/// copper under the mask, so a track ending at the via never shows through
/// it.
const TENT_SAG_MM: f64 = 0.012;
/// How far past the ring the tent's dome slopes down into the mask.
const TENT_SKIRT_MM: f64 = 0.08;
/// Segments around a via and rings across its dome.
const VIA_SEGMENTS: usize = 40;
const DOME_STEPS: usize = 6;
/// The bore shadow lines the barrel this far inside it and is this thick.
const BORE_GAP_MM: f64 = 0.001;
const BORE_SHADOW_MM: f64 = 0.01;

/// A sketched board's solved shape as the display layers draw it,
/// flattened at the display tolerance: the outer outline and the loops
/// cut through it (cutouts, slots and mounting holes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    /// Counter-clockwise.
    pub outer: Vec<Point>,
    /// Cutouts and slots.
    pub cutouts: Vec<Vec<Point>>,
    /// Mounting holes (their drills).
    pub holes: Vec<Vec<Point>>,
}

impl Outline {
    /// The solved shape of a sketched `project`; `None` for a rectangle
    /// (the layers then draw `board_size`'s rectangle, as before).
    pub fn of(project: &Project) -> Option<Self> {
        use sparksmith_core::model::board::flatten;
        if project.board_shape.is_rectangle() {
            return None;
        }
        let board = project.board();
        let tolerance = crate::pad_outline::FLATTEN_TOLERANCE_MM;
        let ccw = |mut points: Vec<Point>| {
            if signed_area(&points) < 0.0 {
                points.reverse();
            }
            points
        };
        Some(Self {
            outer: ccw(flatten(&board.outer, tolerance)),
            cutouts: board
                .cutouts
                .iter()
                .map(|c| ccw(flatten(&c.outline, tolerance)))
                .collect(),
            holes: board
                .holes
                .iter()
                .map(|h| circle(h.at, h.drill / 2.0))
                .collect(),
        })
    }
}

/// Twice the signed area of a loop (positive counter-clockwise).
fn signed_area(points: &[Point]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| points[i][0] * points[(i + 1) % n][1] - points[(i + 1) % n][0] * points[i][1])
        .sum()
}

/// The board the layers sit on, in mm, and the fabrication that sets the
/// mask and finish looks.
#[derive(Clone, Debug)]
pub struct Board {
    pub width: f64,
    pub depth: f64,
    /// A sketched board's solved shape; `None` for the `width` × `depth`
    /// rectangle.
    pub shape: Option<std::sync::Arc<Outline>>,
    /// The films and foils, with their heights.
    pub stack: Stack,
    pub fabrication: Fabrication,
}

impl Board {
    /// The board of `project`, from its outline, stackup and fabrication.
    pub fn of(project: &Project) -> Self {
        Self {
            width: project.board_size[0],
            depth: project.board_size[1],
            shape: Outline::of(project).map(std::sync::Arc::new),
            stack: Stack::of(&project.stackup, project.fabrication.pad_finish),
            fabrication: project.fabrication,
        }
    }

    /// The outer copper foil thickness on a side.
    pub fn copper(&self, layer: Layer) -> f64 {
        self.stack.copper(layer)
    }

    /// Whether a side carries soldermask.
    pub fn mask(&self, layer: Layer) -> bool {
        self.stack.mask(layer)
    }

    /// The mask over a side's copper reaches this far above the laminate.
    pub fn crown(&self, layer: Layer) -> f64 {
        self.copper(layer) + self.stack.mask_mm()
    }

    /// World z of a height above a side's laminate face (below it for the
    /// bottom) ([`Stack::z`]).
    pub fn z(&self, layer: Layer, height: f64) -> f64 {
        self.stack.z(layer, height)
    }

    pub fn span(&self, layer: Layer, low: f64, high: f64) -> (f64, f64) {
        let [a, b] = self.stack.span(layer, low, high);
        (a, b)
    }
}

/// What one pad shows on one outer copper layer, in board coordinates:
/// flattened loops of either orientation.
#[derive(Clone, Debug)]
pub struct Land {
    /// The pad's pick identity; `None` for a pad without copper.
    pub pick: Option<u64>,
    pub layer: Layer,
    /// The copper outline (`None` for a bare hole).
    pub copper: Option<Vec<Point>>,
    /// The finished bore through the copper.
    pub bore: Option<Vec<Point>>,
    /// Soldermask openings: the pad's opening (unless tented) and any drill.
    pub openings: Vec<Vec<Point>>,
}

impl Land {
    /// The parameter interval of the line `point + t·direction` inside this
    /// land's mask openings (one interval spanning them all).
    fn crossing(&self, point: Point, direction: Point) -> Option<(f64, f64)> {
        self.openings
            .iter()
            .filter_map(|opening| line_through(opening, point, direction))
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    }
}

/// Where the line `point + t·direction` enters and last leaves a loop.
fn line_through(points: &[Point], point: Point, direction: Point) -> Option<(f64, f64)> {
    let n = points.len();
    let mut hits: Vec<f64> = Vec::new();
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        let e = [b[0] - a[0], b[1] - a[1]];
        let d = direction[0] * e[1] - direction[1] * e[0];
        if d.abs() < 1e-12 {
            continue;
        }
        let w = [a[0] - point[0], a[1] - point[1]];
        let t = (w[0] * e[1] - w[1] * e[0]) / d;
        let s = (w[0] * direction[1] - w[1] * direction[0]) / d;
        if (0.0..=1.0).contains(&s) {
            hits.push(t);
        }
    }
    let lo = hits.iter().copied().reduce(f64::min)?;
    let hi = hits.iter().copied().reduce(f64::max)?;
    (hi > lo).then_some((lo, hi))
}

/// A board via as the display layers see it: its plated ring, its drill
/// and which sides its mask tents.
#[derive(Clone, Copy, Debug)]
pub struct ViaMark {
    pub pick: u64,
    pub at: Point,
    pub drill: f64,
    pub diameter: f64,
    pub tented: bool,
    /// The mask opening's diameter where the via is open
    /// ([`sparksmith_core::Via::mask_opening`], as the Gerber draws it).
    pub mask_opening: f64,
}

impl ViaMark {
    /// The finished bore radius: the via's stated (finished) drill.
    pub fn bore(&self) -> f64 {
        self.drill / 2.0
    }

    /// Whether the mask covers the via on `side` of `board`.
    pub fn covered(&self, board: &Board, side: Layer) -> bool {
        self.tented && board.mask(side)
    }

    /// The lands of the sides the mask leaves open: a finished ring in a
    /// mask opening, the drill open through it.
    pub fn lands(&self, board: &Board) -> Vec<Land> {
        [Layer::Top, Layer::Bottom]
            .into_iter()
            .filter(|side| !self.covered(board, *side))
            .map(|layer| Land {
                pick: Some(self.pick),
                layer,
                copper: Some(circle(self.at, self.diameter / 2.0)),
                bore: Some(circle(self.at, self.bore())),
                openings: vec![circle(self.at, self.mask_opening / 2.0)],
            })
            .collect()
    }
}

/// A circle as a counter-clockwise loop of [`VIA_SEGMENTS`] points.
pub fn circle(center: Point, radius: f64) -> Vec<Point> {
    (0..VIA_SEGMENTS)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / VIA_SEGMENTS as f64;
            [center[0] + radius * a.cos(), center[1] + radius * a.sin()]
        })
        .collect()
}

/// A track's centre line.
#[derive(Clone, Copy, Debug)]
pub struct Path {
    pub pick: u64,
    pub start: Point,
    pub end: Point,
    pub width: f64,
    pub layer: Layer,
}

/// What a package body is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body {
    /// A chip resistor: alumina with a black overcoat and wrapped ends.
    Resistor,
    /// A multilayer ceramic chip (capacitor class): wrapped ends.
    Ceramic,
    /// A moulded package; at most two pads get a low lead band.
    Moulded,
}

impl Body {
    /// The body a package declares. Explicit data only: never guessed from
    /// a designator, name or value.
    pub fn of(package: &PackageDefinition) -> Self {
        match package.chip_kind() {
            None => Self::Moulded,
            Some(ChipKind::Resistor) => Self::Resistor,
            // An inductor's envelope shows the ceramic chip's wrapped
            // terminations; its generated model is ferrite.
            Some(ChipKind::Capacitor | ChipKind::Inductor | ChipKind::Led) => Self::Ceramic,
        }
    }

    pub fn kind(self) -> BodyKind {
        match self {
            Self::Resistor => BodyKind::ChipResistor,
            Self::Ceramic => BodyKind::ChipCapacitor,
            Self::Moulded => BodyKind::PackageBody,
        }
    }
}

/// A placed package (or the authoring preview's unplaced one).
pub struct Part<'a> {
    pub pick: Option<u64>,
    pub package: &'a PackageDefinition,
    pub footprint: Option<&'a FootprintDefinition>,
    pub body: Body,
    /// Board placement; `None` draws in the package frame on z = 0.
    pub placement: Option<&'a Placement>,
    /// The package has a generated model whose real bodies carry its
    /// terminations and overcoat: only the solder fillets are drawn here,
    /// against the chip's own end faces.
    pub generated: bool,
}

/// Every appearance layer of a board.
pub fn board(
    board: &Board,
    lands: &[Land],
    paths: &[Path],
    vias: &[ViaMark],
    parts: &[Part<'_>],
) -> Result<Vec<Geometry>> {
    board_with(board, lands, paths, vias, parts, &mut Memo::default())
}

/// Display layers made before, by a digest of what each was made from: a
/// side's mask (the board and that side's openings), a land's finish (the
/// board and the land) and a via's dome and bore (the board and the via).
/// [`board_with`] takes each unchanged one from the last call instead of
/// making it again, so an edit remakes only what it changed.
#[derive(Clone, Default)]
pub struct Memo {
    made: std::collections::HashMap<u64, Vec<Geometry>>,
}

impl Memo {
    /// The layers `inputs` (debug-printed: every number exactly) make: the
    /// last call's if it made them, else `make`'s, kept in `next`.
    fn unit(
        &self,
        next: &mut Self,
        out: &mut Vec<Geometry>,
        inputs: &dyn std::fmt::Debug,
        make: impl FnOnce(&mut Vec<Geometry>) -> Result<()>,
    ) -> Result<()> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{inputs:?}").hash(&mut h);
        let key = h.finish();
        let made = match self.made.get(&key).or_else(|| next.made.get(&key)) {
            Some(made) => made.clone(),
            None => {
                let mut made = Vec::new();
                make(&mut made)?;
                made
            }
        };
        out.extend(made.iter().cloned());
        next.made.insert(key, made);
        Ok(())
    }
}

/// [`board`], taking unchanged layers from `memo` and leaving there this
/// call's.
pub fn board_with(
    board: &Board,
    lands: &[Land],
    paths: &[Path],
    vias: &[ViaMark],
    parts: &[Part<'_>],
    memo: &mut Memo,
) -> Result<Vec<Geometry>> {
    let mut next = Memo::default();
    let mut out = Vec::new();
    let mut shared = Shared::default();
    let (fills, edge_cuts) = mask_outline(board);
    for (kind, layer) in [
        (BodyKind::MaskTop, Layer::Top),
        (BodyKind::MaskBottom, Layer::Bottom),
    ] {
        if !board.mask(layer) {
            continue;
        }
        let (z0, z1) = board.span(layer, 0.0, board.stack.mask_laminate_mm());
        let region = Region {
            fills: fills.clone(),
            cuts: lands
                .iter()
                .filter(|land| land.layer == layer)
                .flat_map(|land| land.openings.iter().cloned())
                .chain(edge_cuts.iter().cloned())
                .collect(),
        };
        memo.unit(&mut next, &mut out, &(kind, board, &region), |out| {
            push(
                out,
                Some(board),
                kind,
                None,
                Some(layer.into()),
                slab(&region, z0, z1, MASK_TILE_MM),
            )
        })?;
    }
    // Via picks, looked up per land (a board has thousands of each).
    let via_picks: std::collections::BTreeMap<u64, &ViaMark> =
        vias.iter().map(|v| (v.pick, v)).collect();
    for land in lands {
        let Some(copper) = &land.copper else {
            continue;
        };
        // The finish plates the land's top, its sides and the bore's wall,
        // not its underside: it starts a finish thickness off the laminate,
        // so its bottom never shares the copper's bottom face (the two
        // flickered in streaks seen from below with the laminate hidden).
        let finish = board.stack.finish_mm();
        let (z0, z1) = board.span(land.layer, finish, board.copper(land.layer) + finish);
        let region = Region {
            fills: offset_polygon(copper, finish).into_iter().collect(),
            cuts: land
                .bore
                .iter()
                .filter_map(|bore| offset_polygon(bore, -finish))
                .collect(),
        };
        // A via's finished ring belongs to the via, not to its side's
        // copper: no layer, as its barrel has none.
        let via = land.pick.and_then(|pick| via_picks.get(&pick).copied());
        match via {
            // Made once at the origin per ring shape, placed at each via.
            Some(via) => {
                let local = region.shifted([-via.at[0], -via.at[1]]);
                let mut key = local.key();
                key.extend([z0.to_bits(), z1.to_bits()]);
                shared.place(&mut out, key, Some(via.pick), via.at, |made| {
                    push(
                        made,
                        Some(board),
                        BodyKind::PadFinish,
                        Some(via.pick),
                        None,
                        slab(&local, z0, z1, f64::MAX),
                    )
                })?;
            }
            None => memo.unit(&mut next, &mut out, &(board, land), |out| {
                push(
                    out,
                    Some(board),
                    BodyKind::PadFinish,
                    land.pick,
                    Some(land.layer.into()),
                    slab(&region, z0, z1, f64::MAX),
                )
            })?,
        }
    }
    for path in paths {
        track(&mut out, board, lands, vias, path)?;
    }
    // Vias of one drill, ring and tenting show the same dome and bore: made
    // once at the origin and placed at each.
    for via in vias {
        let key = vec![
            0,
            via.drill.to_bits(),
            via.diameter.to_bits(),
            u64::from(via.tented),
            via.mask_opening.to_bits(),
        ];
        let local = ViaMark {
            at: [0.0, 0.0],
            ..*via
        };
        shared.place(&mut out, key, Some(via.pick), via.at, |made| {
            via_layers(made, board, &local)
        })?;
    }
    for part in parts {
        package(&mut out, Some(board), part)?;
    }
    *memo = next;
    Ok(out)
}

/// The laminate the mask leaves bare on each side with mask: inside the
/// lands' and open vias' mask openings (a fiducial's clearance, a land's
/// expansion), less the outer
/// copper there (lands, tracks and the outlines of the `fills` on that
/// side, all plated) and the drills. The laminate's own face lies under the mask, so
/// the 3D view shades it only where this film lies; the film stands
/// [`PROUD_MM`] off it.
pub fn open_laminate(
    board: &Board,
    lands: &[Land],
    paths: &[Path],
    fills: &[(Layer, Vec<Point>)],
) -> Result<Vec<Geometry>> {
    let mut out = Vec::new();
    let (_, edge_cuts) = mask_outline(board);
    for side in [Layer::Top, Layer::Bottom] {
        if !board.mask(side) {
            continue;
        }
        let on = lands.iter().filter(|land| land.layer == side);
        let copper: Vec<Vec<Point>> = on
            .clone()
            .flat_map(|land| land.copper.iter().chain(&land.bore).cloned())
            .chain(paths.iter().filter(|p| p.layer == side).filter_map(|p| {
                let delta = [p.end[0] - p.start[0], p.end[1] - p.start[1]];
                let length = delta[0].hypot(delta[1]);
                let u = if positive(length) {
                    [delta[0] / length, delta[1] / length]
                } else {
                    [1.0, 0.0]
                };
                let mut outline = Vec::with_capacity(2 * ARC_STEPS + 2);
                end(&mut outline, p.end, u, p.width / 2.0, true);
                end(&mut outline, p.start, [-u[0], -u[1]], p.width / 2.0, true);
                positive(p.width).then_some(outline)
            }))
            .chain(
                fills
                    .iter()
                    .filter(|(layer, _)| *layer == side)
                    .map(|(_, l)| l.clone()),
            )
            .chain(edge_cuts.iter().cloned())
            .collect();
        let (z0, z1) = board.span(side, 0.0, PROUD_MM);
        let layer = Some(CopperLayer::from(side));
        let region = Region {
            fills: on.flat_map(|land| land.openings.iter().cloned()).collect(),
            cuts: copper,
        };
        if region.fills.is_empty() {
            continue;
        }
        let before = out.len();
        push(
            &mut out,
            Some(board),
            BodyKind::OpenLaminate,
            None,
            layer,
            slab(&region, z0, z1, MASK_TILE_MM),
        )?;
        // The laminate in an opening is the outer dielectric's face.
        if let Some(open) = out.get_mut(before) {
            open.dielectric = board.stack.outer_dielectric(side);
        }
    }
    Ok(out)
}

/// Where the board mask lies: its fill loops (the board outline) and the
/// board's own cuts (each cutout, slot and mounting hole), so it never
/// spans an opening through the board. The mask reaches the routed edge,
/// as the Gerber mask does: no rule pulls it back.
pub(crate) fn mask_outline(board: &Board) -> (Vec<Vec<Point>>, Vec<Vec<Point>>) {
    let Some(shape) = &board.shape else {
        let (w, d) = (board.width, board.depth);
        return (
            vec![vec![[0.0, 0.0], [w, 0.0], [w, d], [0.0, d]]],
            Vec::new(),
        );
    };
    let cuts = shape.cutouts.iter().chain(&shape.holes).cloned().collect();
    (vec![shape.outer.clone()], cuts)
}

/// The authoring preview's layers: terminations and overcoat of an unplaced
/// package, from the same rules as on the board.
pub fn preview(part: &Part<'_>) -> Result<Vec<Geometry>> {
    let mut out = Vec::new();
    package(&mut out, None, part)?;
    Ok(out)
}

/// Sorted, merged parameter intervals.
fn merged(mut intervals: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(intervals.len());
    for (a, b) in intervals {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// A track's mask cap, interrupted wherever the track crosses a same-layer
/// mask opening, and finish over its copper inside each opening: all copper
/// a mask opening exposes is plated, as fabricated.
fn track(
    out: &mut Vec<Geometry>,
    board: &Board,
    lands: &[Land],
    vias: &[ViaMark],
    path: &Path,
) -> Result<()> {
    let delta = [path.end[0] - path.start[0], path.end[1] - path.start[1]];
    let length = delta[0].hypot(delta[1]);
    if !positive(length) {
        return Ok(());
    }
    let u = [delta[0] / length, delta[1] / length];
    let normal = [-u[1], u[0]];
    let band = path.width / 2.0 + CAP_MARGIN_MM;
    let at = |t: f64, o: f64| {
        [
            path.start[0] + u[0] * t + normal[0] * o,
            path.start[1] + u[1] * t + normal[1] * o,
        ]
    };
    // Where the cap's whole width is clear of every opening.
    let bare = !board.mask(path.layer);
    let openings = merged(
        lands
            .iter()
            .filter(|land| land.layer == path.layer && !bare)
            .filter_map(|land| {
                [-band, 0.0, band]
                    .into_iter()
                    .filter_map(|o| land.crossing(at(0.0, o), u))
                    .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
            })
            .filter(|(a, b)| *b > -band && *a < length + band)
            // Without mask the whole track is exposed copper.
            .chain(bare.then_some((-band, length + band)))
            .collect(),
    );
    // Where a tented via's dome covers the cap's whole width, the dome is
    // the mask there: the cap stops at it, so the dome's drill dimple
    // shows instead of lying under a track end's rounded cap.
    let domes = vias
        .iter()
        .filter(|via| via.covered(board, path.layer))
        .filter_map(|via| {
            let w = [via.at[0] - path.start[0], via.at[1] - path.start[1]];
            let along = w[0] * u[0] + w[1] * u[1];
            let across = (w[0] * normal[0] + w[1] * normal[1]).abs();
            let r = via.diameter / 2.0;
            let reach = r * r - (across + band) * (across + band);
            (reach > 0.0).then(|| {
                let half = reach.sqrt();
                (along - half, along + half)
            })
        })
        .filter(|(a, b)| *b > 0.0 && *a < length);
    let capped = merged(openings.iter().copied().chain(domes).collect());
    let layer = Some(CopperLayer::from(path.layer));
    let copper = board.copper(path.layer);
    let (c0, c1) = board.span(path.layer, 0.0, board.crown(path.layer));
    let mut from = (0.0, true);
    let mut pieces = Vec::new();
    for &(a, b) in &capped {
        pieces.push((from, (a, false)));
        from = (b, false);
    }
    pieces.push((from, (length, true)));
    for ((a, free_a), (b, free_b)) in pieces {
        let (a, b) = (a.max(0.0), b.min(length));
        let reach = b - a + if free_a { band } else { 0.0 } + if free_b { band } else { 0.0 };
        if b < a || reach < CAP_MIN_MM {
            continue;
        }
        let mut outline = Vec::with_capacity(2 * ARC_STEPS + 4);
        end(&mut outline, at(b, 0.0), u, band, free_b);
        end(&mut outline, at(a, 0.0), [-u[0], -u[1]], band, free_a);
        push(
            out,
            Some(board),
            BodyKind::MaskCap,
            Some(path.pick),
            layer,
            prism(&outline, c0, c1),
        )?;
    }
    // Off the laminate like a land's finish, and lower than it, so the
    // strip's underside meets neither copper's nor the land finish's.
    let finish = board.stack.finish_mm();
    let (f0, f1) = board.span(
        path.layer,
        (1.0 - STRIP_SHARE) * finish,
        copper + STRIP_SHARE * finish,
    );
    let half = path.width / 2.0 + STRIP_SHARE * finish;
    for &(a, b) in &openings {
        let (a, b) = (a.max(0.0), b.min(length));
        if b - a < CAP_MIN_MM {
            continue;
        }
        let mut outline = Vec::with_capacity(4);
        end(&mut outline, at(b, 0.0), u, half, false);
        end(&mut outline, at(a, 0.0), [-u[0], -u[1]], half, false);
        push(
            out,
            Some(board),
            BodyKind::PadFinish,
            Some(path.pick),
            layer,
            prism(&outline, f0, f1),
        )?;
    }
    Ok(())
}

/// One end of a stadium outline, counter-clockwise: a half disc about
/// `center` facing `toward` when `round`, otherwise a straight cut.
fn end(outline: &mut Vec<Point>, center: Point, toward: Point, radius: f64, round: bool) {
    let side = [-toward[1], toward[0]];
    let steps = if round { ARC_STEPS } else { 1 };
    for k in 0..=steps {
        let a = std::f64::consts::PI * (k as f64 / steps as f64 - 0.5);
        let (s, c) = a.sin_cos();
        let (along, across) = if round { (c, s) } else { (0.0, s.signum()) };
        outline.push([
            center[0] + (toward[0] * along + side[0] * across) * radius,
            center[1] + (toward[1] * along + side[1] * across) * radius,
        ]);
    }
}

/// Maps the package frame (x, y on the seating plane, z up from it) to the
/// world.
struct Frame<'a> {
    board: Option<&'a Board>,
    placement: Option<&'a Placement>,
}

impl Frame<'_> {
    fn xy(&self, p: Point) -> Point {
        match self.placement {
            Some(placement) => layout::placed_point(placement, p, [0.0, 0.0]),
            None => p,
        }
    }

    fn layer(&self) -> Layer {
        self.placement.map_or(Layer::Top, |p| p.layer)
    }

    /// Seating-plane height `h` (the land tops on a board).
    fn z(&self, h: f64) -> f64 {
        match self.board {
            Some(board) => board.z(self.layer(), board.copper(self.layer()) + h),
            None => h,
        }
    }

    fn point(&self, p: Vec3) -> Vec3 {
        let [x, y] = self.xy([p.x, p.y]);
        Vec3::new(x, y, self.z(p.z))
    }

    /// A direction's image (rotation, with the bottom side's two mirrors).
    fn direction(&self, d: Vec3) -> Vec3 {
        let o = self.xy([0.0, 0.0]);
        let [x, y] = self.xy([d.x, d.y]);
        let z = if self.board.is_some() && self.layer() == Layer::Bottom {
            -d.z
        } else {
            d.z
        };
        Vec3::new(x - o[0], y - o[1], z)
    }

    /// A box in the package frame.
    fn cuboid(&self, min: [f64; 3], max: [f64; 3]) -> Mesh {
        let outline: Vec<Point> = [
            [min[0], min[1]],
            [max[0], min[1]],
            [max[0], max[1]],
            [min[0], max[1]],
        ]
        .into_iter()
        .map(|p| self.xy(p))
        .collect();
        let (a, b) = (self.z(min[2]), self.z(max[2]));
        prism(&outline, a.min(b), a.max(b))
    }
}

/// Terminations, overcoat and solder fillets of one package.
fn package(out: &mut Vec<Geometry>, board: Option<&Board>, part: &Part<'_>) -> Result<()> {
    let frame = Frame {
        board,
        placement: part.placement,
    };
    let package = part.package;
    let [length, width, height] = package.envelope();
    let offset = package.placement.translation();
    let lo = [offset[0] - length / 2.0, offset[1] - width / 2.0, offset[2]];
    let hi = [
        offset[0] + length / 2.0,
        offset[1] + width / 2.0,
        offset[2] + height,
    ];
    if !positive(height) {
        return Ok(());
    }
    if part.body == Body::Moulded {
        let Some(footprint) = part.footprint.filter(|f| f.pads.len() <= 2) else {
            return Ok(());
        };
        for pad in &footprint.pads {
            let Some((min, max)) = lead_band(package, footprint, pad) else {
                continue;
            };
            let top = offset[2] + LEAD_HEIGHT_SHARE * height;
            if top > 0.001 {
                let mesh = frame.cuboid([min[0], min[1], 0.0], [max[0], max[1], top]);
                push(out, board, BodyKind::Termination, part.pick, None, mesh)?;
            }
        }
        return Ok(());
    }
    // Chip parts: tinned caps wrapped over both ends of the long axis.
    let axis = usize::from(width > length);
    let across = 1 - axis;
    let long = package.envelope()[axis];
    let band = (BAND_SHARE * long).min(BAND_MAX_MM);
    // A generated chip's terminations are real bodies flush with its ends.
    let g = if part.generated {
        0.0
    } else {
        TERMINATION_GROWTH_MM
    };
    for sign in [-1.0, 1.0] {
        if part.generated {
            break;
        }
        let edge = if sign > 0.0 { hi[axis] } else { lo[axis] };
        let (mut min, mut max) = ([0.0; 3], [0.0; 3]);
        min[axis] = if sign > 0.0 { edge - band } else { edge - g };
        max[axis] = if sign > 0.0 { edge + g } else { edge + band };
        min[across] = lo[across] - g;
        max[across] = hi[across] + g;
        min[2] = (lo[2] - g).max(0.0);
        max[2] = hi[2] + g;
        push(
            out,
            board,
            BodyKind::Termination,
            part.pick,
            None,
            frame.cuboid(min, max),
        )?;
    }
    if part.body == Body::Resistor && !part.generated {
        let (mut min, mut max) = ([0.0; 3], [0.0; 3]);
        min[axis] = lo[axis] + band - 2.0 * g;
        max[axis] = hi[axis] - band + 2.0 * g;
        min[across] = lo[across] + OVERCOAT_INSET_MM;
        max[across] = hi[across] - OVERCOAT_INSET_MM;
        min[2] = hi[2] - OVERCOAT_MM;
        max[2] = hi[2] + OVERCOAT_MM;
        if min[axis] < max[axis] && min[across] < max[across] {
            push(
                out,
                board,
                BodyKind::Overcoat,
                part.pick,
                None,
                frame.cuboid(min, max),
            )?;
        }
    }
    let (Some(footprint), Some(_)) = (part.footprint, board) else {
        return Ok(());
    };
    for pad in &footprint.pads {
        let Some((center, extent)) = pad_box(footprint, pad) else {
            continue;
        };
        let sign = if center[axis] >= offset[axis] {
            1.0
        } else {
            -1.0
        };
        let face = if sign > 0.0 { hi[axis] } else { lo[axis] } + sign * g;
        let toe = center[axis] + sign * extent[axis] / 2.0;
        let heel = center[axis] - sign * extent[axis] / 2.0;
        let reach = sign * (toe - face);
        let a = (center[across] - extent[across] / 2.0).max(lo[across]);
        let b = (center[across] + extent[across] / 2.0).min(hi[across]);
        // Solder wets the land only: the end face must stand over it.
        if reach < FILLET_MIN_TOE_MM || sign * (face - heel) < 0.0 || b <= a {
            continue;
        }
        let rise = (FILLET_HEIGHT_SHARE * height).min(FILLET_MAX_MM);
        let mid = (a + b) / 2.0;
        let half = (b - a) / 2.0 * FILLET_WIDTH_SHARE;
        let mesh = fillet(
            &frame,
            axis,
            sign,
            face,
            reach,
            rise,
            [mid - half, mid + half],
        );
        push(out, board, BodyKind::Solder, part.pick, None, mesh)?;
    }
    Ok(())
}

/// The lead band of a pad under a moulded body, in the package frame: the
/// pad ∩ body rectangle, grown where it reaches the body's faces. `None`
/// when they do not overlap.
fn lead_band(
    package: &PackageDefinition,
    footprint: &FootprintDefinition,
    pad: &sparksmith_core::library::Pad,
) -> Option<([f64; 2], [f64; 2])> {
    let [length, width, _] = package.envelope();
    let offset = package.placement.translation();
    let body_min = [offset[0] - length / 2.0, offset[1] - width / 2.0];
    let body_max = [offset[0] + length / 2.0, offset[1] + width / 2.0];
    let (center, extent) = pad_box(footprint, pad)?;
    let mut min = [0.0; 2];
    let mut max = [0.0; 2];
    for axis in 0..2 {
        let lo = (center[axis] - extent[axis] / 2.0).max(body_min[axis]);
        let hi = (center[axis] + extent[axis] / 2.0).min(body_max[axis]);
        if !positive(hi - lo - 1e-6) {
            return None;
        }
        let on = |a: f64, b: f64| (a - b).abs() < 1e-9;
        min[axis] = lo
            - if on(lo, body_min[axis]) {
                TERMINATION_GROWTH_MM
            } else {
                0.0
            };
        max[axis] = hi
            + if on(hi, body_max[axis]) {
                TERMINATION_GROWTH_MM
            } else {
                0.0
            };
    }
    Some((min, max))
}

/// A component-side land's centre and axis-aligned extent in the package
/// frame (the datum's axes). `None` for pads without component-side copper
/// and for pads not turned a whole number of quarter turns from the datum,
/// whose terminations and fillets are not modelled.
fn pad_box(
    footprint: &FootprintDefinition,
    pad: &sparksmith_core::library::Pad,
) -> Option<(Point, Point)> {
    if pad.side != PadSide::Top || pad.kind == PadKind::NonPlatedHole {
        return None;
    }
    let datum = footprint.datum;
    let turn = pad.rotation_deg() - datum.rotation_deg();
    if !is_quarter_turn(turn) {
        return None;
    }
    let (c, s) = cos_sin(-datum.rotation_deg());
    let d = [
        pad.position()[0] - datum.offset()[0],
        pad.position()[1] - datum.offset()[1],
    ];
    let center = [c * d[0] - s * d[1], s * d[0] + c * d[1]];
    let [w, h] = pad.shape.extent();
    let odd = cos_sin(turn).0.abs() < 0.5;
    Some((center, if odd { [h, w] } else { [w, h] }))
}

/// A concave solder meniscus from a termination's end face (`face` along
/// `axis`, facing `sign`) down to the land's toe `reach` further out, `rise`
/// high at the face, between `span` across. Along the land the profile is
/// the quadratic z = rise·(1 − s/reach)², tangent to the land at the toe
/// like a wetted meniscus; across it the solder rounds down to the land at
/// both sides, z ∝ 1 − u⁴ (u = −1..1 over the span). A closed solid with
/// no vertical side walls: every face of its top looks partly up, so it
/// reflects the studio rather than its dark lower walls.
fn fillet(
    frame: &Frame<'_>,
    axis: usize,
    sign: f64,
    face: f64,
    reach: f64,
    rise: f64,
    span: [f64; 2],
) -> Mesh {
    // The foot stands off the land's finish: the meniscus thins to nothing
    // at the toe, and lying on the finish there it flickered against it.
    let base = frame.board.map_or(0.0, |b| b.stack.finish_mm()) + PROUD_MM;
    let mid = (span[0] + span[1]) / 2.0;
    let half = (span[1] - span[0]) / 2.0;
    let local = |s: f64, c: f64, z: f64| {
        let mut p = [0.0; 2];
        p[axis] = face + sign * s;
        p[1 - axis] = c;
        frame.point(Vec3::new(p[0], p[1], z))
    };
    let local_dir = |s: f64, c: f64, z: f64| {
        let mut p = [0.0; 2];
        p[axis] = sign * s;
        p[1 - axis] = c;
        frame
            .direction(Vec3::new(p[0], p[1], z))
            .normalized()
            .unwrap_or(Vec3::Z)
    };
    // Height above the land at (t along, u across), and its outward normal
    // in (s, across, z).
    let surface = |t: f64, u: f64| {
        let along = (1.0 - t) * (1.0 - t);
        let across = 1.0 - u * u * u * u;
        let z = base + rise * along * across;
        let dz_ds = -2.0 * rise * (1.0 - t) * across / reach;
        let dz_dc = -4.0 * rise * along * u * u * u / half;
        (z, local_dir(-dz_ds, -dz_dc, 1.0))
    };
    let mut mesh = Mesh::default();
    // Is the (s, across, z) frame left-handed? Then flip the windings.
    let flip = {
        let mut x = [0.0; 2];
        x[axis] = sign;
        let mut y = [0.0; 2];
        y[1 - axis] = 1.0;
        x[0] * y[1] - x[1] * y[0] < 0.0
    };
    let tri = |mesh: &mut Mesh, a: u32, b: u32, c: u32, face: u32| {
        mesh.triangles
            .push(if flip { [a, c, b] } else { [a, b, c] });
        mesh.tri_face.push(face);
    };
    let vertex = |mesh: &mut Mesh, p: Vec3, n: Vec3| -> u32 {
        mesh.positions.push(p);
        mesh.normals.push(n);
        u32::try_from(mesh.positions.len() - 1).unwrap_or(u32::MAX)
    };
    let across = |j: usize| -1.0 + 2.0 * j as f64 / FILLET_ACROSS_STEPS as f64;
    // Curved top: a grid along (t) and across (u).
    let row = FILLET_ACROSS_STEPS + 1;
    let first = u32::try_from(mesh.positions.len()).unwrap_or(u32::MAX);
    for k in 0..=FILLET_STEPS {
        let t = k as f64 / FILLET_STEPS as f64;
        for j in 0..row {
            let u = across(j);
            let (z, n) = surface(t, u);
            vertex(&mut mesh, local(reach * t, mid + half * u, z), n);
        }
    }
    let at = |k: usize, j: usize| first + u32::try_from(k * row + j).unwrap_or(u32::MAX);
    for k in 0..FILLET_STEPS {
        for j in 0..FILLET_ACROSS_STEPS {
            tri(&mut mesh, at(k, j), at(k + 1, j), at(k + 1, j + 1), 0);
            tri(&mut mesh, at(k, j), at(k + 1, j + 1), at(k, j + 1), 0);
        }
    }
    // The back (against the termination), fanned from its foot's middle.
    let back = local_dir(-1.0, 0.0, 0.0);
    let foot = vertex(&mut mesh, local(0.0, mid, base), back);
    let rim: Vec<u32> = (0..row)
        .map(|j| {
            let u = across(j);
            vertex(
                &mut mesh,
                local(0.0, mid + half * u, surface(0.0, u).0),
                back,
            )
        })
        .collect();
    for w in rim.windows(2) {
        tri(&mut mesh, foot, w[0], w[1], 1);
    }
    // The bottom, on the land.
    let down = local_dir(0.0, 0.0, -1.0);
    let q = [
        vertex(&mut mesh, local(0.0, span[0], base), down),
        vertex(&mut mesh, local(0.0, span[1], base), down),
        vertex(&mut mesh, local(reach, span[1], base), down),
        vertex(&mut mesh, local(reach, span[0], base), down),
    ];
    tri(&mut mesh, q[0], q[1], q[2], 2);
    tri(&mut mesh, q[0], q[2], q[3], 2);
    mesh
}

/// The corners [`prism`] walls `outline` with, in its order: repeated
/// points (and a closing one) dropped, counter-clockwise. Its wall `i`
/// runs from corner `i` to the next and is face `2 + i` (top 0, bottom 1).
pub fn prism_points(outline: &[Point]) -> Vec<Point> {
    let mut points: Vec<Point> = outline.to_vec();
    points.dedup_by(|a, b| (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-9);
    if points.len() > 1
        && let (Some(first), Some(last)) = (points.first(), points.last())
        && (first[0] - last[0]).hypot(first[1] - last[1]) < 1e-9
    {
        points.pop();
    }
    let area: f64 = (0..points.len())
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    if area < 0.0 {
        points.reverse();
    }
    points
}

/// A closed prism over a convex outline from `z0` up to `z1`, outward
/// wound. Walls meeting at a gentle turn share a normal, so round ends shade
/// smoothly.
pub fn prism(outline: &[Point], z0: f64, z1: f64) -> Mesh {
    let points = prism_points(outline);
    let n = points.len();
    let mut mesh = Mesh::default();
    if n < 3 || !positive(z1 - z0) {
        return mesh;
    }
    let index = |mesh: &Mesh| u32::try_from(mesh.positions.len()).unwrap_or(u32::MAX);
    for (z, up) in [(z1, true), (z0, false)] {
        let start = index(&mesh);
        for p in &points {
            mesh.positions.push(Vec3::new(p[0], p[1], z));
            mesh.normals
                .push(Vec3::new(0.0, 0.0, if up { 1.0 } else { -1.0 }));
        }
        for i in 1..n - 1 {
            let (b, c) = (start + i as u32, start + i as u32 + 1);
            mesh.triangles
                .push(if up { [start, b, c] } else { [start, c, b] });
            mesh.tri_face.push(u32::from(!up));
        }
    }
    let wall = |i: usize| {
        let (a, b) = (points[i % n], points[(i + 1) % n]);
        Vec3::new(b[1] - a[1], a[0] - b[0], 0.0)
            .normalized()
            .unwrap_or(Vec3::X)
    };
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        let w = wall(i);
        let at = |k: usize, own: Vec3| {
            let other = if k == i { wall(i + n - 1) } else { wall(i + 1) };
            if own.dot(other) > SMOOTH_TURN.cos() {
                (own + other).normalized().unwrap_or(own)
            } else {
                own
            }
        };
        let (na, nb) = (at(i, w), at(i + 1, w));
        let start = index(&mesh);
        for (p, z, normal) in [(a, z0, na), (b, z0, nb), (b, z1, nb), (a, z1, na)] {
            mesh.positions.push(Vec3::new(p[0], p[1], z));
            mesh.normals.push(normal);
        }
        let face = 2 + u32::try_from(i).unwrap_or(u32::MAX - 2);
        mesh.triangles.push([start, start + 1, start + 2]);
        mesh.triangles.push([start, start + 2, start + 3]);
        mesh.tri_face.extend([face, face]);
    }
    mesh
}

/// Display bodies alike at every via (its dome, bore and finished rings):
/// made once at the origin per key and placed at each via, sharing one
/// mesh, instead of a copy per via.
#[derive(Default)]
struct Shared(std::collections::HashMap<Vec<u64>, Vec<Geometry>>);

impl Shared {
    /// The bodies `make` builds at the origin (made on the key's first use)
    /// placed at `at` with pick identity `item`.
    fn place(
        &mut self,
        out: &mut Vec<Geometry>,
        key: Vec<u64>,
        item: Option<u64>,
        at: Point,
        make: impl FnOnce(&mut Vec<Geometry>) -> Result<()>,
    ) -> Result<()> {
        let bodies = match self.0.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let mut made = Vec::new();
                make(&mut made)?;
                e.insert(made)
            }
        };
        let shift = |p: [f64; 3]| [p[0] + at[0], p[1] + at[1], p[2]];
        for body in bodies.iter() {
            out.push(Geometry {
                item,
                place: Some(Place::translation([at[0], at[1], 0.0])),
                min: shift(body.min),
                max: shift(body.max),
                ..body.clone()
            });
        }
        Ok(())
    }
}

/// Finite and above zero.
fn positive(value: f64) -> bool {
    value > 0.0 && value.is_finite()
}

/// Add an appearance body of `kind` on `layer` (the copper layer it lies
/// on, or the side a film covers as its outer layer; `None` for package
/// layers), unless its mesh is empty.
pub(crate) fn push(
    out: &mut Vec<Geometry>,
    board: Option<&Board>,
    kind: BodyKind,
    item: Option<u64>,
    layer: Option<CopperLayer>,
    mesh: Mesh,
) -> Result<()> {
    if mesh.triangles.is_empty() {
        return Ok(());
    }
    // A package preview has no board: the default looks.
    let (looks, stack) = match board {
        Some(board) => (board.fabrication, std::borrow::Cow::Borrowed(&board.stack)),
        None => (
            Fabrication::default(),
            std::borrow::Cow::Owned(Stack::default()),
        ),
    };
    let bounds = mesh.bounds();
    let volume = mesh.measure().volume;
    if !positive(volume) {
        return Err(Error::Verification(format!(
            "{kind:?} appearance layer is not a closed outward volume"
        )));
    }
    out.push(Geometry {
        item,
        kind,
        layer,
        dielectric: None,
        surface: material::material(kind, &looks, &stack).surface()?,
        detail: material::detail(kind, &looks),
        mesh: Arc::new(mesh),
        place: None,
        volume: crate::scene::Volume::known(volume),
        min: [bounds.min.x, bounds.min.y, bounds.min.z],
        max: [bounds.max.x, bounds.max.y, bounds.max.z],
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn prisms_are_closed_outward_volumes_either_way_round() {
        let square = [[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [0.0, 1.0]];
        let mut reversed = square;
        reversed.reverse();
        for outline in [square, reversed] {
            let mesh = prism(&outline, 0.5, 0.75);
            near(mesh.measure().volume, 2.0 * 1.0 * 0.25);
            assert_eq!(mesh.triangles.len(), 2 + 2 + 8);
            assert_eq!(mesh.tri_face.len(), mesh.triangles.len());
            assert_eq!(mesh.normals.len(), mesh.positions.len());
        }
        assert!(prism(&square, 1.0, 1.0).triangles.is_empty());
        assert!(prism(&square[..2], 0.0, 1.0).triangles.is_empty());
    }

    #[test]
    fn stadium_ends_are_round_and_butt_ends_square() {
        let mut outline = Vec::new();
        end(&mut outline, [4.0, 0.0], [1.0, 0.0], 0.5, true);
        end(&mut outline, [0.0, 0.0], [-1.0, 0.0], 0.5, false);
        let mesh = prism(&outline, 0.0, 1.0);
        let b = mesh.bounds();
        near(b.min.x, 0.0);
        near(b.max.x, 4.5);
        near(b.min.y, -0.5);
        near(b.max.y, 0.5);
        // A rectangle plus a half disc of eight chords, slightly under π r²/2.
        let volume = mesh.measure().volume;
        let ideal = 4.0 + std::f64::consts::PI * 0.25 / 2.0;
        assert!(volume < ideal && volume > ideal - 0.02, "{volume}");
    }

    /// Laminate a mask opening leaves bare (a fiducial's clearance) is its
    /// own film on the laminate: the opening less the copper, on each side
    /// with mask only. The mask reaches the routed edge (no pull-back, as
    /// in the Gerber), so no laminate shows there.
    #[test]
    fn a_mask_opening_shows_the_laminate_around_its_copper() -> Result<()> {
        let stackup = sparksmith_core::Stackup {
            mask_bottom: false,
            ..sparksmith_core::Stackup::default()
        };
        let board = Board {
            width: 10.0,
            depth: 10.0,
            shape: None,
            stack: Stack::of(&stackup, Fabrication::default().pad_finish),
            fabrication: Fabrication::default(),
        };
        let land = Land {
            pick: Some(1),
            layer: Layer::Top,
            copper: Some(circle([5.0, 5.0], 0.5)),
            bore: None,
            openings: vec![circle([5.0, 5.0], 1.0)],
        };
        let films = open_laminate(&board, &[land], &[], &[])?;
        assert_eq!(films.len(), 1, "the opening, top only");
        assert_eq!(
            mask_outline(&board),
            (
                vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]],
                vec![]
            ),
            "the mask reaches the board edge"
        );
        let disc = |r: f64| {
            let n = VIA_SEGMENTS as f64;
            0.5 * n * r * r * (std::f64::consts::TAU / n).sin()
        };
        for (film, area) in films.iter().zip([disc(1.0) - disc(0.5)]) {
            assert_eq!(film.kind, BodyKind::OpenLaminate);
            assert_eq!(film.layer, Some(CopperLayer::Top));
            near(film.min[2], 1.6);
            near(film.max[2], 1.6 + PROUD_MM);
            assert!((film.volume() - area * PROUD_MM).abs() < 1e-9, "{area}");
        }
        Ok(())
    }
}
