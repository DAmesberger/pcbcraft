//! A shoved differential pair moves as one unit, or not at all.
//!
//! Two separately shoved halves pick their sides and bumps on their own,
//! which tears the pair open. So a push on a pair's track takes the
//! coupled stretch's centre line, bends that round the pusher, as one chain
//! as wide as the pair, and lays both halves back along it at their
//! distance from it, with mitred corners. Coupling and gap hold by
//! construction, and the halves' extra lengths cancel at the opposite
//! corners of the bend. A push may add skew only within a share
//! ([`ADDED_SKEW_SHARE`]) of the class's max skew; a class that sets none
//! allows no added skew beyond rounding ([`LENGTH_TOLERANCE_MM`]). Past
//! that, the push is refused.
//!
//! A push that cannot keep the pair whole is refused: a pusher reaching a
//! stretch where the halves do not run coupled (a fan-out, a meander), a
//! partner being routed, a bend tighter than the pair's pitch. Pair vias
//! never move. The whole shove then fails, the router falls back to
//! walking round, and [`Ctx::held`] records the pair for the plan's message.
use super::{
    Hit, Line, MAX_MERGES, Piece, Pusher, State, adopt_line, has_acute, inside_span, length,
    posture_of, simplify, tidy, touches,
};
use crate::geometry::{Point, distance, segment_distance, segments_cross};
use crate::ids::Id;
use crate::layout::impedance;
use crate::layout::length::gap_tolerance;
use crate::layout::meander::LENGTH_TOLERANCE_MM;
use crate::layout::pair::{left, mitre};
use crate::layout::posture::Posture;
use crate::layout::router::hull::{Facets, Hull};
use crate::layout::router::world::{Ctx, MARGIN};
use crate::rules::CopperObject;
use crate::rules::signal::DiffPair;

/// Segments within this angle are parallel, in sine (as the coupling
/// check judges them).
const PARALLEL_SIN: f64 = 0.017;
/// How much two coupled segments' centre distances may differ along one
/// window, mm.
const SAME_PITCH_MM: f64 = 2e-6;
/// The share of the class's max skew one push may add (a bend off the
/// 45° grid does not cancel exactly).
const ADDED_SKEW_SHARE: f64 = 0.5;

fn unit(a: Point, b: Point) -> Option<Point> {
    let l = distance(a, b);
    (l > 1e-12).then(|| [(b[0] - a[0]) / l, (b[1] - a[1]) / l])
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn along(p: Point, d: Point, k: f64) -> Point {
    [p[0] + d[0] * k, p[1] + d[1] * k]
}

fn mid(a: Point, b: Point) -> Point {
    [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])]
}

/// What two halves' segments must be to run coupled.
#[derive(Clone, Copy)]
struct Coupled {
    /// Half the two halves' widths added.
    width: f64,
    gap: f64,
    tolerance: f64,
}

impl Coupled {
    fn of(width_a: f64, width_b: f64, gap: f64) -> Self {
        Self {
            width: 0.5 * (width_a + width_b),
            gap,
            tolerance: gap_tolerance(gap),
        }
    }

    /// The centre distance of segments running side by side at the gap.
    fn beside(&self, a: [Point; 2], b: [Point; 2]) -> Option<f64> {
        let (da, db) = (unit(a[0], a[1])?, unit(b[0], b[1])?);
        if cross(da, db).abs() > PARALLEL_SIN {
            return None;
        }
        let project = |p: Point| dot([p[0] - a[0][0], p[1] - a[0][1]], da);
        let (p, q) = (project(b[0]), project(b[1]));
        let length = distance(a[0], a[1]);
        if p.max(q).min(length) - p.min(q).max(0.0) <= 1e-9 {
            return None;
        }
        let centre = cross(da, [b[0][0] - a[0][0], b[0][1] - a[0][1]]).abs();
        ((centre - self.width - self.gap).abs() <= self.tolerance).then_some(centre)
    }
}

fn segment(points: &[Point], i: usize) -> Option<[Point; 2]> {
    Some([*points.get(i)?, *points.get(i + 1)?])
}

/// The coupled stretch of two equally directed halves round segment `seg`
/// of `a`: the segments it covers and its centre line.
struct Window {
    a: (usize, usize),
    b: (usize, usize),
    centre: Vec<Point>,
    /// Half the halves' centre distance.
    half: f64,
    /// Which side of the centre line `a` runs on: +1 left, −1 right.
    side: f64,
}

impl Window {
    fn of(a: &[Point], b: &[Point], seg: usize, coupled: Coupled) -> Option<Self> {
        let here = segment(a, seg)?;
        let (t, pitch) = (0..b.len().saturating_sub(1)).find_map(|t| {
            let there = segment(b, t)?;
            let (da, db) = (unit(here[0], here[1])?, unit(there[0], there[1])?);
            (dot(da, db) > 0.0)
                .then(|| coupled.beside(here, there))
                .flatten()
                .map(|p| (t, p))
        })?;
        let matches = |i: usize, j: usize| {
            let (Some(x), Some(y)) = (segment(a, i), segment(b, j)) else {
                return false;
            };
            let same_way = unit(x[0], x[1])
                .zip(unit(y[0], y[1]))
                .is_some_and(|(p, q)| dot(p, q) > 0.0);
            same_way
                && coupled
                    .beside(x, y)
                    .is_some_and(|c| (c - pitch).abs() < SAME_PITCH_MM)
        };
        let (mut a_lo, mut b_lo) = (seg, t);
        while a_lo > 0 && b_lo > 0 && matches(a_lo - 1, b_lo - 1) {
            a_lo -= 1;
            b_lo -= 1;
        }
        let (mut a_hi, mut b_hi) = (seg, t);
        while matches(a_hi + 1, b_hi + 1) {
            a_hi += 1;
            b_hi += 1;
        }
        // The first and last stations both halves reach.
        let first = segment(a, a_lo)?;
        let first_b = segment(b, b_lo)?;
        let d0 = unit(first[0], first[1])?;
        let lo = dot(first[0], d0).max(dot(first_b[0], d0));
        let start = mid(
            along(first[0], d0, lo - dot(first[0], d0)),
            along(first_b[0], d0, lo - dot(first_b[0], d0)),
        );
        let last = segment(a, a_hi)?;
        let last_b = segment(b, b_hi)?;
        let dn = unit(last[0], last[1])?;
        let hi = dot(last[1], dn).min(dot(last_b[1], dn));
        let end = mid(
            along(last[1], dn, hi - dot(last[1], dn)),
            along(last_b[1], dn, hi - dot(last_b[1], dn)),
        );
        let mut centre = vec![start];
        for k in 1..=(a_hi - a_lo) {
            centre.push(mid(*a.get(a_lo + k)?, *b.get(b_lo + k)?));
        }
        centre.push(end);
        let line = simplify(&centre);
        if line.len() < 2 {
            return None;
        }
        let rel = [first[0][0] - start[0], first[0][1] - start[1]];
        let side = if cross(d0, rel) < 0.0 { -1.0 } else { 1.0 };
        Some(Self {
            a: (a_lo, a_hi),
            b: (b_lo, b_hi),
            centre: line,
            half: pitch / 2.0,
            side,
        })
    }
}

/// `line` offset by `h` to its left (right when negative), with mitred
/// corners; `None` when a leg would turn back (a bend tighter than `h`).
fn offset(line: &[Point], h: f64) -> Option<Vec<Point>> {
    let dirs: Vec<Point> = line
        .windows(2)
        .map(|w| unit(w[0], w[1]))
        .collect::<Option<_>>()?;
    let mut out = Vec::with_capacity(line.len());
    for (i, p) in line.iter().enumerate() {
        let shift = match (i.checked_sub(1).and_then(|j| dirs.get(j)), dirs.get(i)) {
            (Some(a), Some(b)) => mitre(*a, *b),
            (None, Some(d)) | (Some(d), None) => left(*d),
            (None, None) => return None,
        };
        out.push(along(*p, shift, h));
    }
    let keeps = out
        .windows(2)
        .zip(&dirs)
        .all(|(w, d)| dot([w[1][0] - w[0][0], w[1][1] - w[0][1]], *d) > 1e-6);
    keeps.then_some(out)
}

/// `points` with its segments `lo..=hi` replaced by a path starting and
/// ending on them.
fn splice(points: &[Point], (lo, hi): (usize, usize), middle: &[Point]) -> Option<Vec<Point>> {
    let mut out = points.get(..=lo)?.to_vec();
    out.extend_from_slice(middle);
    out.extend_from_slice(points.get(hi + 1..)?);
    Some(simplify(&out))
}

/// The partner's chain beside segment `here` of a chain: one the shove
/// holds already, else adopted from the board.
fn partner_line(
    ctx: &Ctx<'_>,
    state: &mut State,
    partner: Id,
    line: &Line,
    here: [Point; 2],
    gap: f64,
) -> Option<usize> {
    let coupled = |width: f64| Coupled::of(line.width, width, gap);
    if let Some(m) = state.lines.iter().position(|l| {
        l.net == partner
            && l.layer == line.layer
            && l.points
                .windows(2)
                .any(|w| coupled(l.width).beside(here, [w[0], w[1]]).is_some())
    }) {
        return Some(m);
    }
    let k = ctx.project.tracks.iter().enumerate().find_map(|(k, t)| {
        (t.net == partner
            && t.layer == line.layer
            && !state.hidden.contains(&k)
            && coupled(t.width).beside(here, [t.start, t.end]).is_some())
        .then_some(k)
    })?;
    adopt_line(ctx, state, k)
}

/// The segment of `points` nearest the pusher.
fn nearest_segment(points: &[Point], pusher: &Piece) -> Option<usize> {
    let gap = |w: &[Point]| match w {
        [a, b] => {
            if segments_cross(*a, *b, pusher.core.a, pusher.core.b) {
                0.0
            } else {
                segment_distance(*a, pusher.core.a, pusher.core.b)
                    .min(segment_distance(*b, pusher.core.a, pusher.core.b))
                    .min(segment_distance(pusher.core.a, *a, *b))
                    .min(segment_distance(pusher.core.b, *a, *b))
            }
        }
        _ => f64::INFINITY,
    };
    (0..points.len().saturating_sub(1))
        .min_by(|i, j| gap(&points[*i..*i + 2]).total_cmp(&gap(&points[*j..*j + 2])))
}

/// One proposal for the pair: both halves' new polylines.
struct Bent {
    a: Vec<Point>,
    b: Vec<Point>,
}

/// Bend the pair round `pusher`: chain `l` and its partner move together.
/// The pushers that moved, or `None` when the pair cannot keep whole here.
pub(super) fn push(
    ctx: &Ctx<'_>,
    state: &mut State,
    l: usize,
    pair: &DiffPair,
    pusher: &Piece,
) -> Option<Vec<Pusher>> {
    let line = state.lines.get(l)?.clone();
    let partner = pair.partner(line.net)?;
    if state.frozen.contains(&partner) || state.frozen.contains(&line.net) {
        return None;
    }
    let (_, gap) = impedance::pair_width_gap(ctx.project, pair.positive, line.layer);
    let a = simplify(&line.points);
    let seg = nearest_segment(&a, pusher)?;
    let here = segment(&a, seg)?;
    let m = partner_line(ctx, state, partner, &line, here, gap)?;
    let other = state.lines.get(m)?.clone();
    let coupled = Coupled::of(line.width, other.width, gap);
    let mut b = simplify(&other.points);
    let reversed = !(0..b.len().saturating_sub(1)).any(|t| {
        segment(&b, t).is_some_and(|there| {
            coupled.beside(here, there).is_some()
                && unit(here[0], here[1])
                    .zip(unit(there[0], there[1]))
                    .is_some_and(|(p, q)| dot(p, q) > 0.0)
        })
    });
    if reversed {
        b.reverse();
    }
    let window = Window::of(&a, &b, seg, coupled)?;
    let bent = bend(ctx, state, pair, &line, &other, &a, &b, &window, pusher)?;
    let mut b_points = bent.b;
    if reversed {
        b_points.reverse();
    }
    state.lines.get_mut(l)?.points = bent.a;
    state.lines.get_mut(m)?.points = b_points;
    Some(vec![Pusher::Line(l), Pusher::Line(m)])
}

/// Try both senses round the pusher's hull; immovable copper in the way
/// joins the hull and they are tried again, as for one chain.
#[allow(clippy::too_many_arguments)] // Both halves' chains and polylines.
fn bend(
    ctx: &Ctx<'_>,
    state: &State,
    pair: &DiffPair,
    line: &Line,
    other: &Line,
    a: &[Point],
    b: &[Point],
    window: &Window,
    pusher: &Piece,
) -> Option<Bent> {
    let centre = &window.centre;
    let posture = match posture_of(centre) {
        Posture::Orthogonal if centre.len() > 2 => Posture::Orthogonal,
        Posture::Orthogonal => Posture::Diagonal,
        posture => posture,
    };
    let facets = if posture == Posture::Orthogonal {
        Facets::Four
    } else {
        Facets::Eight
    };
    let width = line.width.max(other.width);
    let grow = window.half + width / 2.0;
    let need = |net| ctx.clearance(CopperObject::Track, net, pusher.object, Some(pusher.net));
    let mut hull = Hull::of_segment(
        pusher.core.a,
        pusher.core.b,
        pusher.core.half + need(line.net).max(need(other.net)) + grow + MARGIN,
        facets,
    )?;
    let own = [line.net, other.net];
    // The centre line as one chain as wide as the pair.
    let envelope = Line {
        width: 2.0 * grow,
        posture,
        ..line.clone()
    };
    let limit = added_skew_limit(ctx, pair);
    let skew_before = (length(a) - length(b)).abs();
    let last = centre.len().checked_sub(2)?;
    for _ in 0..=MAX_MERGES {
        // A window end inside the hull cannot move out of it.
        let ((e_seg, e), (x_seg, x)) = inside_span(centre, &hull)?;
        // The window's ends keep their direction for a lead, so the halves
        // join what lies beyond them where they left it.
        let lead = 2.0 * window.half;
        let head = segment(centre, 0)?;
        let tail = segment(centre, last)?;
        let d0 = unit(head[0], head[1])?;
        let dn = unit(tail[0], tail[1])?;
        let (from, before) = if e_seg == 0 {
            (
                along(head[0], d0, lead.min(distance(head[0], e))),
                vec![head[0]],
            )
        } else {
            (*centre.get(e_seg)?, centre.get(..e_seg)?.to_vec())
        };
        let (to, after) = if x_seg == last {
            (
                along(tail[1], dn, -lead.min(distance(tail[1], x))),
                vec![tail[1]],
            )
        } else {
            (*centre.get(x_seg + 1)?, centre.get(x_seg + 2..)?.to_vec())
        };
        let mut best: Option<(f64, Bent)> = None;
        let mut blocking: Option<(f64, Hit)> = None;
        for ccw in [true, false] {
            let mut span = vec![from];
            span.extend(hull.arc(e, x, ccw));
            span.push(to);
            let span = tidy(ctx, state, &envelope, &simplify(&span), &own);
            let mut new_centre = before.clone();
            new_centre.extend(span);
            new_centre.extend_from_slice(&after);
            let new_centre = simplify(&new_centre);
            let total = length(&new_centre);
            let Some(bent) = lay(a, b, window, &new_centre) else {
                continue;
            };
            let probes = |points: &[Point], l: &Line| -> Option<Hit> {
                points.windows(2).find_map(|w| {
                    let probe = Piece::track(l.net, l.layer, w[0], w[1], l.width);
                    ctx.hits(state, &probe, &own)
                        .into_iter()
                        .find(|h| !ctx.movable(state, h.target) || touches(h, pusher, state))
                })
            };
            match probes(&bent.a, line).or_else(|| probes(&bent.b, other)) {
                None if !has_acute(&bent.a)
                    && !has_acute(&bent.b)
                    && line.keeps_pins(&bent.a)
                    && other.keeps_pins(&bent.b)
                    && (length(&bent.a) - length(&bent.b)).abs() - skew_before <= limit
                    && apart(ctx, line, other, &bent)
                    && best.as_ref().is_none_or(|(l, _)| total < *l) =>
                {
                    best = Some((total, bent));
                }
                Some(hit) if blocking.as_ref().is_none_or(|(l, _)| total < *l) => {
                    blocking = Some((total, hit));
                }
                _ => {}
            }
        }
        if let Some((_, bent)) = best {
            return Some(bent);
        }
        let (_, hit) = blocking?;
        let grown = ctx.hull(
            state,
            hit.target,
            line.net,
            CopperObject::Track,
            grow,
            facets,
        )?;
        hull.merge(&grown);
    }
    None
}

/// The halves laid along a new centre line, spliced into their chains.
fn lay(a: &[Point], b: &[Point], window: &Window, centre: &[Point]) -> Option<Bent> {
    let h = window.half * window.side;
    let along_a = offset(centre, h)?;
    let along_b = offset(centre, -h)?;
    Some(Bent {
        a: splice(a, window.a, &along_a)?,
        b: splice(b, window.b, &along_b)?,
    })
}

/// The halves keep the clearance between them: their fan-outs are where a
/// tight bend could reach the other half.
fn apart(ctx: &Ctx<'_>, line: &Line, other: &Line, bent: &Bent) -> bool {
    let need = ctx.clearance(
        CopperObject::Track,
        line.net,
        CopperObject::Track,
        Some(other.net),
    );
    bent.a.windows(2).all(|p| {
        let probe = Piece::track(line.net, line.layer, p[0], p[1], line.width);
        bent.b.windows(2).all(|q| {
            let piece = Piece::track(other.net, other.layer, q[0], q[1], other.width);
            !probe.ring.closer_than(&piece.ring, need)
        })
    })
}

/// The skew one push may add, mm: a share of the class's max skew, or,
/// when the class sets none, nothing beyond rounding. No limit is assumed
/// that neither a source nor the user set.
fn added_skew_limit(ctx: &Ctx<'_>, pair: &DiffPair) -> f64 {
    let class = crate::layout::net_class(ctx.project, pair.positive);
    match ctx.project.rules.signal(class).pair_max_skew {
        Some(max) => max * ADDED_SKEW_SHARE,
        None => LENGTH_TOLERANCE_MM,
    }
}
