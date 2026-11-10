//! Thin display slabs over planar regions with holes: the soldermask with
//! its openings and the finish over a land with its bore. A region is
//! inside any `fills` loop and outside every `cuts` loop (nonzero winding,
//! so overlapping openings merge). Its faces are triangulated per tile by
//! horizontal slabs: each tile clips the loops to its square, splits at
//! every vertex and crossing height and pairs the edges in each slab into
//! trapezoids. Walls stand wherever the region's inside meets its outside.
use crate::geom::{Mesh, Vec3};
use pcbcraft_core::Point;

/// Most tiles of one region; larger regions get larger tiles.
const MAX_TILES: usize = 16_384;
/// Edges of one tile beyond which crossings are not searched (overlapping
/// openings that dense are not expected on a board).
const MAX_CROSSING_EDGES: usize = 1024;
const EPS: f64 = 1e-9;
/// How far past its tile a wall piece is classified (well above the probe
/// distance, well below any tile).
const WALL_MARGIN_MM: f64 = 1e-4;

#[derive(Debug)]
pub struct Region {
    pub fills: Vec<Vec<Point>>,
    pub cuts: Vec<Vec<Point>>,
}

impl Region {
    /// The region moved by `by`.
    pub fn shifted(&self, by: Point) -> Region {
        let shift = |loops: &[Vec<Point>]| -> Vec<Vec<Point>> {
            loops
                .iter()
                .map(|l| l.iter().map(|p| [p[0] + by[0], p[1] + by[1]]).collect())
                .collect()
        };
        Region {
            fills: shift(&self.fills),
            cuts: shift(&self.cuts),
        }
    }

    /// Names the region's shape to a nanometre: equal keys, equal slabs.
    pub fn key(&self) -> Vec<u64> {
        let mut out = vec![self.fills.len() as u64, self.cuts.len() as u64];
        for l in self.fills.iter().chain(&self.cuts) {
            out.push(l.len() as u64);
            for p in l {
                // Two's-complement bits of the nanometre count.
                out.extend(p.map(|x| (x * 1e6).round() as i64 as u64));
            }
        }
        out
    }
}

#[derive(Clone, Copy)]
struct Edge {
    lo: Point,
    hi: Point,
    /// +1 for an edge drawn upward, -1 downward.
    dir: i32,
    fill: bool,
}

impl Edge {
    fn x_at(&self, y: f64) -> f64 {
        let dy = self.hi[1] - self.lo[1];
        if dy.abs() < EPS {
            return self.lo[0];
        }
        self.lo[0] + (y - self.lo[1]) / dy * (self.hi[0] - self.lo[0])
    }
}

/// A slab mesh of `region` from `z0` up to `z1`, with square tiles of at
/// least `tile` mm.
pub fn slab(region: &Region, z0: f64, z1: f64, tile: f64) -> Mesh {
    let mut mesh = Mesh::default();
    let fills: Vec<Vec<Point>> = region.fills.iter().filter_map(|l| clean(l)).collect();
    let cuts: Vec<Vec<Point>> = region.cuts.iter().filter_map(|l| clean(l)).collect();
    if fills.is_empty() || z1 - z0 <= EPS {
        return mesh;
    }
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in fills.iter().flatten() {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let size = [hi[0] - lo[0], hi[1] - lo[1]];
    if !(size[0] > EPS && size[1] > EPS && size[0].is_finite() && size[1].is_finite()) {
        return mesh;
    }
    let mut step = tile.max(EPS);
    let tiles = |step: f64| {
        let n = |s: f64| ((s / step).ceil() as usize).max(1);
        (n(size[0]), n(size[1]))
    };
    while {
        let (nx, ny) = tiles(step);
        nx.saturating_mul(ny) > MAX_TILES
    } {
        step *= 2.0;
    }
    let (nx, ny) = tiles(step);
    let tile_of = |v: f64, k: usize, n: usize| -> usize {
        (((v - lo[k]) / step).floor().max(0.0) as usize).min(n - 1)
    };
    // Loops per tile, by their boxes.
    let loops: Vec<(&Vec<Point>, bool)> = fills
        .iter()
        .map(|l| (l, true))
        .chain(cuts.iter().map(|l| (l, false)))
        .collect();
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); nx * ny];
    for (index, (points, _)) in loops.iter().enumerate() {
        let (mut a, mut b) = ([f64::MAX; 2], [f64::MIN; 2]);
        for p in points.iter() {
            for k in 0..2 {
                a[k] = a[k].min(p[k]);
                b[k] = b[k].max(p[k]);
            }
        }
        if b[0] < lo[0] || b[1] < lo[1] || a[0] > hi[0] || a[1] > hi[1] {
            continue;
        }
        for ty in tile_of(a[1], 1, ny)..=tile_of(b[1], 1, ny) {
            for tx in tile_of(a[0], 0, nx)..=tile_of(b[0], 0, nx) {
                buckets[ty * nx + tx].push(index);
            }
        }
    }
    let mut walls: Vec<Vec<(Point, Point)>> = vec![Vec::new(); nx * ny];
    for (points, _) in &loops {
        let n = points.len();
        for i in 0..n {
            let (a, b) = (points[i], points[(i + 1) % n]);
            // Pieces cut at the tile lines, so each lies in the tile it is
            // classified in.
            let mut ts = vec![0.0, 1.0];
            for k in 0..2 {
                let d = b[k] - a[k];
                if d.abs() < EPS {
                    continue;
                }
                let (from, to) = (a[k].min(b[k]), a[k].max(b[k]));
                let first = ((from - lo[k]) / step).ceil() as i64;
                let last = ((to - lo[k]) / step).floor() as i64;
                for line in first.max(0)..=last.min(first.max(0) + 4096) {
                    let t = (lo[k] + line as f64 * step - a[k]) / d;
                    if t > 0.0 && t < 1.0 {
                        ts.push(t);
                    }
                }
                // And at the far side of the fills' box (no tile line
                // need lie there): a cut's edge running out of the box is
                // kept up to it, not dropped whole by its midpoint.
                let t = (hi[k] - a[k]) / d;
                if t > 0.0 && t < 1.0 {
                    ts.push(t);
                }
            }
            ts.sort_by(f64::total_cmp);
            ts.dedup_by(|x, y| (*x - *y).abs() < EPS);
            for w in ts.windows(2) {
                let (t0, t1) = (w[0], w[1]);
                let p = [a[0] + (b[0] - a[0]) * t0, a[1] + (b[1] - a[1]) * t0];
                let q = [a[0] + (b[0] - a[0]) * t1, a[1] + (b[1] - a[1]) * t1];
                let m = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0];
                if m[0] < lo[0] || m[1] < lo[1] || m[0] > hi[0] || m[1] > hi[1] {
                    continue;
                }
                walls[tile_of(m[1], 1, ny) * nx + tile_of(m[0], 0, nx)].push((p, q));
            }
        }
    }
    for ty in 0..ny {
        for tx in 0..nx {
            let index = ty * nx + tx;
            let x0 = lo[0] + tx as f64 * step;
            let y0 = lo[1] + ty as f64 * step;
            let window = [x0, y0, (x0 + step).min(hi[0]), (y0 + step).min(hi[1])];
            let clipped: Vec<(Vec<Point>, bool)> = buckets[index]
                .iter()
                .filter_map(|&i| {
                    let (points, fill) = loops[i];
                    let c = clip(points, window);
                    (c.len() >= 3).then_some((c, fill))
                })
                .collect();
            faces(&mut mesh, &edges_of(&clipped), window, z0, z1);
            if walls[index].is_empty() {
                continue;
            }
            // Walls are classified a little past the tile, so a piece on its
            // border is judged on both sides.
            let m = WALL_MARGIN_MM;
            let grown = [window[0] - m, window[1] - m, window[2] + m, window[3] + m];
            let around: Vec<(Vec<Point>, bool)> = buckets[index]
                .iter()
                .filter_map(|&i| {
                    let (points, fill) = loops[i];
                    let c = clip(points, grown);
                    (c.len() >= 3).then_some((c, fill))
                })
                .collect();
            let edges = edges_of(&around);
            // Loops sharing an edge the same way round (two round ends on
            // one centre) stand one wall there, not two.
            let mut stood: std::collections::HashSet<[i64; 4]> = Default::default();
            let key = |p: Point, q: Point| {
                let k = |v: f64| (v * 1e7).round() as i64;
                [k(p[0]), k(p[1]), k(q[0]), k(q[1])]
            };
            for &(p, q) in &walls[index] {
                // A piece another loop crosses is inside on one part and
                // outside on the other: classify each part on its own.
                for (a, b) in split(p, q, &around) {
                    if stood.insert(key(a, b)) {
                        wall(&mut mesh, &edges, a, b, z0, z1);
                    }
                }
            }
        }
    }
    mesh
}

/// The piece `p`–`q` split where any edge of `loops` crosses it.
fn split(p: Point, q: Point, loops: &[(Vec<Point>, bool)]) -> Vec<(Point, Point)> {
    let r = [q[0] - p[0], q[1] - p[1]];
    let mut ts: Vec<f64> = Vec::new();
    let count: usize = loops.iter().map(|(l, _)| l.len()).sum();
    let edges = loops.iter().flat_map(|(l, _)| {
        let n = l.len();
        (0..n).map(move |i| (l[i], l[(i + 1) % n]))
    });
    if count <= MAX_CROSSING_EDGES {
        for (lo, hi) in edges {
            let s = [hi[0] - lo[0], hi[1] - lo[1]];
            let d = r[0] * s[1] - r[1] * s[0];
            if d.abs() < EPS * EPS {
                continue;
            }
            let w = [lo[0] - p[0], lo[1] - p[1]];
            let t = (w[0] * s[1] - w[1] * s[0]) / d;
            let u = (w[0] * r[1] - w[1] * r[0]) / d;
            if t > EPS && t < 1.0 - EPS && (0.0..=1.0).contains(&u) {
                ts.push(t);
            }
        }
    }
    if ts.is_empty() {
        return vec![(p, q)];
    }
    ts.push(0.0);
    ts.push(1.0);
    ts.sort_by(f64::total_cmp);
    ts.dedup_by(|a, b| (*a - *b).abs() < EPS);
    let at = |t: f64| [p[0] + r[0] * t, p[1] + r[1] * t];
    ts.windows(2).map(|w| (at(w[0]), at(w[1]))).collect()
}

/// Distinct points, counter-clockwise, at least a triangle.
fn clean(points: &[Point]) -> Option<Vec<Point>> {
    let mut out: Vec<Point> = points
        .iter()
        .copied()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .collect();
    out.dedup_by(|a, b| (a[0] - b[0]).hypot(a[1] - b[1]) < EPS);
    while out.len() > 1
        && let (Some(first), Some(last)) = (out.first(), out.last())
        && (first[0] - last[0]).hypot(first[1] - last[1]) < EPS
    {
        out.pop();
    }
    if out.len() < 3 {
        return None;
    }
    if signed_area(&out) < 0.0 {
        out.reverse();
    }
    Some(out)
}

fn signed_area(points: &[Point]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

/// Sutherland–Hodgman clip of a loop to an axis-aligned window
/// `[x0, y0, x1, y1]`. Pieces along the window border cancel in winding.
fn clip(points: &[Point], window: [f64; 4]) -> Vec<Point> {
    let mut out = points.to_vec();
    for side in 0..4 {
        if out.is_empty() {
            break;
        }
        let inside = |p: Point| match side {
            0 => p[0] >= window[0],
            1 => p[0] <= window[2],
            2 => p[1] >= window[1],
            _ => p[1] <= window[3],
        };
        let cut = |a: Point, b: Point| {
            let (k, v) = match side {
                0 => (0, window[0]),
                1 => (0, window[2]),
                2 => (1, window[1]),
                _ => (1, window[3]),
            };
            let t = (v - a[k]) / (b[k] - a[k]);
            let mut p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            p[k] = v;
            p
        };
        let input = std::mem::take(&mut out);
        let n = input.len();
        for i in 0..n {
            let (a, b) = (input[(i + n - 1) % n], input[i]);
            match (inside(a), inside(b)) {
                (true, true) => out.push(b),
                (true, false) => out.push(cut(a, b)),
                (false, true) => {
                    out.push(cut(a, b));
                    out.push(b);
                }
                (false, false) => {}
            }
        }
    }
    out
}

fn edges_of(loops: &[(Vec<Point>, bool)]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for (points, fill) in loops {
        let n = points.len();
        for i in 0..n {
            let (a, b) = (points[i], points[(i + 1) % n]);
            if (a[1] - b[1]).abs() < EPS {
                continue;
            }
            let (lo, hi, dir) = if a[1] < b[1] { (a, b, 1) } else { (b, a, -1) };
            edges.push(Edge {
                lo,
                hi,
                dir,
                fill: *fill,
            });
        }
    }
    edges
}

/// Whether a point is in the region, from the tile's clipped edges.
fn inside(edges: &[Edge], p: Point) -> bool {
    let (mut fill, mut cut) = (0, 0);
    for e in edges {
        if e.lo[1] <= p[1] && p[1] < e.hi[1] && e.x_at(p[1]) > p[0] {
            if e.fill {
                fill += e.dir;
            } else {
                cut += e.dir;
            }
        }
    }
    fill != 0 && cut == 0
}

/// Top and bottom faces of a tile's part of the region.
fn faces(mesh: &mut Mesh, edges: &[Edge], window: [f64; 4], z0: f64, z1: f64) {
    if edges.is_empty() {
        return;
    }
    let mut ys: Vec<f64> = edges.iter().flat_map(|e| [e.lo[1], e.hi[1]]).collect();
    if edges.len() <= MAX_CROSSING_EDGES {
        for (i, a) in edges.iter().enumerate() {
            for b in &edges[i + 1..] {
                if let Some(y) = crossing(a, b) {
                    ys.push(y);
                }
            }
        }
    }
    ys.retain(|y| *y >= window[1] - EPS && *y <= window[3] + EPS);
    ys.sort_by(f64::total_cmp);
    ys.dedup_by(|a, b| (*a - *b).abs() < EPS);
    let mut span: Vec<(f64, f64, f64, &Edge)> = Vec::new();
    for w in ys.windows(2) {
        let (ya, yb) = (w[0], w[1]);
        if yb - ya < EPS {
            continue;
        }
        let ym = (ya + yb) / 2.0;
        span.clear();
        span.extend(
            edges
                .iter()
                .filter(|e| e.lo[1] <= ym && e.hi[1] > ym)
                .map(|e| (e.x_at(ym), e.x_at(ya), e.x_at(yb), e)),
        );
        span.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut fill, mut cut) = (0, 0);
        let mut from: Option<(f64, f64)> = None;
        for &(_, xa, xb, e) in &span {
            let was = fill != 0 && cut == 0;
            if e.fill {
                fill += e.dir;
            } else {
                cut += e.dir;
            }
            let now = fill != 0 && cut == 0;
            match (was, now) {
                (false, true) => from = Some((xa, xb)),
                (true, false) => {
                    if let Some((la, lb)) = from.take() {
                        trapezoid(mesh, [la, xa], [lb, xb], ya, yb, z0, z1);
                    }
                }
                _ => {}
            }
        }
    }
}

/// The height where two edges cross strictly inside both, if they do.
fn crossing(a: &Edge, b: &Edge) -> Option<f64> {
    let (p, r) = (a.lo, [a.hi[0] - a.lo[0], a.hi[1] - a.lo[1]]);
    let (q, s) = (b.lo, [b.hi[0] - b.lo[0], b.hi[1] - b.lo[1]]);
    let d = r[0] * s[1] - r[1] * s[0];
    if d.abs() < EPS * EPS {
        return None;
    }
    let qp = [q[0] - p[0], q[1] - p[1]];
    let t = (qp[0] * s[1] - qp[1] * s[0]) / d;
    let u = (qp[0] * r[1] - qp[1] * r[0]) / d;
    (t > EPS && t < 1.0 - EPS && u > EPS && u < 1.0 - EPS).then_some(p[1] + t * r[1])
}

fn vertex(mesh: &mut Mesh, p: Vec3, n: Vec3) -> u32 {
    mesh.positions.push(p);
    mesh.normals.push(n);
    u32::try_from(mesh.positions.len() - 1).unwrap_or(u32::MAX)
}

/// A slab trapezoid between heights `ya` and `yb`: left and right x at
/// each, as top (up) and bottom (down) faces.
fn trapezoid(mesh: &mut Mesh, a: [f64; 2], b: [f64; 2], ya: f64, yb: f64, z0: f64, z1: f64) {
    if a[1] - a[0] < EPS && b[1] - b[0] < EPS {
        return;
    }
    for (z, up) in [(z1, true), (z0, false)] {
        let n = Vec3::new(0.0, 0.0, if up { 1.0 } else { -1.0 });
        let q = [
            vertex(mesh, Vec3::new(a[0], ya, z), n),
            vertex(mesh, Vec3::new(a[1], ya, z), n),
            vertex(mesh, Vec3::new(b[1], yb, z), n),
            vertex(mesh, Vec3::new(b[0], yb, z), n),
        ];
        let face = u32::from(!up);
        // A trapezoid narrowing to a point has one triangle, not a second
        // of no area.
        let (low, high) = (a[1] - a[0] >= EPS, b[1] - b[0] >= EPS);
        if low {
            mesh.triangles.push(if up {
                [q[0], q[1], q[2]]
            } else {
                [q[0], q[2], q[1]]
            });
            mesh.tri_face.push(face);
        }
        if high {
            mesh.triangles.push(if up {
                [q[0], q[2], q[3]]
            } else {
                [q[0], q[3], q[2]]
            });
            mesh.tri_face.push(face);
        }
    }
}

/// A wall along a loop piece where the region's inside meets its outside,
/// facing out of the region.
fn wall(mesh: &mut Mesh, edges: &[Edge], p: Point, q: Point, z0: f64, z1: f64) {
    let d = [q[0] - p[0], q[1] - p[1]];
    let l = d[0].hypot(d[1]);
    if l < EPS {
        return;
    }
    let right = [d[1] / l, -d[0] / l];
    let m = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0];
    let probe = 1e-6;
    let at = |s: f64| [m[0] + right[0] * s, m[1] + right[1] * s];
    let (r, left) = (inside(edges, at(probe)), inside(edges, at(-probe)));
    if r == left {
        return;
    }
    // Facing right (out of the region on that side) keeps p → q order.
    let (a, b, n) = if left {
        (p, q, Vec3::new(right[0], right[1], 0.0))
    } else {
        (q, p, Vec3::new(-right[0], -right[1], 0.0))
    };
    let v = [
        vertex(mesh, Vec3::new(a[0], a[1], z0), n),
        vertex(mesh, Vec3::new(b[0], b[1], z0), n),
        vertex(mesh, Vec3::new(b[0], b[1], z1), n),
        vertex(mesh, Vec3::new(a[0], a[1], z1), n),
    ];
    mesh.triangles.push([v[0], v[1], v[2]]);
    mesh.triangles.push([v[0], v[2], v[3]]);
    mesh.tri_face.extend([2, 2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::prelude::*;

    fn square(c: Point, h: f64) -> Vec<Point> {
        vec![
            [c[0] - h, c[1] - h],
            [c[0] + h, c[1] - h],
            [c[0] + h, c[1] + h],
            [c[0] - h, c[1] + h],
        ]
    }

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    fn face_area(mesh: &Mesh, face: u32) -> f64 {
        mesh.triangles
            .iter()
            .zip(&mesh.tri_face)
            .filter(|(_, f)| **f == face)
            .map(|(t, _)| {
                let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
                (b - a).cross(c - a).len() / 2.0
            })
            .sum()
    }

    #[test]
    fn a_slab_with_holes_is_closed_and_exact_across_tiles() {
        let board = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 6.0], [0.0, 6.0]];
        // One opening straddles tile edges.
        let region = Region {
            fills: vec![board.clone()],
            cuts: vec![square([2.0, 2.0], 0.5), square([4.0, 4.0], 1.0)],
        };
        for tile in [100.0, 4.0, 1.3] {
            let mesh = slab(&region, 0.0, 0.1, tile);
            near(mesh.measure().volume, (60.0 - 1.0 - 4.0) * 0.1);
            near(face_area(&mesh, 0), 55.0);
            near(face_area(&mesh, 1), 55.0);
            near(face_area(&mesh, 2), (32.0 + 4.0 + 8.0) * 0.1);
        }
        // Overlapping openings merge (nonzero winding): the union is open.
        let region = Region {
            fills: vec![board],
            cuts: vec![square([2.0, 2.0], 0.5), square([2.5, 2.2], 0.5)],
        };
        for tile in [100.0, 1.1] {
            let mesh = slab(&region, 0.0, 0.1, tile);
            near(face_area(&mesh, 0), 60.0 - (2.0 - 0.5 * 0.8));
        }
    }

    /// Overlapping fills cross inside wall pieces: the slab stays a closed
    /// solid of the union's volume whatever the tiling.
    #[test]
    fn crossing_fills_make_a_closed_union_on_any_tiling() {
        let bar = |a: Point, b: Point, h: f64| -> Vec<Point> {
            let d = [b[0] - a[0], b[1] - a[1]];
            let l = d[0].hypot(d[1]);
            let n = [-d[1] / l * h, d[0] / l * h];
            vec![
                [a[0] - n[0], a[1] - n[1]],
                [b[0] - n[0], b[1] - n[1]],
                [b[0] + n[0], b[1] + n[1]],
                [a[0] + n[0], a[1] + n[1]],
            ]
        };
        // Two round-ended strokes meeting at a corner share an edge of
        // their ends: the union is less than the two.
        let end = |c: Point, from: f64| -> Vec<Point> {
            let mut out = Vec::new();
            for k in 0..=4 {
                let a = std::f64::consts::PI * (from + k as f64 / 4.0);
                out.push([c[0] + 0.1 * a.cos(), c[1] + 0.1 * a.sin()]);
            }
            out
        };
        let mut along = end([1.0, 0.0], -0.5);
        along.extend(end([0.0, 0.0], 0.5));
        let mut up = end([1.0, 1.0], 0.0);
        up.extend(end([1.0, 0.0], 1.0));
        let one = slab(
            &Region {
                fills: vec![along.clone()],
                cuts: vec![],
            },
            0.0,
            1.0,
            1.0,
        );
        let both = slab(
            &Region {
                fills: vec![along, up],
                cuts: vec![],
            },
            0.0,
            1.0,
            1.0,
        );
        let (v1, v2) = (one.measure().volume, both.measure().volume);
        assert!(v2 < 2.0 * v1 && v2 > 1.8 * v1, "{v1} {v2}");
        near(face_area(&both, 0), v2);
        // A cross of two bars 0.2 wide, 2 long: 0.8 mm², its middle once.
        let region = Region {
            fills: vec![
                bar([-1.0, 0.0], [1.0, 0.0], 0.1),
                bar([0.0, -1.0], [0.0, 1.0], 0.1),
            ],
            cuts: vec![],
        };
        for tile in [100.0, 2.0, 0.3, 0.07] {
            let mesh = slab(&region, 0.0, 1.0, tile);
            near(mesh.measure().volume, 0.8 - 0.04);
            near(face_area(&mesh, 0), 0.76);
            near(face_area(&mesh, 2), 8.0);
        }
    }

    /// An opening reaching out past the fill (a moved part's pad over a
    /// small pour island): its walls stand inside the fill up to the
    /// fill's edge, so the slab stays closed whatever the tiling.
    #[test]
    fn an_opening_running_out_of_the_fill_keeps_its_walls() {
        let region = Region {
            fills: vec![square([0.5, 0.5], 0.5)],
            cuts: vec![vec![[0.5, 0.5], [0.8, 0.5], [0.8, 5.0], [0.5, 5.0]]],
        };
        for tile in [100.0, 0.3, 0.07] {
            let mesh = slab(&region, 0.0, 0.1, tile);
            near(mesh.measure().volume, (1.0 - 0.3 * 0.5) * 0.1);
            near(face_area(&mesh, 2), (4.0 - 0.3 + 0.5 + 0.3 + 0.5) * 0.1);
        }
    }

    #[test]
    fn a_ring_is_the_disc_less_its_bore_either_way_round() {
        let ring = |n: usize, r: f64| -> Vec<Point> {
            (0..n)
                .map(|k| {
                    let a = std::f64::consts::TAU * k as f64 / n as f64;
                    [r * a.cos(), r * a.sin()]
                })
                .collect()
        };
        let mut bore = ring(64, 0.3);
        bore.reverse();
        let region = Region {
            fills: vec![ring(64, 1.0)],
            cuts: vec![bore.clone()],
        };
        let mesh = slab(&region, 0.0, 1.0, 10.0);
        let expected = signed_area(&ring(64, 1.0)) - signed_area(&ring(64, 0.3));
        near(mesh.measure().volume, expected);
        assert!(!mesh.contains(Vec3::new(0.0, 0.0, 0.5)));
        assert!(mesh.contains(Vec3::new(0.6, 0.1, 0.5)));
    }
}
