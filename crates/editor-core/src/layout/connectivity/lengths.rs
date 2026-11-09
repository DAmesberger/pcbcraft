//! Lengths read off the graph: each edge's length (a track stretch's
//! centre line, a via barrel's mid-plane distance), a net's routed length
//! and the signal-path view of one net ([`NetPaths`]) that pair
//! measurement walks.
//!
//! **Entering a land.** A path is measured along track centre lines. Where
//! a track meets a via or pad land, the path enters the land at the track's
//! station nearest the land's centre (a pad's terminal point), unless
//! another track meeting the land carries on from that point nearer the
//! centre: then that track, not the corner that first touches the land,
//! carries the path in. So a route's last leg into a pad is path, and a
//! track whose end overshoots the centre counts only up to it. The joints
//! themselves (which copper is joined) are the graph's; this decides only
//! where along a joined track the path's length stops.
use super::{CopperGraph, Edge, EdgeKind, ElementKind, JOIN_TOLERANCE_MM};
use crate::geometry::{Point, distance};
use crate::ids::{Endpoint, Id};
use crate::layout::length::NetLength;
use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashMap};

#[cfg(test)]
mod tests;

/// The most vertices one pair's measurement may settle across all its
/// shortest-path searches (a target bound). Past it the pair is measured
/// by total copper and [`NetPaths::exhausted`] says so.
pub const MAX_PATH_WORK: usize = 4_000_000;

/// What a step of a signal path is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Walk {
    /// Track `id`'s centre line between two stations.
    Track { id: Id, lo: f64, hi: f64 },
    /// A via barrel between two of its lands.
    Barrel,
    /// Copper meeting copper, or a plated pad's hole: no length.
    Joint,
    /// A hop into or out of a fill island: no length.
    Fill,
}

/// One edge of a net's path view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathEdge {
    pub a: usize,
    pub b: usize,
    pub length: f64,
    pub walk: Walk,
}

/// A pad of the net that copper reaches, as a path endpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct PadEnd {
    /// Index into [`CopperGraph::pads`].
    pub pad: usize,
    pub terminal: Endpoint,
    /// Its lands' vertices in the path view.
    pub lands: Vec<usize>,
}

/// Shortest distances from a set of vertices, and the edge each vertex
/// was reached by.
pub struct PathTree {
    pub distance: Vec<f64>,
    edge: Vec<Option<usize>>,
}

/// One net's copper as a weighted graph for signal paths: the graph's
/// vertices of that net, its stretches, barrels and joints, with tracks
/// entering lands as the module note says.
pub struct NetPaths {
    vertices: usize,
    edges: Vec<PathEdge>,
    around: Vec<Vec<usize>>,
    pads: Vec<PadEnd>,
    /// The pad (index into `pads`) of each land vertex.
    land_pad: HashMap<usize, usize>,
    work: Cell<usize>,
}

#[derive(PartialEq)]
struct Reached(f64, usize);

impl Eq for Reached {}

impl Ord for Reached {
    /// The nearest first in a max-heap; ties by vertex, so results are
    /// deterministic.
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.total_cmp(&self.0).then(other.1.cmp(&self.1))
    }
}

impl PartialOrd for Reached {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn station_of(p: Point, a: Point, b: Point) -> f64 {
    let length = distance(a, b);
    if length < 1e-12 {
        return 0.0;
    }
    let t = ((p[0] - a[0]) * (b[0] - a[0]) + (p[1] - a[1]) * (b[1] - a[1])) / length;
    t.clamp(0.0, length)
}

fn point_at(a: Point, b: Point, s: f64) -> Point {
    let length = distance(a, b);
    if length < 1e-12 {
        return a;
    }
    let t = s / length;
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

impl CopperGraph {
    /// What joins two adjacent vertices.
    fn kind_between(&self, a: usize, b: usize) -> EdgeKind {
        let (Some(x), Some(y)) = (self.vertices.get(a), self.vertices.get(b)) else {
            return EdgeKind::Joint;
        };
        if x.element == y.element {
            return EdgeKind::Stretch;
        }
        match (self.element_kind(x.element), self.element_kind(y.element)) {
            (Some(ElementKind::Via { index: i }), Some(ElementKind::Via { index: j }))
                if i == j =>
            {
                EdgeKind::Barrel
            }
            (Some(ElementKind::Pad { index: i }), Some(ElementKind::Pad { index: j }))
                if i == j =>
            {
                EdgeKind::Plated
            }
            _ => EdgeKind::Joint,
        }
    }

    fn element_kind(&self, element: usize) -> Option<ElementKind> {
        self.elements.get(element).map(|e| e.kind)
    }

    fn vertex_kind(&self, vertex: usize) -> Option<ElementKind> {
        self.element_kind(self.vertices.get(vertex)?.element)
    }

    /// The mid-plane distance between the layers of two via lands, mm.
    fn barrel(&self, a: usize, b: usize) -> f64 {
        let z = |v: usize| {
            let e = self.elements.get(self.vertices.get(v)?.element)?;
            self.mid_planes.get(&e.layer).copied()
        };
        match (z(a), z(b)) {
            (Some(x), Some(y)) => (x - y).abs(),
            _ => 0.0,
        }
    }

    /// An edge's length: a stretch's centre line, a barrel's mid-plane
    /// distance, and 0 for joints and plated holes.
    pub fn edge_length(&self, edge: &Edge) -> f64 {
        match edge.kind {
            EdgeKind::Stretch => match (self.vertices.get(edge.a), self.vertices.get(edge.b)) {
                (Some(a), Some(b)) => (b.station - a.station).abs(),
                _ => 0.0,
            },
            EdgeKind::Barrel => self.barrel(edge.a, edge.b),
            EdgeKind::Plated | EdgeKind::Joint => 0.0,
        }
    }

    /// Every net's routed length (nets without tracks or vias are absent):
    /// its tracks' centre lines plus, at each via, the barrel between the
    /// outermost of its lands a track of the net joins.
    pub fn net_lengths(&self) -> BTreeMap<Id, NetLength> {
        let mut out: BTreeMap<Id, NetLength> = BTreeMap::new();
        for t in &self.tracks {
            let entry = out.entry(t.net).or_default();
            entry.tracks += distance(t.start, t.end);
            entry.segments += 1;
        }
        for (index, via) in self.vias.iter().enumerate() {
            let zs: Vec<f64> = self
                .via_lands
                .get(index)
                .cloned()
                .into_iter()
                .flatten()
                .filter(|e| {
                    self.vertices_of(*e).any(|v| {
                        self.neighbours(v).iter().any(|w| {
                            matches!(self.vertex_kind(*w), Some(ElementKind::Track { .. }))
                        })
                    })
                })
                .filter_map(|e| {
                    let layer = self.elements.get(e)?.layer;
                    self.mid_planes.get(&layer).copied()
                })
                .collect();
            let barrel = match (
                zs.iter().copied().reduce(f64::min),
                zs.iter().copied().reduce(f64::max),
            ) {
                (Some(lo), Some(hi)) => hi - lo,
                _ => 0.0,
            };
            let entry = out.entry(via.net).or_default();
            entry.vias += barrel;
            entry.via_count += 1;
        }
        for value in out.values_mut() {
            value.total = value.tracks + value.vias;
        }
        out
    }

    /// The signal-path view of `net`.
    pub fn net_paths(&self, net: Id) -> NetPaths {
        let mut local: HashMap<usize, usize> = HashMap::new();
        let mut members: Vec<usize> = Vec::new();
        for (i, e) in self.elements.iter().enumerate() {
            if e.net != net {
                continue;
            }
            for v in self.vertices_of(i) {
                local.insert(v, local.len());
            }
            members.push(i);
        }
        let mut view = NetPaths {
            vertices: local.len(),
            edges: Vec::new(),
            around: Vec::new(),
            pads: Vec::new(),
            land_pad: HashMap::new(),
            work: Cell::new(0),
        };
        let is_land = |k: Option<ElementKind>| {
            matches!(k, Some(ElementKind::Via { .. } | ElementKind::Pad { .. }))
        };
        let is_track = |k: Option<ElementKind>| matches!(k, Some(ElementKind::Track { .. }));
        for &element in &members {
            for v in self.vertices_of(element) {
                let Some(&a) = local.get(&v) else {
                    continue;
                };
                for &w in self.neighbours(v).iter().filter(|w| **w > v) {
                    let Some(&b) = local.get(&w) else {
                        continue;
                    };
                    let (kv, kw) = (self.vertex_kind(v), self.vertex_kind(w));
                    let walk = match self.kind_between(v, w) {
                        EdgeKind::Stretch => {
                            let (Some(ElementKind::Track { index }), Some(x), Some(y)) =
                                (kv, self.vertices.get(v), self.vertices.get(w))
                            else {
                                continue;
                            };
                            let Some(id) = self.tracks.get(index).map(|t| t.id) else {
                                continue;
                            };
                            Walk::Track {
                                id,
                                lo: x.station.min(y.station),
                                hi: x.station.max(y.station),
                            }
                        }
                        EdgeKind::Barrel => Walk::Barrel,
                        EdgeKind::Plated => Walk::Joint,
                        EdgeKind::Joint => {
                            // A track meets a land where it enters it, below.
                            if (is_track(kv) && is_land(kw)) || (is_land(kv) && is_track(kw)) {
                                continue;
                            }
                            let fill = |k| matches!(k, Some(ElementKind::Fill { .. }));
                            if fill(kv) || fill(kw) {
                                Walk::Fill
                            } else {
                                Walk::Joint
                            }
                        }
                    };
                    let length = match walk {
                        Walk::Track { lo, hi, .. } => hi - lo,
                        Walk::Barrel => self.barrel(v, w),
                        Walk::Joint | Walk::Fill => 0.0,
                    };
                    view.edges.push(PathEdge { a, b, length, walk });
                }
            }
        }
        for &element in &members {
            self.enter_land(element, &local, &mut view);
        }
        view.around = vec![Vec::new(); view.vertices];
        for (e, edge) in view.edges.iter().enumerate() {
            for v in [edge.a, edge.b] {
                if let Some(list) = view.around.get_mut(v) {
                    list.push(e);
                }
            }
        }
        let outlines = self.tracks.len() + self.vias.len();
        for (index, pad) in self.pads.iter().enumerate() {
            if pad.net != net {
                continue;
            }
            let lands: Vec<usize> = self
                .lands
                .get(outlines + index)
                .cloned()
                .into_iter()
                .flatten()
                .flat_map(|e| self.vertices_of(e))
                .filter_map(|v| local.get(&v).copied())
                .collect();
            let joined = lands.iter().any(|v| {
                view.around.get(*v).is_some_and(|list| {
                    list.iter().any(|e| {
                        view.edges.get(*e).is_some_and(|edge| {
                            let other = if edge.a == *v { edge.b } else { edge.a };
                            !lands.contains(&other)
                        })
                    })
                })
            });
            if !joined {
                continue;
            }
            for v in &lands {
                view.land_pad.insert(*v, view.pads.len());
            }
            view.pads.push(PadEnd {
                pad: index,
                terminal: pad.terminal,
                lands,
            });
        }
        view
    }

    /// Where the tracks meeting the land `element` (a via or pad land)
    /// enter it: see the module note.
    fn enter_land(&self, element: usize, local: &HashMap<usize, usize>, view: &mut NetPaths) {
        let centre = match self.element_kind(element) {
            Some(ElementKind::Via { index }) => self.vias.get(index).map(|v| v.position),
            Some(ElementKind::Pad { index }) => self.pads.get(index).map(|p| p.position),
            _ => None,
        };
        let (Some(centre), Some(&land)) = (centre, local.get(&self.vertices_of(element).start))
        else {
            return;
        };
        let mut meeting: Vec<usize> = self
            .neighbours(self.vertices_of(element).start)
            .iter()
            .filter_map(|w| self.vertices.get(*w))
            .filter(|w| {
                matches!(
                    self.element_kind(w.element),
                    Some(ElementKind::Track { .. })
                )
            })
            .map(|w| w.element)
            .collect();
        meeting.sort_unstable();
        meeting.dedup();
        // (track element, its station nearest the centre, that point, its
        // distance from the centre)
        let entries: Vec<(usize, f64, Point, f64)> = meeting
            .iter()
            .filter_map(|&e| {
                let Some(ElementKind::Track { index }) = self.element_kind(e) else {
                    return None;
                };
                let t = self.tracks.get(index)?;
                let s = station_of(centre, t.start, t.end);
                let at = point_at(t.start, t.end, s);
                Some((e, s, at, distance(at, centre)))
            })
            .collect();
        let track = |e: usize| match self.element_kind(e) {
            Some(ElementKind::Track { index }) => self.tracks.get(index),
            _ => None,
        };
        for &(e, s, at, nearness) in &entries {
            let Some(t) = track(e) else {
                continue;
            };
            let reach = t.width / 2.0 + JOIN_TOLERANCE_MM;
            let carried_on = entries.iter().any(|&(o, _, _, other)| {
                o != e
                    && other < nearness - JOIN_TOLERANCE_MM
                    && track(o).is_some_and(|o| {
                        distance(o.start, at) <= reach || distance(o.end, at) <= reach
                    })
            });
            if carried_on {
                continue;
            }
            let stations: Vec<(f64, usize)> = self
                .vertices_of(e)
                .filter_map(|v| Some((self.vertices.get(v)?.station, *local.get(&v)?)))
                .collect();
            if let Some(&(_, v)) = stations
                .iter()
                .find(|(x, _)| (x - s).abs() <= JOIN_TOLERANCE_MM)
            {
                view.edges.push(PathEdge {
                    a: v,
                    b: land,
                    length: 0.0,
                    walk: Walk::Joint,
                });
                continue;
            }
            let below = stations.iter().rev().find(|(x, _)| *x < s);
            let above = stations.iter().find(|(x, _)| *x > s);
            let (Some(&(lo, a)), Some(&(hi, b))) = (below, above) else {
                continue;
            };
            let x = view.vertices;
            view.vertices += 1;
            view.edges.extend([
                PathEdge {
                    a,
                    b: x,
                    length: s - lo,
                    walk: Walk::Track {
                        id: t.id,
                        lo,
                        hi: s,
                    },
                },
                PathEdge {
                    a: x,
                    b,
                    length: hi - s,
                    walk: Walk::Track {
                        id: t.id,
                        lo: s,
                        hi,
                    },
                },
                PathEdge {
                    a: x,
                    b: land,
                    length: 0.0,
                    walk: Walk::Joint,
                },
            ]);
        }
    }
}

impl NetPaths {
    /// The pads copper reaches, in placement then pad order.
    pub fn pads(&self) -> &[PadEnd] {
        &self.pads
    }

    pub fn edges(&self) -> &[PathEdge] {
        &self.edges
    }

    /// The pad whose land `vertex` is.
    pub fn pad_at(&self, vertex: usize) -> Option<&PadEnd> {
        self.pads.get(*self.land_pad.get(&vertex)?)
    }

    /// Whether a search stopped at [`MAX_PATH_WORK`]: its results are
    /// incomplete and must not be used.
    pub fn exhausted(&self) -> bool {
        self.work.get() > MAX_PATH_WORK
    }

    /// Shortest distances from the nearest of `from`.
    pub fn tree(&self, from: &[usize]) -> PathTree {
        let mut tree = PathTree {
            distance: vec![f64::INFINITY; self.vertices],
            edge: vec![None; self.vertices],
        };
        let mut heap = BinaryHeap::new();
        for v in from {
            if let Some(d) = tree.distance.get_mut(*v) {
                *d = 0.0;
                heap.push(Reached(0.0, *v));
            }
        }
        while let Some(Reached(d, v)) = heap.pop() {
            if tree.distance.get(v).is_none_or(|best| d > *best) {
                continue;
            }
            let work = self.work.get().saturating_add(1);
            self.work.set(work);
            if work > MAX_PATH_WORK {
                break;
            }
            for e in self.around.get(v).into_iter().flatten() {
                let Some(edge) = self.edges.get(*e) else {
                    continue;
                };
                let next = if edge.a == v { edge.b } else { edge.a };
                let reached = d + edge.length;
                if let Some(best) = tree.distance.get_mut(next)
                    && reached < *best
                {
                    *best = reached;
                    if let Some(slot) = tree.edge.get_mut(next) {
                        *slot = Some(*e);
                    }
                    heap.push(Reached(reached, next));
                }
            }
        }
        tree
    }

    /// The nearest of `to` from the nearest of `from`: its distance, the
    /// two end vertices and the path's edges in order.
    pub fn path(&self, from: &[usize], to: &[usize]) -> Option<(f64, usize, usize, Vec<usize>)> {
        let tree = self.tree(from);
        let (&end, &d) = to
            .iter()
            .filter_map(|v| tree.distance.get(*v).map(|d| (v, d)))
            .filter(|(_, d)| d.is_finite())
            .min_by(|a, b| a.1.total_cmp(b.1).then(a.0.cmp(b.0)))?;
        let mut edges = Vec::new();
        let mut at = end;
        while let Some(e) = tree.edge.get(at).copied().flatten() {
            let edge = self.edges.get(e)?;
            edges.push(e);
            at = if edge.a == at { edge.b } else { edge.a };
            if edges.len() > self.edges.len() {
                return None;
            }
        }
        edges.reverse();
        Some((d, at, end, edges))
    }
}
