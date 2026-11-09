//! Wedges: where joined copper of one net on one layer meets at an angle,
//! the bare wedge between the two copper edges (an acid trap when it is
//! acute). Read off the graph's joints, so only copper that joins is
//! asked, and measured on straight edges only:
//!
//! - **Track to track**, where the centre lines meet (a corner, a T or a
//!   crossing): the wedges between consecutive centre-line rays around the
//!   meeting point. A wedge's apex is where the two tracks' facing sides
//!   cross; it is reported only when that point lies on both straight
//!   sides (not past a round end).
//! - **Track to pad**: where a track side crosses a straight pad edge
//!   (rectangle sides, the straight parts of rounded rectangles and oblong
//!   pads, a custom outline's line segments). Round pad edges and vias are
//!   not measured: a circle has no corner with a track side.
//! - **Track to fill**: where a track side crosses an edge of a fresh fill
//!   island of its net, on the stored polygon (the fabricated copper). The
//!   fill edge's direction is its chord over the track's half-width, from
//!   the crossing to where the edge lies that far from the track side
//!   ([`mouth`]): exact for a straight edge, and not one short chord's
//!   direction on a flattened curve.
//!
//! A wedge that other copper of the net on the layer fills (a pad at the
//! corner, a pour over both tracks) is no wedge and is left out.
use super::{CopperGraph, JOIN_TOLERANCE_MM, Meets};
use crate::diagnostics::ObjectRef;
use crate::geometry::{Point, distance, polygon_contains, segment_distance, segments_cross};
use crate::ids::Id;
use crate::layout::WorldPad;
use crate::layout::fill::{self, RegionIndex};
use crate::library::{PAD_ARC_TOLERANCE_MM, PadShape, Segment};
use crate::model::{CopperLayer, Project};
use std::collections::BTreeMap;

/// One bare wedge between two joined copper edges.
#[derive(Clone, Debug, PartialEq)]
pub struct Wedge {
    pub layer: CopperLayer,
    pub net: Id,
    /// The apex: where the two edges meet.
    pub at: Point,
    /// The wedge's angle, degrees (0–180).
    pub angle_deg: f64,
    /// The track, then the track, pad or fill it meets.
    pub objects: [ObjectRef; 2],
}

/// A centre-line ray of a track from a meeting point.
#[derive(Clone, Copy, Debug)]
struct Ray {
    track: usize,
    dir: Point,
    length: f64,
    half: f64,
}

/// A fresh fill island set of one net on one layer, indexed on first use.
struct FillCopper<'a> {
    id: Id,
    net: Id,
    layer: CopperLayer,
    regions: &'a [crate::model::fill::FilledRegion],
    index: Option<RegionIndex>,
}

impl FillCopper<'_> {
    fn index(&mut self) -> &RegionIndex {
        self.index
            .get_or_insert_with(|| RegionIndex::filled(self.regions))
    }
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}
fn scale(a: Point, s: f64) -> Point {
    [a[0] * s, a[1] * s]
}
fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn unit(a: Point) -> Option<Point> {
    let l = a[0].hypot(a[1]);
    (l > 1e-12).then(|| [a[0] / l, a[1] / l])
}
/// The left normal of a unit direction.
fn left(d: Point) -> Point {
    [-d[1], d[0]]
}

/// The angle between unit directions `u` and `v`, radians (0–π).
fn between(u: Point, v: Point) -> f64 {
    cross(u, v).abs().atan2(dot(u, v))
}

/// Where segments `a`–`b` and `c`–`d` cross at one interior point.
fn crossing(a: Point, b: Point, c: Point, d: Point) -> Option<Point> {
    if !segments_cross(a, b, c, d) {
        return None;
    }
    let (r, s) = (sub(b, a), sub(d, c));
    let denominator = cross(r, s);
    if denominator == 0.0 {
        return None;
    }
    let t = cross(sub(c, a), s) / denominator;
    Some(add(a, scale(r, t)))
}

/// A pad's straight copper edges on the board: rectangle sides less their
/// corner radii, a custom outline's line segments. Arcs and circles have
/// none.
fn straight_edges(pad: &WorldPad) -> Vec<(Point, Point)> {
    let mut local: Vec<(Point, Point)> = Vec::new();
    match &pad.shape {
        PadShape::Circle { .. } => {}
        PadShape::Custom { outline } => {
            let mut from = outline.start;
            for segment in &outline.segments {
                let to = segment.to();
                let straight = match segment {
                    Segment::Line { .. } => true,
                    Segment::Arc { bulge, .. } => bulge.abs() <= 1e-12,
                };
                if straight {
                    local.push((from, to));
                }
                from = to;
            }
        }
        shape => {
            if let crate::geometry::Shape::Polygon { points, radii } =
                shape.outline(PAD_ARC_TOLERANCE_MM)
            {
                let n = points.len();
                let radius = |k: usize| radii.get(k % n.max(1)).copied().unwrap_or(0.0);
                for k in 0..n {
                    let (Some(&a), Some(&b)) = (points.get(k), points.get((k + 1) % n)) else {
                        continue;
                    };
                    let length = distance(a, b);
                    let (ra, rb) = (radius(k), radius(k + 1));
                    if length - ra - rb > JOIN_TOLERANCE_MM
                        && let Some(d) = unit(sub(b, a))
                    {
                        local.push((add(a, scale(d, ra)), sub(b, scale(d, rb))));
                    }
                }
            }
        }
    }
    let frame = pad.frame();
    local
        .into_iter()
        .map(|(a, b)| (frame.apply(a), frame.apply(b)))
        .collect()
}

/// Where a fill edge leaving a track side at `p` (on edge `edge` of one of
/// `regions`' rings, followed toward its end when `forward`) first lies
/// `depth` from the side line (through `side`, outward normal `n`): the
/// wedge's mouth at the track's half-width. Its direction from `p` is the
/// edge's direction for a straight edge, and the chord over that depth for a
/// flattened curve, which a single short chord's direction would misstate.
/// `None` when the edge turns back across the side first.
fn mouth(
    regions: &[crate::model::fill::FilledRegion],
    edge: (Point, Point),
    forward: bool,
    p: Point,
    side: Point,
    n: Point,
    depth: f64,
) -> Option<Point> {
    let starts = |ring: &Vec<Point>| {
        let len = ring.len();
        (0..len).find(|&k| ring.get(k) == Some(&edge.0) && ring.get((k + 1) % len) == Some(&edge.1))
    };
    let (ring, start) = regions
        .iter()
        .flat_map(|r| std::iter::once(&r.outer).chain(r.holes.iter()))
        .find_map(|ring| Some((ring, starts(ring)?)))?;
    let len = ring.len();
    let height = |q: Point| dot(sub(q, side), n);
    let (mut previous, mut was) = (p, height(p));
    for step in 0..len {
        let k = if forward {
            (start + 1 + step) % len
        } else {
            (start + len - step) % len
        };
        let q = *ring.get(k)?;
        let h = height(q);
        if h >= depth {
            let t = if h > was {
                (depth - was) / (h - was)
            } else {
                1.0
            };
            return Some(add(previous, scale(sub(q, previous), t.clamp(0.0, 1.0))));
        }
        if h < 0.0 {
            return None;
        }
        (previous, was) = (q, h);
    }
    None
}

impl CopperGraph {
    /// The point of track `index` at `station`, if it exists.
    fn point_on(&self, index: usize, station: f64) -> Option<Point> {
        let t = self.tracks.get(index)?;
        let length = distance(t.start, t.end);
        if length <= 0.0 {
            return Some(t.start);
        }
        let s = (station / length).clamp(0.0, 1.0);
        Some(add(t.start, scale(sub(t.end, t.start), s)))
    }

    /// The centre-line rays of track `index` from `at`, a point of its
    /// centre line: one toward each end it does not sit on.
    fn rays(&self, index: usize, at: Point) -> Vec<Ray> {
        let Some(t) = self.tracks.get(index) else {
            return Vec::new();
        };
        let length = distance(t.start, t.end);
        let Some(d) = unit(sub(t.end, t.start)) else {
            return Vec::new();
        };
        let s = dot(sub(at, t.start), d).clamp(0.0, length);
        let half = t.width / 2.0;
        let mut out = Vec::new();
        if s > JOIN_TOLERANCE_MM {
            out.push(Ray {
                track: index,
                dir: scale(d, -1.0),
                length: s,
                half,
            });
        }
        if length - s > JOIN_TOLERANCE_MM {
            out.push(Ray {
                track: index,
                dir: d,
                length: length - s,
                half,
            });
        }
        out
    }

    /// Whether copper of `net` on `layer` other than the outlines `skip`
    /// and the fill `skip_fill` holds `p`.
    fn covered(
        &self,
        p: Point,
        layer: CopperLayer,
        net: Id,
        skip: &[usize],
        skip_fill: Option<Id>,
        fills: &mut [FillCopper<'_>],
    ) -> bool {
        if let Some(group) = self.groups.get(&net) {
            let probe = crate::geometry::Bounds::point(p);
            let mut near = Vec::new();
            group
                .grid
                .near(&probe, super::build::JOIN_REACH_MM, &mut near);
            for &outline in near.iter().filter_map(|j| group.members.get(*j)) {
                if skip.contains(&outline) {
                    continue;
                }
                let on_layer = self.lands.get(outline).is_some_and(|lands| {
                    lands
                        .clone()
                        .any(|e| self.elements.get(e).is_some_and(|e| e.layer == layer))
                });
                let Some(ring) = self.shapes.get(outline) else {
                    continue;
                };
                let holds = match ring.spine {
                    Some((a, b, r)) => segment_distance(p, a, b) < r,
                    None => polygon_contains(&ring.ring, p),
                };
                if on_layer && holds {
                    return true;
                }
            }
        }
        fills
            .iter_mut()
            .filter(|f| f.net == net && f.layer == layer && Some(f.id) != skip_fill)
            .any(|f| f.index().region_at(p).is_some())
    }

    /// Every bare wedge between joined copper edges narrower than
    /// `below_deg` degrees (see the module notes). `project` is the
    /// revision the graph was built from; its fresh fills give the fill
    /// edges.
    pub fn wedges(&self, project: &Project, below_deg: f64) -> Vec<Wedge> {
        let mut out = Vec::new();
        if below_deg.is_nan() || below_deg <= 0.0 {
            return out;
        }
        let below = below_deg.min(180.0).to_radians();
        let fresh = fill::fresh(project);
        let mut fills: Vec<FillCopper<'_>> = fresh
            .iter()
            .map(|f| FillCopper {
                id: f.id,
                net: f.net,
                layer: f.layer,
                regions: &f.copper.regions,
                index: None,
            })
            .collect();
        // Track to track: rays around each point where centre lines meet.
        let mut nodes: BTreeMap<(CopperLayer, i64, i64), (Point, Vec<Ray>)> = BTreeMap::new();
        let pad_outline = |index: usize| self.tracks.len() + self.vias.len() + index;
        for (index, track) in self.tracks.iter().enumerate() {
            let joints = self.joints_on(index);
            let mut pads: Vec<usize> = Vec::new();
            let mut joins_fill = false;
            for joint in &joints {
                match joint.meets {
                    Meets::Track {
                        index: other,
                        station,
                        ..
                    } => {
                        let Some(there) = self.point_on(other, station) else {
                            continue;
                        };
                        if distance(there, joint.at) > JOIN_TOLERANCE_MM {
                            continue;
                        }
                        let grid = |v: f64| (v / JOIN_TOLERANCE_MM).round() as i64;
                        let node = nodes
                            .entry((track.layer, grid(joint.at[0]), grid(joint.at[1])))
                            .or_insert_with(|| (joint.at, Vec::new()));
                        let at = node.0;
                        for t in [index, other] {
                            if !node.1.iter().any(|r| r.track == t) {
                                node.1.extend(self.rays(t, at));
                            }
                        }
                    }
                    Meets::Pad { index: pad } => {
                        if !pads.contains(&pad) {
                            pads.push(pad);
                        }
                    }
                    Meets::Fill => joins_fill = true,
                    Meets::Via { .. } => {}
                }
            }
            if pads.is_empty() && !joins_fill {
                continue;
            }
            let Some(d) = unit(sub(track.end, track.start)) else {
                continue;
            };
            let length = distance(track.start, track.end);
            let half = track.width / 2.0;
            let me = ObjectRef::Track { id: track.id };
            // The track's two sides, each with its outward normal.
            let sides = [1.0, -1.0].map(|sign| {
                let n = scale(left(d), sign);
                (
                    add(track.start, scale(n, half)),
                    add(track.end, scale(n, half)),
                    n,
                )
            });
            // The wedge at side crossing `p` of rays `u` (along the side,
            // out of the other copper) and `v` (along its edge, away from
            // the track), if acute, open and bare.
            let mut wedge = |p: Point,
                             u: Point,
                             v: Point,
                             room: f64,
                             skip: &[usize],
                             skip_fill: Option<Id>,
                             other: ObjectRef,
                             fills: &mut [FillCopper<'_>]| {
                let angle = between(u, v);
                if !(angle > 0.0 && angle < below) || room <= JOIN_TOLERANCE_MM {
                    return;
                }
                let Some(bisector) = unit(add(u, v)) else {
                    return;
                };
                let probe = add(
                    p,
                    scale(bisector, 2.0 * JOIN_TOLERANCE_MM / (angle / 2.0).sin()),
                );
                if self.covered(probe, track.layer, track.net, skip, skip_fill, fills) {
                    return;
                }
                out.push(Wedge {
                    layer: track.layer,
                    net: track.net,
                    at: p,
                    angle_deg: angle.to_degrees(),
                    objects: [me, other],
                });
            };
            for pad in pads {
                let Some(world) = self.pads.get(pad) else {
                    continue;
                };
                let Some(ring) = self.shapes.get(pad_outline(pad)) else {
                    continue;
                };
                let other = ObjectRef::Pad {
                    instance: world.instance,
                    pad: world.pad,
                };
                for (a, b) in straight_edges(world) {
                    let Some(e) = unit(sub(b, a)) else {
                        continue;
                    };
                    for &(s0, s1, n) in &sides {
                        let Some(p) = crossing(s0, s1, a, b) else {
                            continue;
                        };
                        let step = 2.0 * JOIN_TOLERANCE_MM;
                        let u = if polygon_contains(&ring.ring, add(p, scale(d, step))) {
                            scale(d, -1.0)
                        } else {
                            d
                        };
                        let v = if dot(e, n) > 0.0 { e } else { scale(e, -1.0) };
                        let along = dot(sub(p, s0), d);
                        let room = if dot(u, d) > 0.0 {
                            length - along
                        } else {
                            along
                        };
                        let edge_room = if dot(v, e) > 0.0 {
                            distance(p, b)
                        } else {
                            distance(p, a)
                        };
                        wedge(
                            p,
                            u,
                            v,
                            room.min(edge_room),
                            &[index, pad_outline(pad)],
                            None,
                            other,
                            &mut fills,
                        );
                    }
                }
            }
            if !joins_fill {
                continue;
            }
            for k in 0..fills.len() {
                let Some(f) = fills.get_mut(k) else {
                    continue;
                };
                if f.net != track.net || f.layer != track.layer {
                    continue;
                }
                let id = f.id;
                let mut edges: Vec<(Point, Point)> = Vec::new();
                {
                    let index = f.index();
                    for &(s0, s1, _) in &sides {
                        let lo = [s0[0].min(s1[0]), s0[1].min(s1[1])];
                        let hi = [s0[0].max(s1[0]), s0[1].max(s1[1])];
                        for (_, a, b) in index.edges_near(lo, hi) {
                            if !edges.contains(&(a, b)) {
                                edges.push((a, b));
                            }
                        }
                    }
                }
                for (a, b) in edges {
                    let Some(e) = unit(sub(b, a)) else {
                        continue;
                    };
                    for &(s0, s1, n) in &sides {
                        let Some(p) = crossing(s0, s1, a, b) else {
                            continue;
                        };
                        let step = 2.0 * JOIN_TOLERANCE_MM;
                        let inside = fills
                            .get_mut(k)
                            .is_some_and(|f| f.index().region_at(add(p, scale(d, step))).is_some());
                        let u = if inside { scale(d, -1.0) } else { d };
                        let forward = dot(e, n) > 0.0;
                        let regions = fills.get(k).map_or(&[][..], |f| f.regions);
                        let Some(mouth) = mouth(regions, (a, b), forward, p, s0, n, half) else {
                            continue;
                        };
                        let Some(v) = unit(sub(mouth, p)) else {
                            continue;
                        };
                        let along = dot(sub(p, s0), d);
                        let room = if dot(u, d) > 0.0 {
                            length - along
                        } else {
                            along
                        };
                        wedge(
                            p,
                            u,
                            v,
                            room.min(distance(p, mouth)),
                            &[index],
                            Some(id),
                            ObjectRef::Fill { id },
                            &mut fills,
                        );
                    }
                }
            }
        }
        for ((layer, _, _), (at, mut rays)) in nodes {
            if rays.len() < 2 {
                continue;
            }
            rays.sort_by(|a, b| {
                a.dir[1]
                    .atan2(a.dir[0])
                    .total_cmp(&b.dir[1].atan2(b.dir[0]))
            });
            let n = rays.len();
            for k in 0..n {
                let (Some(r1), Some(r2)) = (rays.get(k), rays.get((k + 1) % n)) else {
                    continue;
                };
                if r1.track == r2.track {
                    continue;
                }
                // r2 lies counter-clockwise of r1, by less than a half turn.
                let angle = cross(r1.dir, r2.dir).atan2(dot(r1.dir, r2.dir));
                if !(angle > 0.0 && angle < below) {
                    continue;
                }
                let (Some(t1), Some(t2)) = (self.tracks.get(r1.track), self.tracks.get(r2.track))
                else {
                    continue;
                };
                // The facing sides: r1's left, r2's right.
                let (n1, n2) = (left(r1.dir), left(r2.dir));
                let w = sub(scale(n2, -r2.half), scale(n1, r1.half));
                let minus = scale(r2.dir, -1.0);
                let denominator = cross(r1.dir, minus);
                if denominator.abs() < 1e-15 {
                    continue;
                }
                let t = cross(w, minus) / denominator;
                let s = cross(r1.dir, w) / denominator;
                if !(0.0..=r1.length).contains(&t) || !(0.0..=r2.length).contains(&s) {
                    continue;
                }
                let apex = add(add(at, scale(n1, r1.half)), scale(r1.dir, t));
                let Some(bisector) = unit(add(r1.dir, r2.dir)) else {
                    continue;
                };
                let probe = add(
                    apex,
                    scale(bisector, 2.0 * JOIN_TOLERANCE_MM / (angle / 2.0).sin()),
                );
                if self.covered(
                    probe,
                    layer,
                    t1.net,
                    &[r1.track, r2.track],
                    None,
                    &mut fills,
                ) {
                    continue;
                }
                out.push(Wedge {
                    layer,
                    net: t1.net,
                    at: apex,
                    angle_deg: angle.to_degrees(),
                    objects: [
                        ObjectRef::Track { id: t1.id },
                        ObjectRef::Track { id: t2.id },
                    ],
                });
            }
        }
        // One wedge per pair of objects (either order): the sharpest, in
        // the order found.
        let same = |a: &Wedge, b: &Wedge| {
            a.layer == b.layer
                && (a.objects == b.objects || a.objects == [b.objects[1], b.objects[0]])
        };
        let mut kept: Vec<Wedge> = Vec::with_capacity(out.len());
        for w in out {
            match kept.iter_mut().find(|k| same(k, &w)) {
                Some(k) if w.angle_deg < k.angle_deg => *k = w,
                Some(_) => {}
                None => kept.push(w),
            }
        }
        kept
    }
}
