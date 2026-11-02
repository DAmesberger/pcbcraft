//! Flat fills of closed loops by the nonzero winding rule, for decals: the
//! plane is cut into horizontal slabs at every vertex height and at every
//! height where two edges cross, so inside one slab no edges cross and the
//! edges ordered by x pair up into trapezoids where the winding is not
//! zero. A glyph's counters (wound against its outline) stay open; loops
//! wound the same way merge. Single-sided: every triangle is
//! counter-clockwise in the plane.
use crate::{Error, Result};
use pcbcraft_core::Point;

/// Edges one fill takes.
pub const MAX_FILL_EDGES: usize = 32_768;
/// Edge pairs tested for crossings in one fill, in total.
const MAX_PAIR_TESTS: usize = 64 << 20;
/// Heights closer than this are one height, mm.
const EPS: f64 = 1e-9;

/// Flat triangles in a decal's plane: the vertices and counter-clockwise
/// index triples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flat {
    pub points: Vec<Point>,
    pub triangles: Vec<[u32; 3]>,
}

impl Flat {
    /// The filled area, mm² (each triangle counted once).
    #[cfg(test)]
    pub fn area(&self) -> f64 {
        self.triangles
            .iter()
            .filter_map(|t| {
                let p = |i: u32| self.points.get(i as usize).copied();
                Some(area2(p(t[0])?, p(t[1])?, p(t[2])?) / 2.0)
            })
            .sum()
    }
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    lo: Point,
    hi: Point,
    /// +1 for an edge running up, −1 for one running down.
    wind: i32,
}

impl Edge {
    fn x_at(&self, y: f64) -> f64 {
        let dy = self.hi[1] - self.lo[1];
        self.lo[0] + (y - self.lo[1]) / dy * (self.hi[0] - self.lo[0])
    }
}

fn area2(a: Point, b: Point, c: Point) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Fill `loops` (each implicitly closed) into `out`, adding at most
/// `budget` triangles; more is a refusal and leaves `out` as it was.
#[cfg(test)]
pub fn fill(loops: &[Vec<Point>], out: &mut Flat, budget: usize) -> Result<()> {
    fill_with(loops, FillRule::NonZero, out, budget)
}

/// How a fill decides what is inside: SVG's `fill-rule`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillRule {
    /// Inside where the winding number is not zero.
    #[default]
    NonZero,
    /// Inside where the winding number is odd.
    EvenOdd,
}

/// Fill `loops` by `rule` into `out`, adding at most `budget` triangles;
/// more is a refusal and leaves `out` as it was. Triangles are
/// counter-clockwise whatever the loops' winding.
pub fn fill_with(
    loops: &[Vec<Point>],
    rule: FillRule,
    out: &mut Flat,
    budget: usize,
) -> Result<()> {
    let inside = |w: i32| match rule {
        FillRule::NonZero => w != 0,
        FillRule::EvenOdd => w % 2 != 0,
    };
    let mut edges: Vec<Edge> = Vec::new();
    for points in loops {
        let n = points.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let (a, b) = (points[i], points[(i + 1) % n]);
            if !(a.iter().chain(&b).all(|v| v.is_finite())) {
                return Err(Error::Invalid(
                    "marking decal: an outline point is not finite".into(),
                ));
            }
            if (a[1] - b[1]).abs() <= EPS {
                continue;
            }
            edges.push(if a[1] < b[1] {
                Edge {
                    lo: a,
                    hi: b,
                    wind: 1,
                }
            } else {
                Edge {
                    lo: b,
                    hi: a,
                    wind: -1,
                }
            });
            if edges.len() > MAX_FILL_EDGES {
                return Err(Error::Invalid(format!(
                    "marking decal: more than {MAX_FILL_EDGES} outline edges"
                )));
            }
        }
    }
    if edges.is_empty() {
        return Ok(());
    }
    edges.sort_by(|a, b| a.lo[1].total_cmp(&b.lo[1]));
    let mut ys: Vec<f64> = edges.iter().flat_map(|e| [e.lo[1], e.hi[1]]).collect();
    ys.sort_by(f64::total_cmp);
    ys.dedup_by(|a, b| (*a - *b).abs() <= EPS);

    let start = (out.points.len(), out.triangles.len());
    let mut added = 0usize;
    let mut tests = 0usize;
    let mut next = 0usize;
    let mut active: Vec<Edge> = Vec::new();
    for w in ys.windows(2) {
        let (y0, y1) = (w[0], w[1]);
        active.retain(|e| e.hi[1] > y0 + EPS);
        while let Some(e) = edges.get(next).filter(|e| e.lo[1] <= y0 + EPS) {
            if e.hi[1] > y0 + EPS {
                active.push(*e);
            }
            next += 1;
        }
        if active.len() < 2 {
            continue;
        }
        // Heights inside the slab where two edges cross.
        let mut cuts = vec![y0, y1];
        tests = tests.saturating_add(active.len() * active.len() / 2);
        if tests > MAX_PAIR_TESTS {
            out.points.truncate(start.0);
            out.triangles.truncate(start.1);
            return Err(Error::Invalid(
                "marking decal: the outline is too dense to fill".into(),
            ));
        }
        for (i, a) in active.iter().enumerate() {
            for b in &active[i + 1..] {
                let d0 = a.x_at(y0) - b.x_at(y0);
                let d1 = a.x_at(y1) - b.x_at(y1);
                if d0 * d1 < 0.0 {
                    let y = y0 + (y1 - y0) * d0 / (d0 - d1);
                    if y > y0 + EPS && y < y1 - EPS {
                        cuts.push(y);
                    }
                }
            }
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() <= EPS);
        for c in cuts.windows(2) {
            let (a, b) = (c[0], c[1]);
            if b - a <= EPS {
                continue;
            }
            let mid = (a + b) / 2.0;
            let mut order: Vec<(f64, &Edge)> = active.iter().map(|e| (e.x_at(mid), e)).collect();
            order.sort_by(|p, q| p.0.total_cmp(&q.0));
            let mut winding = 0i32;
            let mut left: Option<&Edge> = None;
            for (_, e) in order {
                let before = winding;
                winding += e.wind;
                if !inside(before) && inside(winding) {
                    left = Some(e);
                } else if inside(before)
                    && !inside(winding)
                    && let Some(l) = left.take()
                {
                    added += trapezoid(out, l, e, a, b)?;
                    if added > budget {
                        out.points.truncate(start.0);
                        out.triangles.truncate(start.1);
                        return Err(Error::Invalid(format!(
                            "marking decal: more than {budget} triangles"
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

/// The trapezoid between edges `l` (left) and `r` from height `a` to `b`,
/// as up to two counter-clockwise triangles; the number added.
fn trapezoid(out: &mut Flat, l: &Edge, r: &Edge, a: f64, b: f64) -> Result<usize> {
    let p = [
        [l.x_at(a), a],
        [r.x_at(a), a],
        [r.x_at(b), b],
        [l.x_at(b), b],
    ];
    let base = u32::try_from(out.points.len())
        .map_err(|_| Error::Invalid("marking decal: too many vertices".into()))?;
    let mut added = 0;
    let mut used = [false; 4];
    let mut tris: Vec<[usize; 3]> = Vec::with_capacity(2);
    for t in [[0usize, 1, 2], [0, 2, 3]] {
        if area2(p[t[0]], p[t[1]], p[t[2]]) > 1e-14 {
            tris.push(t);
            for i in t {
                used[i] = true;
            }
        }
    }
    // Only the corners a triangle uses become vertices.
    let mut index = [0u32; 4];
    let mut k = 0u32;
    for (i, point) in p.iter().enumerate() {
        if used[i] {
            index[i] = base + k;
            out.points.push(*point);
            k += 1;
        }
    }
    for t in tris {
        out.triangles.push([index[t[0]], index[t[1]], index[t[2]]]);
        added += 1;
    }
    Ok(added)
}
