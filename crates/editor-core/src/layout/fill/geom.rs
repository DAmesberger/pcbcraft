//! Polygon set operations for fills, over the permissive `i_overlay`
//! crate (MIT OR Apache-2.0; integer-snapped, so results are
//! deterministic), and a row index for point-in-region queries on large
//! fills.
//!
//! A [`Ring`] is a closed polygon without a repeated first point. A
//! [`Region`] is an outer ring (counter-clockwise) followed by its holes
//! (clockwise), as `i_overlay` returns them. Every input ring is normalised
//! to counter-clockwise before a union, so overlapping obstacles add up
//! under the non-zero rule whatever their source orientation.
use crate::geometry::Point;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};

pub(crate) type Ring = Vec<Point>;
pub(crate) type Region = Vec<Ring>;

/// Twice the signed area (positive counter-clockwise).
pub(crate) fn area2(ring: &[Point]) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// The region's area: its outer ring less its holes, mm².
pub(crate) fn region_area(region: &Region) -> f64 {
    region.iter().map(|r| area2(r)).sum::<f64>().abs() / 2.0
}

/// `ring` counter-clockwise, or `None` when it has no area.
pub(crate) fn ccw(mut ring: Ring) -> Option<Ring> {
    if ring.len() < 3 {
        return None;
    }
    let a = area2(&ring);
    if !a.is_finite() || a.abs() < 1e-12 {
        return None;
    }
    if a < 0.0 {
        ring.reverse();
    }
    Some(ring)
}

/// The union of counter-clockwise `rings` (non-zero).
pub(crate) fn union(rings: &[Ring]) -> Vec<Region> {
    if rings.is_empty() {
        return Vec::new();
    }
    let none: Vec<Ring> = Vec::new();
    rings.overlay(&none, OverlayRule::Subject, FillRule::NonZero)
}

/// The union of regions.
pub(crate) fn union_regions(regions: &[Region]) -> Vec<Region> {
    if regions.is_empty() {
        return Vec::new();
    }
    let none: Vec<Ring> = Vec::new();
    regions.overlay(&none, OverlayRule::Subject, FillRule::NonZero)
}

/// `a` less `b`.
pub(crate) fn difference(a: &[Region], b: &[Region]) -> Vec<Region> {
    if a.is_empty() || b.is_empty() {
        return a.to_vec();
    }
    a.overlay(&b, OverlayRule::Difference, FillRule::NonZero)
}

/// `a` less counter-clockwise `rings`.
pub(crate) fn difference_rings(a: &[Region], rings: &[Ring]) -> Vec<Region> {
    if a.is_empty() || rings.is_empty() {
        return a.to_vec();
    }
    a.overlay(&rings, OverlayRule::Difference, FillRule::NonZero)
}

/// `a` within `b`.
pub(crate) fn intersect(a: &[Region], b: &[Region]) -> Vec<Region> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    a.overlay(&b, OverlayRule::Intersect, FillRule::NonZero)
}

/// The join step angle (segment length over radius) that keeps an arc of
/// `radius` within `tol` of its chords.
fn step(radius: f64, tol: f64) -> f64 {
    let r = radius.abs().max(tol);
    2.0 * (1.0 - (tol / r).min(1.0)).acos().max(0.01)
}

/// The regions grown by `by` mm (negative shrinks) with round joins whose
/// vertices lie on the true arcs, within `tol`.
pub(crate) fn offset(regions: &[Region], by: f64, tol: f64) -> Vec<Region> {
    if regions.is_empty() || by == 0.0 {
        return regions.to_vec();
    }
    let style = OutlineStyle::new(by).line_join(LineJoin::Round(step(by, tol)));
    regions.outline(&style)
}

/// A circle of `radius` as a counter-clockwise ring whose chords stay
/// outside the true circle by at most `tol`: every vertex sits on the
/// circumscribed radius, so the ring covers the circle.
pub(crate) fn circle(center: Point, radius: f64, tol: f64) -> Ring {
    let sweep = step(radius, tol);
    let n = ((std::f64::consts::TAU / sweep).ceil() as usize).clamp(8, 512);
    let r = radius / (std::f64::consts::PI / n as f64).cos();
    (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            [center[0] + r * a.cos(), center[1] + r * a.sin()]
        })
        .collect()
}

/// The stadium within `half` of segment `a`–`b`, covering the true shape
/// (round caps circumscribed as for [`circle`]).
pub(crate) fn stadium(a: Point, b: Point, half: f64, tol: f64) -> Ring {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0].hypot(d[1]);
    if len < 1e-9 {
        return circle(a, half, tol);
    }
    let base = d[1].atan2(d[0]);
    let sweep = step(half, tol);
    let n = ((std::f64::consts::PI / sweep).ceil() as usize).clamp(4, 256);
    let r = half / (std::f64::consts::FRAC_PI_2 / n as f64).cos();
    let mut ring = Vec::with_capacity(2 * n + 2);
    // Cap around `b` from the right normal to the left, then around `a`.
    for (center, start) in [
        (b, base - std::f64::consts::FRAC_PI_2),
        (a, base + std::f64::consts::FRAC_PI_2),
    ] {
        ring.push([
            center[0] + half * start.cos(),
            center[1] + half * start.sin(),
        ]);
        for i in 0..n {
            let t = start + std::f64::consts::PI * (i as f64 + 0.5) / n as f64;
            ring.push([center[0] + r * t.cos(), center[1] + r * t.sin()]);
        }
        let end = start + std::f64::consts::PI;
        ring.push([center[0] + half * end.cos(), center[1] + half * end.sin()]);
    }
    ring
}

/// A rectangle of `width` along `a`–`b`.
pub(crate) fn bar(a: Point, b: Point, width: f64) -> Option<Ring> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0].hypot(d[1]);
    if len < 1e-9 || width <= 0.0 {
        return None;
    }
    let n = [-d[1] / len * width / 2.0, d[0] / len * width / 2.0];
    ccw(vec![
        [a[0] - n[0], a[1] - n[1]],
        [b[0] - n[0], b[1] - n[1]],
        [b[0] + n[0], b[1] + n[1]],
        [a[0] + n[0], a[1] + n[1]],
    ])
}

/// Axis-aligned bounds of rings: `[min, max]`.
pub(crate) fn bounds<'a>(rings: impl IntoIterator<Item = &'a Ring>) -> Option<[Point; 2]> {
    let mut out: Option<[Point; 2]> = None;
    for p in rings.into_iter().flatten() {
        let b = out.get_or_insert([*p, *p]);
        b[0] = [b[0][0].min(p[0]), b[0][1].min(p[1])];
        b[1] = [b[1][0].max(p[0]), b[1][1].max(p[1])];
    }
    out
}

pub(crate) fn overlaps(a: [Point; 2], b: [Point; 2], margin: f64) -> bool {
    a[0][0] <= b[1][0] + margin
        && b[0][0] <= a[1][0] + margin
        && a[0][1] <= b[1][1] + margin
        && b[0][1] <= a[1][1] + margin
}

/// Edges of many regions binned into horizontal rows, so "which region
/// holds this point" and "how close is the nearest edge" cost a row scan
/// instead of a pass over every vertex.
pub(crate) struct RegionIndex {
    min: Point,
    cell: f64,
    cols: usize,
    rows: usize,
    /// Per cell, the (region, a, b) edges crossing it.
    cells: Vec<Vec<(u32, Point, Point)>>,
}

impl RegionIndex {
    pub(crate) fn new(regions: &[Region]) -> Self {
        let edges: usize = regions.iter().flatten().map(Vec::len).sum();
        let Some([min, max]) = bounds(regions.iter().flatten()) else {
            return Self {
                min: [0.0, 0.0],
                cell: 1.0,
                cols: 0,
                rows: 0,
                cells: Vec::new(),
            };
        };
        let size = [(max[0] - min[0]).max(1e-6), (max[1] - min[1]).max(1e-6)];
        // About four edges per cell, at most 512 × 512 cells.
        let target = ((edges / 4).max(1) as f64).sqrt();
        let cell = (size[0].max(size[1]) / target.clamp(1.0, 512.0)).max(1e-3);
        let cols = ((size[0] / cell).ceil() as usize).clamp(1, 512);
        let rows = ((size[1] / cell).ceil() as usize).clamp(1, 512);
        let mut index = Self {
            min,
            cell,
            cols,
            rows,
            cells: vec![Vec::new(); cols * rows],
        };
        for (r, region) in regions.iter().enumerate() {
            let r = u32::try_from(r).unwrap_or(u32::MAX);
            for ring in region {
                let n = ring.len();
                for i in 0..n {
                    let (a, b) = (ring[i], ring[(i + 1) % n]);
                    let (c0, r0) = index.at([a[0].min(b[0]), a[1].min(b[1])]);
                    let (c1, r1) = index.at([a[0].max(b[0]), a[1].max(b[1])]);
                    for row in r0..=r1 {
                        for col in c0..=c1 {
                            if let Some(cell) = index.cells.get_mut(row * cols + col) {
                                cell.push((r, a, b));
                            }
                        }
                    }
                }
            }
        }
        index
    }

    fn at(&self, p: Point) -> (usize, usize) {
        let f = |v: f64, n: usize| {
            if v.is_finite() && v > 0.0 {
                (v as usize).min(n.saturating_sub(1))
            } else {
                0
            }
        };
        (
            f((p[0] - self.min[0]) / self.cell, self.cols),
            f((p[1] - self.min[1]) / self.cell, self.rows),
        )
    }

    /// The region holding `p` (even-odd over its rings), if any.
    pub(crate) fn region_at(&self, p: Point) -> Option<u32> {
        if self.cells.is_empty() {
            return None;
        }
        let max_y = self.min[1] + self.cell * self.rows as f64;
        if p[1] < self.min[1] || p[1] > max_y || p[0] > self.min[0] + self.cell * self.cols as f64 {
            return None;
        }
        let (col, row) = self.at(p);
        // Cast to +x through the row; an edge stored in several cells is
        // counted only in the cell its crossing lies in.
        let mut parity: Vec<(u32, bool)> = Vec::new();
        for c in col..self.cols {
            let Some(cell) = self.cells.get(row * self.cols + c) else {
                continue;
            };
            for &(r, a, b) in cell {
                if (a[1] > p[1]) == (b[1] > p[1]) {
                    continue;
                }
                let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                if x <= p[0] || self.at([x, p[1]]).0 != c {
                    continue;
                }
                match parity.iter_mut().find(|(id, _)| *id == r) {
                    Some((_, odd)) => *odd = !*odd,
                    None => parity.push((r, true)),
                }
            }
        }
        parity.into_iter().find(|(_, odd)| *odd).map(|(r, _)| r)
    }

    /// The least distance from segment `a`–`b` to an edge within `limit`
    /// (0 when they cross), or `limit`.
    pub(crate) fn segment_distance(&self, a: Point, b: Point, limit: f64) -> f64 {
        if self.cells.is_empty() {
            return limit;
        }
        let lo = [a[0].min(b[0]) - limit, a[1].min(b[1]) - limit];
        let hi = [a[0].max(b[0]) + limit, a[1].max(b[1]) + limit];
        let (c0, r0) = self.at(lo);
        let (c1, r1) = self.at(hi);
        let mut best = limit;
        for row in r0..=r1 {
            for col in c0..=c1 {
                for &(_, c, d) in self.cells.get(row * self.cols + col).into_iter().flatten() {
                    let gap = if crate::geometry::segments_cross(a, b, c, d) {
                        0.0
                    } else {
                        crate::geometry::segment_distance(a, c, d)
                            .min(crate::geometry::segment_distance(b, c, d))
                            .min(crate::geometry::segment_distance(c, a, b))
                            .min(crate::geometry::segment_distance(d, a, b))
                    };
                    best = best.min(gap);
                }
            }
        }
        best
    }
}
