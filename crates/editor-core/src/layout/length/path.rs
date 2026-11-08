//! A pair half measured along its signal path: the copper between the
//! pair's two endpoints, read off the copper graph
//! ([`CopperGraph::net_paths`]). Joining, barrels and where a track enters
//! a pad are the graph's; this module only chooses the endpoints. Copper
//! off the path is ordinary net copper: a track between pins, or a stub,
//! which the board check reports on any net (`board.track_dangling`).
//!
//! **Endpoints.** Those the pair sets ([`PairEndpoints`]), when both halves
//! connect them. Otherwise they are derived: of the parts both halves reach,
//! the two farthest apart along the copper, each half taking its shortest
//! path between them. When the halves share fewer than two parts, each half
//! takes its longest pad-to-pad path. When a half has no such path, because
//! the route is unfinished, the pair is measured by total copper as before.
//! A set endpoint that left its net (a schematic edit) is ignored the same
//! way. So is a pair whose graph is incomplete, or whose search passes
//! [`MAX_PATH_WORK`](crate::layout::connectivity::MAX_PATH_WORK).
//!
//! A path may run through a fill island of its net: it is measured (the
//! hop adds no track length) and marked [`HalfPath::via_fill`].
use super::super::connectivity::{NetPaths, Walk};
use super::super::{CopperGraph, WorldPad, pad_labels, pads};
use crate::ids::{Endpoint, Id, PadId};
use crate::model::Project;
use crate::rules::signal::{DiffPair, PairEndpoints};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
    /// The path runs through a fill island.
    pub via_fill: bool,
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

/// One half's path view with its pads' names.
struct Half<'a> {
    paths: NetPaths,
    graph: &'a CopperGraph,
    names: &'a BTreeMap<(Id, PadId), String>,
    project: &'a Project,
}

impl Half<'_> {
    fn terminals(&self, terminal: Endpoint) -> Vec<usize> {
        self.paths
            .pads()
            .iter()
            .filter(|p| p.terminal == terminal)
            .flat_map(|p| p.lands.iter().copied())
            .collect()
    }

    fn instance_pads(&self, instance: Id) -> Vec<usize> {
        self.paths
            .pads()
            .iter()
            .filter(|p| p.terminal.instance == instance)
            .flat_map(|p| p.lands.iter().copied())
            .collect()
    }

    fn instances(&self) -> BTreeSet<Id> {
        self.paths
            .pads()
            .iter()
            .map(|p| p.terminal.instance)
            .collect()
    }

    fn label(&self, vertex: usize) -> String {
        self.paths
            .pad_at(vertex)
            .and_then(|p| self.graph.pads().get(p.pad))
            .map_or_else(String::new, |pad| pad_name(self.project, self.names, pad))
    }

    /// Lay the path `edges` between pads `start` and `end` out as a
    /// [`HalfPath`].
    fn half(&self, start: usize, end: usize, edges: &[usize]) -> HalfPath {
        let mut out = HalfPath {
            from: self.label(start),
            to: self.label(end),
            tracks: 0.0,
            vias: 0.0,
            spans: BTreeMap::new(),
            via_fill: false,
        };
        for edge in edges.iter().filter_map(|e| self.paths.edges().get(*e)) {
            match edge.walk {
                Walk::Track { id, lo, hi } => {
                    out.tracks += edge.length;
                    out.spans.entry(id).or_default().push((lo, hi));
                }
                Walk::Barrel => out.vias += edge.length,
                Walk::Fill => out.via_fill = true,
                Walk::Joint => {}
            }
        }
        out
    }
}

/// Both halves between the same two parts, the two both reach that lie
/// farthest apart: `(from, to)` pads per half.
fn derived_between_parts(halves: [&Half<'_>; 2]) -> Option<[(Vec<usize>, Vec<usize>); 2]> {
    let [p, n] = halves;
    let shared: Vec<Id> = p
        .instances()
        .intersection(&n.instances())
        .copied()
        .collect();
    // From each shared part, one search per half gives its distance to
    // every other shared part.
    let reach = |h: &Half<'_>| -> Vec<Vec<f64>> {
        shared
            .iter()
            .map(|a| {
                let tree = h.paths.tree(&h.instance_pads(*a));
                shared
                    .iter()
                    .map(|b| {
                        h.instance_pads(*b)
                            .iter()
                            .filter_map(|v| tree.distance.get(*v).copied())
                            .fold(f64::INFINITY, f64::min)
                    })
                    .collect()
            })
            .collect()
    };
    let (dp, dn) = (reach(p), reach(n));
    let mut best: Option<(f64, Id, Id)> = None;
    for (i, a) in shared.iter().enumerate() {
        for (j, b) in shared.iter().enumerate().skip(i + 1) {
            let at = |d: &[Vec<f64>]| d.get(i).and_then(|row| row.get(j)).copied();
            if let (Some(x), Some(y)) = (at(&dp), at(&dn))
                && x.is_finite()
                && y.is_finite()
                && best.is_none_or(|(d, ..)| x + y > d)
            {
                best = Some((x + y, *a, *b));
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
fn longest(h: &Half<'_>) -> Option<(Vec<usize>, Vec<usize>)> {
    let mut best: Option<(f64, usize, usize)> = None;
    let pads = h.paths.pads();
    for (i, from) in pads.iter().enumerate() {
        let tree = h.paths.tree(&from.lands);
        for (j, to) in pads.iter().enumerate() {
            let d = to
                .lands
                .iter()
                .filter_map(|v| tree.distance.get(*v).copied())
                .fold(f64::INFINITY, f64::min);
            if i != j && d.is_finite() && best.is_none_or(|(b, ..)| d > b + 1e-12) {
                best = Some((d, i, j));
            }
        }
    }
    let (_, a, b) = best?;
    let lands = |k: usize| pads.get(k).map(|p| p.lands.clone()).unwrap_or_default();
    Some((lands(a), lands(b)))
}

/// A pad's name in a path ("J1.B6").
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

/// Measure `pair` on `project`, whose copper graph is `graph`: see the
/// module note.
pub fn measure(project: &Project, graph: &CopperGraph, pair: &DiffPair) -> PairMeasure {
    if !graph.complete() {
        return PairMeasure::default();
    }
    let names = pad_labels(project);
    let half = |net: Id| Half {
        paths: graph.net_paths(net),
        graph,
        names: &names,
        project,
    };
    let (hp, hn) = (half(pair.positive), half(pair.negative));
    let lay = |ends: [(Vec<usize>, Vec<usize>); 2]| -> Option<[HalfPath; 2]> {
        let [ep, en] = ends;
        let (_, a, b, edges) = hp.paths.path(&ep.0, &ep.1)?;
        let (_, c, d, others) = hn.paths.path(&en.0, &en.1)?;
        Some([hp.half(a, b, &edges), hn.half(c, d, &others)])
    };
    let exhausted = || hp.paths.exhausted() || hn.paths.exhausted();
    if let Some(PairEndpoints { positive, negative }) = pair.endpoints {
        let ends = [
            (hp.terminals(positive[0]), hp.terminals(positive[1])),
            (hn.terminals(negative[0]), hn.terminals(negative[1])),
        ];
        if let Some(halves) = lay(ends) {
            if exhausted() {
                return PairMeasure::default();
            }
            return PairMeasure {
                measured: PairMeasured::Set,
                halves: Some(halves),
            };
        }
    }
    let derived = derived_between_parts([&hp, &hn])
        .or_else(|| Some([longest(&hp)?, longest(&hn)?]))
        .and_then(lay);
    match derived {
        Some(halves) if !exhausted() => PairMeasure {
            measured: PairMeasured::Derived,
            halves: Some(halves),
        },
        _ => PairMeasure::default(),
    }
}
