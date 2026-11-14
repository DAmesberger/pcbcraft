//! Etch hazards: acute copper angles (acid traps) and unfilled slivers,
//! two warnings of the board checks ([`super::checks_with`]) and so of the
//! production check.
//!
//! - `board.acute_copper`: a bare wedge between joined copper edges on one
//!   layer narrower than the min copper angle ([`EtchRules`]), as the copper
//!   graph finds them ([`CopperGraph::wedges`]).
//! - `board.copper_sliver`: inside a fresh fill, a bare gap narrower than
//!   the min sliver width between two pieces of copper of other nets than
//!   the fill's, or between such copper and the board edge, that the fill
//!   did not flow into. The gap is measured between the exact copper
//!   outlines the clearance check reads ([`CopperRing`]) at its narrowest
//!   point, and is a sliver when that point lies inside the fill's outline
//!   and the board, outside keepouts that forbid fills, and no copper (the
//!   fill's own, another fill's, any track, pad or via) crosses it. Also a
//!   hatched fill whose hatch is narrower than its min fill width: fill
//!   compute removes solid copper narrower than that width (an opening),
//!   but draws the hatch after it.
//!
//! Both rules are unset (0) by default: no citable source gives a number.
use super::connectivity::CopperGraph;
use super::copper::CopperRing;
use super::fill::{self, FILL_CHORD_MM, RegionIndex};
use super::grid::BoundsGrid;
use super::limits::{cite, mm};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{Bounds, Point, distance, polygon_contains, segments_cross};
use crate::ids::Id;
use crate::model::{CopperLayer, Project};
use crate::rules::{EtchRules, RuleKey};
use std::collections::BTreeSet;

/// The etch-hazard findings of `project`, whose copper graph is `graph`.
pub fn checks(project: &Project, graph: &CopperGraph) -> Vec<Finding> {
    let rules = project.rules.effective();
    let mut out = hatch_findings(project, &rules);
    let EtchRules {
        copper_angle_min,
        sliver_width_min,
    } = rules.etch;
    if copper_angle_min > 0.0 {
        acute(project, graph, copper_angle_min, &mut out);
    }
    if sliver_width_min > 0.0 {
        slivers(project, graph, sliver_width_min, &mut out);
    }
    out
}

/// Pad labels, read once per check when a message names a pad.
type Labels = std::cell::OnceCell<std::collections::BTreeMap<(Id, crate::ids::PadId), String>>;

/// How an object is named in a message.
fn describe(project: &Project, labels: &Labels, object: &ObjectRef) -> String {
    match *object {
        ObjectRef::Track { id } => project.tracks.iter().find(|t| t.id == id).map_or_else(
            || "a track".into(),
            |t| format!("a track on {}", project.net_name(t.net)),
        ),
        ObjectRef::Via { id } => project.via(id).map_or_else(
            || "a via".into(),
            |v| format!("a via on {}", project.net_name(v.net)),
        ),
        ObjectRef::Pad { instance, pad } => {
            let labels = labels.get_or_init(|| super::pad_labels(project));
            format!(
                "{} pad {}",
                project
                    .instance(instance)
                    .map_or("component", |i| i.designator.as_str()),
                labels
                    .get(&(instance, pad))
                    .cloned()
                    .unwrap_or_else(|| pad.to_string())
            )
        }
        ObjectRef::Fill { id } => project
            .fill(id)
            .map_or_else(|| "a fill".into(), |f| fill::fill_label(project, f)),
        ObjectRef::Board => "the board edge".into(),
        _ => "copper".into(),
    }
}

fn at(p: Point) -> String {
    format!("({}, {}) mm", mm(p[0]), mm(p[1]))
}

/// `board.acute_copper`: every wedge the graph finds under the rule.
fn acute(project: &Project, graph: &CopperGraph, below: f64, out: &mut Vec<Finding>) {
    let rule = cite(&project.rules, RuleKey::CopperAngleMin);
    let labels = Labels::new();
    for w in graph.wedges(project, below) {
        let [a, b] = w.objects;
        out.push(Finding::new(
            "board.acute_copper",
            format!(
                "Copper angle {}° < {}°: {} / {} on {} at {} ({rule})",
                mm(w.angle_deg),
                mm(below),
                describe(project, &labels, &a),
                describe(project, &labels, &b),
                w.layer.name(),
                at(w.at),
            ),
            vec![a, b],
        ));
    }
}

/// A hatched fill whose hatch lines are narrower than its min fill width.
fn hatch_findings(project: &Project, rules: &crate::rules::DesignRules) -> Vec<Finding> {
    let mut out = Vec::new();
    for f in &project.fills {
        let Some(h) = f.settings.hatch else {
            continue;
        };
        let width = f.settings.width_min.unwrap_or(rules.fill.width_min);
        if h.width < width {
            out.push(Finding::new(
                "board.copper_sliver",
                format!(
                    "{}: hatch lines {} mm wide < its min fill width {} mm",
                    crate::library::names::capitalised(&fill::fill_label(project, f)),
                    mm(h.width),
                    mm(width)
                ),
                vec![ObjectRef::Fill { id: f.id }],
            ));
        }
    }
    out
}

/// One piece of copper on a layer: its object, net and exact outline.
struct Piece {
    object: ObjectRef,
    net: Id,
    ring: CopperRing,
}

/// Whether both are pads of one placed part: their gap is the footprint's,
/// not the layout's (as the clearance check holds them to the profile's
/// pad-to-pad clearance only).
fn one_part(a: ObjectRef, b: ObjectRef) -> bool {
    matches!(
        (a, b),
        (ObjectRef::Pad { instance: x, .. }, ObjectRef::Pad { instance: y, .. }) if x == y
    )
}

/// The copper on `layer`: tracks, via lands and pad lands, as the copper
/// graph holds them (a track's and a via's copper exact from its centre).
fn pieces(project: &Project, graph: &CopperGraph, layer: CopperLayer) -> Vec<Piece> {
    let layers = project.stackup.layers();
    let mut out = Vec::new();
    for t in project.tracks.iter().filter(|t| t.layer == layer) {
        out.push(Piece {
            object: ObjectRef::Track { id: t.id },
            net: t.net,
            ring: super::connectivity::track_copper(t),
        });
    }
    for v in project.vias.iter().filter(|v| v.on(layer, &layers)) {
        out.push(Piece {
            object: ObjectRef::Via { id: v.id },
            net: v.net,
            ring: super::connectivity::via_copper(v),
        });
    }
    for p in graph.pads().iter().filter(|p| p.on(layer)) {
        out.push(Piece {
            object: ObjectRef::Pad {
                instance: p.instance,
                pad: p.pad,
            },
            net: p.net,
            ring: CopperRing::of_pad(p),
        });
    }
    out
}

/// An outline's boundary as segments with the distance its copper reaches
/// past them: a track's centre line and half-width, or a ring's edges.
fn boundary(ring: &CopperRing) -> (Vec<(Point, Point)>, f64) {
    match ring.spine {
        Some((a, b, r)) => (vec![(a, b)], r),
        None => {
            let n = ring.ring.len();
            let edges = (0..n)
                .filter_map(|i| Some((*ring.ring.get(i)?, *ring.ring.get((i + 1) % n)?)))
                .collect();
            (edges, 0.0)
        }
    }
}

/// The nearest points of segments `a`–`b` and `c`–`d`.
fn nearest(a: Point, b: Point, c: Point, d: Point) -> (Point, Point) {
    let project = |p: Point, s: Point, e: Point| {
        let se = [e[0] - s[0], e[1] - s[1]];
        let len2 = se[0] * se[0] + se[1] * se[1];
        let t = if len2 > 0.0 {
            (((p[0] - s[0]) * se[0] + (p[1] - s[1]) * se[1]) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        [s[0] + se[0] * t, s[1] + se[1] * t]
    };
    if segments_cross(a, b, c, d) {
        let (r, s) = ([b[0] - a[0], b[1] - a[1]], [d[0] - c[0], d[1] - c[1]]);
        let den = r[0] * s[1] - r[1] * s[0];
        if den != 0.0 {
            let t = ((c[0] - a[0]) * s[1] - (c[1] - a[1]) * s[0]) / den;
            let x = [a[0] + r[0] * t, a[1] + r[1] * t];
            return (x, x);
        }
    }
    let candidates = [
        (a, project(a, c, d)),
        (b, project(b, c, d)),
        (project(c, a, b), c),
        (project(d, a, b), d),
    ];
    let least = candidates
        .iter()
        .map(|(p, q)| distance(*p, *q))
        .fold(f64::INFINITY, f64::min);
    // Parallel runs are nearest all along their overlap: take its middle.
    let (mut p, mut q, mut n) = ([0.0, 0.0], [0.0, 0.0], 0.0);
    for (x, y) in candidates {
        if distance(x, y) <= least + 1e-9 {
            p = [p[0] + x[0], p[1] + x[1]];
            q = [q[0] + y[0], q[1] + y[1]];
            n += 1.0;
        }
    }
    if n == 0.0 {
        return (a, c);
    }
    ([p[0] / n, p[1] / n], [q[0] / n, q[1] / n])
}

/// The nearest points of two boundaries (each grown by its reach) and the
/// gap between them, if they are apart.
fn witness(
    x: &(Vec<(Point, Point)>, f64),
    y: &(Vec<(Point, Point)>, f64),
) -> Option<(Point, Point, f64)> {
    let mut best: Option<(Point, Point, f64)> = None;
    for &(a, b) in &x.0 {
        for &(c, d) in &y.0 {
            let (p, q) = nearest(a, b, c, d);
            let gap = distance(p, q);
            if best.is_none_or(|(_, _, g)| gap < g) {
                best = Some((p, q, gap));
            }
        }
    }
    let (p, q, core) = best?;
    let gap = core - x.1 - y.1;
    if gap <= 0.0 || core <= 0.0 {
        return None;
    }
    let toward = |from: Point, to: Point, by: f64| {
        let t = by / core;
        [
            from[0] + (to[0] - from[0]) * t,
            from[1] + (to[1] - from[1]) * t,
        ]
    };
    Some((toward(p, q, x.1), toward(q, p, y.1), gap))
}

/// What a sliver test needs of one fill.
struct FillArea<'a> {
    id: Id,
    net: Id,
    layer: CopperLayer,
    outline: Vec<Point>,
    bounds: Bounds,
    regions: &'a [crate::model::fill::FilledRegion],
    index: std::cell::OnceCell<RegionIndex>,
}

impl FillArea<'_> {
    /// The fill's copper, indexed on first use.
    fn index(&self) -> &RegionIndex {
        self.index.get_or_init(|| RegionIndex::filled(self.regions))
    }
}

/// `board.copper_sliver` for bare gaps under `min` mm in fresh fills.
fn slivers(project: &Project, graph: &CopperGraph, min: f64, out: &mut Vec<Finding>) {
    let fresh = fill::fresh(project);
    if fresh.is_empty() {
        return;
    }
    let rule = cite(&project.rules, RuleKey::SliverWidthMin);
    let labels = Labels::new();
    let mut areas: Vec<FillArea<'_>> = Vec::new();
    for f in &fresh {
        let Some(fill) = project.fill(f.id) else {
            continue;
        };
        let outline = fill.outline.flatten(FILL_CHORD_MM);
        let Some(bounds) = Bounds::of(outline.iter().copied()) else {
            continue;
        };
        areas.push(FillArea {
            id: f.id,
            net: f.net,
            layer: f.layer,
            outline,
            bounds,
            regions: &f.copper.regions,
            index: std::cell::OnceCell::new(),
        });
    }
    let board = project.board();
    let outer = board.outer.flatten(super::CURVE_SLACK_MM);
    let cutouts: Vec<Vec<Point>> = board
        .cutouts
        .iter()
        .map(|c| c.outline.flatten(super::CURVE_SLACK_MM))
        .collect();
    let loops: Vec<Vec<(Point, Point)>> = std::iter::once(&outer)
        .chain(cutouts.iter())
        .map(|ring| {
            let n = ring.len();
            (0..n)
                .filter_map(|i| Some((*ring.get(i)?, *ring.get((i + 1) % n)?)))
                .collect()
        })
        .collect();
    let on_board =
        |p: Point| polygon_contains(&outer, p) && !cutouts.iter().any(|c| polygon_contains(c, p));
    let layers: BTreeSet<CopperLayer> = areas.iter().map(|a| a.layer).collect();
    let mut reported: Vec<(ObjectRef, ObjectRef)> = Vec::new();
    for layer in layers {
        let pieces = pieces(project, graph, layer);
        let shapes: Vec<_> = pieces.iter().map(|p| boundary(&p.ring)).collect();
        let grid = BoundsGrid::new(pieces.iter().map(|p| p.ring.bounds), min);
        let keepouts = fill::fill_keepouts(project, layer);
        let on_layer: Vec<&FillArea<'_>> = areas.iter().filter(|a| a.layer == layer).collect();
        let mut near = Vec::new();
        // Whether copper other than pieces `skip` crosses segment p–q.
        let crossed = |p: Point, q: Point, skip: &[usize], near: &mut Vec<usize>| {
            let b = Bounds::point(p).including(q);
            let m = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0];
            grid.near(&b, 0.0, near);
            let pieces_cross = near.iter().filter(|j| !skip.contains(j)).any(|j| {
                shapes.get(*j).is_some_and(|(edges, r)| {
                    edges.iter().any(|&(c, d)| {
                        let (x, y) = nearest(p, q, c, d);
                        distance(x, y) < *r || (*r == 0.0 && segments_cross(p, q, c, d))
                    })
                }) || pieces.get(*j).is_some_and(|piece| {
                    piece.ring.spine.is_none() && polygon_contains(&piece.ring.ring, m)
                })
            });
            pieces_cross
                || on_layer.iter().any(|a| {
                    let index = a.index();
                    index.region_at(p).is_some()
                        || index.region_at(q).is_some()
                        || index.segment_distance(p, q, min) <= 0.0
                })
        };
        for area in &on_layer {
            // Copper the fill clears: of other nets than its own.
            let foreign: Vec<bool> = pieces.iter().map(|p| p.net != area.net).collect();
            for (i, a) in pieces.iter().enumerate() {
                let Some(ab) = a.ring.bounds else {
                    continue;
                };
                let in_reach = ab.max[0] + min >= area.bounds.min[0]
                    && ab.min[0] - min <= area.bounds.max[0]
                    && ab.max[1] + min >= area.bounds.min[1]
                    && ab.min[1] - min <= area.bounds.max[1];
                if !in_reach || !foreign.get(i).copied().unwrap_or(false) {
                    continue;
                }
                let Some(xa) = shapes.get(i) else {
                    continue;
                };
                // Against other copper of other nets than the fill's.
                grid.near(&ab, min, &mut near);
                let others: Vec<usize> = near.iter().copied().filter(|j| *j > i).collect();
                let mut gaps: Vec<(ObjectRef, Point, Point, f64, Vec<usize>)> = Vec::new();
                for j in others {
                    let (Some(b), Some(xb)) = (pieces.get(j), shapes.get(j)) else {
                        continue;
                    };
                    if !foreign.get(j).copied().unwrap_or(false)
                        || one_part(a.object, b.object)
                        || a.ring.gap(&b.ring, min) <= 0.0
                    {
                        continue;
                    }
                    if let Some((p, q, gap)) = witness(xa, xb)
                        && gap < min
                    {
                        gaps.push((b.object, p, q, gap, vec![i, j]));
                    }
                }
                // Against the board edge.
                let reach = ab.inflated(min);
                let edges: Vec<(Point, Point)> = loops
                    .iter()
                    .flatten()
                    .copied()
                    .filter(|&(c, d)| {
                        let e = Bounds::point(c).including(d);
                        e.max[0] >= reach.min[0]
                            && e.min[0] <= reach.max[0]
                            && e.max[1] >= reach.min[1]
                            && e.min[1] <= reach.max[1]
                    })
                    .collect();
                if !edges.is_empty()
                    && let Some((p, q, gap)) = witness(xa, &(edges, 0.0))
                    && gap < min
                {
                    gaps.push((ObjectRef::Board, p, q, gap, vec![i]));
                }
                for (other, p, q, gap, skip) in gaps {
                    let m = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0];
                    let open = polygon_contains(&area.outline, m)
                        && on_board(m)
                        && !keepouts.iter().any(|k| polygon_contains(k, m))
                        && !crossed(p, q, &skip, &mut near);
                    if !open {
                        continue;
                    }
                    let objects = vec![a.object, other, ObjectRef::Fill { id: area.id }];
                    if reported.contains(&(a.object, other)) {
                        continue;
                    }
                    reported.push((a.object, other));
                    out.push(Finding::new(
                        "board.copper_sliver",
                        format!(
                            "Unfilled gap {} mm < {} mm: {} / {} in {} at {} ({rule})",
                            mm(gap),
                            mm(min),
                            describe(project, &labels, &a.object),
                            describe(project, &labels, &other),
                            describe(project, &labels, &ObjectRef::Fill { id: area.id }),
                            at(m),
                        ),
                        objects,
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
