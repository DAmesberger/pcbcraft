//! The route head's corner. A step continues the track that ends at its
//! head, so the head is a corner whose place the click chose (on the
//! 0.5 mm grid, say) without knowing where the next target is. When the
//! step from there would run back against that track (a hook into a pad
//! beside the corner, a reversal, a knee past the target), the corner
//! moves back along the track instead, to the point from which the step
//! turns once, by at most 90°, toward the target: straight along an axis
//! or diagonal of the posture where one fits, else at the posture's elbow.
//! The track is shortened in the same edit set, so one undo step takes
//! both back.
use super::world::Ctx;
use super::{RoutePlan, RouteStepRequest, clean, proposal};
use crate::geometry::{Point, distance};
use crate::layout::connectivity::{Meets, TrackEnd, keeps_joint};
use crate::layout::posture::{MIN_SEGMENT_MM, Posture};
use crate::model::Track;

/// The track a step continues: a board track of its net on its layer
/// with an end at the head.
struct Incoming {
    track: Track,
    /// Which end is at the head.
    end: TrackEnd,
    /// The far end, where the track comes from.
    from: Point,
    /// The unit direction from `from` to the head.
    along: Point,
    length: f64,
    /// The least distance from `from` the shortened track keeps: every
    /// joint on it (a pad, a via, another track) stays on it.
    keep: f64,
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn incoming(ctx: &Ctx<'_>, request: &RouteStepRequest) -> Option<Incoming> {
    if request.via.is_some() {
        return None;
    }
    let graph = ctx.graph();
    ctx.project
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.net == request.net && t.layer == request.layer && !t.locked)
        .find_map(|(index, t)| {
            let (end, from) = if keeps_joint(&[t.end], request.head) {
                (TrackEnd::End, t.start)
            } else if keeps_joint(&[t.start], request.head) {
                (TrackEnd::Start, t.end)
            } else {
                return None;
            };
            let length = distance(t.start, t.end);
            if length < 2.0 * MIN_SEGMENT_MM {
                return None;
            }
            let along = [
                (request.head[0] - from[0]) / length,
                (request.head[1] - from[1]) / length,
            ];
            // Another track ending at the head is a joint at the head end,
            // so it keeps the whole track.
            let keep = graph
                .joints_on(index)
                .iter()
                .filter(|j| j.meets != Meets::Fill)
                .map(|j| match end {
                    TrackEnd::End => j.station,
                    TrackEnd::Start => length - j.station,
                })
                .fold(MIN_SEGMENT_MM, f64::max);
            Some(Incoming {
                track: t.clone(),
                end,
                from,
                along,
                length,
                keep,
            })
        })
}

/// Where the corner may go, nearest the head first: on the track, from
/// the least distance it keeps to a shortest segment short of the head.
/// The foot of the target on the track's line (a 90° turn), and for 45°
/// and 90° routing each point from which an axis or diagonal of the
/// posture turning by at most 90° runs straight to the target.
fn corners(incoming: &Incoming, to: Point, posture: Posture) -> Vec<Point> {
    let rel = [to[0] - incoming.from[0], to[1] - incoming.from[1]];
    let u = incoming.along;
    let mut stations = vec![dot(rel, u)];
    let directions: &[usize] = match posture {
        Posture::Free => &[],
        Posture::Orthogonal => &[0, 2, 4, 6],
        Posture::Diagonal => &[0, 1, 2, 3, 4, 5, 6, 7],
    };
    for &k in directions {
        let angle = (k as f64 * 45.0).to_radians();
        let d = [angle.cos(), angle.sin()];
        let across = cross(u, d);
        if dot(d, u) < -1e-9 || across.abs() < 1e-9 {
            continue;
        }
        // rel = s·u + t·d, with t > 0 the straight leg to the target.
        if cross(u, rel) / across > 0.0 {
            stations.push(cross(rel, d) / across);
        }
    }
    stations.retain(|s| *s >= incoming.keep && *s <= incoming.length - MIN_SEGMENT_MM);
    stations.sort_by(|a, b| b.total_cmp(a));
    stations.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    stations
        .into_iter()
        .map(|s| [incoming.from[0] + s * u[0], incoming.from[1] + s * u[1]])
        .filter(|c| distance(*c, to) >= MIN_SEGMENT_MM)
        .collect()
}

/// The step's own polyline on its layer, in order.
fn head_points(plan: &RoutePlan, request: &RouteStepRequest) -> Vec<Point> {
    let mut points = Vec::new();
    for r in plan
        .edits
        .add
        .iter()
        .filter(|r| r.net == request.net && r.layer == request.layer)
    {
        if points.is_empty() {
            points.push(r.start);
        }
        points.push(r.end);
    }
    points
}

/// `plan` for `request`, or, when its path runs back against the track it
/// continues, the step from a corner moved back along that track, the
/// track shortened to it. The nearest corner whose step reaches the
/// target without turning back or an acute corner wins; when none does,
/// `plan` stands.
pub(super) fn recompute(ctx: &Ctx<'_>, request: &RouteStepRequest, plan: RoutePlan) -> RoutePlan {
    let Some(incoming) = incoming(ctx, request) else {
        return plan;
    };
    if !clean::turns_back(&head_points(&plan, request), incoming.along) {
        return plan;
    }
    for corner in corners(&incoming, request.to, request.posture) {
        let moved = RouteStepRequest {
            head: corner,
            ..request.clone()
        };
        let mut candidate = proposal(ctx, &moved);
        let points = head_points(&candidate, &moved);
        if !candidate.reached
            || points.first().is_none_or(|p| distance(*p, corner) > 1e-9)
            || clean::turns_back(&points, incoming.along)
        {
            continue;
        }
        let mut path = vec![incoming.from];
        path.extend(points);
        if clean::has_acute(&path) {
            continue;
        }
        let mut shortened = incoming.track.clone();
        match incoming.end {
            TrackEnd::End => shortened.end = corner,
            TrackEnd::Start => shortened.start = corner,
        }
        candidate.edits.replace.insert(0, shortened);
        candidate.undo.replace.insert(0, incoming.track.clone());
        candidate.shoved.insert(0, incoming.track.id);
        return candidate;
    }
    plan
}
