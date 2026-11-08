//! Shove: the step's own copper pushes other nets' tracks and vias aside,
//! and what moved pushes in turn, breadth first, to a bounded depth and
//! number of moves.
//!
//! A pushed track moves as its chain ([`Line`]: tracks of one net, layer
//! and width joined end to end where no via, pad or third track meets);
//! the chain's ends stay put. The part of the chain inside the pusher's
//! hull (grown by the clearance and the chain's half-width) is replaced
//! by the hull's boundary on one side or the other, so the chain bends
//! round the pusher in its own posture (45° hulls for 45° and free
//! chains, rectangles for 90° chains). The side whose result is clear of
//! immovable copper wins, the shorter when both are; immovable copper the
//! result would touch joins the hull and the side is chosen again. The
//! result is tidied (short legs dropped, collinear legs merged, loops
//! cut, corners shortcut where clear) and refused if it keeps an acute
//! corner. A pushed via moves the least distance at a multiple of 45°
//! that clears the pusher, and the chains ending on it follow with a 45°
//! elbow. Pads, keepouts, the board edge, locked tracks and vias, the
//! routed nets' own copper, and vias when via shoving is off never move;
//! a push into them fails, and the whole shove with it. A differential
//! pair moves as one unit or not at all ([`pair`]).
use super::clean::{has_acute, length, posture_of, simplify};
use super::hull::{DIRS, Facets, Hull};
use super::world::{Ctx, Hit, Line, MovedVia, Piece, State, Target};
use crate::geometry::{Point, distance};
use crate::layout::CopperGraph;
use crate::layout::connectivity::{Meets, TrackEnd, TrackJoint, keeps_joint};
use crate::layout::posture::{self, Posture};
use crate::rules::CopperObject;
use std::collections::VecDeque;

mod pair;

/// Most generations of pushes (the step pushes, the pushed push, …).
pub(super) const MAX_DEPTH: usize = 8;
/// Most single pushes one shove makes.
pub(super) const MAX_SHOVES: usize = 96;
/// Most collision queries one shove makes before it gives up.
pub(super) const MAX_PROBES: usize = 1_500;
/// Most immovable obstacles merged into one push's hull.
const MAX_MERGES: usize = 4;
/// Most tracks one chain gathers.
const MAX_CHAIN: usize = 64;
/// Corners a shoved chain's shortcut may skip.
const SHORTCUT_SPAN: usize = 8;

/// What pushes: the step's copper, a moved chain or a moved via.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pusher {
    Fixed(usize),
    Line(usize),
    Via(usize),
}

fn pieces(state: &State, pusher: Pusher) -> usize {
    match pusher {
        Pusher::Fixed(_) | Pusher::Via(_) => 1,
        Pusher::Line(l) => state
            .lines
            .get(l)
            .map_or(0, |line| line.points.len().saturating_sub(1)),
    }
}

fn piece(ctx: &Ctx<'_>, state: &State, pusher: Pusher, k: usize) -> Option<Piece> {
    match pusher {
        Pusher::Fixed(f) => state.fixed.get(f).cloned(),
        Pusher::Line(l) => state.lines.get(l)?.piece(k),
        Pusher::Via(m) => Some(Piece::via(&state.vias.get(m)?.via, &ctx.stack)),
    }
}

/// Push everything the fixed copper of `state` collides with out of its
/// way. `Err` names what could not move.
pub(super) fn shove(ctx: &Ctx<'_>, state: &mut State) -> Result<(), Hit> {
    let mut queue: VecDeque<(Pusher, usize)> = (0..state.fixed.len())
        .map(|f| (Pusher::Fixed(f), 0))
        .collect();
    let mut budget = MAX_SHOVES;
    let stop = ctx.probes.get().saturating_add(MAX_PROBES);
    while let Some((pusher, depth)) = queue.pop_front() {
        let mut k = 0;
        while k < pieces(state, pusher) {
            let Some(piece) = piece(ctx, state, pusher, k) else {
                break;
            };
            k += 1;
            // A pair's moved halves are laid together: neither pushes the
            // other.
            let partner = matches!(pusher, Pusher::Line(_))
                .then(|| ctx.pair_of(piece.net).and_then(|p| p.partner(piece.net)))
                .flatten();
            let own = [piece.net, partner.unwrap_or(piece.net)];
            let mut settled = false;
            for _ in 0..12 {
                let hits = ctx.hits(state, &piece, &own);
                let Some(hit) = hits
                    .iter()
                    .find(|h| !ctx.movable(state, h.target))
                    .or(hits.first())
                    .copied()
                else {
                    settled = true;
                    break;
                };
                if !ctx.movable(state, hit.target)
                    || depth + 1 > MAX_DEPTH
                    || budget == 0
                    || ctx.probes.get() > stop
                {
                    note_held_via(ctx, &hit);
                    return Err(hit);
                }
                budget -= 1;
                let moved = push(ctx, state, hit, &piece).ok_or(hit)?;
                for next in moved {
                    if !queue.iter().any(|(p, _)| *p == next) {
                        queue.push_back((next, depth + 1));
                    }
                }
            }
            if !settled {
                let hits = ctx.hits(state, &piece, &own);
                if let Some(hit) = hits.first() {
                    return Err(*hit);
                }
            }
        }
    }
    Ok(())
}

/// A pair's via stopped the shove: pair vias never move.
fn note_held_via(ctx: &Ctx<'_>, hit: &Hit) {
    if let Target::Base(k) = hit.target
        && let Some(pair) = ctx.base_via(k).and_then(|v| ctx.pair_of(v.net))
    {
        ctx.held.set(Some(pair.id));
    }
}

/// Move chain `l` out of `pusher`'s way, with its partner when it is a
/// pair's half; the pushers that moved.
fn push_chain(ctx: &Ctx<'_>, state: &mut State, l: usize, pusher: &Piece) -> Option<Vec<Pusher>> {
    let net = state.lines.get(l)?.net;
    let Some(held) = ctx.pair_of(net) else {
        push_line(ctx, state, l, pusher)?;
        return Some(vec![Pusher::Line(l)]);
    };
    let moved = pair::push(ctx, state, l, held, pusher);
    if moved.is_none() {
        ctx.held.set(Some(held.id));
    }
    moved
}

/// Move `hit` out of `pusher`'s way; the pushers that moved.
fn push(ctx: &Ctx<'_>, state: &mut State, hit: Hit, pusher: &Piece) -> Option<Vec<Pusher>> {
    match hit.target {
        Target::Base(k) => {
            if let Some(track) = ctx.base_track(k) {
                let Some(l) = adopt_line(ctx, state, k) else {
                    if let Some(pair) = ctx.pair_of(track.net) {
                        ctx.held.set(Some(pair.id));
                    }
                    return None;
                };
                push_chain(ctx, state, l, pusher)
            } else {
                let via = ctx.base_via(k)?.clone();
                state.hidden.insert(k);
                state.vias.push(MovedVia {
                    original: via.position,
                    via,
                });
                push_via(ctx, state, state.vias.len() - 1, pusher)
            }
        }
        Target::Line(l, _) => push_chain(ctx, state, l, pusher),
        Target::Via(m) => push_via(ctx, state, m, pusher),
        _ => None,
    }
}

/// The one joint at `end` of board track `track`, fills aside: a fill is
/// refilled round moved copper, so it holds nothing in place.
fn only_joint(graph: &CopperGraph, track: usize, end: TrackEnd) -> Option<TrackJoint> {
    let mut at = graph
        .joints_on(track)
        .into_iter()
        .filter(|j| j.end == Some(end) && j.meets != Meets::Fill);
    let joint = at.next()?;
    at.next().is_none().then_some(joint)
}

fn other_end(end: TrackEnd) -> TrackEnd {
    match end {
        TrackEnd::Start => TrackEnd::End,
        TrackEnd::End => TrackEnd::Start,
    }
}

fn end_point(track: &crate::model::Track, end: TrackEnd) -> Point {
    match end {
        TrackEnd::Start => track.start,
        TrackEnd::End => track.end,
    }
}

/// Take the chain of board track `k` into the overlay; its index. The
/// chain runs on through a track end only where the copper graph has
/// exactly one joint there, end to end with another track of the same
/// width whose end meets nothing else. Joints on its tracks in mid-span
/// become its pins.
fn adopt_line(ctx: &Ctx<'_>, state: &mut State, k: usize) -> Option<usize> {
    let track = ctx.base_track(k)?;
    if track.locked || state.frozen.contains(&track.net) || state.hidden.contains(&k) {
        return None;
    }
    let graph = ctx.graph();
    let (net, layer, width) = (track.net, track.layer, track.width);
    // The track continuing the chain past `end` of `from`, and the end of
    // it the chain leaves by.
    let next = |state: &State, from: usize, end: TrackEnd| -> Option<(usize, TrackEnd)> {
        let link = only_joint(graph, from, end)?;
        let Meets::Track {
            index: other,
            end: Some(back),
            ..
        } = link.meets
        else {
            return None;
        };
        let back_link = only_joint(graph, other, back)?;
        if !link.end_to_end
            || !matches!(back_link.meets, Meets::Track { index, .. } if index == from)
        {
            return None;
        }
        let t = ctx.base_track(other)?;
        if t.locked || (t.width - width).abs() > 1e-9 || state.hidden.contains(&other) {
            return None;
        }
        Some((other, other_end(back)))
    };
    let mut base = vec![k];
    let mut points = vec![track.start, track.end];
    // Backward from the start, then forward from the end.
    for (first, end) in [(true, TrackEnd::Start), (false, TrackEnd::End)] {
        let (mut from, mut leave) = (k, end);
        while base.len() < MAX_CHAIN {
            let Some((other, far)) = next(state, from, leave) else {
                break;
            };
            if base.contains(&other) {
                break;
            }
            let at = end_point(ctx.base_track(other)?, far);
            if first {
                base.insert(0, other);
                points.insert(0, at);
            } else {
                base.push(other);
                points.push(at);
            }
            (from, leave) = (other, far);
        }
    }
    let ids = base
        .iter()
        .filter_map(|b| ctx.base_track(*b).map(|t| t.id))
        .collect::<Vec<_>>();
    if ids.len() != base.len() {
        return None;
    }
    let pins = base
        .iter()
        .flat_map(|b| graph.joints_on(*b))
        .filter(|j| {
            j.end.is_none()
                && j.meets != Meets::Fill
                && !matches!(j.meets, Meets::Track { index, .. } if base.contains(&index))
        })
        .map(|j| j.at)
        .collect();
    state.hidden.extend(base.iter().copied());
    state.lines.push(Line {
        net,
        layer,
        width,
        ids,
        base,
        // One straight track takes 45° jogs; a chain with 90° corners
        // keeps to the axes.
        posture: match posture_of(&points) {
            Posture::Orthogonal if points.len() <= 2 => Posture::Diagonal,
            posture => posture,
        },
        original: points.clone(),
        points,
        pins,
    });
    Some(state.lines.len() - 1)
}

/// Where a polyline is inside `hull`: (segment, point) of the first entry
/// and of the last exit.
type Span = ((usize, Point), (usize, Point));

fn inside_span(points: &[Point], hull: &Hull) -> Option<Span> {
    let at = |a: Point, b: Point, t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    let mut first = None;
    let mut last = None;
    for (i, w) in points.windows(2).enumerate() {
        // Found with a tolerance, placed exactly on the boundary.
        if let Some((t0, t1)) = hull
            .inside_interval(w[0], w[1], 1e-7)
            .and_then(|found| hull.inside_interval(w[0], w[1], 0.0).or(Some(found)))
        {
            if first.is_none() {
                first = Some((i, at(w[0], w[1], t0), t0));
            }
            last = Some((i, at(w[0], w[1], t1), t1));
        }
    }
    let (first, last) = (first?, last?);
    let segments = points.len().saturating_sub(1);
    // A chain end inside the hull cannot move out of it.
    if (first.0 == 0 && first.2 <= 1e-9) || (last.0 + 1 == segments && last.2 >= 1.0 - 1e-9) {
        return None;
    }
    Some(((first.0, first.1), (last.0, last.1)))
}

/// Bend chain `l` round `pusher`.
fn push_line(ctx: &Ctx<'_>, state: &mut State, l: usize, pusher: &Piece) -> Option<()> {
    let line = state.lines.get(l)?.clone();
    let facets = if line.posture == Posture::Orthogonal {
        Facets::Four
    } else {
        Facets::Eight
    };
    let grow = line.width / 2.0;
    let need = ctx.clearance(
        CopperObject::Track,
        line.net,
        pusher.object,
        Some(pusher.net),
    );
    let mut hull = Hull::of_segment(
        pusher.core.a,
        pusher.core.b,
        pusher.core.half + need + grow + super::world::MARGIN,
        facets,
    )?;
    let own = [line.net];
    for _ in 0..=MAX_MERGES {
        let ((e_seg, e), (x_seg, x)) = inside_span(&line.points, &hull)?;
        let mut best: Option<(f64, Vec<Point>)> = None;
        let mut blocking: Option<(f64, Hit)> = None;
        for ccw in [true, false] {
            // Tidy only round the push: the corner before the entry, the
            // arc and the corner after the exit.
            let before = line.points.get(..e_seg)?;
            let after = line.points.get(x_seg + 2..).unwrap_or(&[]);
            let mut span: Vec<Point> = line.points.get(e_seg..=e_seg)?.to_vec();
            span.extend(hull.arc(e, x, ccw));
            span.extend(line.points.get(x_seg + 1).copied());
            let span = tidy(ctx, state, &line, &simplify(&span), &own);
            let mut pts = before.to_vec();
            pts.extend(span);
            pts.extend(after.iter().copied());
            let pts = simplify(&pts);
            let total = length(&pts);
            let block = pts.windows(2).find_map(|w| {
                let probe = Piece::track(line.net, line.layer, w[0], w[1], line.width);
                ctx.hits(state, &probe, &own)
                    .into_iter()
                    .find(|h| !ctx.movable(state, h.target) || touches(h, pusher, state))
            });
            match block {
                None if !has_acute(&pts)
                    && line.keeps_pins(&pts)
                    && best.as_ref().is_none_or(|(b, _)| total < *b) =>
                {
                    best = Some((total, pts));
                }
                Some(hit) if blocking.as_ref().is_none_or(|(b, _)| total < *b) => {
                    blocking = Some((total, hit));
                }
                _ => {}
            }
        }
        if let Some((_, pts)) = best {
            state.lines.get_mut(l)?.points = pts;
            return Some(());
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

/// Whether `hit` is the pusher itself (a result still touching it).
fn touches(hit: &Hit, pusher: &Piece, state: &State) -> bool {
    match hit.target {
        Target::Fixed(f) => state
            .fixed
            .get(f)
            .is_some_and(|p| p.core == pusher.core && p.net == pusher.net),
        _ => false,
    }
}

/// Shortcut runs of a shoved chain's corners with posture paths that
/// touch nothing, then clean up.
fn tidy(
    ctx: &Ctx<'_>,
    state: &State,
    line: &Line,
    points: &[Point],
    own: &[crate::ids::Id],
) -> Vec<Point> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    let posture = match line.posture {
        Posture::Free => Posture::Diagonal,
        p => p,
    };
    let clear = |legs: &[Point]| {
        legs.windows(2).all(|w| {
            let probe = Piece::track(line.net, line.layer, w[0], w[1], line.width);
            ctx.hits(state, &probe, own).is_empty()
        })
    };
    let mut out = points.get(..1).map(<[Point]>::to_vec).unwrap_or_default();
    let mut i = 0;
    while i + 1 < n {
        let mut next = i + 1;
        let mut via: Option<Vec<Point>> = None;
        let far = (i + SHORTCUT_SPAN).min(n - 1);
        'search: for j in (i + 2..=far).rev() {
            let (Some(&a), Some(&b)) = (points.get(i), points.get(j)) else {
                continue;
            };
            // A shortcut keeps the chain's pins on the corners it skips.
            let skipped = points.get(i..=j).unwrap_or(&[]);
            let pinned: Vec<Point> = line
                .pins
                .iter()
                .copied()
                .filter(|p| keeps_joint(skipped, *p))
                .collect();
            for f in [false, true] {
                let legs = posture::path(a, b, posture, f);
                if legs.len() >= 2 && pinned.iter().all(|p| keeps_joint(&legs, *p)) && clear(&legs)
                {
                    via = Some(legs);
                    next = j;
                    break 'search;
                }
            }
        }
        match via {
            Some(legs) => out.extend(legs.iter().skip(1)),
            None => out.extend(points.get(next).copied()),
        }
        i = next;
    }
    simplify(&out)
}

/// Move via `m` out of `pusher`'s way; it and the chains that followed.
fn push_via(ctx: &Ctx<'_>, state: &mut State, m: usize, pusher: &Piece) -> Option<Vec<Pusher>> {
    let moved = state.vias.get(m)?.clone();
    let via = &moved.via;
    let need = ctx.clearance(CopperObject::Via, via.net, pusher.object, Some(pusher.net));
    let hull = Hull::of_segment(
        pusher.core.a,
        pusher.core.b,
        pusher.core.half + need + via.diameter / 2.0 + super::world::MARGIN,
        Facets::Eight,
    )?;
    let c = via.position;
    let mut options: Vec<(f64, Point)> = DIRS
        .iter()
        .enumerate()
        .map(|(k, d)| {
            let t = hull.support(k) - (d[0] * c[0] + d[1] * c[1]);
            (t, [c[0] + d[0] * t, c[1] + d[1] * t])
        })
        .filter(|(t, _)| *t > 0.0 && t.is_finite())
        .collect();
    options.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, to) in options {
        let mut trial = state.clone();
        if let Some(moved) = trial.vias.get_mut(m) {
            moved.via.position = to;
        }
        let Some(lines) = follow(ctx, &mut trial, via, to) else {
            continue;
        };
        let probe = Piece::via(&trial.vias.get(m)?.via, &ctx.stack);
        let own = [via.net];
        let blocked = |h: &Hit| !ctx.movable(&trial, h.target) || touches(h, pusher, &trial);
        if ctx.hits(&trial, &probe, &own).iter().any(blocked) {
            continue;
        }
        let lines_clear = lines.iter().all(|l| {
            trial.lines.get(*l).is_some_and(|line| {
                !has_acute(&line.points)
                    && line.points.windows(2).all(|w| {
                        let p = Piece::track(line.net, line.layer, w[0], w[1], line.width);
                        !ctx.hits(&trial, &p, &own).iter().any(blocked)
                    })
            })
        });
        if !lines_clear {
            continue;
        }
        *state = trial;
        let mut moved = vec![Pusher::Via(m)];
        moved.extend(lines.into_iter().map(Pusher::Line));
        return Some(moved);
    }
    None
}

/// The chains ending on `via` follow it to `to` with a 45° elbow from
/// their last corner, each end keeping its place on the via. Their
/// indices; `None` when one cannot follow, or when copper joins the via
/// other than at a track end (a pad, another via, a track in mid-span):
/// moving the via would detach that copper.
fn follow(
    ctx: &Ctx<'_>,
    state: &mut State,
    via: &crate::model::Via,
    to: Point,
) -> Option<Vec<usize>> {
    let from = via.position;
    let index = ctx.project.vias.iter().position(|v| v.id == via.id)?;
    let original = ctx.project.vias.get(index)?.position;
    let mut ends = Vec::new();
    for meets in ctx.graph().joints_of_via(index) {
        match meets {
            Meets::Track {
                index: k,
                end: Some(end),
                ..
            } => ends.push((k, end)),
            Meets::Fill => {}
            _ => return None,
        }
    }
    // Adopt the board tracks ending on the via first; each end keeps its
    // offset from the via's centre.
    let mut offsets = Vec::with_capacity(ends.len());
    for (k, end) in ends {
        if !state.hidden.contains(&k) {
            adopt_line(ctx, state, k)?;
        }
        let p = end_point(ctx.project.tracks.get(k)?, end);
        offsets.push([p[0] - original[0], p[1] - original[1]]);
    }
    let on_via = |p: Point| {
        offsets
            .iter()
            .copied()
            .find(|o| distance([p[0] - o[0], p[1] - o[1]], from) < 1e-6)
    };
    let mut followed = Vec::new();
    for (l, line) in state.lines.iter_mut().enumerate() {
        if line.net != via.net {
            continue;
        }
        let at_start = line.points.first().and_then(|p| on_via(*p));
        let at_end = line.points.last().and_then(|p| on_via(*p));
        let offset = match (at_start, at_end) {
            (Some(_), Some(_)) => return None,
            (Some(o), None) | (None, Some(o)) => o,
            (None, None) => continue,
        };
        if at_start.is_some() {
            line.points.reverse();
        }
        let n = line.points.len();
        let anchor = *line.points.get(n.checked_sub(2)?)?;
        let old_end = *line.points.get(n - 1)?;
        let target = [to[0] + offset[0], to[1] + offset[1]];
        let old = [old_end[0] - anchor[0], old_end[1] - anchor[1]];
        let elbow = [false, true]
            .into_iter()
            .map(|f| posture::path(anchor, target, Posture::Diagonal, f))
            .max_by(|a, b| keeps(a, old).total_cmp(&keeps(b, old)))?;
        line.points.truncate(n - 1);
        line.points.extend(elbow.iter().skip(1));
        line.points = simplify(&line.points);
        if at_start.is_some() {
            line.points.reverse();
        }
        followed.push(l);
    }
    Some(followed)
}

/// How well an elbow keeps the old leg's direction on its first leg.
fn keeps(elbow: &[Point], old: Point) -> f64 {
    let Some(w) = elbow.get(..2) else {
        return -1.0;
    };
    let d = [w[1][0] - w[0][0], w[1][1] - w[0][1]];
    let (ld, lo) = (d[0].hypot(d[1]), old[0].hypot(old[1]));
    if ld < 1e-12 || lo < 1e-12 {
        return -1.0;
    }
    (d[0] * old[0] + d[1] * old[1]) / (ld * lo)
}
