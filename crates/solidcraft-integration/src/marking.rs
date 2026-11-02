//! Slices D5 and D7: instance marking decals. A core [`MarkingLayout`] in
//! its chart's 2D frame becomes triangles on the model's surface, from
//! Solidcraft's text-outline geometry and the SVG converter over its
//! public reader, drawn with the finish material's look. Display only:
//! never saved to `.solidcraft`, never in fabrication output; a value
//! change rebuilds only the decal keyed by its layout in a [`DecalCache`],
//! never the model.
//!
//! **Approach.** Decals are tessellated to meshes, not rasterised to
//! textures: a vector outline stays sharp at any zoom (a 0.3 mm marking
//! read under a microscope view), it is drawn through the same PBR path
//! and product look as the bodies (Solidcraft's renderer has no texture
//! slot in that path), and identical layouts share one flat decal across
//! instances ([`DecalCache`]). Embedded raster images in artwork become
//! pixel-run rectangles at a bounded resolution.
//!
//! **The layout frame.** Millimetres, origin at the area centre, +x along
//! the chart's u axis and +y along v, so a decal reads upright seen from
//! the side its normal points to. Each element's `origin` is its centre: a
//! text line is centred on it horizontally and its capitals span
//! `height_mm` about it vertically (the core lays out without font
//! metrics, so a line the font sets wider than the area is shrunk about
//! its origin to fit). Bands run from the −x end: band `i` is centred
//! `pitch · (i + 1)` from it and half a pitch wide, across the area.
//!
//! **Placing.** The layout is moved into its chart region by its
//! [`Placement`] (scale, mirror, turn, shift), repeated for a wrap's
//! copies, clipped to the region (a full turn's round side is never
//! clipped: it continues past the seam), then taken onto the surface by
//! a [`map::SurfaceMap`]: a plane's frame, or a cylinder's arc-length wrap
//! subdivided so its chords stay within [`map::WRAP_CHORD_MM`].
//!
//! **Looks** ([`finish_surface_on`]). Laser etch on a moulding is the
//! cited `EpoxyLaserMarked`; on metal `MetalLaserMarked`, a duller, rougher
//! metal that still reflects the studio as the can does; ink and pad print
//! their inks, white or the chosen colour, on a raised layer
//! ([`Finish::lift_mm`]); an engraving the face's own look, darkened as a
//! recess, with walls whose normals tilt into it ([`ENGRAVE_WALL_MM`]):
//! bump only, no geometry. Band colours are estimates of the IEC 60062
//! colour code ([`band_colour`]).
//!
//! **Depth.** Every decal stands at least [`DECAL_OFFSET_MM`] off its face
//! (ink higher), drawn in the same opaque pass as the bodies, so the
//! reversed-float depth buffer keeps it in front ([`DECAL_DEPTH_STEPS_AT_1_M`]).
//!
//! Placement on a board and on a package preview is in [`board`].
pub mod board;
mod fill;
pub mod map;
pub mod paint;
mod shapes;
#[cfg(test)]
mod surface_tests;
#[cfg(test)]
mod tests;

use crate::material::{Detail, Pattern, srgb8_to_linear};
use crate::models::look;
use crate::{Error, Result};
use fill::{FillRule, Flat};
use map::SurfaceMap;
use pcbcraft_core::Point;
use pcbcraft_core::ids::{ContentHash, Id};
use pcbcraft_core::library::marking::surface::{CylinderArea, WrapChart};
use pcbcraft_core::library::marking::{
    Finish, LaidElement, MarkingLayout, Placement, PolarityMark, Symbol,
};
use pcbcraft_core::library::model3d::MarkingArea;
use solidcraft_engine::geom::Vec3;
use solidcraft_engine::render::surface::OpaqueSurface;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

pub use fill::MAX_FILL_EDGES;
pub use shapes::{CHORD_MM, MAX_TEXT_CHARS, TEXT_HEIGHT_MM};

/// The least height of a decal above its face, mm: 5 µm, thinner than
/// marking ink and far below what the eye resolves, yet clear of
/// z-fighting. The viewport's depth buffer is reversed 32-bit float
/// (Solidcraft's `reversed_depth`), whose steps grow with distance: a lift
/// `h` seen from `d` away separates the decal from its face by at least
/// `h / d · 2²³` depth steps, about 42 at 1 m, the farthest a board-fit
/// view of a large board stands ([`DECAL_DEPTH_STEPS_AT_1_M`]). Decals
/// draw in the same opaque pass as the bodies, so Solidcraft's face bias
/// pushes both alike. Ink stands higher ([`Finish::lift_mm`]).
pub const DECAL_OFFSET_MM: f64 = 0.005;
/// The reversed-float depth steps between a decal and its face seen from
/// 1 m (see [`DECAL_OFFSET_MM`]).
pub const DECAL_DEPTH_STEPS_AT_1_M: f64 = DECAL_OFFSET_MM / 1000.0 * 8_388_608.0;
/// Triangles one flat decal may have.
pub const MAX_DECAL_TRIANGLES: usize = 65_536;
/// Triangles one placed decal may have once wrapped round a cylinder.
pub const MAX_PLACED_TRIANGLES: usize = 262_144;
/// Elements one layout may have.
pub const MAX_DECAL_ELEMENTS: usize = 32;
/// Decals one [`DecalCache`] keeps; past it the cache starts again.
pub const MAX_CACHED_DECALS: usize = 4096;
/// Largest marking area side, mm.
pub const MAX_AREA_MM: f64 = 500.0;
/// Width of an engraving's shaded wall inside its outline, mm (est.: the
/// visible bevel of a laser- or mould-engraved mark).
pub const ENGRAVE_WALL_MM: f64 = 0.03;
/// How far an engraved wall's normal tilts into the recess (sine of the
/// tilt, est. 50°).
const ENGRAVE_TILT: f64 = 0.766;
/// The wall stands this much above the floor, so the two never fight, mm.
const WALL_LIFT_MM: f64 = 0.001;
/// An electrolytic stripe's minus signs: width and stroke as shares of the
/// stripe's width, and their pitch along it.
const MINUS_WIDTH: f64 = 0.45;
const MINUS_STROKE: f64 = 0.12;
const MINUS_PITCH: f64 = 0.8;
/// Minus signs one stripe draws at most.
const MAX_MINUS_SIGNS: usize = 64;

/// What a run of a decal's triangles is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ink {
    /// The finish's look.
    Finish,
    /// IEC 60062 colour index ([`band_colour`]).
    Band(u8),
    /// An artwork colour (sRGB) in the finish's ink.
    Srgb([u8; 3]),
    /// An engraving's wall.
    Wall,
}

/// A run of a placed decal's triangles in one ink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InkRun {
    pub ink: Ink,
    /// The run's range of [`DecalMesh::indices`] (whole triangles).
    pub indices: Range<usize>,
}

/// A decal mesh in the model frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecalMesh {
    pub positions: Vec<[f32; 3]>,
    /// Per vertex: the surface's outward normal (a wrapped decal follows
    /// its cylinder; an engraving's walls tilt).
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub finish: Finish,
    /// Runs in inks other than the finish; the rest take the finish.
    pub runs: Vec<InkRun>,
}

impl DecalMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// A laid-out marking as flat triangles in its chart: what the cache
/// keeps, placed on a surface by [`place_on`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlatDecal {
    points: Vec<Point>,
    /// Per point: the chart direction an engraved wall's normal tilts to,
    /// scaled by the sine of the tilt; zero on flat parts.
    tilts: Vec<[f64; 2]>,
    /// Per point: the paint layer it is lifted to (overpainted artwork).
    layers: Vec<u8>,
    triangles: Vec<[u32; 3]>,
    finish: Finish,
    /// Triangle ranges in inks other than the finish.
    runs: Vec<(Ink, Range<usize>)>,
}

impl FlatDecal {
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// The chart points.
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// The counter-clockwise triangles over [`Self::points`].
    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }

    /// The runs of triangles in inks other than the finish.
    pub fn runs(&self) -> &[(Ink, Range<usize>)] {
        &self.runs
    }

    /// The marked area, mm² (each triangle counted once).
    pub fn area_mm2(&self) -> f64 {
        self.area_where(|_| true)
    }

    /// The area drawn in `ink`, mm².
    pub fn ink_area_mm2(&self, ink: Ink) -> f64 {
        self.area_where(|i| i == ink)
    }

    fn ink_of(&self, triangle: usize) -> Ink {
        self.runs
            .iter()
            .find(|(_, r)| r.contains(&triangle))
            .map_or(Ink::Finish, |(ink, _)| *ink)
    }

    fn area_where(&self, keep: impl Fn(Ink) -> bool) -> f64 {
        self.triangles
            .iter()
            .enumerate()
            .filter(|(k, _)| keep(self.ink_of(*k)))
            .filter_map(|(_, t)| {
                let p = |i: u32| self.points.get(i as usize).copied();
                let (a, b, c) = (p(t[0])?, p(t[1])?, p(t[2])?);
                Some(((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2.0)
            })
            .sum()
    }

    /// Whether chart point `p` is drawn (in any ink).
    pub fn covers(&self, p: Point) -> bool {
        self.triangles.iter().any(|t| {
            let q = t.map(|i| {
                self.points
                    .get(i as usize)
                    .copied()
                    .unwrap_or([f64::NAN; 2])
            });
            let s =
                |a: Point, b: Point| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
            s(q[0], q[1]) >= -1e-12 && s(q[1], q[2]) >= -1e-12 && s(q[2], q[0]) >= -1e-12
        })
    }

    /// The chart bounds of what is drawn: [x0, y0, x1, y1].
    pub fn bounds(&self) -> Option<[f64; 4]> {
        let used = self
            .triangles
            .iter()
            .flatten()
            .filter_map(|i| self.points.get(*i as usize));
        let mut b = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        let mut any = false;
        for p in used {
            any = true;
            b = [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ];
        }
        any.then_some(b)
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }
}

/// SVG artwork text by content hash, from wherever the host keeps assets.
pub type ArtworkSource<'a> = &'a dyn Fn(&ContentHash) -> Option<String>;

/// No artwork: artwork elements draw nothing.
pub fn no_artwork(_: &ContentHash) -> Option<String> {
    None
}

/// The chart region a layout is placed in.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    /// The region's size, mm, centred on the chart origin.
    pub size: [f64; 2],
    /// Sides that close on themselves (a full turn round a cylinder):
    /// never clipped, a mark past the edge continues round.
    pub periodic: [bool; 2],
    /// The area one copy of the layout fills (before the placement).
    pub cell: [f64; 2],
    /// Each copy's centre in the chart (a wrap's repeats side by side).
    pub copies: Vec<[f64; 2]>,
}

impl Region {
    /// A plane's region: the face, one copy.
    pub fn plane(size: [f64; 2]) -> Self {
        Self {
            size,
            periodic: [false, false],
            cell: size,
            copies: vec![[0.0, 0.0]],
        }
    }

    /// A wrap's region on its cylinder: the unrolled side, its repeats
    /// side by side round it.
    pub fn wrap(cylinder: &CylinderArea, chart: &WrapChart) -> Self {
        let round = usize::from(cylinder.along_u);
        let n = chart.repeat.max(1);
        let full = chart.span >= std::f64::consts::TAU - 1e-9;
        let mut cell = chart.region;
        cell[round] /= f64::from(n);
        let copies = (0..n)
            .map(|k| {
                let mut c = [0.0, 0.0];
                c[round] = (f64::from(k) + 0.5) * cell[round] - chart.region[round] / 2.0;
                c
            })
            .collect();
        let mut periodic = [false, false];
        periodic[round] = full;
        Self {
            size: chart.region,
            periodic,
            cell,
            copies,
        }
    }

    fn hash(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for v in self
            .size
            .iter()
            .chain(&self.cell)
            .chain(self.copies.iter().flatten())
        {
            v.to_bits().hash(&mut h);
        }
        self.periodic.hash(&mut h);
        h.finish()
    }
}

/// Mesh `layout` on a flat `area`, artwork drawing nothing (see
/// [`decal_with`]).
pub fn decal(layout: &MarkingLayout, area: &MarkingArea) -> Result<DecalMesh> {
    decal_with(layout, area, &no_artwork)
}

/// Mesh `layout` on a flat `area`, reading artwork from `artwork`. An
/// element that cannot be drawn refuses the whole decal, so a marking is
/// shown whole or not at all.
pub fn decal_with(
    layout: &MarkingLayout,
    area: &MarkingArea,
    artwork: ArtworkSource<'_>,
) -> Result<DecalMesh> {
    let map = SurfaceMap::plane(area)?;
    let flat = flatten(layout, &Region::plane(area.size_mm), artwork)?;
    place_on(&flat, &map)
}

/// Mesh a cached flat decal on a flat `area`.
pub fn place(flat: &FlatDecal, area: &MarkingArea) -> Result<DecalMesh> {
    place_on(flat, &SurfaceMap::plane(area)?)
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(format!("marking decal: {}", message.into()))
}

/// One fill group: loops filled together in one ink by one rule, painted
/// over the groups before it (`layer` lifts it clear of them).
struct Group {
    ink: Ink,
    loops: Vec<Vec<Point>>,
    rule: FillRule,
    layer: u8,
}

impl Group {
    fn plain(ink: Ink, loops: Vec<Vec<Point>>) -> Self {
        Self {
            ink,
            loops,
            rule: FillRule::NonZero,
            layer: 0,
        }
    }
}

/// Paint layers one artwork may stack, each lifted [`PAINT_LIFT_MM`] over
/// the last.
const MAX_PAINT_LAYERS: u8 = 16;
/// The lift of one paint layer over the one under it, mm: far below the
/// ink's own thickness, enough for the depth buffer at preview range.
const PAINT_LIFT_MM: f64 = 0.0005;

/// Whether an sRGB colour reads as the light ground of monochrome artwork
/// (a white background box), which a single-colour finish leaves bare.
fn light_ground(c: [u8; 3]) -> bool {
    c.iter().all(|v| *v >= 240)
}

/// The groups of one SVG artwork: each painted region in paint order. A
/// colour finish (ink, pad print) prints each region in its own colour
/// (unpainted-colour regions in the finish's); a single-colour finish
/// (an etch or engraving) marks every region but light grounds.
fn artwork_groups(
    text: &str,
    origin: [f64; 2],
    size: [f64; 2],
    finish: Finish,
) -> Result<Vec<Group>> {
    let coloured = matches!(
        finish,
        Finish::InkWhite | Finish::Ink { .. } | Finish::PadPrint { .. }
    );
    let mut out = Vec::new();
    let mut layer: u8 = 0;
    for painted in paint::artwork(text, origin, size)? {
        let ink = match painted.colour {
            Some(c) if coloured => Ink::Srgb(c),
            Some(c) if light_ground(c) => continue,
            _ => Ink::Finish,
        };
        // Overpainted regions of other colours stand a layer higher.
        if out.last().is_some_and(|g: &Group| g.ink != ink) {
            layer = (layer + 1).min(MAX_PAINT_LAYERS);
        }
        out.push(Group {
            ink,
            loops: painted.loops,
            rule: painted.rule,
            layer,
        });
    }
    Ok(out)
}

/// The fill groups of `layout`'s elements in its layout frame.
fn groups(layout: &MarkingLayout, fit: [f64; 2], artwork: ArtworkSource<'_>) -> Result<Vec<Group>> {
    let mut out = Vec::new();
    let mut finish_loops: Vec<Vec<Point>> = Vec::new();
    for element in &layout.elements {
        match element {
            LaidElement::Text(line) => {
                // Each line on its own: overlapping lines must not cancel.
                out.push(Group::plain(Ink::Finish, shapes::text(line, fit)?));
            }
            LaidElement::Polarity {
                mark: PolarityMark::Stripe,
                origin,
                size_mm,
            } => out.push(Group::plain(Ink::Finish, stripe(*origin, *size_mm)?)),
            LaidElement::Polarity {
                mark,
                origin,
                size_mm,
            } => finish_loops.push(shapes::polarity(*mark, *origin, *size_mm)?),
            LaidElement::Artwork {
                svg,
                origin,
                size_mm,
            } => {
                if let Some(text) = artwork(svg) {
                    out.extend(artwork_groups(&text, *origin, *size_mm, layout.finish)?);
                }
            }
            LaidElement::Bands { colours, pitch_mm } => {
                for (colour, band) in
                    colours
                        .iter()
                        .zip(shapes::bands(colours.len(), *pitch_mm, fit)?)
                {
                    band_colour(*colour)?;
                    if let Some(band) = band {
                        out.push(Group::plain(Ink::Band(*colour), vec![band]));
                    }
                }
            }
            LaidElement::Matrix {
                symbol,
                origin,
                module_mm,
            } => out.push(Group::plain(
                Ink::Finish,
                modules(symbol, *origin, *module_mm)?,
            )),
        }
    }
    // Polarity marks are counter-clockwise; one fill merges them.
    if !finish_loops.is_empty() {
        out.push(Group::plain(Ink::Finish, finish_loops));
    }

    Ok(out)
}

/// A 2D code's dark modules as rectangles, one per run of dark modules
/// in a row (row 0 at +y).
fn modules(symbol: &Symbol, origin: [f64; 2], module: f64) -> Result<Vec<Vec<Point>>> {
    let n = symbol.size;
    if n == 0 || n > 177 || !(module.is_finite() && module > 0.0) {
        return Err(invalid("a 2D code's size is out of range"));
    }
    let side = module * f64::from(n);
    let (x0, y0) = (origin[0] - side / 2.0, origin[1] + side / 2.0);
    let mut out = Vec::new();
    for y in 0..n {
        let mut x = 0;
        while x < n {
            if !symbol.dark(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < n && symbol.dark(x, y) {
                x += 1;
            }
            let (a, b) = (x0 + module * f64::from(start), x0 + module * f64::from(x));
            let (top, bottom) = (y0 - module * f64::from(y), y0 - module * f64::from(y + 1));
            out.push(vec![[a, bottom], [b, bottom], [b, top], [a, top]]);
        }
    }
    Ok(out)
}

/// An electrolytic's stripe: its rectangle with minus signs left open
/// (wound against it), spaced along its long side.
fn stripe(origin: [f64; 2], size: [f64; 2]) -> Result<Vec<Vec<Point>>> {
    let mut loops = vec![shapes::polarity(PolarityMark::Band, origin, size)?];
    let (along, across) = if size[1] >= size[0] { (1, 0) } else { (0, 1) };
    let w = size[across];
    let (dash, stroke) = (MINUS_WIDTH * w, MINUS_STROKE * w);
    let pitch = MINUS_PITCH * w;
    if !(pitch > 0.0 && stroke > 0.0) {
        return Ok(loops);
    }
    let count = ((size[along] / pitch).floor() as usize).min(MAX_MINUS_SIGNS);
    for k in 0..count {
        let offset = (k as f64 + 0.5) * pitch - (count as f64) * pitch / 2.0;
        let mut c = origin;
        c[along] += offset;
        // A minus runs across the stripe, so it reads square to it.
        let mut s = [0.0; 2];
        s[across] = dash;
        s[along] = stroke;
        let mut hole = shapes::rect(c, s);
        hole.reverse();
        loops.push(hole);
    }
    Ok(loops)
}

/// An engraving's walls: a band [`ENGRAVE_WALL_MM`] wide inside every
/// outline edge, its normal tilted into the recess. Each wall is a quad
/// `a, b, b + w·in, a + w·in` with `in` the edge's inward (left) normal.
fn walls(loops: &[Vec<Point>]) -> Vec<([Point; 4], [f64; 2])> {
    let mut out = Vec::new();
    for points in loops {
        let n = points.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let (a, b) = (points[i], points[(i + 1) % n]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = dx.hypot(dy);
            if !(len.is_finite() && len > 1e-9) {
                continue;
            }
            let inward = [-dy / len, dx / len];
            let w = ENGRAVE_WALL_MM.min(len);
            let off = |p: Point| [p[0] + inward[0] * w, p[1] + inward[1] * w];
            out.push(([a, b, off(b), off(a)], inward));
        }
    }
    out
}

/// Loops whose bounding boxes overlap (strictly), joined: the groups a
/// nonzero fill may treat apart. Touching boxes stay apart; their fills
/// meet edge to edge.
fn clusters(loops: &[Vec<Point>]) -> Vec<Vec<Vec<Point>>> {
    let boxes: Vec<[f64; 4]> = loops
        .iter()
        .map(|l| {
            l.iter().fold(
                [
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ],
                |b, p| {
                    [
                        b[0].min(p[0]),
                        b[1].min(p[1]),
                        b[2].max(p[0]),
                        b[3].max(p[1]),
                    ]
                },
            )
        })
        .collect();
    // Union–find over the boxes, swept along x so only boxes whose x
    // ranges overlap are compared.
    let mut parent: Vec<usize> = (0..loops.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut order: Vec<usize> = (0..loops.len()).collect();
    order.sort_by(|a, b| boxes[*a][0].total_cmp(&boxes[*b][0]));
    for (k, &i) in order.iter().enumerate() {
        for &j in &order[k + 1..] {
            if boxes[j][0] >= boxes[i][2] {
                break;
            }
            let overlap = boxes[j][1] < boxes[i][3] && boxes[i][1] < boxes[j][3];
            if overlap {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<Vec<Point>>> = HashMap::new();
    let mut first: Vec<usize> = Vec::new();
    for (i, l) in loops.iter().enumerate() {
        let r = root(&mut parent, i);
        let entry = groups.entry(r).or_default();
        if entry.is_empty() {
            first.push(r);
        }
        entry.push(l.clone());
    }
    first
        .into_iter()
        .filter_map(|r| groups.remove(&r))
        .collect()
}

/// What `flatten` builds: points, tilts and triangles in the chart.
#[derive(Default)]
struct Builder {
    out: FlatDecal,
    bounds: [Option<f64>; 4],
}

impl Builder {
    /// Add one chart triangle, clipped to the region.
    fn triangle(&mut self, tri: [Point; 3], tilts: [[f64; 2]; 3], layer: u8) -> Result<()> {
        let poly = map::clip(tri, self.bounds);
        if poly.len() < 3 {
            return Ok(());
        }
        if self.out.triangles.len() + poly.len() > MAX_DECAL_TRIANGLES {
            return Err(invalid(format!(
                "a decal has more than {MAX_DECAL_TRIANGLES} triangles"
            )));
        }
        // Tilts are constant on a wall and zero elsewhere: take the
        // first corner's (clipping keeps the triangle's plane).
        let tilt = tilts[0];
        let base =
            u32::try_from(self.out.points.len()).map_err(|_| invalid("too many vertices"))?;
        for p in &poly {
            self.out.points.push(*p);
            self.out.tilts.push(tilt);
            self.out.layers.push(layer);
        }
        for k in 1..poly.len() as u32 - 1 {
            self.out.triangles.push([base, base + k, base + k + 1]);
        }
        Ok(())
    }
}

/// The flat triangles of `layout` (its elements only, not its decals) in
/// `region` of its chart: filled in the layout frame, placed by the
/// layout's [`Placement`], repeated for each copy and clipped.
pub fn flatten(
    layout: &MarkingLayout,
    region: &Region,
    artwork: ArtworkSource<'_>,
) -> Result<FlatDecal> {
    let ok = |s: &[f64; 2]| {
        s.iter()
            .all(|v| v.is_finite() && *v > 0.0 && *v <= MAX_AREA_MM)
    };
    if !ok(&region.size) || !ok(&region.cell) || region.copies.is_empty() || region.copies.len() > 8
    {
        return Err(invalid("the marking area size is out of range"));
    }
    if layout.elements.len() > MAX_DECAL_ELEMENTS {
        return Err(invalid(format!("more than {MAX_DECAL_ELEMENTS} elements")));
    }
    let placement: &Placement = &layout.placement;
    if let Some(problem) = placement.problem() {
        return Err(invalid(problem));
    }
    let fit = placement.layout_area(region.cell);
    let groups = groups(layout, fit, artwork)?;
    let (hx, hy) = (region.size[0] / 2.0, region.size[1] / 2.0);
    let open = |periodic: bool, v: f64| (placement.clip && !periodic).then_some(v);
    let mut b = Builder {
        out: FlatDecal {
            finish: layout.finish,
            ..FlatDecal::default()
        },
        bounds: [
            open(region.periodic[0], -hx),
            open(region.periodic[0], hx),
            open(region.periodic[1], -hy),
            open(region.periodic[1], hy),
        ],
    };
    let mirror = placement.mirror;
    let (s, c) = placement.rotation_deg.to_radians().sin_cos();
    // A direction through the placement's linear part (no shift).
    let turn = |d: [f64; 2]| {
        let x = if mirror { -d[0] } else { d[0] };
        [c * x - s * d[1], s * x + c * d[1]]
    };
    let engraved = layout.finish == Finish::Engraved;
    for group in &groups {
        let mut flat = Flat::default();
        // Loops whose boxes do not overlap cannot interact under the
        // nonzero rule: filled apart (glyph by glyph, row by row of a
        // code), the slabs of one never cut another's.
        for cluster in clusters(&group.loops) {
            let left =
                MAX_DECAL_TRIANGLES.saturating_sub(b.out.triangles.len() + flat.triangles.len());
            fill::fill_with(&cluster, group.rule, &mut flat, left)?;
        }
        let start = b.out.triangles.len();
        for copy in &region.copies {
            let at = |p: Point| {
                let q = placement.apply(p);
                [q[0] + copy[0], q[1] + copy[1]]
            };
            for t in &flat.triangles {
                let corner = |i: u32| flat.points.get(i as usize).copied().map(at);
                let (Some(p0), Some(p1), Some(p2)) = (corner(t[0]), corner(t[1]), corner(t[2]))
                else {
                    continue;
                };
                // A mirror turns the winding; keep every triangle facing out.
                let tri = if mirror { [p0, p2, p1] } else { [p0, p1, p2] };
                b.triangle(tri, [[0.0; 2]; 3], group.layer)?;
            }
        }
        let end = b.out.triangles.len();
        if group.ink != Ink::Finish && end > start {
            b.out.runs.push((group.ink, start..end));
        }
        if engraved && group.ink == Ink::Finish {
            let start = b.out.triangles.len();
            for (quad, inward) in walls(&group.loops) {
                let d = turn(inward);
                let tilt = [d[0] * ENGRAVE_TILT, d[1] * ENGRAVE_TILT];
                for copy in &region.copies {
                    let at = |p: Point| {
                        let q = placement.apply(p);
                        [q[0] + copy[0], q[1] + copy[1]]
                    };
                    let q = quad.map(at);
                    let (t0, t1) = if mirror {
                        ([q[0], q[2], q[1]], [q[0], q[3], q[2]])
                    } else {
                        ([q[0], q[1], q[2]], [q[0], q[2], q[3]])
                    };
                    b.triangle(t0, [tilt; 3], group.layer)?;
                    b.triangle(t1, [tilt; 3], group.layer)?;
                }
            }
            let end = b.out.triangles.len();
            if end > start {
                b.out.runs.push((Ink::Wall, start..end));
            }
        }
    }
    Ok(b.out)
}

/// Place a flat decal on its surface `map`, `finish.lift_mm()` off it:
/// each chart point taken onto the surface with its normal (an engraved
/// wall's tilted), triangles round a cylinder subdivided so their chords
/// follow it.
pub fn place_on(flat: &FlatDecal, map: &SurfaceMap) -> Result<DecalMesh> {
    let lift = flat.finish.lift_mm().max(DECAL_OFFSET_MM);
    let mut out = DecalMesh {
        finish: flat.finish,
        ..DecalMesh::default()
    };
    let vertex = |out: &mut DecalMesh, p: Point, tilt: [f64; 2], layer: u8| -> Result<u32> {
        let tilted = tilt[0] != 0.0 || tilt[1] != 0.0;
        let raise = if tilted { WALL_LIFT_MM } else { 0.0 } + f64::from(layer) * PAINT_LIFT_MM;
        let (at, n, u, v) = map.frame(p, lift + raise);
        let k = (1.0 - tilt[0] * tilt[0] - tilt[1] * tilt[1])
            .max(0.0)
            .sqrt();
        let normal = n * k + u * tilt[0] + v * tilt[1];
        let len = normal.dot(normal).sqrt();
        let normal = if len > 1e-9 { normal * (1.0 / len) } else { n };
        let position = at.to_f32();
        if position.iter().any(|x| !x.is_finite()) {
            return Err(invalid("a vertex is out of range"));
        }
        let index = u32::try_from(out.positions.len()).map_err(|_| invalid("too many vertices"))?;
        out.positions.push(position);
        out.normals.push(normal.to_f32());
        Ok(index)
    };
    // Points shared by unsplit triangles are mapped once.
    let mut mapped: Vec<Option<u32>> = vec![None; flat.points.len()];
    let mut run_at: Vec<(Ink, usize)> = Vec::new();
    for (k, t) in flat.triangles.iter().enumerate() {
        let ink = flat.ink_of(k);
        if run_at.last().is_none_or(|(i, _)| *i != ink) {
            run_at.push((ink, out.indices.len()));
        }
        let corner = |i: u32| -> Result<(Point, [f64; 2], u8)> {
            let p = flat
                .points
                .get(i as usize)
                .ok_or_else(|| invalid("a bad index"))?;
            let t = flat.tilts.get(i as usize).copied().unwrap_or([0.0; 2]);
            let l = flat.layers.get(i as usize).copied().unwrap_or(0);
            Ok((*p, t, l))
        };
        let corners = [corner(t[0])?, corner(t[1])?, corner(t[2])?];
        let n = map.splits(corners.map(|c| c.0));
        if n <= 1 {
            for (slot, i) in t.iter().enumerate() {
                let index = match mapped.get(*i as usize).copied().flatten() {
                    Some(index) => index,
                    None => {
                        let (p, tilt, layer) = corners[slot];
                        let index = vertex(&mut out, p, tilt, layer)?;

                        if let Some(m) = mapped.get_mut(*i as usize) {
                            *m = Some(index);
                        }
                        index
                    }
                };
                out.indices.push(index);
            }
        } else {
            // Slabs across the round direction, each at most a split step
            // wide, so every chord follows the curve; a long strip costs
            // a few triangles per slab, not n².
            let tri = corners.map(|c| c.0);
            let k = map.round_axis();
            let lo = tri.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min);
            let hi = tri.iter().map(|p| p[k]).fold(f64::NEG_INFINITY, f64::max);
            let step = (hi - lo) / n as f64;
            for slab in 0..n {
                let mut bounds = [None; 4];
                bounds[2 * k] = Some(lo + step * slab as f64);
                bounds[2 * k + 1] = Some(lo + step * (slab + 1) as f64);
                let poly = map::clip(tri, bounds);
                if poly.len() < 3 {
                    continue;
                }
                let mut ids = Vec::with_capacity(poly.len());
                for p in &poly {
                    let w = barycentric(tri, *p);
                    let tilt = [
                        w[0] * corners[0].1[0] + w[1] * corners[1].1[0] + w[2] * corners[2].1[0],
                        w[0] * corners[0].1[1] + w[1] * corners[1].1[1] + w[2] * corners[2].1[1],
                    ];
                    ids.push(vertex(&mut out, *p, tilt, corners[0].2)?);
                }
                for j in 1..ids.len() - 1 {
                    out.indices.extend([ids[0], ids[j], ids[j + 1]]);
                }
            }
        }
        if out.triangle_count() > MAX_PLACED_TRIANGLES {
            return Err(invalid(format!(
                "a placed decal has more than {MAX_PLACED_TRIANGLES} triangles"
            )));
        }
    }
    let total = out.indices.len();
    for (k, (ink, start)) in run_at.iter().enumerate() {
        let end = run_at.get(k + 1).map_or(total, |(_, s)| *s);
        if *ink != Ink::Finish && end > *start {
            out.runs.push(InkRun {
                ink: *ink,
                indices: *start..end,
            });
        }
    }
    Ok(out)
}

/// The barycentric weights of `p` in `tri` (equal thirds for a
/// degenerate triangle).
fn barycentric(tri: [Point; 3], p: Point) -> [f64; 3] {
    let [a, b, c] = tri;
    let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if d.abs() < 1e-18 {
        return [1.0 / 3.0; 3];
    }
    let u = ((b[1] - c[1]) * (p[0] - c[0]) + (c[0] - b[0]) * (p[1] - c[1])) / d;
    let v = ((c[1] - a[1]) * (p[0] - c[0]) + (a[0] - c[0]) * (p[1] - c[1])) / d;
    [u, v, 1.0 - u - v]
}

/// The product look of a finish: its cited material's surface, with the
/// chosen colour where it has one.
pub fn finish_surface(finish: Finish) -> Result<OpaqueSurface> {
    finish_surface_on(finish, None)
}

/// The look of `finish` on a face whose own look is `base`: an engraving
/// is the face darkened and roughened as a recess (est.: a recess's floor
/// catches less light); the rest are their materials ([`finish_surface`]).
pub fn finish_surface_on(finish: Finish, base: Option<OpaqueSurface>) -> Result<OpaqueSurface> {
    if finish == Finish::Engraved
        && let Some(base) = base
    {
        return shade(base, 0.55, 0.2);
    }
    let mut def = look::definition(finish.material())?;
    if let Some(colour) = finish.colour() {
        def.base_linear = colour.map(srgb8_to_linear);
    }
    look::surface(&def)
}

/// `surface` with its colour scaled by `k` and its roughness raised by
/// `rough`.
fn shade(surface: OpaqueSurface, k: f64, rough: f64) -> Result<OpaqueSurface> {
    OpaqueSurface::new(
        surface.base_color() * k,
        surface.metallic(),
        (surface.roughness() + rough).min(1.0),
        surface.dielectric_ior(),
    )
    .map_err(|e| invalid(format!("shade: {e}")))
}

/// The look of `ink` in a decal of `finish` on a face of look `base`.
pub fn ink_surface(ink: Ink, finish: Finish, base: Option<OpaqueSurface>) -> Result<OpaqueSurface> {
    match ink {
        Ink::Finish => finish_surface_on(finish, base),
        Ink::Band(c) => band_surface(c),
        Ink::Srgb(colour) => {
            let mut def = look::definition(finish.material())?;
            def.base_linear = colour.map(srgb8_to_linear);
            look::surface(&def)
        }
        Ink::Wall => match base {
            Some(base) => shade(base, 0.8, 0.1),
            None => finish_surface(finish),
        },
    }
}

/// The procedural detail a finish draws with: screen ink shows the
/// silkscreen's grain; the others are flat.
pub fn finish_detail(finish: Finish) -> Result<Detail> {
    Ok(match finish {
        Finish::InkWhite | Finish::Ink { .. } => Detail {
            pattern: Pattern::Silkscreen,
            strength: 0.6,
        },
        Finish::Engraved => Detail::NONE,
        other => look::detail(&look::definition(other.material())?),
    })
}

/// IEC 60062 colour code: 0–9 the digit colours (black, brown, red,
/// orange, yellow, green, blue, violet, grey, white), 10 gold, 11 silver.
/// Display colours as sRGB, estimates of printed lacquer bands.
const BAND_SRGB: [[u8; 3]; 12] = [
    [24, 24, 24],
    [110, 62, 30],
    [196, 30, 30],
    [232, 120, 24],
    [236, 206, 40],
    [40, 140, 60],
    [36, 76, 186],
    [128, 60, 160],
    [128, 128, 128],
    [236, 236, 232],
    [196, 160, 70],
    [190, 192, 196],
];

/// The linear colour of IEC 60062 colour index `colour`.
pub fn band_colour(colour: u8) -> Result<[f64; 3]> {
    BAND_SRGB
        .get(usize::from(colour))
        .map(|c| c.map(srgb8_to_linear))
        .ok_or_else(|| Error::Invalid(format!("marking decal: no IEC 60062 band colour {colour}")))
}

/// The look of a band colour: a satin lacquer; gold and silver metallic.
pub fn band_surface(colour: u8) -> Result<OpaqueSurface> {
    let [r, g, b] = band_colour(colour)?;
    let metallic = if colour >= 10 { 0.8 } else { 0.0 };
    OpaqueSurface::new(Vec3::new(r, g, b), metallic, 0.45, 1.5)
        .map_err(|e| Error::Invalid(format!("band colour {colour}: {e}")))
}

/// A layout's identity for the cache: everything that draws (its
/// elements, finish and placement), not its advisory findings nor its
/// decals (each is cached on its own).
pub fn layout_hash(layout: &MarkingLayout) -> Result<u64> {
    let bytes = serde_json::to_vec(&(&layout.elements, layout.finish, &layout.placement))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(hasher.finish())
}

type Key = (u64, u64);

/// Flat decals keyed by (layout hash, region): an unchanged marking is
/// never laid out or filled again, a changed value refills only its own
/// decal, and instances showing the same marking on the same region share
/// one flat decal. Entries not used in a pass are dropped by
/// [`DecalCache::sweep`].
#[derive(Default)]
pub struct DecalCache {
    entries: HashMap<Key, (Arc<FlatDecal>, bool)>,
    built: u64,
}

impl DecalCache {
    /// The flat decal of `layout` in `region`, from the cache or built now.
    pub fn flat_in(
        &mut self,
        layout: &MarkingLayout,
        region: &Region,
        artwork: ArtworkSource<'_>,
    ) -> Result<Arc<FlatDecal>> {
        let key = (layout_hash(layout)?, region.hash());
        if let Some((flat, used)) = self.entries.get_mut(&key) {
            *used = true;
            return Ok(flat.clone());
        }
        let flat = Arc::new(flatten(layout, region, artwork)?);
        if self.entries.len() >= MAX_CACHED_DECALS {
            self.entries.clear();
        }
        self.entries.insert(key, (flat.clone(), true));
        self.built = self.built.saturating_add(1);
        Ok(flat)
    }

    /// The flat decal of `layout` on a flat area `area_mm` in size
    /// ([`Self::flat_in`]). `instance` is not part of the key: it names
    /// whose decal is asked for, for callers that track it.
    pub fn flat(
        &mut self,
        _instance: Id,
        layout: &MarkingLayout,
        area_mm: [f64; 2],
        artwork: ArtworkSource<'_>,
    ) -> Result<Arc<FlatDecal>> {
        self.flat_in(layout, &Region::plane(area_mm), artwork)
    }

    /// Drop the entries no `flat` call asked for since the last sweep.
    pub fn sweep(&mut self) {
        self.entries.retain(|_, (_, used)| std::mem::take(used));
    }

    /// How many decals were built (not taken from the cache) so far.
    pub fn built(&self) -> u64 {
        self.built
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
