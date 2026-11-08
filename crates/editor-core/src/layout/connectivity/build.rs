//! Building the graph: elements, the one joining rule, joints with
//! stations, vertices, edges and islands.
use super::{CopperGraph, Dsu, Edge, EdgeKind, Element, ElementKind, Group, JOIN_EPS, Vertex};
use crate::diagnostics::ObjectRef;
use crate::geometry::{Point, polygon_contains, segment_distance, segments_cross};
use crate::ids::{Id, PadId};
use crate::layout::copper::{CURVE_SLACK_MM, CopperRing};
use crate::layout::grid::BoundsGrid;
use crate::layout::{MAX_CHECK_PAIRS, WorldPad, fill};
use crate::model::{CopperLayer, Project};
use std::collections::{BTreeMap, HashMap};

/// The farthest apart two outlines' bounds may be and still join.
pub(super) const JOIN_REACH_MM: f64 = 2.0 * CURVE_SLACK_MM + 2.0 * JOIN_EPS;
/// Most joints per element; past it the graph is partial.
const MAX_JOINTS_PER_ELEMENT: usize = 4;

/// Whether two outlines join: their copper overlaps or touches within
/// [`JOIN_EPS`] plus the curve slack of either. The one joining rule.
pub(super) fn joins(a: &CopperRing, b: &CopperRing) -> bool {
    let slack = a.slack + b.slack;
    a.gap(b, JOIN_EPS + slack + JOIN_EPS) <= JOIN_EPS + slack
}

/// One meeting of two elements, with the station on each (0 off tracks).
#[derive(Clone, Copy, Debug)]
struct Joint {
    a: usize,
    station_a: f64,
    b: usize,
    station_b: f64,
}

/// The geometry a joint's stations are read from.
#[derive(Clone, Copy)]
enum Geo<'a> {
    /// A track's centre line, half-width and length.
    Track {
        start: Point,
        end: Point,
        radius: f64,
        length: f64,
    },
    /// A land and its centre (a pad's terminal point, a via's centre).
    Land { centre: Point, ring: &'a CopperRing },
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// The station of the point of segment `a`–`b` (of `length`) nearest `p`.
fn station(p: Point, a: Point, b: Point, length: f64) -> f64 {
    let ab = sub(b, a);
    let len2 = dot(ab, ab);
    if len2 <= 0.0 {
        return 0.0;
    }
    (dot(sub(p, a), ab) / len2).clamp(0.0, 1.0) * length
}

fn at(a: Point, b: Point, length: f64, s: f64) -> Point {
    if length <= 0.0 {
        return a;
    }
    let t = s / length;
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// The nearest stations of segments `a`–`b` and `c`–`d`: where they
/// cross, or the closest pair of points.
fn nearest(a: Point, b: Point, ab: f64, c: Point, d: Point, cd: f64) -> (f64, f64) {
    if segments_cross(a, b, c, d) {
        let denominator = cross(sub(b, a), sub(d, c));
        if denominator != 0.0 {
            let t = (cross(sub(c, a), sub(d, c)) / denominator).clamp(0.0, 1.0);
            let s = t * ab;
            return (s, station(at(a, b, ab, s), c, d, cd));
        }
    }
    let candidates = [
        (segment_distance(a, c, d), 0.0, station(a, c, d, cd)),
        (segment_distance(b, c, d), ab, station(b, c, d, cd)),
        (segment_distance(c, a, b), station(c, a, b, ab), 0.0),
        (segment_distance(d, a, b), station(d, a, b, ab), cd),
    ];
    let best = candidates
        .into_iter()
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .unwrap_or((0.0, 0.0, 0.0));
    (best.1, best.2)
}

/// The stations where two joined elements meet. A track end whose copper
/// meets the other element joins there; otherwise the track joins at its
/// station nearest the other copper (a crossing, or a land's centre).
fn stations(x: Geo<'_>, y: Geo<'_>) -> Vec<(f64, f64)> {
    match (x, y) {
        (Geo::Land { .. }, Geo::Land { .. }) => vec![(0.0, 0.0)],
        (Geo::Track { .. }, Geo::Land { .. }) => track_land(x, y),
        (Geo::Land { .. }, Geo::Track { .. }) => {
            track_land(y, x).into_iter().map(|(s, t)| (t, s)).collect()
        }
        (
            Geo::Track {
                start: a,
                end: b,
                radius: r,
                length: ab,
            },
            Geo::Track {
                start: c,
                end: d,
                radius: q,
                length: cd,
            },
        ) => {
            let reach = r + q + JOIN_EPS;
            let mut out = Vec::new();
            for (end, s) in [(a, 0.0), (b, ab)] {
                if segment_distance(end, c, d) <= reach {
                    out.push((s, station(end, c, d, cd)));
                }
            }
            for (end, s) in [(c, 0.0), (d, cd)] {
                if segment_distance(end, a, b) <= reach {
                    out.push((station(end, a, b, ab), s));
                }
            }
            if out.is_empty() {
                out.push(nearest(a, b, ab, c, d, cd));
            }
            out
        }
    }
}

fn track_land(track: Geo<'_>, land: Geo<'_>) -> Vec<(f64, f64)> {
    let (
        Geo::Track {
            start,
            end,
            radius,
            length,
        },
        Geo::Land { centre, ring },
    ) = (track, land)
    else {
        return Vec::new();
    };
    let mut out: Vec<(f64, f64)> = [(start, 0.0), (end, length)]
        .into_iter()
        .filter(|(p, _)| joins(&CopperRing::capsule(*p, *p, radius), ring))
        .map(|(_, s)| (s, 0.0))
        .collect();
    if out.is_empty() {
        out.push((station(centre, start, end, length), 0.0));
    }
    out
}

/// The distance from `p` to a fill island (0 inside its copper).
fn region_distance(p: Point, outer: &[Point], holes: &[Vec<Point>]) -> f64 {
    let inside = polygon_contains(outer, p) && !holes.iter().any(|h| polygon_contains(h, p));
    if inside {
        return 0.0;
    }
    std::iter::once(outer)
        .chain(holes.iter().map(Vec::as_slice))
        .flat_map(edges)
        .map(|(a, b)| segment_distance(p, a, b))
        .fold(f64::INFINITY, f64::min)
}

fn edges(ring: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    ring.iter()
        .zip(ring.iter().cycle().skip(1))
        .map(|(a, b)| (*a, *b))
}

/// Where a track joined to a fill island meets it: each end whose copper
/// reaches the island, else the station nearest the island's edge.
fn fill_stations(
    start: Point,
    end: Point,
    radius: f64,
    length: f64,
    outer: &[Point],
    holes: &[Vec<Point>],
) -> Vec<f64> {
    let reach = radius + JOIN_EPS;
    let mut out: Vec<f64> = [(start, 0.0), (end, length)]
        .into_iter()
        .filter(|(p, _)| region_distance(*p, outer, holes) <= reach)
        .map(|(_, s)| s)
        .collect();
    if out.is_empty() {
        let mut best = (f64::INFINITY, 0.0);
        for (c, d) in std::iter::once(outer)
            .chain(holes.iter().map(Vec::as_slice))
            .flat_map(edges)
        {
            let cd = (d[0] - c[0]).hypot(d[1] - c[1]);
            let (s, t) = nearest(start, end, length, c, d, cd);
            let gap = {
                let p = at(start, end, length, s);
                let q = at(c, d, cd, t);
                (p[0] - q[0]).hypot(p[1] - q[1])
            };
            if gap < best.0 {
                best = (gap, s);
            }
        }
        out.push(best.1);
    }
    out
}

impl CopperGraph {
    /// Build the graph of `project`'s copper. The engine caches it per
    /// revision ([`crate::Engine::copper_graph`]).
    pub fn of(project: &Project) -> Self {
        Self::with_pads(project, crate::layout::pads(project))
    }

    /// The graph of `project`'s copper with `pads` as its connected pads,
    /// each on the net it carries. Update PCB builds the updated board with
    /// every pad on the net its copper had, to ask which pads that copper
    /// would join after the update.
    pub fn with_pads(project: &Project, pads: Vec<WorldPad>) -> Self {
        let layers = project.stackup.layers();
        let tracks = project.tracks.clone();
        let vias = project.vias.clone();
        let mut elements = Vec::new();
        let mut shapes = Vec::new();
        let element = |kind, net, layer| Element {
            kind,
            net,
            layer,
            first: 0,
            count: 0,
        };
        let mut lands = Vec::with_capacity(tracks.len() + vias.len() + pads.len());
        for (index, track) in tracks.iter().enumerate() {
            lands.push(elements.len()..elements.len() + 1);
            shapes.push(CopperRing::capsule(
                track.start,
                track.end,
                track.width / 2.0,
            ));
            elements.push(element(
                ElementKind::Track { index },
                track.net,
                track.layer,
            ));
        }
        let mut via_lands = Vec::with_capacity(vias.len());
        for (index, via) in vias.iter().enumerate() {
            let first = elements.len();
            shapes.push(CopperRing::capsule(
                via.position,
                via.position,
                via.diameter / 2.0,
            ));
            for layer in layers.iter().filter(|l| via.on(**l, &layers)) {
                elements.push(element(ElementKind::Via { index }, via.net, *layer));
            }
            via_lands.push(first..elements.len());
            lands.push(first..elements.len());
        }
        let mut pad_lands = Vec::with_capacity(pads.len());
        for (index, pad) in pads.iter().enumerate() {
            let first = elements.len();
            pad_lands.push(first);
            shapes.push(CopperRing::of_pad(pad));
            for layer in layers.iter().filter(|l| pad.on(**l)) {
                elements.push(element(ElementKind::Pad { index }, pad.net, *layer));
            }
            lands.push(first..elements.len());
        }
        let links = fill::links(project);
        let first_fill = elements.len();
        for link in &links {
            let Some((net, layer)) = project
                .fill(link.fill)
                .and_then(|f| f.net.map(|n| (n, f.layer)))
            else {
                continue;
            };
            elements.push(element(
                ElementKind::Fill {
                    fill: link.fill,
                    region: link.region,
                },
                net,
                layer,
            ));
        }
        let mut graph = Self {
            layers,
            pads,
            tracks,
            vias,
            elements,
            shapes,
            lands,
            via_lands,
            pad_lands,
            ..Self::default()
        };
        let mut joints = graph.join();
        joints.extend(graph.fill_joints(project, &links, first_fill));
        graph.connect(joints);
        graph
    }

    /// The station geometry of `element` with outline `ring`.
    fn geo<'a>(&'a self, element: &Element, ring: &'a CopperRing) -> Option<Geo<'a>> {
        Some(match element.kind {
            ElementKind::Track { index } => {
                let t = self.tracks.get(index)?;
                Geo::Track {
                    start: t.start,
                    end: t.end,
                    radius: t.width / 2.0,
                    length: (t.end[0] - t.start[0]).hypot(t.end[1] - t.start[1]),
                }
            }
            ElementKind::Via { index } => Geo::Land {
                centre: self.vias.get(index)?.position,
                ring,
            },
            ElementKind::Pad { index } => Geo::Land {
                centre: self.pads.get(index)?.position,
                ring,
            },
            ElementKind::Fill { .. } => return None,
        })
    }

    /// File the outlines by net, and find every joint between them. Each
    /// outline is tested once, whatever the layers its lands are on: two
    /// outlines that join meet on every layer both have a land on.
    fn join(&mut self) -> Vec<Joint> {
        let mut members: BTreeMap<Id, Vec<usize>> = BTreeMap::new();
        for (shape, lands) in self.lands.iter().enumerate() {
            if let Some(e) = self.elements.get(lands.start) {
                members.entry(e.net).or_default().push(shape);
            }
        }
        for (net, members) in members {
            let grid = BoundsGrid::new(
                members
                    .iter()
                    .map(|s| self.shapes.get(*s).and_then(|r| r.bounds)),
                JOIN_REACH_MM,
            );
            self.groups.insert(net, Group { grid, members });
        }
        let (joints, complete) = self.scan();
        self.partial = !complete;
        joints
    }

    /// The land elements of outlines `a` and `b` on the layers both reach.
    pub(super) fn common_lands(&self, a: usize, b: usize) -> Vec<(usize, usize)> {
        let (Some(x), Some(y)) = (self.lands.get(a), self.lands.get(b)) else {
            return Vec::new();
        };
        let layer = |e: usize| self.elements.get(e).map(|e| e.layer);
        x.clone()
            .flat_map(|ea| {
                y.clone()
                    .filter(move |eb| layer(ea).is_some() && layer(ea) == layer(*eb))
                    .map(move |eb| (ea, eb))
            })
            .collect()
    }

    /// Every joint between filed outlines, and whether the scan finished
    /// within its limits.
    fn scan(&self) -> (Vec<Joint>, bool) {
        let limit = self.elements.len().saturating_mul(MAX_JOINTS_PER_ELEMENT);
        let mut joints = Vec::new();
        let mut near = Vec::new();
        let mut pairs = 0usize;
        for group in self.groups.values() {
            for (local, &a) in group.members.iter().enumerate() {
                let Some(ring_a) = self.shapes.get(a) else {
                    continue;
                };
                let Some(bounds) = ring_a.bounds else {
                    continue;
                };
                group.grid.near(&bounds, JOIN_REACH_MM, &mut near);
                for &b in near
                    .iter()
                    .filter(|j| **j > local)
                    .filter_map(|j| group.members.get(*j))
                {
                    pairs += 1;
                    if pairs > MAX_CHECK_PAIRS || joints.len() > limit {
                        return (joints, false);
                    }
                    let Some(ring_b) = self.shapes.get(b) else {
                        continue;
                    };
                    let common = self.common_lands(a, b);
                    let Some(&(ea, eb)) = common.first() else {
                        continue;
                    };
                    if !joins(ring_a, ring_b) {
                        continue;
                    }
                    let (Some(x), Some(y)) = (
                        self.elements.get(ea).and_then(|e| self.geo(e, ring_a)),
                        self.elements.get(eb).and_then(|e| self.geo(e, ring_b)),
                    ) else {
                        continue;
                    };
                    let stations = stations(x, y);
                    for (ea, eb) in common {
                        joints.extend(stations.iter().map(|&(sa, sb)| Joint {
                            a: ea,
                            station_a: sa,
                            b: eb,
                            station_b: sb,
                        }));
                    }
                }
            }
        }
        (joints, true)
    }

    /// The joints fill compute stored for each fresh fill island, with the
    /// stations where tracks meet the island.
    fn fill_joints(
        &self,
        project: &Project,
        links: &[fill::FillLink],
        first_fill: usize,
    ) -> Vec<Joint> {
        if links.is_empty() {
            return Vec::new();
        }
        // Tracks are the first elements, one each, in document order.
        let by_track: HashMap<Id, usize> = self
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id, i))
            .collect();
        let by_via: HashMap<Id, usize> = self
            .vias
            .iter()
            .enumerate()
            .map(|(i, v)| (v.id, i))
            .collect();
        let by_pad: HashMap<(Id, PadId), usize> = self
            .pads
            .iter()
            .enumerate()
            .map(|(i, p)| ((p.instance, p.pad), i))
            .collect();
        let land = |lands: Option<std::ops::Range<usize>>, layer: CopperLayer| {
            lands?.find(|e| self.elements.get(*e).is_some_and(|e| e.layer == layer))
        };
        let mut out = Vec::new();
        for (f, e) in self.elements.iter().enumerate().skip(first_fill) {
            let ElementKind::Fill { fill, region } = e.kind else {
                continue;
            };
            let Some(link) = links.iter().find(|l| l.fill == fill && l.region == region) else {
                continue;
            };
            let island = project
                .fill(fill)
                .and_then(|f| f.computed.as_ref())
                .and_then(|c| c.regions.get(usize::try_from(region).ok()?));
            for member in &link.members {
                match *member {
                    ObjectRef::Pad { instance, pad } => {
                        // Outlines run tracks, vias, then pads.
                        let lands = by_pad.get(&(instance, pad)).and_then(|i| {
                            self.lands
                                .get(self.tracks.len() + self.vias.len() + i)
                                .cloned()
                        });
                        if let Some(p) = land(lands, e.layer) {
                            out.push(Joint {
                                a: p,
                                station_a: 0.0,
                                b: f,
                                station_b: 0.0,
                            });
                        }
                    }
                    ObjectRef::Via { id } => {
                        let lands = by_via
                            .get(&id)
                            .and_then(|i| self.via_lands.get(*i).cloned());
                        if let Some(v) = land(lands, e.layer) {
                            out.push(Joint {
                                a: v,
                                station_a: 0.0,
                                b: f,
                                station_b: 0.0,
                            });
                        }
                    }
                    ObjectRef::Track { id } => {
                        let Some(&t) = by_track.get(&id) else {
                            continue;
                        };
                        let Some(track) = self.elements.get(t).and_then(|e| match e.kind {
                            ElementKind::Track { index } => self.tracks.get(index),
                            _ => None,
                        }) else {
                            continue;
                        };
                        let length =
                            (track.end[0] - track.start[0]).hypot(track.end[1] - track.start[1]);
                        let stations = match island {
                            Some(r) => fill_stations(
                                track.start,
                                track.end,
                                track.width / 2.0,
                                length,
                                &r.outer,
                                &r.holes,
                            ),
                            None => vec![0.0],
                        };
                        out.extend(stations.into_iter().map(|s| Joint {
                            a: t,
                            station_a: s,
                            b: f,
                            station_b: 0.0,
                        }));
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// Lay out the vertices (track stations), the edges and the islands.
    fn connect(&mut self, joints: Vec<Joint>) {
        // Stations per track element.
        let mut stations: BTreeMap<usize, Vec<f64>> = BTreeMap::new();
        for (i, e) in self.elements.iter().enumerate() {
            if let ElementKind::Track { index } = e.kind
                && let Some(t) = self.tracks.get(index)
            {
                let length = (t.end[0] - t.start[0]).hypot(t.end[1] - t.start[1]);
                stations.insert(i, vec![0.0, length]);
            }
        }
        for j in &joints {
            for (element, s) in [(j.a, j.station_a), (j.b, j.station_b)] {
                if let Some(list) = stations.get_mut(&element) {
                    list.push(s);
                }
            }
        }
        for list in stations.values_mut() {
            list.sort_by(f64::total_cmp);
            list.dedup_by(|later, kept| (*later - *kept).abs() <= JOIN_EPS);
        }
        let mut vertices = Vec::new();
        for (i, e) in self.elements.iter_mut().enumerate() {
            e.first = vertices.len();
            match stations.get(&i) {
                Some(list) => vertices.extend(list.iter().map(|&station| Vertex {
                    element: i,
                    station,
                })),
                None => vertices.push(Vertex {
                    element: i,
                    station: 0.0,
                }),
            }
            e.count = vertices.len() - e.first;
        }
        self.vertices = vertices;
        let mut edges = Vec::new();
        let mut previous: Option<&Element> = None;
        for e in &self.elements {
            for v in e.first + 1..e.first + e.count {
                edges.push(Edge {
                    a: v - 1,
                    b: v,
                    kind: EdgeKind::Stretch,
                });
            }
            // Lands of one via or pad are consecutive elements.
            if let Some(p) = previous {
                let kind = match (p.kind, e.kind) {
                    (ElementKind::Via { index: x }, ElementKind::Via { index: y }) if x == y => {
                        Some(EdgeKind::Barrel)
                    }
                    (ElementKind::Pad { index: x }, ElementKind::Pad { index: y }) if x == y => {
                        Some(EdgeKind::Plated)
                    }
                    _ => None,
                };
                if let Some(kind) = kind {
                    edges.push(Edge {
                        a: p.first,
                        b: e.first,
                        kind,
                    });
                }
            }
            previous = Some(e);
        }
        let mut joined: Vec<(usize, usize)> = joints
            .iter()
            .filter_map(|j| {
                let a = self.vertex_at(j.a, j.station_a)?;
                let b = self.vertex_at(j.b, j.station_b)?;
                (a != b).then_some((a.min(b), a.max(b)))
            })
            .collect();
        joined.sort_unstable();
        joined.dedup();
        edges.extend(joined.into_iter().map(|(a, b)| Edge {
            a,
            b,
            kind: EdgeKind::Joint,
        }));
        let n = self.vertices.len();
        let mut degree = vec![0usize; n + 1];
        for e in &edges {
            for v in [e.a, e.b] {
                if let Some(d) = degree.get_mut(v + 1) {
                    *d += 1;
                }
            }
        }
        for v in 0..n {
            let before = degree.get(v).copied().unwrap_or(0);
            if let Some(d) = degree.get_mut(v + 1) {
                *d += before;
            }
        }
        let mut fill = degree.clone();
        let mut adjacent = vec![0usize; edges.len() * 2];
        for e in &edges {
            for (from, to) in [(e.a, e.b), (e.b, e.a)] {
                if let Some(slot) = fill.get_mut(from)
                    && let Some(cell) = adjacent.get_mut(*slot)
                {
                    *cell = to;
                    *slot += 1;
                }
            }
        }
        let mut dsu = Dsu::new(n);
        for e in &edges {
            dsu.union(e.a, e.b);
        }
        let mut number = vec![usize::MAX; n];
        let mut islands = 0usize;
        let mut island = Vec::with_capacity(n);
        for v in 0..n {
            let root = dsu.find(v);
            let Some(slot) = number.get_mut(root) else {
                continue;
            };
            if *slot == usize::MAX {
                *slot = islands;
                islands += 1;
            }
            island.push(*slot);
        }
        self.islands = islands;
        self.island = island;
        self.offsets = degree;
        self.adjacent = adjacent;
        self.edges = edges;
    }

    /// The vertex of `element` at `station` (the nearest of a track's).
    fn vertex_at(&self, element: usize, station: f64) -> Option<usize> {
        let e = self.elements.get(element)?;
        let list = self.vertices.get(e.first..e.first + e.count)?;
        let at = list.partition_point(|v| v.station < station);
        let best = [at.checked_sub(1), Some(at)]
            .into_iter()
            .flatten()
            .filter(|k| *k < list.len())
            .min_by(|x, y| {
                let dx = list
                    .get(*x)
                    .map_or(f64::INFINITY, |v| (v.station - station).abs());
                let dy = list
                    .get(*y)
                    .map_or(f64::INFINITY, |v| (v.station - station).abs());
                dx.total_cmp(&dy)
            })?;
        Some(e.first + best)
    }
}
