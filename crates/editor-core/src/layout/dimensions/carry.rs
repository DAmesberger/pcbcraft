//! Copper carried by a solved part (docs/board-sketch-references.md §5.2):
//! when the parts solve moves or turns a part, the copper attached to its
//! pads goes with it in the same operation, so a parametric edit keeps
//! the board connected.
//!
//! What is attached comes from the copper graph of the document before the
//! operation ([`CopperGraph`]); nothing here decides whether copper joins.
//! The copper is taken island by island: an **island** is a net's tracks
//! and vias joined to one another, apart from the joints a pad makes (two
//! tracks ending on one pad are two islands, each touching that pad).
//!
//! - An island whose pads all move with one motion moves rigidly with
//!   them: the route between two parts that move together keeps its shape.
//! - An island between moved and unmoved pads, when the parts slid without
//!   turning, keeps the direction of every segment, as a dragged track
//!   does ([`super::super::router::drag`]): from each moved pad, a segment
//!   running along the move stretches; else, at a corner where it meets
//!   one other segment end to end, it slides with the pad and its
//!   neighbour stretches along its own line to meet it; else it moves with
//!   the pad, and so does the copper at its far end. A differential pair's
//!   halves, parallel where they leave the part, change alike, so their
//!   coupling and skew are kept.
//! - Otherwise (a turn, two different motions, locked or edited copper,
//!   a move that would reach an unmoved pad or flip a segment) a **carried
//!   point** moves rigidly with its part: a via joined to the part's pads
//!   and no other pad, and a track end on a carried pad or via. The
//!   track's other end stays unless it is attached too, so the segment
//!   stretches and every corner beyond it stays where it was. When
//!   stretching would take the segment off another joint along it (copper
//!   meeting it in mid-span, by [`keeps_joint`]), the segment stays and a
//!   new segment like it joins its old end to the carried one instead.
//!
//! Locked tracks and vias stay, and so does copper the operation itself
//! changed. Selected copper a group drag names (`loose`) that no moved part
//! carries slides with the drag. The carried copper is then handed to the router
//! (`router::reroute::dragged`): what now lands too close to other copper,
//! or no longer reaches its pads, is re-routed with push-and-shove, and
//! what cannot be is removed and left unrouted, in the same operation.
use crate::diagnostics::Result;
use crate::geometry::{Point, distance};
use crate::ids::Id;
use crate::layout::arrange::CopperMove;
use crate::layout::connectivity::{CopperGraph, Meets, TrackEnd, keeps_joint};
use crate::layout::posture::MIN_SEGMENT_MM;
use crate::layout::router::reroute::{self, Rerouted};
use crate::model::{Placement, Project, Track};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// A rigid motion of the plane: turn by `turn_deg` about `from`, then put
/// `from` at `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Motion {
    from: Point,
    to: Point,
    cos: f64,
    sin: f64,
}

/// Two parts' motions within this of each other move copper alike (mm, and
/// the sine and cosine of their turns).
const SAME_MOTION: f64 = 1e-6;

impl Motion {
    /// The motion taking `old`'s frame to `new`'s. The side (and so the
    /// mirror) is the same for both, so it cancels.
    fn between(old: &Placement, new: &Placement) -> Option<Self> {
        if old.layer != new.layer {
            return None;
        }
        let (sin, cos) = (new.rotation_deg - old.rotation_deg).to_radians().sin_cos();
        Some(Self {
            from: old.position,
            to: new.position,
            cos,
            sin,
        })
    }

    fn apply(&self, p: Point) -> Point {
        let (x, y) = (p[0] - self.from[0], p[1] - self.from[1]);
        [
            self.to[0] + self.cos * x - self.sin * y,
            self.to[1] + self.sin * x + self.cos * y,
        ]
    }

    /// Whether `other` moves every point as this does.
    fn same(&self, other: &Self) -> bool {
        let (p, q) = (self.apply(self.from), other.apply(self.from));
        (self.cos - other.cos).abs() <= SAME_MOTION
            && (self.sin - other.sin).abs() <= SAME_MOTION
            && distance(p, q) <= SAME_MOTION
    }

    /// The slide, when the motion does not turn.
    fn slide(&self) -> Option<Point> {
        (self.sin.abs() <= f64::EPSILON && self.cos > 0.0)
            .then(|| [self.to[0] - self.from[0], self.to[1] - self.from[1]])
    }
}

/// Carry the copper attached to `moved` (instances whose pose the parts
/// solve set) from their poses in `before` to those in `project`. `graph`
/// is the copper graph of `before`. `loose` names tracks and vias that
/// slide by its offset unless a moved part carries them. Returns what the
/// router re-routed and what it left unrouted.
pub fn carry(
    before: &Project,
    project: &mut Project,
    graph: &CopperGraph,
    moved: &[Id],
    loose: Option<&CopperMove>,
) -> Result<Rerouted> {
    let mut motions = BTreeMap::new();
    for &instance in moved {
        if let (Some(old), Some(new)) = (
            before.placement_of(instance),
            project.placement_of(instance),
        ) && (old.position != new.position || old.rotation_deg != new.rotation_deg)
            && let Some(m) = Motion::between(old, new)
        {
            motions.insert(instance, m);
        }
    }
    let loose = loose.filter(|c| c.by != [0.0, 0.0]);
    if motions.is_empty() && loose.is_none() {
        return Ok(Rerouted::default());
    }
    let pad_motion = |index: usize| {
        graph
            .pads()
            .get(index)
            .and_then(|p| motions.get(&p.instance))
            .copied()
    };
    // Unlocked and as the operation found it.
    let free_tracks: Vec<bool> = {
        let now: BTreeMap<Id, &Track> = project.tracks.iter().map(|t| (t.id, t)).collect();
        before
            .tracks
            .iter()
            .map(|old| !old.locked && now.get(&old.id).copied() == Some(old))
            .collect()
    };
    let free_vias: Vec<bool> = before
        .vias
        .iter()
        .map(|old| !old.locked && project.via(old.id) == Some(old))
        .collect();
    let free_track = |k: usize| free_tracks.get(k).copied().unwrap_or(false);
    let free_via = |v: usize| free_vias.get(v).copied().unwrap_or(false);
    let mut tracks: BTreeMap<usize, (Point, Point)> = BTreeMap::new();
    let mut vias: BTreeMap<usize, Point> = BTreeMap::new();
    let mut stretched = Islands::default();
    // Copper a moved part carries; the rest of `loose` slides by itself.
    let mut carried = Islands::default();
    for island in islands(before, graph) {
        let pads: Vec<Option<Motion>> = island.pads.iter().map(|&p| pad_motion(p)).collect();
        let Some(m) = pads.iter().flatten().next().copied() else {
            continue;
        };
        carried.tracks.extend(&island.tracks);
        carried.vias.extend(&island.vias);
        let alike = pads.iter().flatten().all(|n| n.same(&m))
            && island.tracks.iter().all(|&k| free_track(k))
            && island.vias.iter().all(|&v| free_via(v));
        let done = alike
            && if pads.iter().all(Option::is_some) {
                rigid(before, &island, &m, &mut tracks, &mut vias);
                true
            } else {
                m.slide().is_some_and(|d| {
                    follow(before, graph, &island, &|p| pad_motion(p).is_some(), d)
                        .map(|(t, v)| {
                            tracks.extend(t);
                            vias.extend(v);
                        })
                        .is_some()
                })
            };
        if !done {
            stretched.tracks.extend(island.tracks);
            stretched.vias.extend(island.vias);
        }
    }
    let (edits, via_motion) = stretch(before, graph, &pad_motion, &stretched);
    for (index, m) in via_motion {
        if let Some(old) = before.vias.get(index) {
            vias.insert(index, m.apply(old.position));
        }
    }
    if let Some(loose) = loose {
        let slide = |p: Point| add(p, loose.by);
        for (k, t) in before.tracks.iter().enumerate() {
            if loose.tracks.contains(&t.id) && !carried.tracks.contains(&k) {
                tracks.insert(k, (slide(t.start), slide(t.end)));
            }
        }
        for (v, via) in before.vias.iter().enumerate() {
            if loose.vias.contains(&via.id) && !carried.vias.contains(&v) {
                vias.insert(v, slide(via.position));
            }
        }
    }
    let dragged_vias = move_vias(before, project, &vias, &free_via);
    let dragged = move_tracks(before, project, &tracks, &edits, &free_track)?;
    let moved = motions.keys().copied().collect();
    reroute::dragged(before, graph, project, &moved, &dragged, &dragged_vias)
}

/// One island's copper by index into the board's tracks and vias, and
/// the pads (by index into [`CopperGraph::pads`]) it touches.
#[derive(Default)]
struct Islands {
    tracks: BTreeSet<usize>,
    vias: BTreeSet<usize>,
    pads: BTreeSet<usize>,
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while let Some(&p) = parent.get(i) {
        if p == i {
            break;
        }
        let up = parent.get(p).copied().unwrap_or(p);
        if let Some(slot) = parent.get_mut(i) {
            *slot = up;
        }
        i = up;
    }
    i
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (a, b) = (find(parent, a), find(parent, b));
    if let Some(slot) = parent.get_mut(a.max(b)) {
        *slot = a.min(b);
    }
}

/// The islands of `before`'s copper: tracks (`0..n`) and vias (`n..`)
/// joined other than at a pad.
fn islands(before: &Project, graph: &CopperGraph) -> Vec<Islands> {
    let n = before.tracks.len();
    let mut parent: Vec<usize> = (0..n + before.vias.len()).collect();
    let mut pads: Vec<(usize, usize)> = Vec::new();
    for k in 0..n {
        let joints = graph.joints_on(k);
        // An end on a pad is joined through the pad, not to what else
        // ends there.
        let on_pad = |end: Option<TrackEnd>| {
            end.is_some()
                && joints
                    .iter()
                    .any(|j| j.end == end && matches!(j.meets, Meets::Pad { .. }))
        };
        for j in &joints {
            match j.meets {
                Meets::Pad { index } => pads.push((k, index)),
                _ if on_pad(j.end) => {}
                Meets::Track { index, .. } => union(&mut parent, k, index),
                Meets::Via { index } => union(&mut parent, k, n + index),
                Meets::Fill => {}
            }
        }
    }
    for v in 0..before.vias.len() {
        for meets in graph.joints_of_via(v) {
            if let Meets::Pad { index } = meets {
                pads.push((n + v, index));
            }
        }
    }
    let mut out: BTreeMap<usize, Islands> = BTreeMap::new();
    for node in 0..parent.len() {
        let island = out.entry(find(&mut parent, node)).or_default();
        if node < n {
            island.tracks.insert(node);
        } else {
            island.vias.insert(node - n);
        }
    }
    for (node, pad) in pads {
        if let Some(island) = out.get_mut(&find(&mut parent, node)) {
            island.pads.insert(pad);
        }
    }
    out.into_values().filter(|i| !i.pads.is_empty()).collect()
}

/// An island between pads that all move with `m` moves with them.
fn rigid(
    before: &Project,
    island: &Islands,
    m: &Motion,
    tracks: &mut BTreeMap<usize, (Point, Point)>,
    vias: &mut BTreeMap<usize, Point>,
) {
    for &k in &island.tracks {
        if let Some(t) = before.tracks.get(k) {
            tracks.insert(k, (m.apply(t.start), m.apply(t.end)));
        }
    }
    for &v in &island.vias {
        if let Some(via) = before.vias.get(v) {
            vias.insert(v, m.apply(via.position));
        }
    }
}

/// What one track of an island between moved and unmoved pads does.
#[derive(Clone, Copy)]
enum Fate {
    /// Slides with the pads.
    Slide,
    /// Its `end` slides along its own line; the other stays.
    Stretch(TrackEnd),
    /// Its `end` slides with the pads and its other end goes to `at`, on
    /// the line of the neighbour that meets it there.
    Bend(TrackEnd, Point),
    /// The neighbour of a bend: its `end` goes to `at` along its line.
    Bent(TrackEnd, Point),
}

fn other(end: TrackEnd) -> TrackEnd {
    match end {
        TrackEnd::Start => TrackEnd::End,
        TrackEnd::End => TrackEnd::Start,
    }
}

fn ends(t: &Track, end: TrackEnd) -> (Point, Point) {
    match end {
        TrackEnd::Start => (t.start, t.end),
        TrackEnd::End => (t.end, t.start),
    }
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}

fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// Whether `from → to` runs the way `along` does for at least
/// [`MIN_SEGMENT_MM`].
fn keeps_way(from: Point, to: Point, along: Point) -> bool {
    let length = distance([0.0, 0.0], along);
    length > 0.0
        && dot(sub(to, from), along) / length >= MIN_SEGMENT_MM
        && cross(sub(to, from), along).abs() <= 1e-9 * length.max(1.0) * distance(from, to).max(1.0)
}

/// Lines parallel within this (the sine of the angle between them).
const PARALLEL: f64 = 1e-9;

/// The island's copper with its moved pads slid by `d`, keeping every
/// segment's direction, or `None` when it cannot: new track ends and via
/// positions by index.
#[allow(clippy::type_complexity)]
fn follow(
    before: &Project,
    graph: &CopperGraph,
    island: &Islands,
    moved: &dyn Fn(usize) -> bool,
    d: Point,
) -> Option<(BTreeMap<usize, (Point, Point)>, BTreeMap<usize, Point>)> {
    let mut fate: BTreeMap<usize, Fate> = BTreeMap::new();
    let mut slid: BTreeSet<usize> = BTreeSet::new();
    let mut queue: VecDeque<(usize, TrackEnd)> = VecDeque::new();
    // What meets a point that slides slides too: an unmoved pad, or a
    // track's mid-span, cannot. Other islands' copper on a moved pad is
    // their own.
    let reach = |meets: Meets,
                 queue: &mut VecDeque<(usize, TrackEnd)>,
                 vias: &mut Vec<usize>|
     -> Option<()> {
        match meets {
            Meets::Pad { index } => moved(index).then_some(()),
            Meets::Via { index } if island.vias.contains(&index) => {
                vias.push(index);
                Some(())
            }
            Meets::Track {
                index,
                end: Some(end),
                ..
            } if island.tracks.contains(&index) => {
                queue.push_back((index, end));
                Some(())
            }
            Meets::Track {
                index, end: None, ..
            } if island.tracks.contains(&index) => None,
            Meets::Track { .. } | Meets::Via { .. } | Meets::Fill => Some(()),
        }
    };
    let mut pending: Vec<usize> = Vec::new();
    for &k in &island.tracks {
        for j in graph.joints_on(k) {
            if let (Some(end), Meets::Pad { index }) = (j.end, j.meets)
                && moved(index)
            {
                queue.push_back((k, end));
            }
        }
    }
    for &v in &island.vias {
        if graph
            .joints_of_via(v)
            .iter()
            .any(|m| matches!(m, Meets::Pad { index } if moved(*index)))
        {
            pending.push(v);
        }
    }
    let budget = 4 * (island.tracks.len() + island.vias.len()) + 16;
    let mut steps = 0;
    loop {
        while let Some(v) = pending.pop() {
            if slid.insert(v) {
                for meets in graph.joints_of_via(v) {
                    reach(meets, &mut queue, &mut pending)?;
                }
            }
        }
        let Some((k, end)) = queue.pop_front() else {
            break;
        };
        steps += 1;
        if steps > budget {
            return None;
        }
        let t = before.tracks.get(k)?;
        let joints = graph.joints_on(k);
        let slide_all = |queue: &mut VecDeque<(usize, TrackEnd)>, pending: &mut Vec<usize>| {
            joints
                .iter()
                .try_for_each(|j| reach(j.meets, queue, pending))
        };
        match fate.get(&k).copied() {
            Some(Fate::Slide) => continue,
            Some(Fate::Stretch(e)) if e == end => continue,
            Some(Fate::Stretch(_)) => {
                // Reached from both ends: it slides whole.
                fate.insert(k, Fate::Slide);
                slide_all(&mut queue, &mut pending)?;
                continue;
            }
            Some(Fate::Bend(..) | Fate::Bent(..)) => return None,
            None => {}
        }
        let (near, far) = ends(t, end);
        let along = sub(far, near);
        let length = distance(near, far);
        if length <= 0.0 {
            return None;
        }
        let moved_near = add(near, d);
        // Copper at the sliding end slides with it.
        let at_near: Vec<Meets> = joints
            .iter()
            .filter(|j| j.end == Some(end))
            .map(|j| j.meets)
            .collect();
        let pins: Vec<Point> = joints
            .iter()
            .filter(|j| j.end.is_none() && j.meets != Meets::Fill)
            .map(|j| j.at)
            .collect();
        // A segment running along the move stretches.
        if cross(along, d).abs() <= PARALLEL * length * distance([0.0, 0.0], d)
            && keeps_way(moved_near, far, along)
            && pins.iter().all(|p| keeps_joint(&[moved_near, far], *p))
        {
            fate.insert(k, Fate::Stretch(end));
            for meets in at_near {
                reach(meets, &mut queue, &mut pending)?;
            }
            continue;
        }
        // At a plain corner, the neighbour stretches along its own line to
        // meet this segment slid.
        let at_far: Vec<_> = joints
            .iter()
            .filter(|j| j.end == Some(other(end)) && j.meets != Meets::Fill)
            .collect();
        let bend = match at_far.as_slice() {
            [j] if j.end_to_end && pins.is_empty() => match j.meets {
                Meets::Track {
                    index: n,
                    end: Some(n_end),
                    ..
                } if n != k && !fate.contains_key(&n) => {
                    before.tracks.get(n).and_then(|nt| {
                        let (corner, n_far) = ends(nt, n_end);
                        let n_along = sub(n_far, corner);
                        let denom = cross(along, n_along);
                        if nt.layer != t.layer
                            || denom.abs() <= PARALLEL * length * distance(corner, n_far)
                        {
                            return None;
                        }
                        // moved_near + s·along = n_far + u·n_along.
                        let s = cross(sub(n_far, moved_near), n_along) / denom;
                        let at = [moved_near[0] + s * along[0], moved_near[1] + s * along[1]];
                        let n_pins: Vec<Point> = graph
                            .joints_on(n)
                            .iter()
                            .filter(|j| j.end.is_none() && j.meets != Meets::Fill)
                            .map(|j| j.at)
                            .collect();
                        (keeps_way(moved_near, at, along)
                            && keeps_way(at, n_far, n_along)
                            && n_pins.iter().all(|p| keeps_joint(&[at, n_far], *p)))
                        .then_some((n, n_end, at))
                    })
                }
                _ => None,
            },
            _ => None,
        };
        if let Some((n, n_end, at)) = bend {
            fate.insert(k, Fate::Bend(end, at));
            fate.insert(n, Fate::Bent(n_end, at));
            for meets in at_near {
                reach(meets, &mut queue, &mut pending)?;
            }
            continue;
        }
        // Else it slides whole, and the copper at its far end with it.
        fate.insert(k, Fate::Slide);
        slide_all(&mut queue, &mut pending)?;
    }
    let mut tracks = BTreeMap::new();
    for (k, f) in fate {
        let t = before.tracks.get(k)?;
        let set = |end: TrackEnd, p: Point, (s, e): (Point, Point)| match end {
            TrackEnd::Start => (p, e),
            TrackEnd::End => (s, p),
        };
        let now = (t.start, t.end);
        let ends = match f {
            Fate::Slide => (add(t.start, d), add(t.end, d)),
            Fate::Stretch(end) => set(end, add(ends(t, end).0, d), now),
            Fate::Bend(end, at) => set(other(end), at, set(end, add(ends(t, end).0, d), now)),
            Fate::Bent(end, at) => set(end, at, now),
        };
        tracks.insert(k, ends);
    }
    let vias = slid
        .into_iter()
        .filter_map(|v| before.vias.get(v).map(|via| (v, add(via.position, d))))
        .collect();
    Some((tracks, vias))
}

/// What carrying does to one track of `before`, by index, when its end
/// follows a carried point.
enum Edit {
    /// Its ends go to `to`.
    Stretch { track: usize, to: (Point, Point) },
    /// It stays; a segment like it joins `from`, its old end, to `to`.
    Join {
        track: usize,
        from: Point,
        to: Point,
    },
}

/// The carried points of the `islands` copper: track edits, and the vias
/// that move rigidly (by index) with their motions.
fn stretch(
    before: &Project,
    graph: &CopperGraph,
    pad_motion: &dyn Fn(usize) -> Option<Motion>,
    islands: &Islands,
) -> (Vec<Edit>, BTreeMap<usize, Motion>) {
    // Vias joined to pads of exactly one carried part, and to no other pad.
    let mut via_motion = graph.carried_vias(pad_motion);
    via_motion.retain(|v, _| islands.vias.contains(v));
    // Track ends joined to a carried pad or via.
    let moving = |meets: Meets| match meets {
        Meets::Pad { index } => pad_motion(index),
        Meets::Via { index } => via_motion.get(&index).copied(),
        _ => None,
    };
    let mut edits = Vec::new();
    for (index, old) in before.tracks.iter().enumerate() {
        if !islands.tracks.contains(&index) {
            continue;
        }
        let joints = graph.joints_on(index);
        // Per end: the one motion it follows (two different motions: none).
        let mut ends: [Option<Option<Motion>>; 2] = [None, None];
        for joint in &joints {
            let (Some(end), Some(m)) = (joint.end, moving(joint.meets)) else {
                continue;
            };
            let slot = &mut ends[usize::from(end == TrackEnd::End)];
            *slot = match *slot {
                None => Some(Some(m)),
                Some(Some(f)) if f == m => Some(Some(m)),
                _ => Some(None),
            };
        }
        let [start, end] = ends.map(Option::flatten);
        if start.is_none() && end.is_none() {
            continue;
        }
        let to = (
            start.map_or(old.start, |m| m.apply(old.start)),
            end.map_or(old.end, |m| m.apply(old.end)),
        );
        // One end carried: the segment stretches when it keeps every other
        // joint on its centre line; otherwise it stays and a new segment
        // joins its old end to the new one, so its corner is kept.
        let carried = match (start, end) {
            (Some(_), None) => Some(TrackEnd::Start),
            (None, Some(_)) => Some(TrackEnd::End),
            _ => None,
        };
        let keeps = carried.is_none_or(|c| {
            joints
                .iter()
                .filter(|j| j.end != Some(c) && moving(j.meets).is_none())
                .all(|j| keeps_joint(&[to.0, to.1], j.at))
        });
        edits.push(if keeps {
            Edit::Stretch { track: index, to }
        } else {
            let (from, to) = if carried == Some(TrackEnd::Start) {
                (old.start, to.0)
            } else {
                (old.end, to.1)
            };
            Edit::Join {
                track: index,
                from,
                to,
            }
        });
    }
    (edits, via_motion)
}

fn move_vias(
    before: &Project,
    project: &mut Project,
    moves: &BTreeMap<usize, Point>,
    free: &dyn Fn(usize) -> bool,
) -> BTreeSet<Id> {
    let moves: BTreeMap<Id, Point> = moves
        .iter()
        .filter(|(v, _)| free(**v))
        .filter_map(|(v, to)| before.vias.get(*v).map(|old| (old.id, *to)))
        .collect();
    for via in project.vias.iter_mut() {
        if let Some(to) = moves.get(&via.id) {
            via.position = *to;
        }
    }
    moves.into_keys().collect()
}

fn move_tracks(
    before: &Project,
    project: &mut Project,
    moved: &BTreeMap<usize, (Point, Point)>,
    edits: &[Edit],
    free: &dyn Fn(usize) -> bool,
) -> Result<BTreeSet<Id>> {
    // Locked tracks, and tracks the operation itself changed, stay.
    let id = |index: usize| before.tracks.get(index).filter(|_| free(index));
    let mut ends: BTreeMap<Id, (Point, Point)> = moved
        .iter()
        .filter_map(|(k, to)| id(*k).map(|old| (old.id, *to)))
        .collect();
    let mut joins = Vec::new();
    for edit in edits {
        match *edit {
            Edit::Stretch { track, to } => {
                if let Some(old) = id(track) {
                    ends.insert(old.id, to);
                }
            }
            Edit::Join { track, from, to } => {
                if let Some(old) = id(track) {
                    joins.push(Track {
                        start: from,
                        end: to,
                        ..old.clone()
                    });
                }
            }
        }
    }
    if !ends.is_empty() {
        for track in project.tracks.iter_mut() {
            if let Some((start, end)) = ends.get(&track.id) {
                track.start = *start;
                track.end = *end;
            }
        }
    }
    let mut dragged: BTreeSet<Id> = ends.into_keys().collect();
    for mut track in joins {
        track.id = project.allocate()?;
        track.locked = false;
        dragged.insert(track.id);
        project.tracks.push(track);
    }
    Ok(dragged)
}
