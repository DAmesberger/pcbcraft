//! The outline of a union of closed loops: the boundary pieces with the
//! union's inside on their left, chained into counter-clockwise outer loops
//! and clockwise holes. Native cuts sketch it, so overlapping copper (a
//! track running into a via's ring) is removed from the laminate once.
use sparksmith_core::Point;
use std::collections::BTreeMap;

const EPS: f64 = 1e-9;
/// Most edges one union takes (a bound, not a target).
pub const MAX_UNION_EDGES: usize = 1 << 18;
/// Grid cells an edge is registered in, at most.
const MAX_CELLS_PER_EDGE: usize = 4096;

#[derive(Clone, Copy)]
struct Edge {
    a: Point,
    b: Point,
}

impl Edge {
    fn lo(&self) -> Point {
        [self.a[0].min(self.b[0]), self.a[1].min(self.b[1])]
    }
    fn hi(&self) -> Point {
        [self.a[0].max(self.b[0]), self.a[1].max(self.b[1])]
    }
}

/// A uniform grid of edges by their boxes.
struct Grid {
    lo: Point,
    step: f64,
    n: [usize; 2],
    cells: Vec<Vec<usize>>,
}

impl Grid {
    fn new(edges: &[Edge]) -> Self {
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for e in edges {
            for k in 0..2 {
                lo[k] = lo[k].min(e.lo()[k]);
                hi[k] = hi[k].max(e.hi()[k]);
            }
        }
        let size = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(EPS);
        let per_axis = (edges.len() as f64).sqrt().ceil().clamp(1.0, 512.0);
        let step = size / per_axis;
        let n = [
            (((hi[0] - lo[0]) / step).ceil() as usize).clamp(1, 1024),
            (((hi[1] - lo[1]) / step).ceil() as usize).clamp(1, 1024),
        ];
        let mut grid = Self {
            lo,
            step,
            n,
            cells: vec![Vec::new(); n[0] * n[1]],
        };
        for (i, e) in edges.iter().enumerate() {
            let (a, b) = (grid.cell(e.lo()), grid.cell(e.hi()));
            let mut count = 0;
            'cells: for y in a[1]..=b[1] {
                for x in a[0]..=b[0] {
                    grid.cells[y * n[0] + x].push(i);
                    count += 1;
                    if count >= MAX_CELLS_PER_EDGE {
                        break 'cells;
                    }
                }
            }
        }
        grid
    }

    fn cell(&self, p: Point) -> [usize; 2] {
        std::array::from_fn(|k| {
            (((p[k] - self.lo[k]) / self.step).floor().max(0.0) as usize).min(self.n[k] - 1)
        })
    }

    /// Edges whose boxes may hold `p`'s row from `p` rightwards.
    fn row(&self, p: Point) -> impl Iterator<Item = usize> + '_ {
        let [x, y] = self.cell(p);
        (x..self.n[0]).flat_map(move |cx| self.cells[y * self.n[0] + cx].iter().copied())
    }
}

/// Where segments `e` and `f` cross, as parameters along `e`.
fn crossings(e: &Edge, f: &Edge, out: &mut Vec<f64>) {
    let r = [e.b[0] - e.a[0], e.b[1] - e.a[1]];
    let s = [f.b[0] - f.a[0], f.b[1] - f.a[1]];
    let d = r[0] * s[1] - r[1] * s[0];
    let qp = [f.a[0] - e.a[0], f.a[1] - e.a[1]];
    let rr = r[0] * r[0] + r[1] * r[1];
    if rr < EPS * EPS {
        return;
    }
    if d.abs() < EPS * EPS {
        // Collinear overlap: split at the other segment's ends.
        if (qp[0] * r[1] - qp[1] * r[0]).abs() > EPS * rr.sqrt() {
            return;
        }
        for p in [f.a, f.b] {
            let t = ((p[0] - e.a[0]) * r[0] + (p[1] - e.a[1]) * r[1]) / rr;
            if t > EPS && t < 1.0 - EPS {
                out.push(t);
            }
        }
        return;
    }
    let t = (qp[0] * s[1] - qp[1] * s[0]) / d;
    let u = (qp[0] * r[1] - qp[1] * r[0]) / d;
    if t > EPS && t < 1.0 - EPS && (-EPS..=1.0 + EPS).contains(&u) {
        out.push(t);
    }
}

/// Nonzero winding of the loops' edges around `p`.
fn winding(edges: &[Edge], grid: &Grid, p: Point, seen: &mut [bool]) -> i32 {
    let mut w = 0;
    let mut touched = Vec::new();
    for i in grid.row(p) {
        if seen[i] {
            continue;
        }
        seen[i] = true;
        touched.push(i);
        let e = &edges[i];
        let (lo, hi, dir) = if e.a[1] < e.b[1] {
            (e.a, e.b, 1)
        } else {
            (e.b, e.a, -1)
        };
        if lo[1] <= p[1] && p[1] < hi[1] {
            let x = lo[0] + (p[1] - lo[1]) / (hi[1] - lo[1]) * (hi[0] - lo[0]);
            if x > p[0] {
                w += dir;
            }
        }
    }
    for i in touched {
        seen[i] = false;
    }
    w
}

fn key(p: Point) -> (i64, i64) {
    let q = |v: f64| (v * 1e7).round() as i64;
    (q(p[0]), q(p[1]))
}

/// The outline of the union of `loops` (either orientation each).
/// Returns `None` when the loops have more than [`MAX_UNION_EDGES`] edges.
pub fn union(loops: &[Vec<Point>]) -> Option<Vec<Vec<Point>>> {
    let mut edges = Vec::new();
    for points in loops {
        let mut p: Vec<Point> = points.clone();
        p.dedup_by(|a, b| (a[0] - b[0]).hypot(a[1] - b[1]) < EPS);
        while p.len() > 1
            && p.first()
                .zip(p.last())
                .is_some_and(|(a, b)| key(*a) == key(*b))
        {
            p.pop();
        }
        if p.len() < 3 {
            continue;
        }
        let n = p.len();
        let area: f64 = (0..n)
            .map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1])
            .sum();
        if area.abs() < EPS {
            continue;
        }
        if area < 0.0 {
            p.reverse();
        }
        for i in 0..n {
            edges.push(Edge {
                a: p[i],
                b: p[(i + 1) % n],
            });
        }
        if edges.len() > MAX_UNION_EDGES {
            return None;
        }
    }
    if edges.is_empty() {
        return Some(Vec::new());
    }
    let grid = Grid::new(&edges);
    let mut seen = vec![false; edges.len()];
    let mut pieces: BTreeMap<(i64, i64), Vec<(Point, Point)>> = BTreeMap::new();
    let mut kept: std::collections::BTreeSet<((i64, i64), (i64, i64))> = Default::default();
    let mut ts = Vec::new();
    for (i, e) in edges.iter().enumerate() {
        ts.clear();
        ts.extend([0.0, 1.0]);
        let (a, b) = (grid.cell(e.lo()), grid.cell(e.hi()));
        let mut others: Vec<usize> = Vec::new();
        for y in a[1]..=b[1] {
            for x in a[0]..=b[0] {
                others.extend(grid.cells[y * grid.n[0] + x].iter().copied());
            }
        }
        others.sort_unstable();
        others.dedup();
        for j in others {
            if j == i {
                continue;
            }
            let f = &edges[j];
            let (flo, fhi, elo, ehi) = (f.lo(), f.hi(), e.lo(), e.hi());
            if fhi[0] < elo[0] - EPS
                || flo[0] > ehi[0] + EPS
                || fhi[1] < elo[1] - EPS
                || flo[1] > ehi[1] + EPS
            {
                continue;
            }
            crossings(e, f, &mut ts);
        }
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|x, y| (*x - *y).abs() < EPS);
        let d = [e.b[0] - e.a[0], e.b[1] - e.a[1]];
        let len = d[0].hypot(d[1]);
        let left = [-d[1] / len, d[0] / len];
        for w in ts.windows(2) {
            let p = [e.a[0] + d[0] * w[0], e.a[1] + d[1] * w[0]];
            let q = [e.a[0] + d[0] * w[1], e.a[1] + d[1] * w[1]];
            if (q[0] - p[0]).hypot(q[1] - p[1]) < EPS {
                continue;
            }
            let m = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0];
            let probe = 1e-6;
            let inside = |s: f64, seen: &mut [bool]| {
                winding(
                    &edges,
                    &grid,
                    [m[0] + left[0] * s, m[1] + left[1] * s],
                    seen,
                ) != 0
            };
            if inside(probe, &mut seen)
                && !inside(-probe, &mut seen)
                && kept.insert((key(p), key(q)))
            {
                pieces.entry(key(p)).or_default().push((p, q));
            }
        }
    }
    // Chain the pieces end to start.
    let mut out = Vec::new();
    while let Some(start) = pieces.keys().next().copied() {
        let mut loop_points = Vec::new();
        let mut at = start;
        while let Some(list) = pieces.get_mut(&at) {
            let Some((p, q)) = list.pop() else {
                pieces.remove(&at);
                break;
            };
            if list.is_empty() {
                pieces.remove(&at);
            }
            loop_points.push(p);
            at = key(q);
            if at == start {
                break;
            }
        }
        if loop_points.len() >= 3 {
            out.push(loop_points);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(p: &[Point]) -> f64 {
        let n = p.len();
        (0..n)
            .map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1])
            .sum::<f64>()
            / 2.0
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    #[test]
    fn overlapping_loops_merge_into_one_outline_with_their_union_area() {
        let out = union(&[rect(0.0, 0.0, 2.0, 1.0), rect(1.0, -1.0, 3.0, 0.5)]).unwrap_or_default();
        assert_eq!(out.len(), 1);
        let total: f64 = out.iter().map(|l| area(l)).sum();
        assert!((total - (2.0 + 3.0 - 0.5)).abs() < 1e-9, "{total}");
        // Disjoint loops stay apart; a clockwise loop counts the same.
        let mut cw = rect(5.0, 5.0, 6.0, 6.0);
        cw.reverse();
        let out = union(&[rect(0.0, 0.0, 1.0, 1.0), cw]).unwrap_or_default();
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|l| (area(l) - 1.0).abs() < 1e-9));
    }

    #[test]
    fn a_ring_of_loops_leaves_a_hole_and_shared_edges_vanish() {
        // Four bars around a square hole.
        let bars = [
            rect(0.0, 0.0, 3.0, 1.0),
            rect(2.0, 0.0, 3.0, 3.0),
            rect(0.0, 2.0, 3.0, 3.0),
            rect(0.0, 0.0, 1.0, 3.0),
        ];
        let out = union(&bars).unwrap_or_default();
        assert_eq!(out.len(), 2);
        let total: f64 = out.iter().map(|l| area(l)).sum();
        assert!((total - 8.0).abs() < 1e-9, "{total}");
        assert!(
            out.iter().any(|l| (area(l) + 1.0).abs() < 1e-9),
            "a clockwise hole"
        );
        // Two squares sharing an edge are one rectangle.
        let out = union(&[rect(0.0, 0.0, 1.0, 1.0), rect(1.0, 0.0, 2.0, 1.0)]).unwrap_or_default();
        assert_eq!(out.len(), 1);
        assert!((area(&out[0]) - 2.0).abs() < 1e-9);
    }
}
