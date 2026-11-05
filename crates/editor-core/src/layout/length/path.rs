//! A pair half measured along its signal path: the copper between the
//! pair's two endpoints. Whatever else the net's copper reaches (the second
//! plug orientation of a USB-C receptacle, a test point) is a stub, listed
//! on its own and left out of skew and uncoupled length.
//!
//! **Graph.** Each half's copper is a graph: tracks split where another
//! track, a via or a pad joins them, via barrels (one vertex per layer
//! the net's tracks reach, weighted by the foil mid-plane distance, the
//! length model's barrel), and pads as zero-length joints.
//!
//! **Endpoints.** Those the pair sets ([`PairEndpoints`]), when both halves
//! connect them. Otherwise they are derived: of the parts both halves reach,
//! the two farthest apart along the copper, each half taking its shortest
//! path between them. When the halves share fewer than two parts, each half
//! takes its longest pad-to-pad path. When a half has no such path, because
//! the route is unfinished, the pair is measured by total copper as before.
//! A set endpoint that left its net (a schematic edit) is ignored the same
//! way.
use super::super::{CopperRing, WorldPad, pad_labels, pads};
use super::mid_z;
use crate::geometry::{Point, distance, segment_distance};
use crate::ids::{Endpoint, Id, PadId};
use crate::model::{CopperLayer, Project, Track};
use crate::rules::signal::{DiffPair, PairEndpoints};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

/// The most tracks a half may have and still be measured by path (a target
/// bound; past it the pair is measured by total copper).
const MAX_PATH_TRACKS: usize = 2048;
/// The most connected pads a half may have and still be measured by path.
const MAX_PATH_PADS: usize = 64;
/// A track end this close to another's centre line joins it exactly, mm.
const ON_SPINE_MM: f64 = 1e-6;
/// Stations along a track closer than this are one joint, mm.
const STATION_MM: f64 = 1e-7;

/// How a pair's lengths were measured.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairMeasured {
    /// Along the path between the endpoints the pair sets.
    Set,
    /// Along the path between endpoints derived from the copper.
    Derived,
    /// By each half's total copper: no path joins two pads yet.
    #[default]
    Copper,
}

/// Copper of a half off its signal path.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PairStub {
    pub net: Id,
    /// The net's name.
    pub name: String,
    /// Tracks and via barrels of the branch, mm.
    pub length: f64,
    pub tracks: Vec<Id>,
    /// The pads the branch reaches ("J1.A6"), when it reaches any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reaches: Vec<String>,
}

/// One half's signal path.
#[derive(Clone, Debug, PartialEq)]
pub struct HalfPath {
    /// The endpoints' pads ("J1.B6").
    pub from: String,
    pub to: String,
    /// Track centre lines on the path, mm.
    pub tracks: f64,
    /// Via barrels on the path, mm.
    pub vias: f64,
    /// The stretches of each track on the path, as distances from its start.
    pub spans: BTreeMap<Id, Vec<(f64, f64)>>,
    pub stubs: Vec<PairStub>,
}

impl HalfPath {
    pub fn total(&self) -> f64 {
        self.tracks + self.vias
    }
}

/// A pair as measured: both halves' paths, or `None` for total copper.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PairMeasure {
    pub measured: PairMeasured,
    pub halves: Option<[HalfPath; 2]>,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Track { id: Id, lo: f64, hi: f64 },
    Barrel,
    Joint,
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    a: usize,
    b: usize,
    length: f64,
    kind: Kind,
}

/// A connected pad of the half.
struct PadVertex {
    vertex: usize,
    terminal: Endpoint,
    label: String,
}

/// One half's copper as a graph.
struct Graph {
    vertices: usize,
    edges: Vec<Edge>,
    around: Vec<Vec<usize>>,
    pads: Vec<PadVertex>,
}

fn along_track(track: &Track, p: Point) -> f64 {
    let length = distance(track.start, track.end);
    if length < 1e-12 {
        return 0.0;
    }
    let d = [track.end[0] - track.start[0], track.end[1] - track.start[1]];
    let t = ((p[0] - track.start[0]) * d[0] + (p[1] - track.start[1]) * d[1]) / length;
    t.clamp(0.0, length)
}

/// The stations where a track is joined, sorted, and their vertices.
struct Stations {
    at: Vec<(f64, usize)>,
}

impl Stations {
    fn vertex(&self, s: f64) -> Option<usize> {
        self.at
            .iter()
            .min_by(|a, b| (a.0 - s).abs().total_cmp(&(b.0 - s).abs()))
            .map(|(_, v)| *v)
    }
}

/// A joint found while scanning: a station on a track and what meets it.
enum Meets {
    TrackEnd(usize, f64),
    Via(usize),
    Pad(usize),
}

/// Where tracks join `pad`: each touching track at its point nearest the
/// pad's centre, unless another touching track carries on from there
/// nearer to the centre. So a route's last leg, not the corner that first
/// touches the pad, joins it, and that leg counts as path, not as a stub.
fn pad_joints(tracks: &[&Track], pad: &WorldPadRing<'_>) -> Vec<(usize, f64)> {
    let touching: Vec<(usize, f64, Point, f64)> = tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| pad.pad.on(t.layer) && CopperRing::of_track(t).touches(&pad.ring))
        .map(|(i, t)| {
            let s = along_track(t, pad.pad.position);
            let length = distance(t.start, t.end).max(1e-12);
            let at = [
                t.start[0] + (t.end[0] - t.start[0]) * s / length,
                t.start[1] + (t.end[1] - t.start[1]) * s / length,
            ];
            (i, s, at, distance(at, pad.pad.position))
        })
        .collect();
    touching
        .iter()
        .filter(|(i, _, at, nearness)| {
            let reach = tracks.get(*i).map_or(0.0, |t| t.width / 2.0) + 1e-6;
            !touching.iter().any(|(j, _, _, other)| {
                j != i
                    && *other < nearness - 1e-6
                    && tracks.get(*j).is_some_and(|o| {
                        o.layer == tracks.get(*i).map_or(o.layer, |t| t.layer)
                            && (distance(o.start, *at) <= reach || distance(o.end, *at) <= reach)
                    })
            })
        })
        .map(|(i, s, ..)| (*i, *s))
        .collect()
}

/// The copper of `net` as a graph; `None` past the bounds.
fn graph(project: &Project, net: Id, pads: &[WorldPadRing<'_>]) -> Option<Graph> {
    let tracks: Vec<&Track> = project.tracks.iter().filter(|t| t.net == net).collect();
    if tracks.len() > MAX_PATH_TRACKS || pads.len() > MAX_PATH_PADS {
        return None;
    }
    let layers = project.stackup.layers();
    let vias: Vec<_> = project.vias.iter().filter(|v| v.net == net).collect();
    let mut stations: Vec<Vec<f64>> = tracks
        .iter()
        .map(|t| vec![0.0, distance(t.start, t.end)])
        .collect();
    let mut meets: Vec<(usize, f64, Meets)> = Vec::new();
    // The stations already joined exactly, per track.
    let mut joined_at: Vec<Vec<f64>> = vec![Vec::new(); tracks.len()];
    let mut note_joined = |i: usize, s: f64| {
        if let Some(list) = joined_at.get_mut(i) {
            list.push(s);
        }
    };
    for (i, t) in tracks.iter().enumerate() {
        for (j, o) in tracks.iter().enumerate() {
            if i == j || o.layer != t.layer {
                continue;
            }
            for (end, at) in [(0.0, o.start), (distance(o.start, o.end), o.end)] {
                if segment_distance(at, t.start, t.end) <= ON_SPINE_MM {
                    meets.push((i, along_track(t, at), Meets::TrackEnd(j, end)));
                    note_joined(j, end);
                    note_joined(i, along_track(t, at));
                }
            }
        }
        for (v, via) in vias.iter().enumerate() {
            if via.on(t.layer, &layers)
                && segment_distance(via.position, t.start, t.end) <= via.diameter / 2.0 + 1e-6
            {
                let s = along_track(t, via.position);
                meets.push((i, s, Meets::Via(v)));
                note_joined(i, s);
            }
        }
    }
    for (p, pad) in pads.iter().enumerate() {
        for (i, s) in pad_joints(&tracks, pad) {
            meets.push((i, s, Meets::Pad(p)));
            note_joined(i, s);
        }
    }
    // An end joined to nothing exactly may still lie on another track's
    // copper, which connects it. Trying this only for such ends keeps a
    // short leg's far end from shortcutting the leg through the copper of
    // the track after it.
    for (j, o) in tracks.iter().enumerate() {
        for (end, at) in [(0.0, o.start), (distance(o.start, o.end), o.end)] {
            let joined = joined_at
                .get(j)
                .is_some_and(|list| list.iter().any(|s| (s - end).abs() <= ON_SPINE_MM));
            if joined {
                continue;
            }
            for (i, t) in tracks.iter().enumerate() {
                if i != j
                    && o.layer == t.layer
                    && segment_distance(at, t.start, t.end) <= t.width / 2.0 + 1e-6
                {
                    meets.push((i, along_track(t, at), Meets::TrackEnd(j, end)));
                }
            }
        }
    }
    for (i, s, _) in &meets {
        if let Some(list) = stations.get_mut(*i) {
            list.push(*s);
        }
    }
    let mut g = Graph {
        vertices: 0,
        edges: Vec::new(),
        around: Vec::new(),
        pads: Vec::new(),
    };
    let mut placed: Vec<Stations> = Vec::with_capacity(tracks.len());
    for (t, list) in tracks.iter().zip(&mut stations) {
        list.sort_by(f64::total_cmp);
        list.dedup_by(|a, b| (*a - *b).abs() < STATION_MM);
        let at: Vec<(f64, usize)> = list
            .iter()
            .map(|s| {
                let v = g.vertices;
                g.vertices += 1;
                (*s, v)
            })
            .collect();
        for w in at.windows(2) {
            g.edges.push(Edge {
                a: w[0].1,
                b: w[1].1,
                length: w[1].0 - w[0].0,
                kind: Kind::Track {
                    id: t.id,
                    lo: w[0].0,
                    hi: w[1].0,
                },
            });
        }
        placed.push(Stations { at });
    }
    // A via has one vertex per layer the net's tracks reach it on, joined
    // by the barrels between neighbouring layers.
    let mut via_layers: Vec<BTreeMap<CopperLayer, usize>> = vec![BTreeMap::new(); vias.len()];
    for (i, _, meet) in &meets {
        if let Meets::Via(v) = meet
            && let (Some(t), Some(map)) = (tracks.get(*i), via_layers.get_mut(*v))
        {
            map.entry(t.layer).or_insert_with(|| {
                let vertex = g.vertices;
                g.vertices += 1;
                vertex
            });
        }
    }
    for map in &via_layers {
        let mut by_z: Vec<(f64, usize)> = map
            .iter()
            .filter_map(|(layer, v)| mid_z(project, *layer).map(|z| (z, *v)))
            .collect();
        by_z.sort_by(|a, b| a.0.total_cmp(&b.0));
        for w in by_z.windows(2) {
            g.edges.push(Edge {
                a: w[0].1,
                b: w[1].1,
                length: w[1].0 - w[0].0,
                kind: Kind::Barrel,
            });
        }
    }
    let pad_vertices: Vec<usize> = pads
        .iter()
        .map(|_| {
            let v = g.vertices;
            g.vertices += 1;
            v
        })
        .collect();
    let joint = |g: &mut Graph, a: usize, b: usize| {
        g.edges.push(Edge {
            a,
            b,
            length: 0.0,
            kind: Kind::Joint,
        });
    };
    let mut joined: BTreeSet<usize> = BTreeSet::new();
    for (i, s, meet) in &meets {
        let (Some(here), Some(track)) =
            (placed.get(*i).and_then(|st| st.vertex(*s)), tracks.get(*i))
        else {
            continue;
        };
        match meet {
            Meets::TrackEnd(j, end) => {
                if let Some(there) = placed.get(*j).and_then(|st| st.vertex(*end)) {
                    joint(&mut g, here, there);
                }
            }
            Meets::Via(v) => {
                if let Some(there) = via_layers.get(*v).and_then(|m| m.get(&track.layer)) {
                    joint(&mut g, here, *there);
                }
            }
            Meets::Pad(p) => {
                if let Some(there) = pad_vertices.get(*p) {
                    joint(&mut g, here, *there);
                    joined.insert(*p);
                }
            }
        }
    }
    // A via in a pad joins it without counting a barrel (the length model).
    for (v, via) in vias.iter().enumerate() {
        let ring = CopperRing::of_via(via);
        for (p, pad) in pads.iter().enumerate() {
            let shared = layers
                .iter()
                .find(|l| via.on(**l, &layers) && pad.pad.on(**l));
            if let (Some(layer), Some(map), Some(there)) =
                (shared, via_layers.get(v), pad_vertices.get(p))
                && ring.touches(&pad.ring)
                && let Some(here) = map.get(layer).or_else(|| map.values().next())
            {
                joint(&mut g, *here, *there);
                joined.insert(p);
            }
        }
    }
    for (p, pad) in pads.iter().enumerate() {
        if joined.contains(&p)
            && let Some(vertex) = pad_vertices.get(p)
        {
            g.pads.push(PadVertex {
                vertex: *vertex,
                terminal: pad.pad.terminal,
                label: pad.label.clone(),
            });
        }
    }
    g.around = vec![Vec::new(); g.vertices];
    for (e, edge) in g.edges.iter().enumerate() {
        if let Some(list) = g.around.get_mut(edge.a) {
            list.push(e);
        }
        if let Some(list) = g.around.get_mut(edge.b) {
            list.push(e);
        }
    }
    Some(g)
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

/// Distances from the nearest of `from`, and the edge each vertex was
/// reached by.
struct Tree {
    distance: Vec<f64>,
    edge: Vec<Option<usize>>,
}

impl Graph {
    fn tree(&self, from: &[usize]) -> Tree {
        let mut tree = Tree {
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

    fn terminals(&self, terminal: Endpoint) -> Vec<usize> {
        self.pads
            .iter()
            .filter(|p| p.terminal == terminal)
            .map(|p| p.vertex)
            .collect()
    }

    fn instance_pads(&self, instance: Id) -> Vec<usize> {
        self.pads
            .iter()
            .filter(|p| p.terminal.instance == instance)
            .map(|p| p.vertex)
            .collect()
    }

    fn instances(&self) -> BTreeSet<Id> {
        self.pads.iter().map(|p| p.terminal.instance).collect()
    }

    fn label(&self, vertex: usize) -> String {
        self.pads
            .iter()
            .find(|p| p.vertex == vertex)
            .map_or_else(String::new, |p| p.label.clone())
    }

    /// The nearest of `to` from the nearest of `from`: its distance and
    /// the path as its edges with its two pads.
    fn path(&self, from: &[usize], to: &[usize]) -> Option<(f64, usize, usize, Vec<usize>)> {
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

    /// Lay the path `edges` between pads `start` and `end` out as a
    /// [`HalfPath`], everything else as its stubs.
    fn half(
        &self,
        project: &Project,
        net: Id,
        start: usize,
        end: usize,
        edges: &[usize],
    ) -> HalfPath {
        let on_path: BTreeSet<usize> = edges.iter().copied().collect();
        let mut path_vertices: BTreeSet<usize> = [start, end].into_iter().collect();
        let mut out = HalfPath {
            from: self.label(start),
            to: self.label(end),
            tracks: 0.0,
            vias: 0.0,
            spans: BTreeMap::new(),
            stubs: Vec::new(),
        };
        for e in edges {
            let Some(edge) = self.edges.get(*e) else {
                continue;
            };
            path_vertices.extend([edge.a, edge.b]);
            match edge.kind {
                Kind::Track { id, lo, hi } => {
                    out.tracks += edge.length;
                    out.spans.entry(id).or_default().push((lo, hi));
                }
                Kind::Barrel => out.vias += edge.length,
                Kind::Joint => {}
            }
        }
        out.stubs = self.stubs(project, net, &on_path, &path_vertices);
        out
    }

    /// The copper off the path, one stub per branch: branches meet only
    /// on the path, whose vertices therefore never join two of them.
    fn stubs(
        &self,
        project: &Project,
        net: Id,
        on_path: &BTreeSet<usize>,
        path_vertices: &BTreeSet<usize>,
    ) -> Vec<PairStub> {
        let mut parent: Vec<usize> = (0..self.vertices + self.edges.len()).collect();
        fn find(parent: &mut [usize], mut v: usize) -> usize {
            while let Some(&p) = parent.get(v) {
                if p == v {
                    break;
                }
                let up = parent.get(p).copied().unwrap_or(p);
                if let Some(slot) = parent.get_mut(v) {
                    *slot = up;
                }
                v = up;
            }
            v
        }
        // Each off-path edge is its own element, offset past the vertices,
        // joined to the off-path vertices at its ends.
        for (e, edge) in self.edges.iter().enumerate() {
            if on_path.contains(&e) {
                continue;
            }
            for v in [edge.a, edge.b] {
                if !path_vertices.contains(&v) {
                    let (a, b) = (find(&mut parent, v), find(&mut parent, self.vertices + e));
                    if let Some(slot) = parent.get_mut(a) {
                        *slot = b;
                    }
                }
            }
        }
        let mut branches: BTreeMap<usize, PairStub> = BTreeMap::new();
        let name = project.net_name(net).to_owned();
        for (e, edge) in self.edges.iter().enumerate() {
            if on_path.contains(&e) {
                continue;
            }
            let root = find(&mut parent, self.vertices + e);
            let stub = branches.entry(root).or_insert_with(|| PairStub {
                net,
                name: name.clone(),
                length: 0.0,
                tracks: Vec::new(),
                reaches: Vec::new(),
            });
            stub.length += edge.length;
            if let Kind::Track { id, .. } = edge.kind
                && !stub.tracks.contains(&id)
            {
                stub.tracks.push(id);
            }
        }
        for pad in &self.pads {
            if path_vertices.contains(&pad.vertex) {
                continue;
            }
            let root = find(&mut parent, pad.vertex);
            if let Some(stub) = branches.get_mut(&root) {
                stub.reaches.push(pad.label.clone());
            }
        }
        let mut stubs: Vec<PairStub> = branches.into_values().filter(|s| s.length > 1e-9).collect();
        for stub in &mut stubs {
            stub.tracks.sort_unstable();
            stub.reaches.sort();
        }
        stubs.sort_by(|a, b| b.length.total_cmp(&a.length).then(a.tracks.cmp(&b.tracks)));
        stubs
    }
}

struct WorldPadRing<'a> {
    pad: &'a WorldPad,
    ring: CopperRing,
    label: String,
}

/// Both halves between the same two parts, the two both reach that lie
/// farthest apart: `(from, to)` pads per half.
fn derived_between_parts(g: [&Graph; 2]) -> Option<[(Vec<usize>, Vec<usize>); 2]> {
    let [p, n] = g;
    let shared: Vec<Id> = p
        .instances()
        .intersection(&n.instances())
        .copied()
        .collect();
    let mut best: Option<(f64, Id, Id)> = None;
    for (i, a) in shared.iter().enumerate() {
        for b in shared.iter().skip(i + 1) {
            let reach = |g: &Graph| {
                g.path(&g.instance_pads(*a), &g.instance_pads(*b))
                    .map(|(d, ..)| d)
            };
            if let (Some(dp), Some(dn)) = (reach(p), reach(n))
                && best.is_none_or(|(d, ..)| dp + dn > d)
            {
                best = Some((dp + dn, *a, *b));
            }
        }
    }
    let (_, a, b) = best?;
    Some([
        (p.instance_pads(a), p.instance_pads(b)),
        (n.instance_pads(a), n.instance_pads(b)),
    ])
}

/// A half's longest pad-to-pad path, as its two pads.
fn longest(g: &Graph) -> Option<(Vec<usize>, Vec<usize>)> {
    let mut best: Option<(f64, usize, usize)> = None;
    for from in &g.pads {
        let tree = g.tree(&[from.vertex]);
        for to in &g.pads {
            let d = tree
                .distance
                .get(to.vertex)
                .copied()
                .unwrap_or(f64::INFINITY);
            if to.vertex != from.vertex && d.is_finite() && best.is_none_or(|(b, ..)| d > b + 1e-12)
            {
                best = Some((d, from.vertex, to.vertex));
            }
        }
    }
    best.map(|(_, a, b)| (vec![a], vec![b]))
}

fn set_ends(g: &Graph, ends: [Endpoint; 2]) -> (Vec<usize>, Vec<usize>) {
    (g.terminals(ends[0]), g.terminals(ends[1]))
}

/// A pad's name in a path or stub ("J1.B6").
fn pad_name(project: &Project, labels: &BTreeMap<(Id, PadId), String>, pad: &WorldPad) -> String {
    format!(
        "{}.{}",
        project
            .instance(pad.instance)
            .map_or("?", |i| i.designator.as_str()),
        labels
            .get(&(pad.instance, pad.pad))
            .cloned()
            .unwrap_or_else(|| pad.pad.to_string())
    )
}

/// The pads of `net` a pair endpoint can name, with their names, sorted.
pub fn net_pads(project: &Project, net: Id) -> Vec<(Endpoint, String)> {
    let labels = pad_labels(project);
    let mut out: Vec<(Endpoint, String)> = pads(project)
        .iter()
        .filter(|p| p.net == net)
        .map(|p| (p.terminal, pad_name(project, &labels, p)))
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// Measure `pair` on `project`: see the module note.
pub fn measure(project: &Project, pair: &DiffPair) -> PairMeasure {
    let all = pads(project);
    let labels: BTreeMap<(Id, PadId), String> = pad_labels(project);
    let rings = |net: Id| -> Vec<WorldPadRing<'_>> {
        all.iter()
            .filter(|p| p.net == net)
            .map(|pad| WorldPadRing {
                pad,
                ring: CopperRing::of_pad(pad),
                label: pad_name(project, &labels, pad),
            })
            .collect()
    };
    let (pads_p, pads_n) = (rings(pair.positive), rings(pair.negative));
    let (Some(gp), Some(gn)) = (
        graph(project, pair.positive, &pads_p),
        graph(project, pair.negative, &pads_n),
    ) else {
        return PairMeasure::default();
    };
    let lay = |ends: [(Vec<usize>, Vec<usize>); 2]| -> Option<[HalfPath; 2]> {
        let [ep, en] = ends;
        let (_, a, b, edges) = gp.path(&ep.0, &ep.1)?;
        let (_, c, d, others) = gn.path(&en.0, &en.1)?;
        Some([
            gp.half(project, pair.positive, a, b, &edges),
            gn.half(project, pair.negative, c, d, &others),
        ])
    };
    if let Some(PairEndpoints { positive, negative }) = pair.endpoints
        && let Some(halves) = lay([set_ends(&gp, positive), set_ends(&gn, negative)])
    {
        return PairMeasure {
            measured: PairMeasured::Set,
            halves: Some(halves),
        };
    }
    let derived = derived_between_parts([&gp, &gn])
        .or_else(|| Some([longest(&gp)?, longest(&gn)?]))
        .and_then(lay);
    match derived {
        Some(halves) => PairMeasure {
            measured: PairMeasured::Derived,
            halves: Some(halves),
        },
        None => PairMeasure::default(),
    }
}
