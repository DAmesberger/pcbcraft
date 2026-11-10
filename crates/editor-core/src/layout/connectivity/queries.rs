//! What the graph answers: stubs, the connectivity findings and airwires.
use super::build::joins;
use super::{CopperGraph, Dsu, EdgeKind, ElementKind, JOIN_TOLERANCE_MM};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::Point;
use crate::ids::Id;
use crate::layout::limits::mm;
use crate::layout::{WorldPad, pad_labels};
use crate::model::{CopperLayer, Project};
use std::collections::{BTreeMap, BTreeSet};

/// Copper that leads to no pad: a connected piece the dead-end peeling
/// removed, or an island holding no pad at all.
#[derive(Clone, Debug, PartialEq)]
pub struct Stub {
    pub net: Id,
    /// Its tracks, in document order.
    pub tracks: Vec<Id>,
    /// The vias wholly in it, in document order.
    pub vias: Vec<Id>,
    /// Track ends that meet nothing.
    pub open_ends: Vec<Point>,
    /// Vias it ends at (joined to one piece of copper only).
    pub via_ends: Vec<Point>,
    /// Its track copper along the centre lines, mm.
    pub length: f64,
    /// Whether its island holds no pad.
    pub floating: bool,
}

/// The result of peeling: which vertices lead to no pad.
struct Peeled {
    removed: Vec<bool>,
    /// Islands that hold a pad.
    terminated: Vec<bool>,
}

impl CopperGraph {
    fn is_pad(&self, vertex: usize) -> bool {
        self.element_of(vertex)
            .is_some_and(|k| matches!(k, ElementKind::Pad { .. }))
    }

    fn element_of(&self, vertex: usize) -> Option<ElementKind> {
        let v = self.vertices.get(vertex)?;
        Some(self.elements.get(v.element)?.kind)
    }

    /// Peel dead ends that are not pins until none is left; then every
    /// island without a pad is removed whole (a floating loop has no dead
    /// end). Pads and fill islands are never removed: a pour is meant to
    /// reach no pad over most of its area.
    fn peel(&self) -> Peeled {
        let n = self.vertices.len();
        let mut terminated = vec![false; self.islands];
        for v in (0..n).filter(|v| self.is_pad(*v)) {
            if let Some(slot) = self.island.get(v).and_then(|i| terminated.get_mut(*i)) {
                *slot = true;
            }
        }
        let fixed = |v: usize| {
            self.element_of(v)
                .is_some_and(|k| matches!(k, ElementKind::Pad { .. } | ElementKind::Fill { .. }))
        };
        let mut degree: Vec<usize> = (0..n).map(|v| self.neighbours(v).len()).collect();
        let mut removed = vec![false; n];
        let mut queue: Vec<usize> = (0..n)
            .filter(|v| !fixed(*v) && degree.get(*v).is_some_and(|d| *d <= 1))
            .collect();
        while let Some(v) = queue.pop() {
            match removed.get_mut(v) {
                Some(r) if !*r => *r = true,
                _ => continue,
            }
            for &w in self.neighbours(v) {
                if removed.get(w).copied().unwrap_or(true) {
                    continue;
                }
                if let Some(d) = degree.get_mut(w) {
                    *d = d.saturating_sub(1);
                    if *d <= 1 && !fixed(w) {
                        queue.push(w);
                    }
                }
            }
        }
        for v in 0..n {
            let floating = self
                .island
                .get(v)
                .and_then(|i| terminated.get(*i))
                .is_some_and(|t| !t);
            if floating
                && !fixed(v)
                && let Some(r) = removed.get_mut(v)
            {
                *r = true;
            }
        }
        Peeled {
            removed,
            terminated,
        }
    }

    /// Every stub, in order of its first vertex. A via that keeps a land
    /// on copper leading to a pad is not part of a stub, so its peeled
    /// lands split what hangs off it into separate stubs.
    pub fn stubs(&self) -> Vec<Stub> {
        if self.partial {
            return Vec::new();
        }
        self.stubs_of(&self.peel())
    }

    fn stubs_of(&self, peeled: &Peeled) -> Vec<Stub> {
        let removed = |v: usize| peeled.removed.get(v).copied().unwrap_or(false);
        // A via wholly removed, by index.
        let gone: Vec<bool> = self
            .via_lands
            .iter()
            .map(|lands| lands.clone().all(|e| self.vertices_of(e).all(&removed)))
            .collect();
        let part = |v: usize| {
            removed(v)
                && match self.element_of(v) {
                    Some(ElementKind::Via { index }) => gone.get(index).copied().unwrap_or(false),
                    Some(_) => true,
                    None => false,
                }
        };
        let n = self.vertices.len();
        let mut dsu = Dsu::new(n);
        for e in &self.edges {
            if part(e.a) && part(e.b) {
                dsu.union(e.a, e.b);
            }
        }
        // Number the pieces densely and measure their track copper in one
        // pass over the edges.
        let mut piece = vec![usize::MAX; n];
        let mut number = vec![usize::MAX; n];
        let mut pieces: Vec<Vec<usize>> = Vec::new();
        for v in (0..n).filter(|v| part(*v)) {
            let root = dsu.find(v);
            let Some(slot) = number.get_mut(root) else {
                continue;
            };
            if *slot == usize::MAX {
                *slot = pieces.len();
                pieces.push(Vec::new());
            }
            if let Some(members) = pieces.get_mut(*slot) {
                members.push(v);
            }
            if let Some(p) = piece.get_mut(v) {
                *p = *slot;
            }
        }
        let mut lengths = vec![0.0; pieces.len()];
        for e in self.edges.iter().filter(|e| e.kind == EdgeKind::Stretch) {
            let at = |v: usize| piece.get(v).copied().filter(|p| *p != usize::MAX);
            let (Some(p), Some(a), Some(b)) = (
                at(e.a).or_else(|| at(e.b)),
                self.vertices.get(e.a),
                self.vertices.get(e.b),
            ) else {
                continue;
            };
            if let Some(length) = lengths.get_mut(p) {
                *length += (b.station - a.station).abs();
            }
        }
        let mut out = Vec::new();
        for (members, length) in pieces.iter().zip(lengths) {
            let mut tracks = BTreeSet::new();
            let mut vias = BTreeSet::new();
            let mut open_ends = Vec::new();
            let mut net = None;
            for &v in members {
                let (Some(vertex), Some(kind)) = (self.vertices.get(v), self.element_of(v)) else {
                    continue;
                };
                net = net.or_else(|| self.elements.get(vertex.element).map(|e| e.net));
                match kind {
                    ElementKind::Track { index } => {
                        tracks.insert(index);
                        let Some(t) = self.tracks.get(index) else {
                            continue;
                        };
                        let length = (t.end[0] - t.start[0]).hypot(t.end[1] - t.start[1]);
                        let end = if vertex.station <= JOIN_TOLERANCE_MM {
                            Some(t.start)
                        } else if vertex.station >= length - JOIN_TOLERANCE_MM {
                            Some(t.end)
                        } else {
                            None
                        };
                        if let Some(end) = end
                            && self.neighbours(v).len() <= 1
                        {
                            open_ends.push(end);
                        }
                    }
                    ElementKind::Via { index } => {
                        vias.insert(index);
                    }
                    ElementKind::Pad { .. } | ElementKind::Fill { .. } => {}
                }
            }
            let Some(net) = net else {
                continue;
            };
            let via_ends = vias
                .iter()
                .filter(|&&index| {
                    let joints: usize = self
                        .via_lands
                        .get(index)
                        .into_iter()
                        .flat_map(|lands| lands.clone())
                        .flat_map(|e| self.vertices_of(e))
                        .map(|v| {
                            self.neighbours(v)
                                .iter()
                                .filter(|w| {
                                    !matches!(
                                        self.element_of(**w),
                                        Some(ElementKind::Via { index: i }) if i == index
                                    )
                                })
                                .count()
                        })
                        .sum();
                    joints <= 1
                })
                .filter_map(|&index| self.vias.get(index).map(|v| v.position))
                .collect();
            let floating = members
                .first()
                .and_then(|v| self.island.get(*v))
                .and_then(|i| peeled.terminated.get(*i))
                .is_some_and(|t| !t);
            out.push(Stub {
                net,
                tracks: tracks
                    .iter()
                    .filter_map(|i| self.tracks.get(*i).map(|t| t.id))
                    .collect(),
                vias: vias
                    .iter()
                    .filter_map(|i| self.vias.get(*i).map(|v| v.id))
                    .collect(),
                open_ends,
                via_ends,
                length,
                floating,
            });
        }
        out
    }

    /// Whether any land of the via at `index` meets other copper.
    fn via_joins_anything(&self, index: usize) -> bool {
        self.via_lands.get(index).is_some_and(|lands| {
            lands.clone().flat_map(|e| self.vertices_of(e)).any(|v| {
                self.neighbours(v).iter().any(|w| {
                    !matches!(
                        self.element_of(*w),
                        Some(ElementKind::Via { index: i }) if i == index
                    )
                })
            })
        })
    }

    /// The connectivity findings: `board.unrouted` (one per pad of a net
    /// whose pads lie in more than one island), `board.track_dangling` (one
    /// per stub with tracks, naming its tracks and vias) and
    /// `board.via_dangling` (a via joining nothing, or whose island holds
    /// no pad, that no stub finding names). A partial graph reports only
    /// `board.check_limit`.
    pub fn findings(&self, project: &Project) -> Vec<Finding> {
        if self.partial {
            return vec![crate::layout::copper::limit_finding()];
        }
        let mut out = Vec::new();
        let labels = pad_labels(project);
        let mut islands: BTreeMap<Id, BTreeSet<usize>> = BTreeMap::new();
        for (index, pad) in self.pads.iter().enumerate() {
            if let Some(island) = self.pad_island(index) {
                islands.entry(pad.net).or_default().insert(island);
            }
        }
        for pad in &self.pads {
            if islands.get(&pad.net).is_some_and(|i| i.len() > 1) {
                let label = format!(
                    "{} pad {}",
                    project
                        .instance(pad.instance)
                        .map(|i| i.designator.as_str())
                        .unwrap_or("component"),
                    labels
                        .get(&(pad.instance, pad.pad))
                        .cloned()
                        .unwrap_or_else(|| pad.pad.to_string())
                );
                out.push(Finding::new(
                    "board.unrouted",
                    format!("Unrouted: {label} ({})", project.net_name(pad.net)),
                    vec![
                        ObjectRef::Pad {
                            instance: pad.instance,
                            pad: pad.pad,
                        },
                        ObjectRef::Net { id: pad.net },
                    ],
                ));
            }
        }
        let peeled = self.peel();
        let stubs = self.stubs_of(&peeled);
        let named: BTreeSet<Id> = stubs
            .iter()
            .filter(|s| !s.tracks.is_empty())
            .flat_map(|s| s.vias.iter().copied())
            .collect();
        for (index, via) in self.vias.iter().enumerate() {
            if named.contains(&via.id) {
                continue;
            }
            let joined = self.via_joins_anything(index);
            let floating = self
                .via_lands
                .get(index)
                .and_then(|lands| lands.clone().next())
                .and_then(|e| self.island_of(self.vertices_of(e).start))
                .and_then(|i| peeled.terminated.get(i))
                .is_some_and(|t| !t);
            if !joined || floating {
                let what = if joined {
                    "reaches no pad of its net"
                } else {
                    "joins no copper of its net"
                };
                out.push(Finding::new(
                    "board.via_dangling",
                    format!("a via on {} {what}", project.net_name(via.net)),
                    vec![
                        ObjectRef::Via { id: via.id },
                        ObjectRef::Net { id: via.net },
                    ],
                ));
            }
        }
        let point = |p: &Point| format!("({}, {}) mm", mm(p[0]), mm(p[1]));
        let track_layers: BTreeMap<Id, CopperLayer> =
            self.tracks.iter().map(|t| (t.id, t.layer)).collect();
        for stub in stubs.iter().filter(|s| !s.tracks.is_empty()) {
            let layers: BTreeSet<CopperLayer> = stub
                .tracks
                .iter()
                .filter_map(|id| track_layers.get(id).copied())
                .collect();
            let layers = self
                .layers
                .iter()
                .filter(|l| layers.contains(l))
                .map(|l| l.name())
                .collect::<Vec<_>>()
                .join(", ");
            let ends: Vec<String> = stub
                .open_ends
                .iter()
                .map(point)
                .chain(
                    stub.via_ends
                        .iter()
                        .map(|p| format!("a via at {}", point(p))),
                )
                .collect();
            let net = project.net_name(stub.net);
            let message = if ends.is_empty() {
                format!("Track on {net} ({layers}) reaches no pad")
            } else {
                format!(
                    "Track on {net} ({layers}) ends at {} joining nothing",
                    ends.join(" and ")
                )
            };
            let objects = stub
                .tracks
                .iter()
                .map(|&id| ObjectRef::Track { id })
                .chain(stub.vias.iter().map(|&id| ObjectRef::Via { id }))
                .chain([ObjectRef::Net { id: stub.net }])
                .collect();
            out.push(Finding::new("board.track_dangling", message, objects));
        }
        out.extend(self.unjoined_islands(project));
        out
    }

    /// `fill.island_unconnected`: per fresh fill, the islands that join no
    /// copper of its net. Fill compute removes these unless the fill keeps
    /// every island; it decides by the same rule.
    fn unjoined_islands(&self, project: &Project) -> Vec<Finding> {
        let mut dead: Vec<(Id, usize)> = Vec::new();
        for (i, e) in self.elements.iter().enumerate() {
            let ElementKind::Fill { fill, .. } = e.kind else {
                continue;
            };
            let joined = self.vertices_of(i).any(|v| !self.neighbours(v).is_empty());
            match dead.last_mut() {
                Some((id, count)) if *id == fill => *count += usize::from(!joined),
                _ => dead.push((fill, usize::from(!joined))),
            }
        }
        dead.into_iter()
            .filter(|(_, count)| *count > 0)
            .filter_map(|(id, count)| {
                let fill = project.fill(id)?;
                let label = crate::layout::fill::fill_label(project, fill);
                Some(Finding::new(
                    "fill.island_unconnected",
                    format!("{label} has {count} unconnected copper islands"),
                    vec![ObjectRef::Fill { id }],
                ))
            })
            .collect()
    }

    /// Airwires: per net, a minimum spanning tree over the islands holding
    /// its pads, each joining the nearest pads of two islands.
    pub fn airwires(&self) -> Vec<(WorldPad, WorldPad)> {
        let island: Vec<usize> = (0..self.pads.len())
            .map(|i| self.pad_island(i).unwrap_or(usize::MAX - i))
            .collect();
        spanning(&self.pads, &island)
    }

    /// The airwires with one placement's pads moved by `delta`, as a pure
    /// move of that placement gives, without rebuilding the graph: only the
    /// moved pads' lands are joined again, against their new neighbours.
    pub fn moved_airwires(&self, instance: Id, delta: Point) -> Vec<(WorldPad, WorldPad)> {
        self.airwires_moving(instance, delta, false)
    }

    /// The airwires with one placement moved by `delta` as `layout.move`
    /// moves it (`dimensions::carry`), without rebuilding the graph: the
    /// vias it carries move with its pads, its unlocked tracks stretch and
    /// stay joined, and the moved lands are joined again against their new
    /// neighbours.
    pub fn carried_airwires(&self, instance: Id, delta: Point) -> Vec<(WorldPad, WorldPad)> {
        self.airwires_moving(instance, delta, true)
    }

    fn airwires_moving(
        &self,
        instance: Id,
        delta: Point,
        carry: bool,
    ) -> Vec<(WorldPad, WorldPad)> {
        let own = |index: usize| self.pads.get(index).is_some_and(|p| p.instance == instance);
        let vias: BTreeSet<usize> = if carry {
            self.carried_vias(|pad| own(pad).then_some(()))
                .into_keys()
                .filter(|v| self.vias.get(*v).is_some_and(|v| !v.locked))
                .collect()
        } else {
            BTreeSet::new()
        };
        let moved = |kind: ElementKind| match kind {
            ElementKind::Pad { index } => own(index),
            ElementKind::Via { index } => vias.contains(&index),
            _ => false,
        };
        let stretches = |k: Option<ElementKind>| {
            carry
                && matches!(k, Some(ElementKind::Track { index })
                    if self.tracks.get(index).is_some_and(|t| !t.locked))
        };
        let element_moved = |v: usize| self.element_of(v).is_some_and(moved);
        let mut dsu = Dsu::new(self.vertices.len());
        for e in &self.edges {
            let keep = e.kind != EdgeKind::Joint || {
                let (a, b) = (self.element_of(e.a), self.element_of(e.b));
                // Fill joins stay as stored, as the committed board's do.
                let fill = |k: Option<ElementKind>| matches!(k, Some(ElementKind::Fill { .. }));
                fill(a)
                    || fill(b)
                    || element_moved(e.a) == element_moved(e.b)
                    || stretches(a)
                    || stretches(b)
            };
            if keep {
                dsu.union(e.a, e.b);
            }
        }
        let mut near = Vec::new();
        let shape_moved = |shape: usize| {
            self.lands
                .get(shape)
                .and_then(|l| self.elements.get(l.start))
                .is_some_and(|e| moved(e.kind))
        };
        for (shape, lands) in self.lands.iter().enumerate() {
            if !shape_moved(shape) {
                continue;
            }
            let (Some(ring), Some(net)) = (
                self.shapes.get(shape).map(|r| r.translated(delta)),
                self.elements.get(lands.start).map(|e| e.net),
            ) else {
                continue;
            };
            let (Some(group), Some(bounds)) = (self.groups.get(&net), ring.bounds) else {
                continue;
            };
            group
                .grid
                .near(&bounds, super::build::JOIN_REACH_MM, &mut near);
            for &other in near.iter().filter_map(|j| group.members.get(*j)) {
                if shape_moved(other) || !self.shapes.get(other).is_some_and(|r| joins(&ring, r)) {
                    continue;
                }
                for (a, b) in self.common_lands(shape, other) {
                    dsu.union(self.vertices_of(a).start, self.vertices_of(b).start);
                }
            }
        }
        let pads: Vec<WorldPad> = self
            .pads
            .iter()
            .map(|p| {
                let mut p = p.clone();
                if p.instance == instance {
                    p.position = [p.position[0] + delta[0], p.position[1] + delta[1]];
                }
                p
            })
            .collect();
        let island: Vec<usize> = (0..pads.len())
            .map(|i| {
                self.pad_lands
                    .get(i)
                    .map_or(usize::MAX - i, |e| dsu.find(self.vertices_of(*e).start))
            })
            .collect();
        spanning(&pads, &island)
    }
}

/// Per net, Prim's minimum spanning tree over the islands of its pads
/// (`island` per pad): each airwire joins the nearest pads of the tree and
/// a new island. Ties go to the earlier pad, so the result depends only on
/// which pads share an island.
pub(super) fn spanning(pads: &[WorldPad], island: &[usize]) -> Vec<(WorldPad, WorldPad)> {
    let mut nets: BTreeMap<Id, Vec<usize>> = BTreeMap::new();
    for (i, pad) in pads.iter().enumerate() {
        nets.entry(pad.net).or_default().push(i);
    }
    let mut out = Vec::new();
    let island_of = |i: usize| island.get(i).copied().unwrap_or(usize::MAX);
    let position = |i: usize| pads.get(i).map_or([0.0, 0.0], |p| p.position);
    for members in nets.values() {
        let distinct: BTreeSet<usize> = members.iter().map(|&i| island_of(i)).collect();
        if distinct.len() < 2 {
            continue;
        }
        let n = members.len();
        let mut in_tree = vec![false; n];
        let mut best = vec![(f64::INFINITY, usize::MAX); n];
        let mut fresh = Vec::new();
        let join = |at: usize, in_tree: &mut Vec<bool>, fresh: &mut Vec<usize>| {
            let target = members.get(at).map(|&i| island_of(i));
            for (k, &i) in members.iter().enumerate() {
                if Some(island_of(i)) == target
                    && let Some(flag) = in_tree.get_mut(k)
                    && !*flag
                {
                    *flag = true;
                    fresh.push(k);
                }
            }
        };
        join(0, &mut in_tree, &mut fresh);
        loop {
            for &k in &fresh {
                let from = members.get(k).map_or([0.0, 0.0], |&i| position(i));
                for (j, &i) in members.iter().enumerate() {
                    if in_tree.get(j).copied().unwrap_or(true) {
                        continue;
                    }
                    let p = position(i);
                    let d = (p[0] - from[0]).hypot(p[1] - from[1]);
                    if let Some(slot) = best.get_mut(j)
                        && d < slot.0
                    {
                        *slot = (d, k);
                    }
                }
            }
            fresh.clear();
            let next = (0..n)
                .filter(|j| !in_tree.get(*j).copied().unwrap_or(true))
                .min_by(|x, y| {
                    let dx = best.get(*x).map_or(f64::INFINITY, |b| b.0);
                    let dy = best.get(*y).map_or(f64::INFINITY, |b| b.0);
                    dx.total_cmp(&dy).then(x.cmp(y))
                });
            let Some(j) = next else {
                break;
            };
            let from = best.get(j).map(|b| b.1);
            if let (Some(a), Some(b)) = (
                from.and_then(|k| members.get(k)).and_then(|&i| pads.get(i)),
                members.get(j).and_then(|&i| pads.get(i)),
            ) {
                out.push((a.clone(), b.clone()));
            }
            join(j, &mut in_tree, &mut fresh);
        }
    }
    out
}
