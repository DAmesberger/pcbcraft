//! The geometry-fit check's outlines (slice G1): a flat body's top and
//! bottom faces as the exact model holds them, read from the B-rep's face
//! loops (each edge's own curve, sampled to [`CHORD_MM`]), and a plan
//! body's loops flattened to the same tolerance; and how far two outlines
//! lie apart: the exact area where they differ (their symmetric
//! difference on a nanometre grid, `i_overlay`'s integer overlay), the
//! farthest a point of either lies from the other, and the holes in each.
//!
//! A body built as planned differs from its plan only by how finely each
//! side follows its curves: every point sampled lies on the exact curve,
//! so a point of either outline lies within [`CHORD_MM`] of the other, and
//! the area between them is at most their length times twice that.
use crate::pad_outline::Contour;
use crate::{Error, Result};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::Overlay;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::i_float::int::point::IntPoint;
use solidcore_record::brep::{Curve, Loop, Surface};
use std::collections::HashMap;

/// How far a sampled outline may stand off its exact curves, mm: a tenth
/// of a micrometre, ten times finer than [`super::plan::PARITY_MM`].
pub const CHORD_MM: f64 = 1e-4;

/// A plan contour too long to flatten at [`CHORD_MM`] (more than
/// [`crate::pad_outline::MAX_FLAT_POINTS`] points) is flattened at this.
const COARSE_CHORD_MM: f64 = 1e-3;

/// The farthest the check looks for the other outline from a point, mm:
/// a point farther than this from it is reported this far off.
pub const SEARCH_MM: f64 = 1.0;

/// Points one body's outline may hold, both faces together.
const MAX_POINTS: usize = 4_000_000;

/// Halvings of one edge piece while sampling it.
const MAX_DEPTH: u32 = 20;

/// Grid units per millimetre: a nanometre.
const UNITS: f64 = 1e6;

/// Largest coordinate on the grid (a metre).
const MAX_UNITS: f64 = 1e9;

type Ring = Vec<IntPoint<i64>>;

/// A face's or a plan body's outline: closed rings on the nanometre grid
/// and the tolerance its curves were sampled at. A face's loops nest
/// even-odd (they never cross); a plan body's nest by containment
/// ([`nested`]): its holes may cross (a via ring through a track's cut in
/// a foil level), and the material is outside all of them.
#[derive(Clone, Debug, Default)]
pub struct Rings {
    rings: Vec<Ring>,
    chord: f64,
    by_containment: bool,
}

impl Rings {
    /// `other`'s rings with these (a slab's pieces, disjoint, as one).
    pub fn extend(&mut self, other: Rings) {
        self.rings.extend(other.rings);
        self.chord = self.chord.max(other.chord);
        self.by_containment |= other.by_containment;
    }

    /// The outline as compared: its region normalised, its holes counted,
    /// its edges indexed for distances.
    pub fn prepare(&self) -> Prepared {
        let shapes = if self.rings.is_empty() {
            Vec::new()
        } else if self.by_containment {
            nested(&self.rings)
        } else {
            Overlay::<i64>::from_subj(&self.rings).overlay(OverlayRule::Subject, FillRule::EvenOdd)
        };
        let holes = shapes.iter().map(|s| s.len().saturating_sub(1)).sum();
        let mut perimeter = 0.0;
        let mut segments = Vec::new();
        for ring in shapes.iter().flatten() {
            let points: Vec<[f64; 2]> = ring.iter().map(|p| from_grid(*p)).collect();
            for (i, a) in points.iter().enumerate() {
                let b = points.get(i + 1).or(points.first()).copied().unwrap_or(*a);
                perimeter += (b[0] - a[0]).hypot(b[1] - a[1]);
                segments.push((*a, b));
            }
        }
        Prepared {
            area: region_area(&shapes),
            near: Near::new(segments),
            shapes,
            holes,
            perimeter,
            chord: self.chord,
        }
    }
}

/// An outline ready to compare ([`Rings::prepare`]).
pub struct Prepared {
    shapes: Vec<Vec<Ring>>,
    /// Holes in the region (inner rings), after normalising.
    pub holes: usize,
    /// mm².
    pub area: f64,
    /// Length of every ring, mm.
    pub perimeter: f64,
    chord: f64,
    near: Near,
}

/// How two outlines differ.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Apart {
    /// The area inside exactly one of them, mm².
    pub area: f64,
    /// The farthest a ring point of either lies from the other's rings,
    /// mm (at most [`SEARCH_MM`]).
    pub deviation: f64,
    /// The most `area` may be for two outlines of the same exact curves:
    /// their length times the tolerances they were sampled at.
    pub allowed_area: f64,
}

impl Prepared {
    /// How `self` and `other` differ.
    pub fn apart(&self, other: &Prepared) -> Apart {
        let area = if self.shapes.is_empty() && other.shapes.is_empty() {
            0.0
        } else {
            region_area(
                &Overlay::<i64>::from_subj_and_clip(&self.shapes, &other.shapes)
                    .overlay(OverlayRule::Xor, FillRule::EvenOdd),
            )
        };
        let deviation = self
            .near
            .farthest(&other.near)
            .max(other.near.farthest(&self.near));
        Apart {
            area,
            deviation,
            // Each chord of either stands within its tolerance of the curve:
            // the area between them is at most the length times both
            // tolerances; a nanometre each side for the grid.
            allowed_area: (self.perimeter + other.perimeter)
                * (self.chord + other.chord + 2.0 / UNITS),
        }
    }
}

/// The region `rings` bound when each ring is material or a cut by how
/// many others hold it: the outer boundary, less the union of the rings
/// inside it (its holes, which may cross), with the union of the rings
/// inside those (islands) added back, and so on. For rings that do not
/// cross it is their even-odd region.
fn nested(rings: &[Ring]) -> Vec<Vec<Ring>> {
    let boxes: Vec<_> = rings.iter().map(ring_box).collect();
    let mut levels: Vec<Vec<Ring>> = Vec::new();
    for (i, ring) in rings.iter().enumerate() {
        let depth = rings
            .iter()
            .enumerate()
            .filter(|(j, other)| {
                let (Some(inner), Some(outer)) = (boxes.get(i), boxes.get(*j)) else {
                    return false;
                };
                // A ring equal to another is held by the earlier one only.
                *j != i
                    && (other.as_slice() != ring.as_slice() || *j < i)
                    && outer.0.x <= inner.0.x
                    && outer.0.y <= inner.0.y
                    && outer.1.x >= inner.1.x
                    && outer.1.y >= inner.1.y
                    && ring.iter().all(|p| !outside(*p, other))
            })
            .count();
        if levels.len() <= depth {
            levels.resize(depth + 1, Vec::new());
        }
        if let Some(level) = levels.get_mut(depth) {
            // Counter-clockwise, so the level's rings unite.
            let mut ring = ring.clone();
            if ring_area(&ring) < 0.0 {
                ring.reverse();
            }
            level.push(ring);
        }
    }
    let mut region: Vec<Vec<Ring>> = Vec::new();
    for (depth, level) in levels.iter().enumerate() {
        if level.is_empty() {
            continue;
        }
        let united =
            Overlay::<i64>::from_subj(level).overlay(OverlayRule::Subject, FillRule::NonZero);
        let rule = if depth % 2 == 0 {
            OverlayRule::Union
        } else {
            OverlayRule::Difference
        };
        region =
            Overlay::<i64>::from_subj_and_clip(&region, &united).overlay(rule, FillRule::EvenOdd);
    }
    region
}

/// A ring's lowest and highest corner.
fn ring_box(ring: &Ring) -> (IntPoint<i64>, IntPoint<i64>) {
    let mut lo = IntPoint::new(i64::MAX, i64::MAX);
    let mut hi = IntPoint::new(i64::MIN, i64::MIN);
    for p in ring {
        lo = IntPoint::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = IntPoint::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    (lo, hi)
}

/// Whether `p` lies strictly outside `ring` (a point on it is not).
fn outside(p: IntPoint<i64>, ring: &Ring) -> bool {
    let mut inside = false;
    for (i, a) in ring.iter().enumerate() {
        let b = ring.get(i + 1).or(ring.first()).copied().unwrap_or(*a);
        let (ax, ay, bx, by) = (
            i128::from(a.x),
            i128::from(a.y),
            i128::from(b.x),
            i128::from(b.y),
        );
        let (px, py) = (i128::from(p.x), i128::from(p.y));
        let cross = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
        if cross == 0
            && px >= ax.min(bx)
            && px <= ax.max(bx)
            && py >= ay.min(by)
            && py <= ay.max(by)
        {
            return false;
        }
        if (ay > py) != (by > py) {
            // The edge crosses the point's level: right of the point?
            let right = if by > ay { cross > 0 } else { cross < 0 };
            if right {
                inside = !inside;
            }
        }
    }
    !inside
}

/// The region's area: each shape's outer ring less its holes.
fn region_area(shapes: &[Vec<Ring>]) -> f64 {
    shapes
        .iter()
        .map(|shape| {
            let mut rings = shape.iter();
            let outer = rings.next().map_or(0.0, |r| ring_area(r).abs());
            outer - rings.map(|r| ring_area(r).abs()).sum::<f64>()
        })
        .sum()
}

fn ring_area(ring: &Ring) -> f64 {
    let mut twice = 0.0;
    for (i, a) in ring.iter().enumerate() {
        let b = ring.get(i + 1).or(ring.first()).copied().unwrap_or(*a);
        let (a, b) = (from_grid(*a), from_grid(b));
        twice += a[0] * b[1] - b[0] * a[1];
    }
    twice / 2.0
}

fn to_grid(p: [f64; 2]) -> Result<IntPoint<i64>> {
    let at = |v: f64| {
        let u = (v * UNITS).round();
        if u.is_finite() && u.abs() <= MAX_UNITS {
            Ok(u as i64)
        } else {
            Err(Error::Verification(format!(
                "an outline point lies beyond a metre of the origin ({v} mm)"
            )))
        }
    };
    Ok(IntPoint::new(at(p[0])?, at(p[1])?))
}

fn from_grid(p: IntPoint<i64>) -> [f64; 2] {
    [p.x as f64 / UNITS, p.y as f64 / UNITS]
}

/// `points` as one closed ring on the grid; `None` for fewer than three
/// distinct points.
fn ring(points: &[[f64; 2]]) -> Result<Option<Ring>> {
    let mut out: Ring = Vec::with_capacity(points.len());
    for p in points {
        let q = to_grid(*p)?;
        if out.last() != Some(&q) {
            out.push(q);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    Ok((out.len() >= 3).then_some(out))
}

/// A plan body's loops on the board as an outline: each flattened at
/// [`CHORD_MM`] (a contour too long for that at [`COARSE_CHORD_MM`]).
pub fn plan_rings(loops: &[Contour]) -> Result<Rings> {
    let mut rings = Vec::with_capacity(loops.len());
    let mut chord = CHORD_MM;
    for contour in loops {
        let points = match contour.flatten(CHORD_MM) {
            Ok(points) => points,
            Err(_) => {
                chord = COARSE_CHORD_MM;
                contour.flatten(COARSE_CHORD_MM)?
            }
        };
        if let Some(r) = ring(&points)? {
            rings.push(r);
        }
    }
    Ok(Rings {
        rings,
        chord,
        by_containment: true,
    })
}

/// The top (`high`) or bottom face of `body`, which lies at height `z`:
/// the loops of every flat face there facing up or down, each edge read
/// from its own curve. Empty when no face lies flat there.
pub fn face_rings(body: &solidcraft_engine::kernel::Body, z: f64) -> Result<Rings> {
    let record = body.record();
    let mut rings = Vec::new();
    let mut budget = MAX_POINTS;
    for face in &record.faces {
        let (origin, u, v) = match &face.surface {
            Surface::Plane {
                origin,
                u_axis,
                v_axis,
            } => (*origin, *u_axis, *v_axis),
            Surface::PlaneThrough {
                origin,
                u_point,
                v_point,
            } => (
                *origin,
                std::array::from_fn(|i| u_point[i] - origin[i]),
                std::array::from_fn(|i| v_point[i] - origin[i]),
            ),
            _ => continue,
        };
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if length.is_nan() || length <= 0.0 || (n[2] / length).abs() < 1.0 - 1e-9 {
            continue;
        }
        if (origin[2] - z).abs() > super::plan::PARITY_MM {
            continue;
        }
        for face_loop in &face.loops {
            let Loop::Edges { edges } = face_loop else {
                continue;
            };
            let mut points: Vec<[f64; 2]> = Vec::new();
            for used in edges {
                let edge = record
                    .edges
                    .get(usize::try_from(used.edge).unwrap_or(usize::MAX))
                    .ok_or_else(|| {
                        Error::Verification(format!("a face loop names no edge {}", used.edge))
                    })?;
                let mut piece = sample(edge.parameters(), edge.interval, &mut budget)?;
                if !used.forward {
                    piece.reverse();
                }
                points.extend(piece);
            }
            if let Some(r) = ring(&points)? {
                rings.push(r);
            }
        }
    }
    Ok(Rings {
        rings,
        chord: CHORD_MM,
        by_containment: false,
    })
}

/// Points along `curve` over `interval` (its plan view), each on the
/// curve, no chord standing off it by more than [`CHORD_MM`].
fn sample(curve: &Curve, interval: [f64; 2], budget: &mut usize) -> Result<Vec<[f64; 2]>> {
    use solidcraft_engine::geom::brep::eval::CurveEval;
    let eval = CurveEval::new(curve)
        .map_err(|e| Error::Verification(format!("an edge's curve does not evaluate: {e}")))?;
    let at = |t: f64| -> Result<[f64; 2]> {
        eval.point(t)
            .filter(|p| p.x.is_finite() && p.y.is_finite())
            .map(|p| [p.x, p.y])
            .ok_or_else(|| Error::Verification(format!("an edge has no point at {t}")))
    };
    let [t0, t1] = interval;
    if !(t0.is_finite() && t1.is_finite()) {
        return Err(Error::Verification(
            "an edge's interval is not finite".into(),
        ));
    }
    // A line is its ends; any other curve starts in eighths, so a curve
    // that turns back on itself is not taken for its chord.
    let pieces: u32 = if matches!(curve, Curve::Line { .. }) {
        1
    } else {
        8
    };
    let mut out = vec![at(t0)?];
    for k in 0..pieces {
        let a = t0 + (t1 - t0) * f64::from(k) / f64::from(pieces);
        let b = t0 + (t1 - t0) * f64::from(k + 1) / f64::from(pieces);
        refine(&at, (a, at(a)?), (b, at(b)?), 0, &mut out, budget)?;
    }
    Ok(out)
}

/// The points after `from` up to `to` along the curve: the piece halved
/// while its middle stands off its chord by more than [`CHORD_MM`].
fn refine(
    at: &dyn Fn(f64) -> Result<[f64; 2]>,
    from: (f64, [f64; 2]),
    to: (f64, [f64; 2]),
    depth: u32,
    out: &mut Vec<[f64; 2]>,
    budget: &mut usize,
) -> Result<()> {
    let middle = (from.0 + to.0) / 2.0;
    let m = at(middle)?;
    if depth < MAX_DEPTH && off_chord(m, from.1, to.1) > CHORD_MM {
        refine(at, from, (middle, m), depth + 1, out, budget)?;
        refine(at, (middle, m), to, depth + 1, out, budget)?;
        return Ok(());
    }
    *budget = budget.checked_sub(1).ok_or_else(|| {
        Error::Verification(format!(
            "a body's faces need more than {MAX_POINTS} outline points"
        ))
    })?;
    out.push(to.1);
    Ok(())
}

/// How far `p` lies from the segment `a`–`b`.
fn off_chord(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let length = d[0] * d[0] + d[1] * d[1];
    let t = if length > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
}

/// An outline's edges on a grid of square cells, for the nearest edge to
/// a point.
struct Near {
    segments: Vec<([f64; 2], [f64; 2])>,
    cells: HashMap<(i64, i64), Vec<u32>>,
    cell: f64,
    /// The lowest and highest cell holding an edge; `None` for no edge.
    span: Option<((i64, i64), (i64, i64))>,
}

/// Edge pieces one outline's index may hold; past this its cells widen.
const MAX_PIECES: f64 = 8e6;

impl Near {
    /// The index of `segments`, each cut into pieces no longer than a
    /// cell (the pieces' starts are the points [`Near::farthest`] reads, so
    /// a long edge is followed along its length, not only at its ends).
    fn new(segments: Vec<([f64; 2], [f64; 2])>) -> Self {
        let length = |(a, b): &([f64; 2], [f64; 2])| (b[0] - a[0]).hypot(b[1] - a[1]);
        let total: f64 = segments.iter().map(length).sum();
        let count = segments.len().max(1) as f64;
        // About an edge per cell.
        let mut cell = (total / count * 2.0).clamp(1e-3, SEARCH_MM);
        while total / cell + count > MAX_PIECES && cell < SEARCH_MM {
            cell *= 2.0;
        }
        let mut pieces = Vec::with_capacity(segments.len());
        for segment in &segments {
            let (a, b) = *segment;
            let n = (length(segment) / cell).ceil().clamp(1.0, MAX_PIECES) as usize;
            let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            for k in 0..n {
                pieces.push((at(k as f64 / n as f64), at((k + 1) as f64 / n as f64)));
            }
        }
        let segments = pieces;
        let mut cells: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
        let mut span: Option<((i64, i64), (i64, i64))> = None;
        for (index, (a, b)) in segments.iter().enumerate() {
            let Ok(index) = u32::try_from(index) else {
                break;
            };
            let (x0, x1) = (key(a[0].min(b[0]), cell), key(a[0].max(b[0]), cell));
            let (y0, y1) = (key(a[1].min(b[1]), cell), key(a[1].max(b[1]), cell));
            span = Some(match span {
                None => ((x0, y0), (x1, y1)),
                Some((lo, hi)) => ((lo.0.min(x0), lo.1.min(y0)), (hi.0.max(x1), hi.1.max(y1))),
            });
            for x in x0..=x1 {
                for y in y0..=y1 {
                    cells.entry((x, y)).or_default().push(index);
                }
            }
        }
        Self {
            segments,
            cells,
            cell,
            span,
        }
    }

    /// The distance from `p` to the nearest edge, at most [`SEARCH_MM`].
    fn distance(&self, p: [f64; 2]) -> f64 {
        let Some((lo, hi)) = self.span else {
            return SEARCH_MM;
        };
        let (cx, cy) = (key(p[0], self.cell), key(p[1], self.cell));
        // No ring past the search, or past the last cell holding an edge.
        let reach = (cx - lo.0)
            .abs()
            .max((hi.0 - cx).abs())
            .max((cy - lo.1).abs())
            .max((hi.1 - cy).abs());
        let rings = ((SEARCH_MM / self.cell).ceil() as i64 + 1).min(reach);
        let mut best = SEARCH_MM;
        let look = |x: i64, y: i64, best: &mut f64| {
            for &i in self.cells.get(&(x, y)).into_iter().flatten() {
                if let Some((a, b)) = self.segments.get(i as usize) {
                    *best = best.min(off_chord(p, *a, *b));
                }
            }
        };
        for r in 0..=rings {
            if r == 0 {
                look(cx, cy, &mut best);
            } else {
                for x in (cx - r)..=(cx + r) {
                    look(x, cy - r, &mut best);
                    look(x, cy + r, &mut best);
                }
                for y in (cy - r + 1)..(cy + r) {
                    look(cx - r, y, &mut best);
                    look(cx + r, y, &mut best);
                }
            }
            // Every edge within `r` cells is seen: none nearer is left.
            if best <= r as f64 * self.cell {
                break;
            }
        }
        best
    }

    /// The farthest a point of `other`'s edges lies from these.
    fn farthest(&self, other: &Near) -> f64 {
        other
            .segments
            .iter()
            .map(|(a, _)| self.distance(*a))
            .fold(0.0, f64::max)
    }
}

fn key(v: f64, cell: f64) -> i64 {
    (v / cell).floor() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pad_outline::Contour;

    fn square(x: f64, y: f64, side: f64) -> Contour {
        crate::project::polygon(&[[x, y], [x + side, y], [x + side, y + side], [x, y + side]])
            .unwrap_or(Contour::Circle {
                center: [x, y],
                radius: side,
            })
    }

    /// The same loops agree exactly; a dropped hole shows in the holes,
    /// the area and the deviation; an edge moved by 10 µm in the
    /// deviation, and in the area by its length times the move.
    #[test]
    fn outlines_differ_by_area_deviation_and_holes() -> Result<()> {
        let hole = Contour::Circle {
            center: [5.0, 5.0],
            radius: 0.5,
        };
        let planned = plan_rings(&[square(0.0, 0.0, 10.0), hole.clone()])?.prepare();
        assert_eq!(planned.holes, 1);
        let same = plan_rings(&[square(0.0, 0.0, 10.0), hole])?.prepare();
        let apart = planned.apart(&same);
        assert_eq!(apart.area, 0.0);
        assert_eq!(apart.deviation, 0.0);

        let dropped = plan_rings(&[square(0.0, 0.0, 10.0)])?.prepare();
        assert_eq!(dropped.holes, 0);
        let apart = planned.apart(&dropped);
        assert!(
            (apart.area - std::f64::consts::PI * 0.25).abs() < 1e-3,
            "{apart:?}"
        );
        assert!(apart.area > apart.allowed_area);
        // The hole's edge lies 4.5 mm from the square's: past the search.
        assert_eq!(apart.deviation, SEARCH_MM, "{apart:?}");

        let moved = plan_rings(&[square(0.0, 0.0, 10.0)])?.prepare();
        let wider = plan_rings(&[crate::project::polygon(&[
            [0.0, 0.0],
            [10.01, 0.0],
            [10.01, 10.0],
            [0.0, 10.0],
        ])
        .unwrap_or(Contour::Circle {
            center: [0.0, 0.0],
            radius: 1.0,
        })])?
        .prepare();
        let apart = moved.apart(&wider);
        assert!((apart.deviation - 0.01).abs() < 1e-6, "{apart:?}");
        assert!((apart.area - 0.1).abs() < 1e-6, "{apart:?}");
        Ok(())
    }

    /// A point's nearest edge is found across cells, and a point farther
    /// than the search reads as [`SEARCH_MM`].
    #[test]
    fn the_nearest_edge_is_found_across_cells() {
        let near = Near::new(vec![([0.0, 0.0], [0.001, 0.0]), ([5.0, 0.0], [5.001, 0.0])]);
        assert!((near.distance([0.0005, 0.3]) - 0.3).abs() < 1e-12);
        assert_eq!(near.distance([2.5, 3.0]), SEARCH_MM);
    }
}
