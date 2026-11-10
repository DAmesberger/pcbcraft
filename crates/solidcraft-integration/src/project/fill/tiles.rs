//! A fill's copper in tiles: its regions cut along a fixed grid of the
//! board into pieces of at most one cell each, each piece a body of its
//! own. Extruding one planar profile costs more than linearly in its
//! edges, and a pour holds thousands; in tiles the cost of a fill is about
//! linear in its edges. A refill that changes the copper near a part
//! changes only the tiles there, and an update builds only those again
//! ([`crate::native::Native::unit`]): a tile is known by its fill, its cell
//! and its place in the cell, and by a digest of its outline.
//!
//! The cut is exact on a nanometre grid (`i_overlay`'s integer overlay, so
//! a cell's pieces depend only on the copper inside it, whatever the fill
//! holds elsewhere). Each piece's loops are then simplified to the model's
//! chord tolerance with every point on the cell's border kept, so the
//! neighbouring tiles meet along the same cut edges: copper joining copper.
use super::{douglas_peucker, simplify_loop};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::Overlay;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::i_float::int::point::IntPoint;
use pcbcraft_core::Point;
use pcbcraft_core::model::fill::FilledRegion;

/// The side of a tile's cell, mm. A cell of the ESP32 example's pours
/// holds up to about 400 edges, extruded in about 0.1 s. On that board
/// 12 mm built the model and its refill faster than 4 or 8 mm: more tiles
/// cost Solidcraft more bookkeeping than their smaller extrusions save.
pub(in crate::project) const TILE_MM: f64 = 12.0;

/// Grid units per millimetre: a nanometre.
const UNITS: f64 = 1e6;

/// Largest coordinate on the grid (a metre), well inside the integer
/// overlay's range.
const MAX_UNITS: i64 = 1_000_000_000;

type Ring = Vec<IntPoint<i64>>;
type Shape = Vec<Ring>;

/// One piece of a fill's copper: in cell (`column`, `row`), the `piece`th
/// there (pieces ordered by their lowest point), and its outline.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::project) struct Tile {
    pub column: i64,
    pub row: i64,
    pub piece: usize,
    pub region: FilledRegion,
}

fn to_grid(p: Point) -> Option<IntPoint<i64>> {
    let at = |v: f64| {
        let u = (v * UNITS).round();
        (u.is_finite() && u.abs() <= MAX_UNITS as f64).then_some(u as i64)
    };
    Some(IntPoint::new(at(p[0])?, at(p[1])?))
}

fn from_grid(p: IntPoint<i64>) -> Point {
    [p.x as f64 / UNITS, p.y as f64 / UNITS]
}

/// A rectangle's ring, counter-clockwise.
fn rectangle(x0: i64, y0: i64, x1: i64, y1: i64) -> Ring {
    vec![
        IntPoint::new(x0, y0),
        IntPoint::new(x1, y0),
        IntPoint::new(x1, y1),
        IntPoint::new(x0, y1),
    ]
}

/// `shapes` within the rectangle.
fn clip(shapes: &[Shape], rect: Ring) -> Vec<Shape> {
    if shapes.is_empty() {
        return Vec::new();
    }
    Overlay::<i64>::from_subj_and_clip(shapes, &rect)
        .overlay(OverlayRule::Intersect, FillRule::EvenOdd)
}

/// The tiles of `regions` on a grid of `cell` mm from the board origin,
/// each loop simplified to `tolerance` mm with the cell's border kept. A
/// region with a point beyond a metre of the origin, or a cell size below a
/// micrometre, gives none.
pub(in crate::project) fn tiles(regions: &[FilledRegion], cell: f64, tolerance: f64) -> Vec<Tile> {
    let side = (cell * UNITS).round();
    if !(side >= 1e3 && side <= MAX_UNITS as f64) {
        return Vec::new();
    }
    let side = side as i64;
    // Every loop as one shape of contours, nested even-odd (the regions
    // are disjoint, each outer boundary round its holes).
    let mut loops: Shape = Vec::new();
    for region in regions {
        for ring in std::iter::once(&region.outer).chain(&region.holes) {
            if ring.len() < 3 {
                continue;
            }
            let Some(ring) = ring.iter().map(|p| to_grid(*p)).collect::<Option<Ring>>() else {
                return Vec::new();
            };
            loops.push(ring);
        }
    }
    let Some((lo, hi)) = bounds(loops.iter().flatten()) else {
        return Vec::new();
    };
    let shapes = vec![loops];
    let mut out = Vec::new();
    // Column by column, then each column cell by cell.
    for column in lo.x.div_euclid(side)..=hi.x.div_euclid(side) {
        let (x0, x1) = (column * side, (column + 1) * side);
        let strip = clip(&shapes, rectangle(x0, lo.y - 1, x1, hi.y + 1));
        let Some((low, high)) = bounds(strip.iter().flatten().flatten()) else {
            continue;
        };
        for row in low.y.div_euclid(side)..=high.y.div_euclid(side) {
            let (y0, y1) = (row * side, (row + 1) * side);
            let border = |p: &IntPoint<i64>| p.x == x0 || p.x == x1 || p.y == y0 || p.y == y1;
            let mut pieces: Vec<FilledRegion> = clip(&strip, rectangle(x0, y0, x1, y1))
                .into_iter()
                .filter_map(|shape| {
                    let mut rings = shape.into_iter();
                    let outer = simplified(&rings.next()?, &border, tolerance);
                    let holes = rings.map(|h| simplified(&h, &border, tolerance)).collect();
                    (outer.len() >= 3).then_some(FilledRegion { outer, holes })
                })
                .collect();
            pieces.sort_by(|a, b| lowest(&a.outer).total_cmp_key(&lowest(&b.outer)));
            for (piece, region) in pieces.into_iter().enumerate() {
                out.push(Tile {
                    column,
                    row,
                    piece,
                    region,
                });
            }
        }
    }
    out
}

/// The box of grid points.
fn bounds<'a>(
    points: impl Iterator<Item = &'a IntPoint<i64>>,
) -> Option<(IntPoint<i64>, IntPoint<i64>)> {
    points.fold(None, |b, p| {
        Some(match b {
            None => (*p, *p),
            Some((lo, hi)) => (
                IntPoint::new(lo.x.min(p.x), lo.y.min(p.y)),
                IntPoint::new(hi.x.max(p.x), hi.y.max(p.y)),
            ),
        })
    })
}

/// A loop's lowest point (least y, then least x).
fn lowest(points: &[Point]) -> Point {
    points
        .iter()
        .copied()
        .min_by(|a, b| a[1].total_cmp(&b[1]).then(a[0].total_cmp(&b[0])))
        .unwrap_or([0.0, 0.0])
}

trait PointOrder {
    fn total_cmp_key(&self, other: &Self) -> std::cmp::Ordering;
}

impl PointOrder for Point {
    fn total_cmp_key(&self, other: &Self) -> std::cmp::Ordering {
        self[1]
            .total_cmp(&other[1])
            .then(self[0].total_cmp(&other[0]))
    }
}

/// A ring simplified to `tolerance` (mm): every point on the cell's
/// `border` kept, and Douglas–Peucker between consecutive kept points; a
/// ring touching no border is simplified from its lowest point
/// ([`simplify_loop`]), so its outline does not depend on where it starts.
fn simplified(ring: &Ring, border: &impl Fn(&IntPoint<i64>) -> bool, tolerance: f64) -> Vec<Point> {
    let n = ring.len();
    let points: Vec<Point> = ring.iter().map(|p| from_grid(*p)).collect();
    let pins: Vec<usize> = (0..n).filter(|&k| border(&ring[k])).collect();
    let Some(&first) = pins.first() else {
        let start = (0..n)
            .min_by(|&a, &b| points[a].total_cmp_key(&points[b]))
            .unwrap_or(0);
        let mut turned = points.clone();
        turned.rotate_left(start);
        return simplify_loop(&turned, tolerance);
    };
    // From the first pin round to it again.
    let mut turned = points.clone();
    turned.rotate_left(first);
    turned.push(turned[0]);
    let pinned: Vec<usize> = pins
        .iter()
        .map(|&k| (k + n - first) % n)
        .chain(std::iter::once(n))
        .collect();
    let mut keep = vec![false; n + 1];
    for pair in pinned.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        keep[a] = true;
        keep[b] = true;
        douglas_peucker(&turned, a, b, tolerance, &mut keep);
    }
    let out: Vec<Point> = turned
        .iter()
        .take(n)
        .zip(&keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect();
    if out.len() < 3 { points } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, s: f64) -> Vec<Point> {
        vec![[x0, y0], [x0 + s, y0], [x0 + s, y0 + s], [x0, y0 + s]]
    }

    fn area(ring: &[Point]) -> f64 {
        let n = ring.len();
        (0..n)
            .map(|i| {
                let (a, b) = (ring[i], ring[(i + 1) % n]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f64>()
            / 2.0
    }

    fn region_area(r: &FilledRegion) -> f64 {
        area(&r.outer).abs() - r.holes.iter().map(|h| area(h).abs()).sum::<f64>()
    }

    /// The tiles hold the region's area exactly (cut edges are shared), and
    /// none leaves its cell.
    #[test]
    fn tiles_cover_the_region_within_their_cells() {
        let region = FilledRegion {
            outer: square(1.0, 1.0, 10.0),
            holes: vec![square(3.5, 3.5, 2.0)],
        };
        let tiles = tiles(std::slice::from_ref(&region), 4.0, 0.01);
        let total: f64 = tiles.iter().map(|t| region_area(&t.region)).sum();
        assert!((total - (100.0 - 4.0)).abs() < 1e-9, "{total}");
        for t in &tiles {
            let (x0, y0) = (t.column as f64 * 4.0, t.row as f64 * 4.0);
            for p in t.region.outer.iter().chain(t.region.holes.iter().flatten()) {
                assert!(p[0] >= x0 - 1e-9 && p[0] <= x0 + 4.0 + 1e-9, "{p:?}");
                assert!(p[1] >= y0 - 1e-9 && p[1] <= y0 + 4.0 + 1e-9, "{p:?}");
            }
        }
        // 10 mm from 1 mm on a 4 mm grid: cells 0..=2 each way.
        assert_eq!(tiles.len(), 9);
    }

    /// Copper changed in one cell changes only that cell's tiles, even when
    /// the region's loop starts elsewhere.
    #[test]
    fn a_local_change_changes_only_its_cells_tiles() {
        // A fine circle-like outline, as a pour's clearance is flattened.
        let ring = |notch: bool| -> Vec<Point> {
            let mut out = Vec::new();
            for k in 0..400 {
                let a = std::f64::consts::TAU * k as f64 / 400.0;
                let r = if notch && (100..104).contains(&k) {
                    9.0
                } else {
                    10.0
                };
                out.push([12.0 + r * a.cos(), 12.0 + r * a.sin()]);
            }
            out
        };
        let before = tiles(
            &[FilledRegion {
                outer: ring(false),
                holes: vec![],
            }],
            4.0,
            0.01,
        );
        let mut turned = ring(true);
        turned.rotate_left(37);
        let after = tiles(
            &[FilledRegion {
                outer: turned,
                holes: vec![],
            }],
            4.0,
            0.01,
        );
        assert_eq!(before.len(), after.len());
        let changed: Vec<(i64, i64)> = before
            .iter()
            .zip(&after)
            .filter(|(a, b)| a != b)
            .map(|(a, _)| (a.column, a.row))
            .collect();
        // The notch at about 90°, (12, 22): the cells of row 5 near x = 12.
        assert!(!changed.is_empty() && changed.len() <= 2, "{changed:?}");
        assert!(changed.iter().all(|&(_, row)| row == 5), "{changed:?}");
    }
}
