//! Convex hulls whose edges run in fixed directions: eight (every 45°)
//! for the 45° and free postures, four (axes only) for 90°. A hull is
//! the intersection of half-planes `d·p ≤ h(d)`, where `h` is the support
//! function of an obstacle's core shape grown by a distance `r`. Because
//! each half-plane supports the grown shape, every point outside the hull
//! is farther than `r` from the core: a centre line running on or outside
//! the hull keeps the clearance the growth encodes, and one running along
//! its edges hugs the obstacle at exactly that distance in the posture of
//! the edges. Hulls only guide the router; every segment it proposes is
//! checked against the exact copper before it is used.
use crate::geometry::{Point, distance, segment_distance};

const S: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// Unit directions at k·45°, counter-clockwise from +x.
pub(super) const DIRS: [Point; 8] = [
    [1.0, 0.0],
    [S, S],
    [0.0, 1.0],
    [-S, S],
    [-1.0, 0.0],
    [-S, -S],
    [0.0, -1.0],
    [S, -S],
];

/// Which directions a hull's edges take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Facets {
    /// Every 45°: octagons.
    Eight,
    /// Axes only: rectangles.
    Four,
}

impl Facets {
    fn uses(self, k: usize) -> bool {
        matches!(self, Facets::Eight) || k % 2 == 0
    }
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// A convex polygon with edges in the [`Facets`] directions.
#[derive(Clone, Debug)]
pub(super) struct Hull {
    support: [f64; 8],
    facets: Facets,
    /// Counter-clockwise corners (consecutive duplicates removed).
    points: Vec<Point>,
}

impl Hull {
    /// The hull of a support function.
    pub(super) fn from_support(support: [f64; 8], facets: Facets) -> Option<Self> {
        if support.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let mut hull = Self {
            support,
            facets,
            points: Vec::new(),
        };
        hull.rebuild();
        (hull.points.len() >= 3).then_some(hull)
    }

    fn rebuild(&mut self) {
        let s = self.support;
        // The axis box, then each diagonal half-plane clips it.
        let mut poly = vec![[-s[4], -s[6]], [s[0], -s[6]], [s[0], s[2]], [-s[4], s[2]]];
        if self.facets == Facets::Eight {
            for k in [1, 3, 5, 7] {
                poly = clip(&poly, DIRS[k], s[k]);
            }
        }
        let mut points: Vec<Point> = Vec::with_capacity(poly.len());
        for p in poly {
            if points.last().is_none_or(|q| distance(*q, p) > 1e-9) {
                points.push(p);
            }
        }
        while points.len() > 1
            && points
                .first()
                .zip(points.last())
                .is_some_and(|(a, b)| distance(*a, *b) <= 1e-9)
        {
            points.pop();
        }
        self.points = points;
    }

    /// Points within `r` of segment `a`–`b`, hulled.
    pub(super) fn of_segment(a: Point, b: Point, r: f64, facets: Facets) -> Option<Self> {
        let support = DIRS.map(|d| dot(d, a).max(dot(d, b)) + r);
        Self::from_support(support, facets)
    }

    /// Points within `r` of the polygon `ring` (its corners), hulled.
    pub(super) fn of_points(ring: &[Point], r: f64, facets: Facets) -> Option<Self> {
        if ring.is_empty() {
            return None;
        }
        let support = DIRS.map(|d| {
            ring.iter()
                .map(|p| dot(d, *p))
                .fold(f64::NEG_INFINITY, f64::max)
                + r
        });
        Self::from_support(support, facets)
    }

    /// The hull of both (the larger support in every direction).
    pub(super) fn merge(&mut self, other: &Hull) {
        for (s, o) in self.support.iter_mut().zip(other.support) {
            *s = s.max(o);
        }
        self.rebuild();
    }

    #[cfg(test)]
    pub(super) fn points(&self) -> &[Point] {
        &self.points
    }

    /// How far the hull reaches in direction `DIRS[k]`.
    pub(super) fn support(&self, k: usize) -> f64 {
        self.support.get(k).copied().unwrap_or(f64::INFINITY)
    }

    /// Whether `p` lies inside, more than `eps` from every edge.
    #[cfg(test)]
    pub(super) fn strictly_contains(&self, p: Point, eps: f64) -> bool {
        (0..8)
            .filter(|k| self.facets.uses(*k))
            .all(|k| dot(DIRS[k], p) < self.support[k] - eps)
    }

    /// Pull the hull back to `p` across the edge nearest it, so that a
    /// point inside the hull's corner overreach (but legal against the
    /// exact copper) lies on the boundary.
    pub(super) fn exclude(&mut self, p: Point) {
        let best = (0..8)
            .filter(|k| self.facets.uses(*k))
            .max_by(|a, b| {
                let ga = dot(DIRS[*a], p) - self.support[*a];
                let gb = dot(DIRS[*b], p) - self.support[*b];
                ga.total_cmp(&gb)
            });
        if let Some(k) = best {
            self.support[k] = dot(DIRS[k], p);
            self.rebuild();
        }
    }

    /// The parameter interval of segment `a`–`b` lying strictly inside
    /// (more than `eps` from the edges), or `None` when it stays out.
    pub(super) fn inside_interval(&self, a: Point, b: Point, eps: f64) -> Option<(f64, f64)> {
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let ab = [b[0] - a[0], b[1] - a[1]];
        for k in (0..8).filter(|k| self.facets.uses(*k)) {
            let d = DIRS[k];
            let limit = self.support[k] - eps;
            let start = dot(d, a) - limit;
            let rate = dot(d, ab);
            if rate.abs() < 1e-15 {
                if start >= 0.0 {
                    return None;
                }
                continue;
            }
            let t = -start / rate;
            if rate > 0.0 {
                t1 = t1.min(t);
            } else {
                t0 = t0.max(t);
            }
            if t0 >= t1 {
                return None;
            }
        }
        (t1 - t0 > 1e-12).then_some((t0, t1))
    }

    /// Whether the polyline enters the hull's interior.
    pub(super) fn crossed_by(&self, points: &[Point], eps: f64) -> bool {
        points
            .windows(2)
            .any(|w| self.inside_interval(w[0], w[1], eps).is_some())
    }

    /// The edge `k` (from corner `k` to `k + 1`) nearest `p`.
    fn edge_of(&self, p: Point) -> usize {
        let n = self.points.len();
        (0..n)
            .min_by(|i, j| {
                let d = |k: usize| {
                    segment_distance(
                        p,
                        self.points.get(k).copied().unwrap_or(p),
                        self.points.get((k + 1) % n).copied().unwrap_or(p),
                    )
                };
                d(*i).total_cmp(&d(*j))
            })
            .unwrap_or(0)
    }

    /// The corners met walking the boundary from `from` (on it), counter-
    /// clockwise when `ccw`, in order, one full turn at most.
    pub(super) fn corners_from(&self, from: Point, ccw: bool) -> Vec<Point> {
        let n = self.points.len();
        if n == 0 {
            return Vec::new();
        }
        let edge = self.edge_of(from);
        (0..n)
            .filter_map(|step| {
                let k = if ccw {
                    (edge + 1 + step) % n
                } else {
                    (edge + n - step) % n
                };
                self.points.get(k).copied()
            })
            .collect()
    }

    /// The boundary from `from` to `to` (both on it): `from`, the corners
    /// between, `to`.
    pub(super) fn arc(&self, from: Point, to: Point, ccw: bool) -> Vec<Point> {
        let n = self.points.len();
        let mut out = vec![from];
        if n == 0 {
            out.push(to);
            return out;
        }
        let (a, b) = (self.edge_of(from), self.edge_of(to));
        let same_edge_ahead = a == b && {
            // On one edge: going the edge's way reaches `to` directly when
            // it lies ahead of `from`.
            let p = self.points.get(a).copied().unwrap_or(from);
            let q = self.points.get((a + 1) % n).copied().unwrap_or(from);
            let dir = [q[0] - p[0], q[1] - p[1]];
            let ahead = dot(dir, [to[0] - from[0], to[1] - from[1]]) >= 0.0;
            ahead == ccw
        };
        if !same_edge_ahead {
            for step in 0..n {
                let k = if ccw {
                    (a + 1 + step) % n
                } else {
                    (a + n - step) % n
                };
                if let Some(p) = self.points.get(k) {
                    out.push(*p);
                }
                let reached = if ccw { k == b } else { (k + n - 1) % n == b };
                if reached {
                    break;
                }
            }
        }
        out.push(to);
        out
    }
}

/// Sutherland–Hodgman: the part of convex `poly` with `d·p ≤ h`.
fn clip(poly: &[Point], d: Point, h: f64) -> Vec<Point> {
    let mut out = Vec::with_capacity(poly.len() + 1);
    let n = poly.len();
    for i in 0..n {
        let (Some(&a), Some(&b)) = (poly.get(i), poly.get((i + 1) % n)) else {
            continue;
        };
        let (fa, fb) = (dot(d, a) - h, dot(d, b) - h);
        if fa <= 0.0 {
            out.push(a);
        }
        if (fa < 0.0 && fb > 0.0) || (fa > 0.0 && fb < 0.0) {
            let t = fa / (fa - fb);
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grown segment's octagon keeps every outside point farther than
    /// the growth from the segment, and its edges are at 45° multiples.
    #[test]
    fn an_octagon_contains_the_grown_segment() {
        let Some(h) = Hull::of_segment([0.0, 0.0], [4.0, 0.0], 1.0, Facets::Eight) else {
            panic!("no hull");
        };
        assert_eq!(h.points().len(), 8);
        for w in h.points().windows(2) {
            let a = (w[1][1] - w[0][1])
                .atan2(w[1][0] - w[0][0])
                .to_degrees()
                .rem_euclid(45.0);
            assert!(a < 1e-6 || 45.0 - a < 1e-6);
        }
        // Points nearer than the growth are inside.
        for k in 0..64 {
            let t = f64::from(k) / 64.0 * std::f64::consts::TAU;
            assert!(h.strictly_contains([4.0 + 0.99 * t.cos(), 0.99 * t.sin()], 0.0));
        }
        assert!(h.inside_interval([-3.0, 0.5], [7.0, 0.5], 0.0).is_some());
        assert!(h.inside_interval([-3.0, 1.5], [7.0, 1.5], 0.0).is_none());
        let r = Hull::of_segment([0.0, 0.0], [4.0, 0.0], 1.0, Facets::Four);
        assert_eq!(r.map(|h| h.points().len()), Some(4));
    }

    /// The arc between two boundary points runs over the corners between
    /// them in the chosen sense.
    #[test]
    fn arcs_walk_the_boundary_both_ways() {
        let Some(h) = Hull::of_segment([0.0, 0.0], [0.0, 0.0], 1.0, Facets::Four) else {
            panic!("no hull");
        };
        // A unit square around the origin; from the bottom edge to the top.
        let ccw = h.arc([0.0, -1.0], [0.0, 1.0], true);
        assert_eq!(ccw, vec![[0.0, -1.0], [1.0, -1.0], [1.0, 1.0], [0.0, 1.0]]);
        let cw = h.arc([0.0, -1.0], [0.0, 1.0], false);
        assert_eq!(cw, vec![[0.0, -1.0], [-1.0, -1.0], [-1.0, 1.0], [0.0, 1.0]]);
        let straight = h.arc([-0.5, -1.0], [0.5, -1.0], true);
        assert_eq!(straight, vec![[-0.5, -1.0], [0.5, -1.0]]);
    }
}
