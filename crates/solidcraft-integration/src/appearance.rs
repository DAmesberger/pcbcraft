//! Display-only appearance layers over the native model: soldermask, finish
//! over exposed copper, package terminations, resistor overcoat and solder
//! fillets. They are thin films and decorations derived from the project, in
//! Solidcraft's mesh type, built on the model worker. They are drawn and
//! picked with the native bodies but never stored in the `.solidcraft` file,
//! so the saved model's bodies stay disjoint (except copper joining copper)
//! and its mass properties stay physical. Dimensions and sources are in
//! `docs/materials.md`.
use crate::material::{self, BodyKind};
use crate::scene::Geometry;
use crate::{Error, Result};
use pcbcraft_core::library::{FootprintDefinition, PackageBody, PackageDefinition};
use pcbcraft_core::{Fabrication, Layer, Placement, Point, layout};
use solidcraft_engine::geom::{Mesh, Vec3};
use std::sync::Arc;

/// Soldermask over bare laminate (display thickness; see `docs/materials.md`).
pub const MASK_THICKNESS_MM: f64 = 0.020;
/// Soldermask above a track's copper.
pub const MASK_CAP_MM: f64 = 0.015;
/// ENIG nickel (3–6 µm, IPC-4552); the immersion gold is negligible.
pub const FINISH_THICKNESS_MM: f64 = 0.005;
/// Mask opening beyond each land (a common 50 µm fabrication default).
pub const MASK_EXPANSION_MM: f64 = 0.05;
/// How far a track's mask cap overhangs its copper on each side.
const CAP_MARGIN_MM: f64 = 0.01;
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
const FILLET_STEPS: usize = 10;
/// Adjacent prism walls turning less than this share a smoothed normal.
const SMOOTH_TURN: f64 = 0.55;

/// The board the layers sit on, in mm, and the fabrication that sets the
/// mask and finish looks.
#[derive(Clone, Copy, Debug)]
pub struct Board {
    pub width: f64,
    pub depth: f64,
    pub thickness: f64,
    pub copper: f64,
    pub fabrication: Fabrication,
}

impl Board {
    /// World z of a height above a copper layer's board face (below it for
    /// the bottom layer).
    fn z(&self, layer: Layer, height: f64) -> f64 {
        match layer {
            Layer::Top => self.thickness + height,
            Layer::Bottom => -height,
        }
    }

    fn span(&self, layer: Layer, low: f64, high: f64) -> (f64, f64) {
        let (a, b) = (self.z(layer, low), self.z(layer, high));
        (a.min(b), a.max(b))
    }
}

/// A land's copper rectangle in board coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Land {
    pub pick: u64,
    pub center: Point,
    pub half: Point,
    pub angle: f64,
    pub layer: Layer,
}

impl Land {
    fn local(&self, v: Point) -> Point {
        let (s, c) = self.angle.sin_cos();
        [c * v[0] + s * v[1], -s * v[0] + c * v[1]]
    }

    /// The parameter interval of the line `point + t·direction` (unit
    /// direction) inside this rectangle grown by `grow`.
    fn crossing(&self, point: Point, direction: Point, grow: f64) -> Option<(f64, f64)> {
        let q = self.local([point[0] - self.center[0], point[1] - self.center[1]]);
        let v = self.local(direction);
        let (mut enter, mut leave) = (f64::NEG_INFINITY, f64::INFINITY);
        for i in 0..2 {
            let half = self.half[i] + grow;
            if v[i].abs() < 1e-12 {
                if q[i].abs() > half {
                    return None;
                }
                continue;
            }
            let a = (-half - q[i]) / v[i];
            let b = (half - q[i]) / v[i];
            enter = enter.max(a.min(b));
            leave = leave.min(a.max(b));
        }
        (enter < leave).then_some((enter, leave))
    }

    fn corners(&self, grow: f64) -> Vec<Point> {
        let (s, c) = self.angle.sin_cos();
        let [hx, hy] = [self.half[0] + grow, self.half[1] + grow];
        [[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]]
            .into_iter()
            .map(|[x, y]| {
                [
                    self.center[0] + c * x - s * y,
                    self.center[1] + s * x + c * y,
                ]
            })
            .collect()
    }
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
        match package.body {
            PackageBody::Moulded => Self::Moulded,
            PackageBody::ChipResistor => Self::Resistor,
            PackageBody::ChipCapacitor => Self::Ceramic,
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
}

/// Every appearance layer of a board.
pub fn board(
    board: &Board,
    lands: &[Land],
    paths: &[Path],
    parts: &[Part<'_>],
) -> Result<Vec<Geometry>> {
    let mut out = Vec::new();
    let outline = [
        [0.0, 0.0],
        [board.width, 0.0],
        [board.width, board.depth],
        [0.0, board.depth],
    ];
    for (kind, layer) in [
        (BodyKind::MaskTop, Layer::Top),
        (BodyKind::MaskBottom, Layer::Bottom),
    ] {
        let (z0, z1) = board.span(layer, 0.0, MASK_THICKNESS_MM);
        push(
            &mut out,
            &board.fabrication,
            kind,
            None,
            prism(&outline, z0, z1),
        )?;
    }
    for land in lands {
        let (z0, z1) = board.span(land.layer, 0.0, board.copper + FINISH_THICKNESS_MM);
        let mesh = prism(&land.corners(FINISH_THICKNESS_MM), z0, z1);
        push(
            &mut out,
            &board.fabrication,
            BodyKind::PadFinish,
            Some(land.pick),
            mesh,
        )?;
    }
    for path in paths {
        track(&mut out, board, lands, path)?;
    }
    for part in parts {
        package(&mut out, Some(board), part)?;
    }
    Ok(out)
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
fn track(out: &mut Vec<Geometry>, board: &Board, lands: &[Land], path: &Path) -> Result<()> {
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
    let openings = merged(
        lands
            .iter()
            .filter(|land| land.layer == path.layer)
            .filter_map(|land| {
                [-band, 0.0, band]
                    .into_iter()
                    .filter_map(|o| land.crossing(at(0.0, o), u, MASK_EXPANSION_MM))
                    .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
            })
            .filter(|(a, b)| *b > -band && *a < length + band)
            .collect(),
    );
    let (c0, c1) = board.span(path.layer, 0.0, board.copper + MASK_CAP_MM);
    let mut from = (0.0, true);
    let mut pieces = Vec::new();
    for &(a, b) in &openings {
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
            &board.fabrication,
            BodyKind::MaskCap,
            Some(path.pick),
            prism(&outline, c0, c1),
        )?;
    }
    let (f0, f1) = board.span(
        path.layer,
        0.0,
        board.copper + STRIP_SHARE * FINISH_THICKNESS_MM,
    );
    let half = path.width / 2.0 + STRIP_SHARE * FINISH_THICKNESS_MM;
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
            &board.fabrication,
            BodyKind::PadFinish,
            Some(path.pick),
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
            Some(board) => board.z(self.layer(), board.copper + h),
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
    let looks = board.map_or_else(Fabrication::default, |b| b.fabrication);
    let package = part.package;
    let [length, width, height] = package.body_size;
    let offset = package.model_offset;
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
                push(out, &looks, BodyKind::Termination, part.pick, mesh)?;
            }
        }
        return Ok(());
    }
    // Chip parts: tinned caps wrapped over both ends of the long axis.
    let axis = usize::from(width > length);
    let across = 1 - axis;
    let long = package.body_size[axis];
    let band = (BAND_SHARE * long).min(BAND_MAX_MM);
    let g = TERMINATION_GROWTH_MM;
    for sign in [-1.0, 1.0] {
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
            &looks,
            BodyKind::Termination,
            part.pick,
            frame.cuboid(min, max),
        )?;
    }
    if part.body == Body::Resistor {
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
                &looks,
                BodyKind::Overcoat,
                part.pick,
                frame.cuboid(min, max),
            )?;
        }
    }
    let (Some(footprint), Some(_)) = (part.footprint, board) else {
        return Ok(());
    };
    for pad in &footprint.pads {
        let center = [
            pad.position[0] - footprint.datum_offset[0],
            pad.position[1] - footprint.datum_offset[1],
        ];
        let sign = if center[axis] >= offset[axis] {
            1.0
        } else {
            -1.0
        };
        let face = if sign > 0.0 { hi[axis] } else { lo[axis] } + sign * g;
        let toe = center[axis] + sign * pad.size[axis] / 2.0;
        let heel = center[axis] - sign * pad.size[axis] / 2.0;
        let reach = sign * (toe - face);
        let a = (center[across] - pad.size[across] / 2.0).max(lo[across]);
        let b = (center[across] + pad.size[across] / 2.0).min(hi[across]);
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
        push(out, &looks, BodyKind::Solder, part.pick, mesh)?;
    }
    Ok(())
}

/// The lead band of a pad under a moulded body, in the package frame: the
/// pad ∩ body rectangle, grown where it reaches the body's faces. `None`
/// when they do not overlap.
fn lead_band(
    package: &PackageDefinition,
    footprint: &FootprintDefinition,
    pad: &pcbcraft_core::library::Pad,
) -> Option<([f64; 2], [f64; 2])> {
    let [length, width, _] = package.body_size;
    let offset = package.model_offset;
    let body_min = [offset[0] - length / 2.0, offset[1] - width / 2.0];
    let body_max = [offset[0] + length / 2.0, offset[1] + width / 2.0];
    let mut min = [0.0; 2];
    let mut max = [0.0; 2];
    for axis in 0..2 {
        let center = pad.position[axis] - footprint.datum_offset[axis];
        let lo = (center - pad.size[axis] / 2.0).max(body_min[axis]);
        let hi = (center + pad.size[axis] / 2.0).min(body_max[axis]);
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

/// A concave solder meniscus from a termination's end face (`face` along
/// `axis`, facing `sign`) down to the land's toe `reach` further out, `rise`
/// high at the face, between `span` across. The profile is the quadratic
/// z = rise·(1 − s/reach)², tangent to the land at the toe like a wetted
/// meniscus.
fn fillet(
    frame: &Frame<'_>,
    axis: usize,
    sign: f64,
    face: f64,
    reach: f64,
    rise: f64,
    span: [f64; 2],
) -> Mesh {
    let base = FINISH_THICKNESS_MM;
    // Profile in (s outward, z); the region under it is star-shaped from
    // the face foot (0, base), so its side walls fan from there.
    let profile: Vec<(f64, f64, f64, f64)> = (0..=FILLET_STEPS)
        .map(|k| {
            let t = k as f64 / FILLET_STEPS as f64;
            let s = reach * t;
            let z = base + rise * (1.0 - t) * (1.0 - t);
            // Outward normal of the curve: (−dz/ds, 1), normalised.
            let slope = -2.0 * rise * (1.0 - t) / reach;
            let n = (-slope).hypot(1.0);
            (s, z, -slope / n, 1.0 / n)
        })
        .collect();
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
    // Curved top: rows at both span ends.
    let first = u32::try_from(mesh.positions.len()).unwrap_or(u32::MAX);
    for &(s, z, ns, nz) in &profile {
        let n = local_dir(ns, 0.0, nz);
        vertex(&mut mesh, local(s, span[0], z), n);
        vertex(&mut mesh, local(s, span[1], z), n);
    }
    for k in 0..FILLET_STEPS {
        let i = first + 2 * k as u32;
        tri(&mut mesh, i, i + 2, i + 3, 0);
        tri(&mut mesh, i, i + 3, i + 1, 0);
    }
    // Side walls, fanned from the face foot.
    for (c, outward, face_id) in [(span[0], -1.0, 1), (span[1], 1.0, 2)] {
        let n = local_dir(0.0, outward, 0.0);
        let foot = vertex(&mut mesh, local(0.0, c, base), n);
        let rim: Vec<u32> = profile
            .iter()
            .map(|&(s, z, _, _)| vertex(&mut mesh, local(s, c, z), n))
            .collect();
        for w in rim.windows(2) {
            if outward > 0.0 {
                tri(&mut mesh, foot, w[0], w[1], face_id);
            } else {
                tri(&mut mesh, foot, w[1], w[0], face_id);
            }
        }
    }
    // The back (against the termination) and the bottom (on the land).
    let top = base + rise;
    let back = local_dir(-1.0, 0.0, 0.0);
    let q = [
        vertex(&mut mesh, local(0.0, span[0], base), back),
        vertex(&mut mesh, local(0.0, span[1], base), back),
        vertex(&mut mesh, local(0.0, span[1], top), back),
        vertex(&mut mesh, local(0.0, span[0], top), back),
    ];
    tri(&mut mesh, q[0], q[2], q[1], 3);
    tri(&mut mesh, q[0], q[3], q[2], 3);
    let down = local_dir(0.0, 0.0, -1.0);
    let q = [
        vertex(&mut mesh, local(0.0, span[0], base), down),
        vertex(&mut mesh, local(0.0, span[1], base), down),
        vertex(&mut mesh, local(reach, span[1], base), down),
        vertex(&mut mesh, local(reach, span[0], base), down),
    ];
    tri(&mut mesh, q[0], q[1], q[2], 4);
    tri(&mut mesh, q[0], q[2], q[3], 4);
    mesh
}

/// A closed prism over a convex outline from `z0` up to `z1`, outward
/// wound. Walls meeting at a gentle turn share a normal, so round ends shade
/// smoothly.
pub fn prism(outline: &[Point], z0: f64, z1: f64) -> Mesh {
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

/// Finite and above zero.
fn positive(value: f64) -> bool {
    value > 0.0 && value.is_finite()
}

fn push(
    out: &mut Vec<Geometry>,
    looks: &Fabrication,
    kind: BodyKind,
    item: Option<u64>,
    mesh: Mesh,
) -> Result<()> {
    if mesh.triangles.is_empty() {
        return Ok(());
    }
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
        surface: material::material(kind, looks).surface()?,
        detail: material::detail(kind, looks),
        mesh: Arc::new(mesh),
        volume,
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
}
