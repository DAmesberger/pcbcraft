//! Re-routing copper a moved part dragged along (docs/push-and-shove.md).
//!
//! When parts move, whether by `layout.move` or because a board parameter
//! or a dimension moved them, [`super::super::dimensions::carry`] carries
//! the copper on their pads, then calls [`dragged`] in the same operation,
//! so the re-route is in the same undo step:
//!
//! 1. Re-routed is carried copper too close to other copper or a keepout,
//!    or off a pad it joined; unlocked copper left in place that the moved
//!    parts now come too close to; and, when the pair now breaks its
//!    class's limits, a pair half's track carried out of coupling with its
//!    partner, with the partner's tracks beside it.
//! 2. A **connection** is one net's such copper joined into one piece (by
//!    the copper graph); a pair's two connections are routed together.
//! 3. First only the offending copper is removed and the places it met are
//!    joined again, nearest first, one route step each (Shove), so the
//!    parts of the route that are still fine stay; a pair's two gaps are
//!    one pair step ([`super::pair_plan`]) at the pair's width and gap.
//!    When that fails, all of the connection's carried copper is removed
//!    and routed again the same way.
//! 4. A connection that offends a rule and cannot be routed has its
//!    carried copper removed: it is left unrouted (an airwire), never a
//!    clearance error. A pair re-routed only for coupling keeps the new
//!    route only when it comes out better.
//!
//! Connections are taken in order of their smallest identity and every
//! attempt is a pure function of the board, so the same edit gives the
//! same result.
use super::world::{Ctx, Piece, State, Target};
use super::{RouteIndex, RouteMode, RouteStepRequest, pair_plan, plan};
use crate::diagnostics::Result;
use crate::geometry::{Point, distance};
use crate::ids::{Id, PadId};
use crate::layout::connectivity::{CopperGraph, Meets};
use crate::layout::length;
use crate::layout::pair::{self, PairHead, PairStepRequest};
use crate::layout::posture::Posture;
use crate::layout::{RouteEditSet, apply_routes};
use crate::model::{CopperLayer, Project};
use crate::rules::signal::DiffPairId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// What re-routing the carried copper did: the nets of the connections
/// it re-routed and of those it left unrouted, one entry per connection.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rerouted {
    pub rerouted: Vec<String>,
    pub unrouted: Vec<String>,
}

impl Rerouted {
    pub fn is_empty(&self) -> bool {
        self.rerouted.is_empty() && self.unrouted.is_empty()
    }

    /// One line for a toast: "Re-routed 6 connections; 1 left unrouted
    /// (USB_DP)", "Re-routed 2 connections" or "1 connection left unrouted
    /// (GND)". `None` when nothing was re-routed or left unrouted.
    pub fn line(&self) -> Option<String> {
        let count = |n: usize| {
            if n == 1 {
                "1 connection".to_owned()
            } else {
                format!("{n} connections")
            }
        };
        let names: BTreeSet<&str> = self.unrouted.iter().map(String::as_str).collect();
        let names = names.into_iter().collect::<Vec<_>>().join(", ");
        match (self.rerouted.len(), self.unrouted.len()) {
            (0, 0) => None,
            (r, 0) => Some(format!("Re-routed {}", count(r))),
            (0, u) => Some(format!("{} left unrouted ({names})", count(u))),
            (r, u) => Some(format!(
                "Re-routed {}; {u} left unrouted ({names})",
                count(r)
            )),
        }
    }

    /// This report followed by `later`'s (a batch's steps).
    pub fn extend(&mut self, later: Rerouted) {
        self.rerouted.extend(later.rerouted);
        self.unrouted.extend(later.unrouted);
    }
}

/// One gap to route: between `a` and `b` on `layer`, at `width`.
#[derive(Clone, Copy, Debug)]
struct Gap {
    net: Id,
    layer: CopperLayer,
    width: f64,
    a: Point,
    b: Point,
}

/// The connections (sets of track and via identities) of the carried
/// copper on `project`, in order of their smallest identity.
fn connections(
    project: &Project,
    graph: &CopperGraph,
    tracks: &BTreeSet<Id>,
    vias: &BTreeSet<Id>,
) -> Vec<(BTreeSet<Id>, BTreeSet<Id>)> {
    let n = project.tracks.len();
    let mut parent: Vec<usize> = (0..n + project.vias.len()).collect();
    let find = |parent: &mut Vec<usize>, mut i: usize| {
        while let Some(&p) = parent.get(i) {
            if p == i {
                break;
            }
            i = p;
        }
        i
    };
    let carried_track = |k: usize| {
        project
            .tracks
            .get(k)
            .is_some_and(|t| tracks.contains(&t.id))
    };
    let carried_via = |v: usize| project.vias.get(v).is_some_and(|x| vias.contains(&x.id));
    for k in (0..n).filter(|k| carried_track(*k)) {
        for j in graph.joints_on(k) {
            let other = match j.meets {
                Meets::Track { index, .. } if carried_track(index) => index,
                Meets::Via { index } if carried_via(index) => n + index,
                _ => continue,
            };
            let (a, b) = (find(&mut parent, k), find(&mut parent, other));
            if let Some(slot) = parent.get_mut(a.max(b)) {
                *slot = a.min(b);
            }
        }
    }
    let mut out: BTreeMap<usize, (BTreeSet<Id>, BTreeSet<Id>)> = BTreeMap::new();
    for k in (0..n).filter(|k| carried_track(*k)) {
        let root = find(&mut parent, k);
        if let Some(t) = project.tracks.get(k) {
            out.entry(root).or_default().0.insert(t.id);
        }
    }
    for v in (0..project.vias.len()).filter(|v| carried_via(*v)) {
        let root = find(&mut parent, n + v);
        if let Some(x) = project.vias.get(v) {
            out.entry(root).or_default().1.insert(x.id);
        }
    }
    let mut out: Vec<_> = out.into_values().collect();
    out.sort_by_key(|(t, v)| t.first().or(v.first()).copied());
    out
}

/// The pads (instance and pad) each track of `project` joins, by identity.
fn pads_joined(project: &Project, graph: &CopperGraph) -> BTreeMap<Id, BTreeSet<(Id, PadId)>> {
    let mut out: BTreeMap<Id, BTreeSet<(Id, PadId)>> = BTreeMap::new();
    for (k, t) in project.tracks.iter().enumerate() {
        for j in graph.joints_on(k) {
            if let Meets::Pad { index } = j.meets
                && let Some(pad) = graph.pads().get(index)
            {
                out.entry(t.id).or_default().insert((pad.instance, pad.pad));
            }
        }
    }
    out
}

/// Each pair's coupled track couples (positive, negative identities).
fn coupled(project: &Project) -> BTreeSet<(Id, Id)> {
    project
        .rules
        .diff_pairs
        .iter()
        .flat_map(|pair| length::coupling(project, pair).coupled_runs)
        .map(|(p, n, ..)| (p, n))
        .collect()
}

/// The carried copper to re-route, by identity: `hard` is what offends a
/// rule (too close to other copper or a keepout, or off a pad it joined
/// before), the rest a pair half's track that ran coupled with its partner
/// before and no longer does, with the partner's tracks it ran beside.
struct Offending {
    tracks: BTreeSet<Id>,
    vias: BTreeSet<Id>,
    hard: BTreeSet<Id>,
}

fn offending(
    before: (&Project, &CopperGraph),
    project: &Project,
    index: &RouteIndex,
    tracks: &BTreeSet<Id>,
    vias: &BTreeSet<Id>,
) -> Offending {
    let ctx = Ctx::new(project, index);
    let state = State::default();
    // The router's `hits`, which the engine's checks agree with.
    let collides = |piece: &Piece| {
        ctx.hits(&state, piece, &[piece.net])
            .iter()
            .any(|h| matches!(h.target, Target::Base(_) | Target::Keepout(_)))
    };
    let had = pads_joined(before.0, before.1);
    let has = pads_joined(project, ctx.graph());
    let detached = |id: Id| {
        had.get(&id)
            .is_some_and(|pads| has.get(&id).is_none_or(|now| !pads.is_subset(now)))
    };
    let mut hard: BTreeSet<Id> = project
        .tracks
        .iter()
        .filter(|t| tracks.contains(&t.id))
        .filter(|t| {
            detached(t.id) || collides(&Piece::track(t.net, t.layer, t.start, t.end, t.width))
        })
        .map(|t| t.id)
        .collect();
    let bad_vias: BTreeSet<Id> = project
        .vias
        .iter()
        .filter(|v| vias.contains(&v.id))
        .filter(|v| collides(&Piece::via(v, &ctx.stack)))
        .map(|v| v.id)
        .collect();
    let was_coupled = coupled(before.0);
    let is_coupled = coupled(project);
    let half_of = |set: &BTreeSet<(Id, Id)>, id: Id| set.iter().any(|(p, n)| *p == id || *n == id);
    let mut bad: BTreeSet<Id> = tracks
        .iter()
        .copied()
        .filter(|id| {
            hard.contains(id) || (half_of(&was_coupled, *id) && !half_of(&is_coupled, *id))
        })
        .collect();
    // A pair's halves are re-routed alike: the partner's unlocked track
    // that ran beside a re-routed one goes with it, carried or not.
    let unlocked: BTreeSet<Id> = project
        .tracks
        .iter()
        .filter(|t| !t.locked)
        .map(|t| t.id)
        .collect();
    loop {
        let more: Vec<Id> = was_coupled
            .iter()
            .flat_map(|(p, n)| [(*p, *n), (*n, *p)])
            .filter(|(a, b)| bad.contains(a) && !bad.contains(b) && unlocked.contains(b))
            .map(|(_, b)| b)
            .collect();
        if more.is_empty() {
            break;
        }
        bad.extend(more);
    }
    hard.extend(bad_vias.iter().copied());
    Offending {
        tracks: bad,
        vias: bad_vias,
        hard,
    }
}

/// The gaps that removing `tracks` and `vias` from `project` leaves: each
/// piece of them joined together, between the copper it meets, nearest
/// first. `None` when a piece's places are on layers no step joins, or a
/// removed via joins other copper.
fn gaps(
    project: &Project,
    graph: &CopperGraph,
    tracks: &BTreeSet<Id>,
    vias: &BTreeSet<Id>,
) -> Option<Vec<Gap>> {
    // What stays joined once the copper is removed needs no gap.
    let rest = CopperGraph::of(&without(project, tracks, vias));
    let mut out = Vec::new();
    for (piece, piece_vias) in connections(project, graph, tracks, vias) {
        // The places the piece meets, one per pad or via, with the layer
        // and width of the track that meets it.
        let mut ends: Vec<(Place, Point, CopperLayer, f64)> = Vec::new();
        let mut net = None;
        for (k, t) in project.tracks.iter().enumerate() {
            if !piece.contains(&t.id) {
                continue;
            }
            net = Some(t.net);
            for j in graph.joints_on(k) {
                let place = match j.meets {
                    Meets::Track { index, .. } => project
                        .tracks
                        .get(index)
                        .filter(|o| !piece.contains(&o.id))
                        .map(|_| Place::At),
                    Meets::Via { index } => project
                        .vias
                        .get(index)
                        .filter(|o| !piece_vias.contains(&o.id))
                        .map(|_| Place::Via(index)),
                    Meets::Pad { index } => Some(Place::Pad(index)),
                    Meets::Fill => None,
                };
                let Some(place) = place else {
                    continue;
                };
                let known = ends.iter().any(|(q, p, l, _)| match place {
                    Place::At => *q == Place::At && *l == t.layer && distance(*p, j.at) < 1e-6,
                    _ => *q == place,
                });
                if !known {
                    ends.push((place, j.at, t.layer, t.width));
                }
            }
        }
        // A removed via that joins copper other than the piece's tracks.
        for (v, via) in project.vias.iter().enumerate() {
            if !piece_vias.contains(&via.id) {
                continue;
            }
            let outside = graph.joints_of_via(v).iter().any(|m| match *m {
                Meets::Track { index, .. } => project
                    .tracks
                    .get(index)
                    .is_none_or(|o| !piece.contains(&o.id)),
                Meets::Fill => false,
                _ => true,
            });
            if outside {
                return None;
            }
        }
        let Some(net) = net else {
            continue;
        };
        // Join the places nearest first (a spanning tree on one layer)
        // where the copper left does not join them already; copper leading
        // nowhere, or to one place only, is just removed.
        let islands: Vec<BTreeSet<usize>> = ends
            .iter()
            .map(|(_, at, layer, _)| rest.islands_at(*at, *layer, net).into_iter().collect())
            .collect();
        let mut joined: BTreeSet<usize> = BTreeSet::new();
        let mut reached = vec![false; ends.len()];
        let reach = |i: usize, joined: &mut BTreeSet<usize>, reached: &mut Vec<bool>| {
            if let Some(found) = islands.get(i) {
                joined.extend(found.iter().copied());
            }
            for (k, found) in islands.iter().enumerate() {
                if (k == i || found.iter().any(|x| joined.contains(x)))
                    && let Some(slot) = reached.get_mut(k)
                {
                    *slot = true;
                }
            }
        };
        if !ends.is_empty() {
            reach(0, &mut joined, &mut reached);
        }
        while reached.iter().any(|r| !r) {
            let mut best: Option<(f64, usize, usize)> = None;
            for (i, (_, a, la, _)) in ends.iter().enumerate() {
                if !reached.get(i).copied().unwrap_or(false) {
                    continue;
                }
                for (j, (_, b, lb, _)) in ends.iter().enumerate() {
                    let d = distance(*a, *b);
                    if !reached.get(j).copied().unwrap_or(true)
                        && la == lb
                        && best.is_none_or(|(e, ..)| d < e)
                    {
                        best = Some((d, i, j));
                    }
                }
            }
            let (_, i, j) = best?;
            let (Some(from), Some(to)) = (ends.get(i), ends.get(j)) else {
                return None;
            };
            out.push(Gap {
                net,
                layer: from.2,
                width: to.3,
                a: from.1,
                b: to.1,
            });
            reach(j, &mut joined, &mut reached);
        }
    }
    Some(out)
}

/// What a removed piece of copper meets: a pad or via (by index), or a
/// kept track at a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Pad(usize),
    Via(usize),
    At,
}

fn without(project: &Project, tracks: &BTreeSet<Id>, vias: &BTreeSet<Id>) -> Project {
    let mut next = project.clone();
    next.tracks.retain(|t| !tracks.contains(&t.id));
    next.vias.retain(|v| !vias.contains(&v.id));
    next
}

/// `edits` applied to a copy of `project` as `layout.route_apply` applies
/// them, or `None` when the engine refuses them.
fn applied(project: &Project, edits: RouteEditSet) -> Option<Project> {
    if edits.add.is_empty() && edits.replace.is_empty() {
        return None;
    }
    let mut next = project.clone();
    apply_routes(&mut next, edits).ok()?;
    Some(next)
}

/// One gap routed by a route step, from either end with either elbow.
fn route_gap(project: &Project, gap: &Gap) -> Option<Project> {
    let index = RouteIndex::of(project);
    for (head, to) in [(gap.a, gap.b), (gap.b, gap.a)] {
        for flipped in [false, true] {
            let request = RouteStepRequest {
                net: gap.net,
                width: gap.width,
                layer: gap.layer,
                head,
                to,
                posture: Posture::Diagonal,
                flipped,
                via: None,
                via_layer: None,
                mode: RouteMode::Shove,
                move_vias: true,
                max_detour: None,
            };
            let step = plan(project, &index, &request);
            if step.reached
                && let Some(next) = applied(project, step.edits)
            {
                return Some(next);
            }
        }
    }
    None
}

/// Two gaps of a pair's halves `[positive, negative]` routed as one pair
/// step, fanning out of one end and into the other.
fn route_pair_gap(project: &Project, nets: [Id; 2], halves: [&Gap; 2]) -> Option<Project> {
    let [p, n] = halves;
    if p.layer != n.layer {
        return None;
    }
    // Pair the negative half's ends with the positive half's nearer ones.
    let (na, nb) =
        if distance(p.a, n.a) + distance(p.b, n.b) <= distance(p.a, n.b) + distance(p.b, n.a) {
            (n.a, n.b)
        } else {
            (n.b, n.a)
        };
    let index = RouteIndex::of(project);
    for (from, to) in [([p.a, na], [p.b, nb]), ([p.b, nb], [p.a, na])] {
        let mut style = pair::style(project, nets[0], p.layer);
        style.fan_width = pair::fan_width(project, nets[0], from, style.width);
        for flipped in [false, true] {
            let request = PairStepRequest {
                nets,
                layer: p.layer,
                style,
                posture: Posture::Diagonal,
                flipped,
                head: PairHead::at(from[0], from[1]),
                last: None,
                via: None,
                to: [0.5 * (to[0][0] + to[1][0]), 0.5 * (to[0][1] + to[1][1])],
                finish: Some(to),
            };
            let step = pair_plan(project, &index, &request, RouteMode::Shove, true);
            if step.reached
                && let Some(next) = applied(project, step.step.edits)
            {
                return Some(next);
            }
        }
    }
    None
}

/// Route `gaps` one after another on `project`; the pair's two halves,
/// when `pair` names them and each has one gap, as one pair step.
fn route_gaps(project: Project, gaps: &[Gap], pair: Option<[Id; 2]>) -> Option<Project> {
    if let Some(nets) = pair
        && let [a, b] = gaps
        && a.net != b.net
    {
        let halves = if a.net == nets[0] { [a, b] } else { [b, a] };
        return route_pair_gap(&project, nets, halves);
    }
    gaps.iter()
        .try_fold(project, |project, gap| route_gap(&project, gap))
}

/// Pair `id`'s row in the length report of `project`.
fn pair_row(project: &Project, id: DiffPairId) -> Option<length::PairRow> {
    length::report(project)
        .pairs
        .into_iter()
        .find(|r| r.pair == id)
}

/// Whether a pair breaks its class's uncoupled-length or skew limit.
fn breaks_limits(row: &length::PairRow) -> bool {
    row.max_uncoupled.is_some_and(|max| row.uncoupled > max)
        || row.max_skew.is_some_and(|max| row.skew > max)
}

/// Whether pair `id` runs less uncoupled on `after` than on `before`,
/// with no more skew than its class allows or it had.
fn better(before: &length::PairRow, after: &Project, id: DiffPairId) -> bool {
    pair_row(after, id).is_some_and(|b| {
        b.uncoupled < before.uncoupled
            && (b.skew <= before.skew || b.max_skew.is_some_and(|max| b.skew <= max))
    })
}

/// Connections routed together (by index), the pair's `[positive,
/// negative]` nets when they are its halves, and their net names.
type Job = (Vec<usize>, Option<[Id; 2]>, Vec<String>);

/// Re-route the copper the carry dragged (`tracks` and `vias` on
/// `project`) where it now offends, and the unlocked tracks the `moved`
/// parts' pads now come too close to; `before` is the board before the
/// operation and `graph` its copper graph.
pub(crate) fn dragged(
    before: &Project,
    graph: &CopperGraph,
    project: &mut Project,
    moved: &BTreeSet<Id>,
    tracks: &BTreeSet<Id>,
    vias: &BTreeSet<Id>,
) -> Result<Rerouted> {
    let mut report = Rerouted::default();
    if moved.is_empty() {
        return Ok(report);
    }
    let now = Arc::new(CopperGraph::of(project));
    let index = RouteIndex::with_graph(project, now.clone());
    // Copper the operation left in place that the moved parts now come
    // too close to (it was clear before) is re-routed too.
    let given = (tracks, vias);
    let mut tracks = tracks.clone();
    let mut vias = vias.clone();
    {
        let then = RouteIndex::of(before);
        let (ctx, was) = (Ctx::new(project, &index), Ctx::new(before, &then));
        let state = State::default();
        let offends = |ctx: &Ctx<'_>, piece: &Piece| {
            ctx.hits(&state, piece, &[piece.net])
                .iter()
                .any(|h| matches!(h.target, Target::Base(_) | Target::Keepout(_)))
        };
        let old: BTreeMap<Id, &crate::model::Track> =
            before.tracks.iter().map(|t| (t.id, t)).collect();
        for t in project
            .tracks
            .iter()
            .filter(|t| !t.locked && !given.0.contains(&t.id))
        {
            let piece = Piece::track(t.net, t.layer, t.start, t.end, t.width);
            if old.get(&t.id).is_some_and(|o| *o == t)
                && offends(&ctx, &piece)
                && !offends(&was, &piece)
            {
                tracks.insert(t.id);
            }
        }
        for v in project
            .vias
            .iter()
            .filter(|v| !v.locked && !given.1.contains(&v.id))
        {
            let piece = Piece::via(v, &ctx.stack);
            if before.via(v.id) == Some(v) && offends(&ctx, &piece) && !offends(&was, &piece) {
                vias.insert(v.id);
            }
        }
    }
    let vias = &vias;
    let tracks = &tracks;
    if tracks.is_empty() && vias.is_empty() {
        return Ok(report);
    }
    let Offending {
        tracks: bad_tracks,
        vias: bad_vias,
        hard,
    } = offending((before, graph), project, &index, tracks, vias);
    if bad_tracks.is_empty() && bad_vias.is_empty() {
        return Ok(report);
    }
    // The offending connections, a pair's halves together.
    let all = connections(project, &now, tracks, vias);
    let offends = |(t, v): &(BTreeSet<Id>, BTreeSet<Id>)| {
        t.iter().any(|id| bad_tracks.contains(id)) || v.iter().any(|id| bad_vias.contains(id))
    };
    let net_of = |(t, v): &(BTreeSet<Id>, BTreeSet<Id>)| {
        t.first()
            .and_then(|id| project.tracks.iter().find(|x| x.id == *id).map(|x| x.net))
            .or_else(|| {
                v.first()
                    .and_then(|id| project.vias.iter().find(|x| x.id == *id).map(|x| x.net))
            })
    };
    let nets: Vec<Option<Id>> = all.iter().map(net_of).collect();
    let net_of = |i: usize| nets.get(i).copied().flatten();
    let mut done = vec![false; all.len()];
    let mut jobs: Vec<Job> = Vec::new();
    for (i, connection) in all.iter().enumerate() {
        if done.get(i).copied().unwrap_or(true) || !offends(connection) {
            continue;
        }
        let mut members = vec![i];
        let pair = net_of(i)
            .and_then(|net| project.rules.pair_of(net))
            .map(|p| [p.positive, p.negative]);
        if let (Some(nets), Some(net)) = (pair, net_of(i)) {
            let partner = if net == nets[0] { nets[1] } else { nets[0] };
            // The partner's carried connection nearest this one.
            let ends = |c: &(BTreeSet<Id>, BTreeSet<Id>)| -> Vec<Point> {
                project
                    .tracks
                    .iter()
                    .filter(|t| c.0.contains(&t.id))
                    .flat_map(|t| [t.start, t.end])
                    .collect()
            };
            let mine = ends(connection);
            let near = |c: &(BTreeSet<Id>, BTreeSet<Id>)| {
                ends(c)
                    .iter()
                    .flat_map(|p| mine.iter().map(move |q| distance(*p, *q)))
                    .fold(f64::INFINITY, f64::min)
            };
            if let Some((j, _)) = all
                .iter()
                .enumerate()
                .filter(|(j, _)| {
                    !done.get(*j).copied().unwrap_or(true) && *j != i && net_of(*j) == Some(partner)
                })
                .map(|(j, c)| (j, near(c)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
            {
                members.push(j);
            }
        }
        for &m in &members {
            if let Some(slot) = done.get_mut(m) {
                *slot = true;
            }
        }
        let pair = pair.filter(|_| members.len() == 2);
        let names = members
            .iter()
            .filter_map(|m| net_of(*m))
            .map(|net| project.net_name(net).to_owned())
            .collect();
        jobs.push((members, pair, names));
    }
    for (members, pair, names) in jobs {
        let mut all_tracks = BTreeSet::new();
        let mut all_vias = BTreeSet::new();
        let mut job_nets = BTreeSet::new();
        for m in &members {
            if let Some((t, v)) = all.get(*m) {
                all_tracks.extend(t.iter().copied());
                all_vias.extend(v.iter().copied());
            }
            job_nets.extend(net_of(*m));
        }
        // The offending tracks of the job's nets, with the uncarried
        // partner tracks a pair brings along.
        let job_bad: BTreeSet<Id> = project
            .tracks
            .iter()
            .filter(|t| job_nets.contains(&t.net) && bad_tracks.contains(&t.id))
            .map(|t| t.id)
            .collect();
        // The offending copper alone first, then all of the connection's
        // carried copper.
        let tiers: [(BTreeSet<Id>, BTreeSet<Id>); 2] = [
            (job_bad, all_vias.intersection(&bad_vias).copied().collect()),
            (all_tracks.clone(), all_vias.clone()),
        ];
        let offends = all_tracks
            .iter()
            .chain(&all_vias)
            .any(|id| hard.contains(id));
        // Carried out of coupling only: re-routed when the pair now breaks
        // its class's limits, and kept only when it comes out better.
        let carried = job_nets
            .iter()
            .find_map(|net| project.rules.pair_of(*net))
            .and_then(|p| Some((p.id, pair_row(project, p.id)?)));
        if !offends && !carried.as_ref().is_some_and(|(_, row)| breaks_limits(row)) {
            continue;
        }
        let mut routed = None;
        for (remove_tracks, remove_vias) in &tiers {
            let graph = CopperGraph::of(project);
            let found = gaps(project, &graph, remove_tracks, remove_vias);
            let Some(gaps) = found else {
                continue;
            };
            if let Some(next) =
                route_gaps(without(project, remove_tracks, remove_vias), &gaps, pair)
                && (offends
                    || carried
                        .as_ref()
                        .is_some_and(|(id, row)| better(row, &next, *id)))
            {
                routed = Some(next);
                break;
            }
        }
        match routed {
            Some(next) => {
                *project = next;
                report.rerouted.extend(names);
            }
            // Copper that offends a rule goes; a pair carried out of
            // coupling but clear of everything stays as it was carried.
            None if offends => {
                *project = without(project, &all_tracks, &all_vias);
                report.unrouted.extend(names);
            }
            None => {}
        }
    }
    Ok(report)
}
