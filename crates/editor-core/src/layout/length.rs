//! Routed length, pair coupling and the high-speed checks.
//!
//! Every length is read off the copper graph ([`CopperGraph`]), which
//! decides which copper is joined and carries each via barrel's length.
//!
//! **Length model.** A net's routed length is the sum of its track
//! segments' centre-line lengths plus, for each via, the barrel between
//! the outermost of its lands that the net's tracks join: the distance
//! between those layers' foil mid-planes in the stackup
//! ([`CopperGraph::net_lengths`]). A through via's stub beyond them,
//! pad-internal paths and via-in-pad joins are not counted. For a two-pin
//! net this is the pad-to-pad path length; for a branched net it is the
//! total copper length.
//!
//! **Pair halves** are measured along their signal path instead, the copper
//! between the pair's endpoints ([`path`]). Copper off the path is the
//! net's like any other: in its total length, and a stub if it reaches no
//! pad. Skew, uncoupled length, a half's tuning target and its length in a
//! group all read the path ([`signal_length`]).
//!
//! **Coupling.** Two segments of a pair's halves on one layer run coupled
//! where they are parallel and their edge gap is the pair gap within
//! [`gap_tolerance`]; the rest of each half (pad fan-out included) is
//! uncoupled. Parallel stretches closer than the gap are `pair.gap`
//! findings.
use super::{CopperGraph, impedance};
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::geometry::Point;
use crate::ids::Id;
use crate::model::{CopperLayer, Project, Track};
use crate::rules::RulesCommand;
use crate::rules::signal::{DiffPair, DiffPairId, LengthGroupId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub mod path;

pub use path::{PairMeasure, PairMeasured};

/// The most length one tuning adds, mm.
pub const MAX_TUNE_MM: f64 = 1000.0;
/// Segments within this angle are parallel, in sine.
const PARALLEL_SIN: f64 = 0.017;

/// One net's routed length, mm.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct NetLength {
    /// Track centre lines.
    pub tracks: f64,
    /// Via barrels between the layers the net uses there.
    pub vias: f64,
    pub total: f64,
    pub segments: usize,
    pub via_count: usize,
}

/// Every net's routed length (nets without copper are absent); see
/// [`CopperGraph::net_lengths`].
pub fn net_lengths(project: &Project) -> BTreeMap<Id, NetLength> {
    CopperGraph::of(project).net_lengths()
}

/// One net's routed length.
pub fn net_length(project: &Project, net: Id) -> NetLength {
    net_lengths(project).get(&net).copied().unwrap_or_default()
}

/// The lengths the checks compare: each net's copper and, for a pair's
/// halves, their signal paths.
struct Measured {
    copper: BTreeMap<Id, NetLength>,
    pairs: BTreeMap<DiffPairId, PairMeasure>,
    paths: BTreeMap<Id, f64>,
}

impl Measured {
    fn of(project: &Project, graph: &CopperGraph) -> Self {
        Self::of_pairs(project, graph, &project.rules.diff_pairs)
    }

    /// [`Measured::of`] with the signal paths of `pairs` alone.
    fn of_pairs(project: &Project, graph: &CopperGraph, pairs: &[DiffPair]) -> Self {
        let mut out = Self {
            copper: graph.net_lengths(),
            pairs: BTreeMap::new(),
            paths: BTreeMap::new(),
        };
        for pair in pairs {
            let measure = path::measure(project, graph, pair);
            if let Some([p, n]) = &measure.halves {
                out.paths.insert(pair.positive, p.total());
                out.paths.insert(pair.negative, n.total());
            }
            out.pairs.insert(pair.id, measure);
        }
        out
    }

    /// The length of `net` the checks compare.
    fn length(&self, net: Id) -> f64 {
        self.paths
            .get(&net)
            .copied()
            .or_else(|| self.copper.get(&net).map(|l| l.total))
            .unwrap_or(0.0)
    }

    /// Each half's uncoupled length.
    fn uncoupled(&self, pair: DiffPairId, coupling: &PairCoupling) -> [f64; 2] {
        match self.pairs.get(&pair).and_then(|m| m.halves.as_ref()) {
            Some([p, n]) => [coupling.uncoupled_on(0, p), coupling.uncoupled_on(1, n)],
            None => coupling.coupling.uncoupled,
        }
    }
}

/// The length of `net` its pair, group and tuning targets compare: a pair
/// half's signal path, otherwise its total copper.
pub fn signal_length(project: &Project, net: Id) -> f64 {
    signal_length_with(project, &CopperGraph::of(project), net)
}

/// [`signal_length`] on `graph`, the copper graph of `project`.
pub fn signal_length_with(project: &Project, graph: &CopperGraph, net: Id) -> f64 {
    match project.rules.pair_of(net) {
        Some(_) => Measured::of(project, graph).length(net),
        None => graph
            .net_lengths()
            .get(&net)
            .map_or(0.0, |length| length.total),
    }
}

/// How far a coupled stretch's gap may stray from the pair gap, mm: the
/// pair class's (its positive net's class) `pair_gap_tolerance`, else the
/// rules' share of `gap` and at least their minimum.
pub fn gap_tolerance(project: &Project, positive: Id, gap: f64) -> f64 {
    super::net_class(project, positive)
        .and_then(|c| project.rules.class(c))
        .and_then(|c| c.signal.pair_gap_tolerance)
        .unwrap_or_else(|| {
            let pair = project.rules.effective().pair;
            (pair.gap_tolerance_share * gap).max(pair.gap_tolerance)
        })
}

/// How many pair gaps apart two parallel stretches of a pair's halves may
/// run and still count as one pair spaced too close (`pair.gap`); farther
/// apart they are separate traces. The factor is the widely quoted "3W"
/// spacing rule of thumb, beyond which neighbouring traces are commonly
/// treated as uncoupled (Altium, "Differential Crosstalk and Differential
/// Pair Spacing in PCB Design",
/// <https://resources.altium.com/p/differential-crosstalk-and-spacing-between-differential-pairs>,
/// read 2026-11-11).
pub const TOO_CLOSE_GAPS: f64 = 3.0;

/// A pair's coupling on the board.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Coupling {
    /// Coupled length of [positive, negative], mm.
    pub coupled: [f64; 2],
    /// Uncoupled length of [positive, negative], mm.
    pub uncoupled: [f64; 2],
    /// Parallel stretches closer than the gap: (positive track, negative
    /// track, layer, edge gap, the pair gap there).
    pub too_close: Vec<(Id, Id, CopperLayer, f64, f64)>,
    /// Coupled stretches: (positive track, negative track, layer, width,
    /// edge gap), for the impedance check.
    pub coupled_runs: Vec<(Id, Id, CopperLayer, f64, f64)>,
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

/// Merged length of `intervals` (each `[lo, hi]`).
fn covered(mut intervals: Vec<(f64, f64)>) -> f64 {
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut total = 0.0;
    let mut current: Option<(f64, f64)> = None;
    for (lo, hi) in intervals {
        current = match current {
            Some((a, b)) if lo <= b => Some((a, b.max(hi))),
            Some((a, b)) => {
                total += b - a;
                Some((lo, hi))
            }
            None => Some((lo, hi)),
        };
    }
    if let Some((a, b)) = current {
        total += b - a;
    }
    total
}

/// The overlap of `other` projected onto `track`'s axis, as an interval
/// of distances from `track.start`.
fn overlap(track: &Track, other: &Track) -> Option<(f64, f64)> {
    let length = crate::geometry::distance(track.start, track.end);
    if length < 1e-12 {
        return None;
    }
    let d = [
        (track.end[0] - track.start[0]) / length,
        (track.end[1] - track.start[1]) / length,
    ];
    let a = dot(sub(other.start, track.start), d);
    let b = dot(sub(other.end, track.start), d);
    let (lo, hi) = (a.min(b).max(0.0), a.max(b).min(length));
    (hi - lo > 1e-9).then_some((lo, hi))
}

/// A pair's coupling with where on each half's tracks it holds.
struct PairCoupling {
    coupling: Coupling,
    /// Coupled intervals per `[positive, negative]` track, as distances
    /// from its start.
    intervals: [BTreeMap<Id, Vec<(f64, f64)>>; 2],
}

impl PairCoupling {
    /// How much of half `side`'s signal path runs uncoupled.
    fn uncoupled_on(&self, side: usize, path: &path::HalfPath) -> f64 {
        let mut coupled = 0.0;
        for (track, spans) in &path.spans {
            let Some(intervals) = self.intervals.get(side).and_then(|m| m.get(track)) else {
                continue;
            };
            let within: Vec<(f64, f64)> = intervals
                .iter()
                .flat_map(|(a, b)| {
                    spans
                        .iter()
                        .map(move |(lo, hi)| (a.max(*lo), b.min(*hi)))
                        .filter(|(x, y)| y > x)
                })
                .collect();
            coupled += covered(within);
        }
        (path.tracks - coupled).max(0.0)
    }
}

/// Where the halves of `pair` run coupled at the gap their layers give.
pub fn coupling(project: &Project, pair: &DiffPair) -> Coupling {
    pair_coupling(project, pair).coupling
}

fn pair_coupling(project: &Project, pair: &DiffPair) -> PairCoupling {
    let halves = [pair.positive, pair.negative];
    let tracks: [Vec<&Track>; 2] =
        halves.map(|net| project.tracks.iter().filter(|t| t.net == net).collect());
    let mut gaps: BTreeMap<CopperLayer, f64> = BTreeMap::new();
    let mut gap_on = |layer: CopperLayer| {
        *gaps
            .entry(layer)
            .or_insert_with(|| impedance::pair_width_gap(project, pair.positive, layer).1)
    };
    let mut out = Coupling::default();
    let mut intervals: [BTreeMap<Id, Vec<(f64, f64)>>; 2] = Default::default();
    for p in &tracks[0] {
        let lp = crate::geometry::distance(p.start, p.end);
        if lp < 1e-12 {
            continue;
        }
        let dp = [(p.end[0] - p.start[0]) / lp, (p.end[1] - p.start[1]) / lp];
        for n in tracks[1].iter().filter(|n| n.layer == p.layer) {
            let ln = crate::geometry::distance(n.start, n.end);
            if ln < 1e-12 {
                continue;
            }
            let dn = [(n.end[0] - n.start[0]) / ln, (n.end[1] - n.start[1]) / ln];
            if cross(dp, dn).abs() > PARALLEL_SIN {
                continue;
            }
            let (Some(on_p), Some(on_n)) = (overlap(p, n), overlap(n, p)) else {
                continue;
            };
            let centre = cross(dp, sub(n.start, p.start)).abs();
            let edge = centre - (p.width + n.width) / 2.0;
            let gap = gap_on(p.layer);
            let tol = gap_tolerance(project, pair.positive, gap);
            if (edge - gap).abs() <= tol {
                intervals[0].entry(p.id).or_default().push(on_p);
                intervals[1].entry(n.id).or_default().push(on_n);
                out.coupled_runs
                    .push((p.id, n.id, p.layer, (p.width + n.width) / 2.0, edge));
            } else if edge < gap - tol && edge < TOO_CLOSE_GAPS * gap {
                out.too_close
                    .push((p.id, n.id, p.layer, edge.max(0.0), gap));
            }
        }
    }
    for (side, list) in tracks.iter().enumerate() {
        let total: f64 = list
            .iter()
            .map(|t| crate::geometry::distance(t.start, t.end))
            .sum();
        let coupled: f64 = intervals[side].values().map(|v| covered(v.clone())).sum();
        if let (Some(c), Some(u)) = (out.coupled.get_mut(side), out.uncoupled.get_mut(side)) {
            *c = coupled;
            *u = (total - coupled).max(0.0);
        }
    }
    PairCoupling {
        coupling: out,
        intervals,
    }
}

/// One net in the length report.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct NetRow {
    pub net: Id,
    pub name: String,
    pub length: NetLength,
    /// A pair half's signal path, which its target and delta compare; the
    /// rest of `length` is copper off the path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<f64>,
    /// The length it must reach (its group's, or its pair partner's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
    /// Length minus target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
    /// Within the tolerance of the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
}

/// One pair in the length report.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PairRow {
    pub pair: DiffPairId,
    pub name: String,
    /// How the lengths below were measured.
    #[serde(default)]
    pub measured: PairMeasured,
    /// The signal paths' pads, `[positive, negative]`, unless measured by
    /// total copper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<[PairPath; 2]>,
    /// Each half's measured length, mm.
    pub positive: f64,
    pub negative: f64,
    pub skew: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_skew: Option<f64>,
    /// The larger half's uncoupled length.
    pub uncoupled: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uncoupled: Option<f64>,
}

/// The pads one half's signal path joins ("J1.B6" to "U1.43").
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PairPath {
    pub from: String,
    pub to: String,
    /// The path runs through a fill island of its net.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub via_fill: bool,
}

/// One length group in the report.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupRow {
    pub group: LengthGroupId,
    pub name: String,
    /// The explicit target, else the longest member's length.
    pub target: f64,
    pub explicit: bool,
    pub tolerance: f64,
    pub members: Vec<Id>,
    /// The largest deviation of a routed member from its window, mm (0
    /// when all lie within).
    pub worst: f64,
    pub ok: bool,
}

/// `layout.length_report`: routed lengths against targets.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct LengthReport {
    pub nets: Vec<NetRow>,
    pub pairs: Vec<PairRow>,
    pub groups: Vec<GroupRow>,
}

/// A group's window: `(target, low, high)`.
fn window(target: f64, explicit: bool, tolerance: f64) -> (f64, f64) {
    if explicit {
        (target - tolerance, target + tolerance)
    } else {
        (target - tolerance, target)
    }
}

fn group_rows(project: &Project, lengths: &Measured) -> Vec<GroupRow> {
    let rules = &project.rules;
    rules
        .length_groups
        .iter()
        .map(|g| {
            let members = g.member_nets(rules);
            let len = |n: &Id| lengths.length(*n);
            let longest = members.iter().map(len).fold(0.0, f64::max);
            let (target, explicit) = match g.target {
                Some(t) => (t, true),
                None => (longest, false),
            };
            let (lo, hi) = window(target, explicit, g.tolerance);
            let worst = members
                .iter()
                .map(len)
                .filter(|l| *l > 0.0)
                .map(|l| (lo - l).max(l - hi).max(0.0))
                .fold(0.0, f64::max);
            GroupRow {
                group: g.id,
                name: g.name.clone(),
                target,
                explicit,
                tolerance: g.tolerance,
                members,
                worst,
                ok: worst <= 1e-6,
            }
        })
        .collect()
}

/// The length `net` is tuned to and its tolerance: the most demanding of
/// its groups' targets and, when its pair has a skew limit, its longer
/// partner's length. `None` when nothing sets one.
pub fn tune_target(project: &Project, net: Id) -> Option<(f64, f64)> {
    tune_target_with(project, &CopperGraph::of(project), net)
}

/// [`tune_target`] on `graph`, the copper graph of `project`.
pub fn tune_target_with(project: &Project, graph: &CopperGraph, net: Id) -> Option<(f64, f64)> {
    tune_target_of(project, &Measured::of(project, graph), net)
}

fn tune_target_of(project: &Project, lengths: &Measured, net: Id) -> Option<(f64, f64)> {
    let rules = &project.rules;
    let mut best: Option<(f64, f64)> = None;
    let mut consider = |target: f64, tolerance: f64| {
        if best.is_none_or(|(t, _)| target > t) {
            best = Some((target, tolerance));
        }
    };
    for row in group_rows(project, lengths) {
        if row.members.contains(&net) {
            consider(row.target, row.tolerance);
        }
    }
    if let Some(pair) = rules.pair_of(net)
        && let Some(partner) = pair.partner(net)
    {
        let class = super::net_class(project, pair.positive);
        if let Some(skew) = rules.signal(class).pair_max_skew {
            let other = lengths.length(partner);
            if other > 0.0 {
                consider(other, skew);
            }
        }
    }
    best
}

/// `layout.length_report`.
pub fn report(project: &Project) -> LengthReport {
    report_with(project, &CopperGraph::of(project))
}

/// [`report`] on `graph`, the copper graph of `project`.
pub fn report_with(project: &Project, graph: &CopperGraph) -> LengthReport {
    let lengths = Measured::of(project, graph);
    let rules = &project.rules;
    let mut wanted: BTreeSet<Id> = lengths.copper.keys().copied().collect();
    wanted.extend(
        rules
            .diff_pairs
            .iter()
            .flat_map(|p| [p.positive, p.negative]),
    );
    for g in &rules.length_groups {
        wanted.extend(g.member_nets(rules));
    }
    let mut nets: Vec<NetRow> = wanted
        .into_iter()
        .filter(|n| project.net(*n).is_some())
        .map(|net| {
            let length = lengths.copper.get(&net).copied().unwrap_or_default();
            let target = tune_target_of(project, &lengths, net);
            let measured = lengths.length(net);
            let delta = target.map(|(t, _)| measured - t);
            NetRow {
                net,
                name: project.net_name(net).to_owned(),
                length,
                path: lengths.paths.get(&net).copied(),
                target: target.map(|t| t.0),
                tolerance: target.map(|t| t.1),
                delta,
                ok: target.map(|(t, tol)| (measured - t).abs() <= tol + 1e-6),
            }
        })
        .collect();
    nets.sort_by(|a, b| a.name.cmp(&b.name).then(a.net.cmp(&b.net)));
    let pairs = rules
        .diff_pairs
        .iter()
        .map(|p| {
            let (a, b) = (lengths.length(p.positive), lengths.length(p.negative));
            let signal = rules.signal(super::net_class(project, p.positive));
            let uncoupled = lengths.uncoupled(p.id, &pair_coupling(project, p));
            let measure = lengths.pairs.get(&p.id);
            let halves = measure.and_then(|m| m.halves.as_ref());
            PairRow {
                pair: p.id,
                name: p.name.clone(),
                measured: measure.map_or(PairMeasured::Copper, |m| m.measured),
                paths: halves.map(|hs| {
                    hs.each_ref().map(|h| PairPath {
                        from: h.from.clone(),
                        to: h.to.clone(),
                        via_fill: h.via_fill,
                    })
                }),
                positive: a,
                negative: b,
                skew: (a - b).abs(),
                max_skew: signal.pair_max_skew,
                uncoupled: uncoupled[0].max(uncoupled[1]),
                max_uncoupled: signal.pair_max_uncoupled,
            }
        })
        .collect();
    LengthReport {
        nets,
        pairs,
        groups: group_rows(project, &lengths),
    }
}

fn mm(v: f64) -> String {
    super::limits::mm(v)
}

/// The pair, group and impedance findings of `project`, whose copper
/// graph is `graph`.
pub fn checks(project: &Project, graph: &CopperGraph) -> Vec<Finding> {
    let rules = &project.rules;
    let mut findings = Vec::new();
    let has_targets = rules.net_classes.iter().any(|c| !c.signal.is_empty());
    if rules.diff_pairs.is_empty() && rules.length_groups.is_empty() && !has_targets {
        return findings;
    }
    let lengths = Measured::of(project, graph);
    for pair in &rules.diff_pairs {
        check_pair(project, pair, &lengths, &mut findings);
    }
    let len = |n: Id| lengths.length(n);
    let net = |id: Id| ObjectRef::Net { id };
    for g in group_rows(project, &lengths) {
        let (lo, hi) = window(g.target, g.explicit, g.tolerance);
        for member in &g.members {
            let l = len(*member);
            if l <= 0.0 || (l >= lo - 1e-6 && l <= hi + 1e-6) {
                continue;
            }
            let (word, by) = if l < lo {
                ("short", lo - l)
            } else {
                ("long", l - hi)
            };
            findings.push(Finding::new(
                "length.mismatch",
                format!(
                    "{} is {} mm too {word} for length group {}; {} mm, needs {} ± {} mm",
                    project.net_name(*member),
                    mm(by),
                    g.name,
                    mm(l),
                    mm(if g.explicit {
                        g.target
                    } else {
                        g.target - g.tolerance / 2.0
                    }),
                    mm(if g.explicit {
                        g.tolerance
                    } else {
                        g.tolerance / 2.0
                    })
                ),
                vec![net(*member)],
            ));
        }
    }
    single_ended_impedance(project, &mut findings);
    findings
}

/// The skew, uncoupled-length, gap and impedance findings of `pair` alone,
/// as [`checks`] reports them, on `graph`: the copper graph of `project`,
/// or the graph of the pair's own nets ([`CopperGraph::of_nets`]), which
/// measures the pair the same.
pub fn pair_checks(project: &Project, graph: &CopperGraph, pair: &DiffPair) -> Vec<Finding> {
    let mut findings = Vec::new();
    let lengths = Measured::of_pairs(project, graph, std::slice::from_ref(pair));
    check_pair(project, pair, &lengths, &mut findings);
    findings
}

/// [`pair_checks`] with the lengths `lengths`, into `findings`.
fn check_pair(project: &Project, pair: &DiffPair, lengths: &Measured, findings: &mut Vec<Finding>) {
    let rules = &project.rules;
    let len = |n: Id| lengths.length(n);
    let net = |id: Id| ObjectRef::Net { id };
    let class = super::net_class(project, pair.positive);
    let class_name = class
        .and_then(|c| rules.class(c))
        .map_or_else(String::new, |c| c.name.clone());
    let signal = rules.signal(class);
    let (a, b) = (len(pair.positive), len(pair.negative));
    let objects = vec![net(pair.positive), net(pair.negative)];
    // Skew is checked only against a limit the net class sets.
    if let Some(max_skew) = signal.pair_max_skew
        && a > 0.0
        && b > 0.0
        && (a - b).abs() > max_skew + 1e-6
    {
        findings.push(Finding::new(
            "pair.skew",
            format!(
                "{}: halves differ by {} mm; needs at most {} mm (net class {class_name}: max skew)",
                pair.name,
                mm((a - b).abs()),
                mm(max_skew)
            ),
            objects.clone(),
        ));
    }
    let coupled = pair_coupling(project, pair);
    let uncoupled = lengths.uncoupled(pair.id, &coupled);
    let c = coupled.coupling;
    if let Some(max) = signal.pair_max_uncoupled {
        let worst = uncoupled[0].max(uncoupled[1]);
        if worst > max + 1e-6 && (a > 0.0 || b > 0.0) {
            findings.push(Finding::new(
                "pair.uncoupled",
                format!(
                    "{}: {} mm runs uncoupled; needs at most {} mm (net class {class_name}: max uncoupled)",
                    pair.name,
                    mm(worst),
                    mm(max)
                ),
                objects.clone(),
            ));
        }
    }
    let mut worst: BTreeMap<CopperLayer, (f64, f64, Id, Id)> = BTreeMap::new();
    for (p, n, layer, edge, gap) in &c.too_close {
        let entry = worst.entry(*layer).or_insert((*edge, *gap, *p, *n));
        if *edge < entry.0 {
            *entry = (*edge, *gap, *p, *n);
        }
    }
    for (layer, (edge, gap, p, n)) in worst {
        findings.push(Finding::new(
            "pair.gap",
            format!(
                "{}: halves {} mm apart on {}; needs a {} mm pair gap",
                pair.name,
                mm(edge),
                layer.name(),
                mm(gap)
            ),
            vec![
                net(pair.positive),
                net(pair.negative),
                ObjectRef::Track { id: p },
                ObjectRef::Track { id: n },
            ],
        ));
    }
    if let Some(target) = signal.diff_impedance {
        impedance_of_runs(project, pair, target, signal.tolerance_pct(), &c, findings);
    }
}

/// Each routed net whose class has a single-ended target: its worst
/// track per layer against the target.
fn single_ended_impedance(project: &Project, findings: &mut Vec<Finding>) {
    let rules = &project.rules;
    if !rules
        .net_classes
        .iter()
        .any(|c| c.signal.impedance.is_some())
    {
        return;
    }
    let mut lines = BTreeMap::new();
    let mut worst: BTreeMap<(Id, CopperLayer), (f64, f64, Id, f64)> = BTreeMap::new();
    let mut unreferenced: BTreeSet<(Id, CopperLayer)> = BTreeSet::new();
    for track in &project.tracks {
        let class = super::net_class(project, track.net);
        let signal = rules.signal(class);
        let Some(target) = signal.impedance else {
            continue;
        };
        let line = lines
            .entry(track.layer)
            .or_insert_with(|| impedance::structure(&project.stackup, track.layer).ok());
        let Some(line) = line else {
            unreferenced.insert((track.net, track.layer));
            continue;
        };
        let z = line.structure.z0(track.width, line.t);
        let off = (z - target).abs() / target * 100.0;
        if off > signal.tolerance_pct() + 1e-9 {
            let entry = worst
                .entry((track.net, track.layer))
                .or_insert((off, z, track.id, target));
            if off > entry.0 {
                *entry = (off, z, track.id, target);
            }
        }
    }
    for ((net, layer), (_, z, track, target)) in worst {
        findings.push(Finding::new(
            "impedance.mismatch",
            format!(
                "{} on {} is {:.1} Ω; needs {:.1} Ω ± {} %",
                project.net_name(net),
                layer.name(),
                z,
                target,
                mm(rules.signal(super::net_class(project, net)).tolerance_pct())
            ),
            vec![ObjectRef::Net { id: net }, ObjectRef::Track { id: track }],
        ));
    }
    for (net, layer) in unreferenced {
        findings.push(Finding::new(
            "impedance.mismatch",
            format!(
                "{} on {} has no reference plane for its impedance target",
                project.net_name(net),
                layer.name()
            ),
            vec![ObjectRef::Net { id: net }],
        ));
    }
}

/// A pair's coupled runs against its differential target.
fn impedance_of_runs(
    project: &Project,
    pair: &DiffPair,
    target: f64,
    tolerance: f64,
    coupling: &Coupling,
    findings: &mut Vec<Finding>,
) {
    let mut worst: BTreeMap<CopperLayer, (f64, f64, Id)> = BTreeMap::new();
    let mut unreferenced = BTreeSet::new();
    for (p, _, layer, width, edge) in &coupling.coupled_runs {
        let Ok(line) = impedance::structure(&project.stackup, *layer) else {
            unreferenced.insert(*layer);
            continue;
        };
        let z = line.structure.zdiff(*width, *edge, line.t);
        let off = (z - target).abs() / target * 100.0;
        if off > tolerance + 1e-9 {
            let entry = worst.entry(*layer).or_insert((off, z, *p));
            if off > entry.0 {
                *entry = (off, z, *p);
            }
        }
    }
    for (layer, (_, z, track)) in worst {
        findings.push(Finding::new(
            "impedance.mismatch",
            format!(
                "{} on {} is {:.1} Ω differential; needs {:.1} Ω ± {} %",
                pair.name,
                layer.name(),
                z,
                target,
                mm(tolerance)
            ),
            vec![
                ObjectRef::Net { id: pair.positive },
                ObjectRef::Net { id: pair.negative },
                ObjectRef::Track { id: track },
            ],
        ));
    }
    for layer in unreferenced {
        findings.push(Finding::new(
            "impedance.mismatch",
            format!(
                "{} on {} has no reference plane for its impedance target",
                pair.name,
                layer.name()
            ),
            vec![
                ObjectRef::Net { id: pair.positive },
                ObjectRef::Net { id: pair.negative },
            ],
        ));
    }
}

/// A pair or length group command names nets that exist in `project`.
/// The engine checks it before the command, since the nets follow-up
/// after it would drop a pair of a missing net instead of refusing it.
pub fn check_command_nets(project: &Project, command: &RulesCommand) -> Result<()> {
    let nets: Vec<Id> = match command {
        RulesCommand::AddDiffPair { pair } => vec![pair.positive, pair.negative],
        RulesCommand::EditDiffPair { pair } => {
            check_endpoints(project, pair)?;
            vec![pair.positive, pair.negative]
        }
        RulesCommand::AddLengthGroup { group } => group.nets.clone(),
        RulesCommand::EditLengthGroup { group } => group.nets.clone(),
        _ => return Ok(()),
    };
    if nets.iter().any(|n| project.net(*n).is_none()) {
        return Err(Error::NotFound(
            "diff_pair.net_missing: a chosen net no longer exists".into(),
        ));
    }
    Ok(())
}

/// A pair's set endpoints are two different terminals of each half's net.
fn check_endpoints(project: &Project, pair: &DiffPair) -> Result<()> {
    let Some(ends) = pair.endpoints else {
        return Ok(());
    };
    for (net, [start, end]) in [
        (pair.positive, ends.positive),
        (pair.negative, ends.negative),
    ] {
        let on_net = |e| project.net(net).is_some_and(|n| n.terminals.contains(&e));
        if start == end || !on_net(start) || !on_net(end) {
            return Err(Error::Invalid(format!(
                "diff_pair.endpoints: pair {} needs two different pads on {}",
                pair.name,
                project.net_name(net)
            )));
        }
    }
    Ok(())
}

/// Pairs and length groups name nets that exist.
pub fn validate(project: &Project) -> Result<()> {
    let rules = &project.rules;
    if rules.diff_pairs.is_empty() && rules.length_groups.is_empty() {
        return Ok(());
    }
    let live: BTreeSet<Id> = project.nets.iter().map(|n| n.id).collect();
    for pair in &rules.diff_pairs {
        if !live.contains(&pair.positive) || !live.contains(&pair.negative) {
            return Err(Error::NotFound(format!(
                "diff_pair.net_missing: pair {} names a net that does not exist",
                pair.name
            )));
        }
    }
    for group in &rules.length_groups {
        if group.nets.iter().any(|n| !live.contains(n)) {
            return Err(Error::NotFound(format!(
                "length_group.net_missing: length group {} names a net that does not exist",
                group.name
            )));
        }
    }
    Ok(())
}
