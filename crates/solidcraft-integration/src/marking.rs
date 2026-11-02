//! Slice D5: instance marking decals. A core [`MarkingLayout`] in the
//! marking area's 2D frame becomes a flat triangle mesh [`DECAL_OFFSET_MM`]
//! above the face, from Solidcraft's text-outline geometry and the SVG
//! converter over its public reader, drawn with the finish material's
//! look. Display only: never saved to `.solidcraft`, never in fabrication
//! output; a value change rebuilds only the decal keyed by
//! (instance, layout hash) in a [`DecalCache`], never the model.
//!
//! **The layout frame.** Millimetres, origin at the area centre, +x along
//! the area's u axis and +y along `normal × u`, so a decal reads upright
//! seen from the side its normal points to. Each element's `origin` is its
//! centre: a text line is centred on it horizontally and its capitals
//! span `height_mm` about it vertically (the core lays out without font
//! metrics, so a line the font sets wider than the area is shrunk about
//! its origin to fit). Bands run from the −x end: band `i` is centred
//! `pitch · (i + 1)` from it and half a pitch wide, across the area.
//!
//! **Looks.** Laser etch is the cited `EpoxyLaserMarked` (lighter, rougher
//! grey matte), white ink `MarkingInkWhite`, through the same product look
//! as generated bodies ([`crate::models::look`]). Band colours are
//! estimates of the IEC 60062 colour code (see [`band_colour`]).
//!
//! Placement on a board is in [`board`].
pub mod board;
mod fill;
pub mod paint;
mod shapes;
#[cfg(test)]
mod tests;

use crate::material::{Detail, srgb8_to_linear};
use crate::models::look;
use crate::{Error, Result};
use fill::Flat;
use pcbcraft_core::Point;
use pcbcraft_core::ids::{ContentHash, Id};
use pcbcraft_core::library::marking::{Finish, LaidElement, MarkingLayout};
use pcbcraft_core::library::model3d::MarkingArea;
use solidcraft_engine::geom::Vec3;
use solidcraft_engine::render::surface::OpaqueSurface;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

pub use fill::MAX_FILL_EDGES;
pub use shapes::{CHORD_MM, MAX_TEXT_CHARS, TEXT_HEIGHT_MM};

/// Height of a decal above its face, mm: 5 µm, thinner than marking ink
/// and far below what the eye resolves, yet clear of z-fighting. The
/// viewport's depth buffer is reversed 32-bit float (Solidcraft's
/// `reversed_depth`), whose steps grow with distance: a lift `h` seen from
/// `d` away separates the decal from its face by at least `h / d · 2²³`
/// depth steps, about 42 at 1 m, the farthest a board-fit view of a large
/// board stands ([`DECAL_DEPTH_STEPS_AT_1_M`]). Decals draw in the same
/// opaque pass as the bodies, so Solidcraft's face bias pushes both alike.
pub const DECAL_OFFSET_MM: f64 = 0.005;
/// The reversed-float depth steps between a decal and its face seen from
/// 1 m (see [`DECAL_OFFSET_MM`]).
pub const DECAL_DEPTH_STEPS_AT_1_M: f64 = DECAL_OFFSET_MM / 1000.0 * 8_388_608.0;
/// Triangles one decal may have.
pub const MAX_DECAL_TRIANGLES: usize = 65_536;
/// Elements one layout may have.
pub const MAX_DECAL_ELEMENTS: usize = 32;
/// Decals one [`DecalCache`] keeps; past it the cache starts again.
pub const MAX_CACHED_DECALS: usize = 4096;
/// Largest marking area side, mm.
pub const MAX_AREA_MM: f64 = 500.0;

/// A run of a decal's triangles drawn in a band colour instead of the
/// finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BandRun {
    /// IEC 60062 colour index ([`band_colour`]).
    pub colour: u8,
    /// The run's range of [`DecalMesh::indices`] (whole triangles).
    pub indices: Range<usize>,
}

/// A decal mesh in the model frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecalMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub finish: Finish,
    /// The face normal every vertex shares (the area's).
    pub normal: [f32; 3],
    /// Triangles drawn in band colours; the rest take the finish.
    pub bands: Vec<BandRun>,
}

impl DecalMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// A laid-out marking as flat triangles in its area's 2D frame: what the
/// cache keeps, placed on an area by [`place`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlatDecal {
    flat: Flat,
    finish: Finish,
    /// Triangle ranges of [`Self::flat`] in band colours.
    bands: Vec<(u8, Range<usize>)>,
}

impl FlatDecal {
    pub fn triangle_count(&self) -> usize {
        self.flat.triangles.len()
    }

    /// The marked area, mm².
    pub fn area_mm2(&self) -> f64 {
        self.flat.area()
    }

    pub fn is_empty(&self) -> bool {
        self.flat.triangles.is_empty()
    }
}

/// SVG artwork text by content hash, from wherever the host keeps assets.
pub type ArtworkSource<'a> = &'a dyn Fn(&ContentHash) -> Option<String>;

/// No artwork: artwork elements draw nothing.
pub fn no_artwork(_: &ContentHash) -> Option<String> {
    None
}

/// Mesh `layout` on `area`. Artwork elements draw nothing (see
/// [`decal_with`]).
pub fn decal(layout: &MarkingLayout, area: &MarkingArea) -> Result<DecalMesh> {
    decal_with(layout, area, &no_artwork)
}

/// Mesh `layout` on `area`, reading artwork from `artwork`. An element
/// that cannot be drawn refuses the whole decal, so a marking is shown
/// whole or not at all.
pub fn decal_with(
    layout: &MarkingLayout,
    area: &MarkingArea,
    artwork: ArtworkSource<'_>,
) -> Result<DecalMesh> {
    let frame = Frame::of(area)?;
    let flat = flatten(layout, area.size_mm, artwork)?;
    frame.place(&flat)
}

/// Mesh a cached flat decal on `area`.
pub fn place(flat: &FlatDecal, area: &MarkingArea) -> Result<DecalMesh> {
    Frame::of(area)?.place(flat)
}

/// The flat triangles of `layout` in an area `area_mm` in size.
pub fn flatten(
    layout: &MarkingLayout,
    area_mm: [f64; 2],
    artwork: ArtworkSource<'_>,
) -> Result<FlatDecal> {
    if !area_mm
        .iter()
        .all(|s| s.is_finite() && *s > 0.0 && *s <= MAX_AREA_MM)
    {
        return Err(Error::Invalid(
            "marking decal: the marking area size is out of range".into(),
        ));
    }
    if layout.elements.len() > MAX_DECAL_ELEMENTS {
        return Err(Error::Invalid(format!(
            "marking decal: more than {MAX_DECAL_ELEMENTS} elements"
        )));
    }
    let mut out = FlatDecal {
        finish: layout.finish,
        ..FlatDecal::default()
    };
    let budget = |flat: &Flat| MAX_DECAL_TRIANGLES.saturating_sub(flat.triangles.len());
    let mut finish_loops: Vec<Vec<Point>> = Vec::new();
    for element in &layout.elements {
        match element {
            LaidElement::Matrix { .. } => {}
            LaidElement::Text(line) => {
                // Each line on its own: overlapping lines must not cancel.
                let loops = shapes::text(line, area_mm)?;
                fill_in(&loops, &mut out.flat, budget)?;
            }
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
                    let loops = shapes::artwork(&text, *origin, *size_mm)?;
                    fill_in(&loops, &mut out.flat, budget)?;
                }
            }
            LaidElement::Bands { colours, pitch_mm } => {
                for (colour, band) in
                    colours
                        .iter()
                        .zip(shapes::bands(colours.len(), *pitch_mm, area_mm)?)
                {
                    band_colour(*colour)?;
                    let Some(band) = band else {
                        continue;
                    };
                    let start = out.flat.triangles.len();
                    fill_in(&[band], &mut out.flat, budget)?;
                    let end = out.flat.triangles.len();
                    if end > start {
                        out.bands.push((*colour, start..end));
                    }
                }
            }
        }
    }
    // Polarity marks are counter-clockwise; one fill merges them.
    fill_in(&finish_loops, &mut out.flat, budget)?;
    Ok(out)
}

/// Fill `loops` into `flat` within the triangles `budget` leaves.
fn fill_in(loops: &[Vec<Point>], flat: &mut Flat, budget: impl Fn(&Flat) -> usize) -> Result<()> {
    let left = budget(flat);
    fill::fill(loops, flat, left)
}

/// An area's orthonormal frame.
struct Frame {
    origin: Vec3,
    u: Vec3,
    v: Vec3,
    n: Vec3,
}

impl Frame {
    fn of(area: &MarkingArea) -> Result<Self> {
        let bad = || Error::Invalid("marking decal: the marking area is degenerate".into());
        let vec = |a: [f64; 3]| Vec3::new(a[0], a[1], a[2]);
        let all = area
            .origin
            .iter()
            .chain(&area.normal)
            .chain(&area.u_axis)
            .chain(&area.size_mm);
        if !all.clone().all(|v| v.is_finite()) || area.origin.iter().any(|v| v.abs() > 1e5) {
            return Err(bad());
        }
        let n = unit(vec(area.normal)).ok_or_else(bad)?;
        let u = vec(area.u_axis);
        // The u axis made square to the normal.
        let u = unit(u - n * u.dot(n)).ok_or_else(bad)?;
        let v = n.cross(u);
        Ok(Self {
            origin: vec(area.origin) + n * DECAL_OFFSET_MM,
            u,
            v,
            n,
        })
    }

    fn place(&self, flat: &FlatDecal) -> Result<DecalMesh> {
        let positions: Vec<[f32; 3]> = flat
            .flat
            .points
            .iter()
            .map(|p| (self.origin + self.u * p[0] + self.v * p[1]).to_f32())
            .collect();
        if positions.iter().flatten().any(|v| !v.is_finite()) {
            return Err(Error::Invalid(
                "marking decal: a vertex is out of range".into(),
            ));
        }
        let indices: Vec<u32> = flat.flat.triangles.iter().flatten().copied().collect();
        Ok(DecalMesh {
            positions,
            indices,
            finish: flat.finish,
            normal: self.n.to_f32(),
            bands: flat
                .bands
                .iter()
                .map(|(colour, run)| BandRun {
                    colour: *colour,
                    indices: run.start * 3..run.end * 3,
                })
                .collect(),
        })
    }
}

fn unit(v: Vec3) -> Option<Vec3> {
    let len = v.dot(v).sqrt();
    (len.is_finite() && len > 1e-9).then(|| v * (1.0 / len))
}

/// The product look of a finish: its cited material's surface.
pub fn finish_surface(finish: Finish) -> Result<OpaqueSurface> {
    look::surface(&look::definition(finish.material())?)
}

/// The procedural detail a finish draws with (none: a marking is flat).
pub fn finish_detail(finish: Finish) -> Result<Detail> {
    Ok(look::detail(&look::definition(finish.material())?))
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

/// A layout's identity for the cache: everything that draws (its elements
/// and finish), not its advisory findings.
pub fn layout_hash(layout: &MarkingLayout) -> Result<u64> {
    let bytes = serde_json::to_vec(&(&layout.elements, layout.finish))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(hasher.finish())
}

type Key = (Id, u64, [u64; 2]);

/// Flat decals per instance, keyed by (instance, layout hash, area size):
/// an unchanged marking is never laid out or filled again, and a changed
/// value refills only its own instance. Entries not used in a pass are
/// dropped by [`DecalCache::sweep`].
#[derive(Default)]
pub struct DecalCache {
    entries: HashMap<Key, (Arc<FlatDecal>, bool)>,
    built: u64,
}

impl DecalCache {
    /// The flat decal of `instance`'s `layout` on an area `area_mm` in
    /// size, from the cache or built now.
    pub fn flat(
        &mut self,
        instance: Id,
        layout: &MarkingLayout,
        area_mm: [f64; 2],
        artwork: ArtworkSource<'_>,
    ) -> Result<Arc<FlatDecal>> {
        let key = (instance, layout_hash(layout)?, area_mm.map(f64::to_bits));
        if let Some((flat, used)) = self.entries.get_mut(&key) {
            *used = true;
            return Ok(flat.clone());
        }
        let flat = Arc::new(flatten(layout, area_mm, artwork)?);
        if self.entries.len() >= MAX_CACHED_DECALS {
            self.entries.clear();
        }
        self.entries.insert(key, (flat.clone(), true));
        self.built = self.built.saturating_add(1);
        Ok(flat)
    }

    /// Drop the entries no [`flat`](Self::flat) call asked for since the
    /// last sweep.
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
