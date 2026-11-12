//! Board copper as exact outlines: every connected pad is its core pad
//! outline ([`WorldPad::outline`], built on `library/footprint/geometry.rs`)
//! and every track its round-capped stadium ([`super::track_shape`]).
//! Clearance, edge and routing checks measure these rings; which copper is
//! joined is the copper graph's alone ([`super::connectivity`]).
//!
//! Curved edges are flattened within [`CURVE_SLACK_MM`]. Each ring records
//! that slack, so the checks stay exact for straight-sided copper. On
//! curved edges, touching is accepted within the slack, and clearance is
//! reported conservatively: a curved gap may be flagged up to the slack
//! before it is truly under the rule, and is never missed. A track's round
//! caps are exact instead: its gaps are measured from its centre line less
//! its half-width.
use super::board_edge::{BoardEdges, EdgeVerdict};
use super::connectivity::CopperGraph;
use super::grid::BoundsGrid;
use super::limits::{BoardRules, mm};
use super::{CLEARANCE_MM, EPS, MAX_CHECK_PAIRS, WorldPad, courtyard, pad_labels, sync_preview};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{
    Bounds, Point, Shape, polygons_overlap, polyline_polygon_distance, segments_gap,
};
use crate::ids::Id;
use crate::library::{PAD_ARC_TOLERANCE_MM, PadShape, Segment};
use crate::model::{CopperLayer, Project, Track, Via};
use crate::rules::CopperObject;

/// The chord tolerance curved copper edges are flattened at, in mm, and so
/// the most a measured gap on a curved edge differs from the exact one.
pub const CURVE_SLACK_MM: f64 = PAD_ARC_TOLERANCE_MM;

/// The farthest apart two outlines' bounds may be for any board check to
/// compare them under `limits`: the largest clearance the project's rules
/// and net classes resolve to (copper, drill to copper, hole to hole) plus
/// both outlines' curve slack. Shapes farther apart are never compared,
/// which [`CopperRing::gap`] would reject by their bounds anyway.
pub(super) fn check_reach(limits: &BoardRules<'_>) -> f64 {
    limits.max_clearance() + SLACK_REACH_MM
}

/// How far two outlines' measured gaps may stray from the exact ones: both
/// outlines' curve slack, and rounding.
const SLACK_REACH_MM: f64 = 2.0 * CURVE_SLACK_MM + 1e-6;

/// One copper outline in board coordinates: a closed ring, the exact
/// bounds of the shape it approximates, and how far the ring may stray from
/// that shape.
#[derive(Clone, Debug, PartialEq)]
pub struct CopperRing {
    pub ring: Vec<Point>,
    /// Exact bounds of the true outline (curves included), or `None` for a
    /// degenerate outline, which has no copper.
    pub bounds: Option<Bounds>,
    /// 0 for straight edges; [`CURVE_SLACK_MM`] when the ring flattens a
    /// curve.
    pub slack: f64,
    /// A track's centre line and half-width: its copper is exactly the
    /// points within that distance of the segment, so gaps to it are
    /// measured from the segment, not from the flattened ring (whose
    /// slack is then 0).
    pub spine: Option<(Point, Point, f64)>,
}

impl CopperRing {
    /// A board outline, flattened within [`CURVE_SLACK_MM`]. `curved` says
    /// whether it (or the pad shape it came from) has curved edges.
    pub fn of_shape(shape: &Shape, curved: bool) -> Self {
        let ring = shape.flatten(CURVE_SLACK_MM);
        let bounds = (ring.len() >= 3).then(|| shape.bounds(0.0));
        Self {
            ring,
            bounds,
            slack: if curved { CURVE_SLACK_MM } else { 0.0 },
            spine: None,
        }
    }

    /// A placed pad's copper outline.
    pub fn of_pad(pad: &WorldPad) -> Self {
        Self::of_shape(&pad.outline(), curved(&pad.shape))
    }

    /// A placed pad's copper outline, the same as [`Self::of_pad`] gives
    /// for its connected pad.
    pub fn of_placed_pad(pad: &super::PlacedPad) -> Self {
        Self::of_shape(&pad.outline(), curved(&pad.pad.shape))
    }

    /// A via's annular pad, a circle of its diameter.
    pub fn of_via(via: &Via) -> Self {
        Self::of_circle(via.position, via.diameter)
    }

    /// A via's drilled hole, a circle of its drill diameter.
    pub fn of_drill(via: &Via) -> Self {
        Self::of_circle(via.position, via.drill)
    }

    /// A circle of `diameter` at `center` (a drilled hole).
    pub fn of_circle(center: Point, diameter: f64) -> Self {
        Self::of_shape(
            &Shape::Circle {
                center,
                radius: diameter / 2.0,
            },
            true,
        )
    }

    /// A track's stadium, round end caps included. Its gaps are exact:
    /// measured from the centre line less the half-width.
    pub fn of_track(track: &Track) -> Self {
        Self {
            slack: 0.0,
            spine: Some((track.start, track.end, track.width / 2.0)),
            ..Self::of_shape(&super::track_shape(track), true)
        }
    }

    /// The points within `radius` of segment `a`–`b` (a disc when they
    /// coincide), measured exactly from its spine. It carries no flattened
    /// ring, so it is only compared with other outlines through
    /// [`Self::gap`], which reads the spine.
    pub(super) fn capsule(a: Point, b: Point, radius: f64) -> Self {
        Self {
            ring: Vec::new(),
            bounds: Some(Bounds {
                min: [a[0].min(b[0]) - radius, a[1].min(b[1]) - radius],
                max: [a[0].max(b[0]) + radius, a[1].max(b[1]) + radius],
            }),
            slack: 0.0,
            spine: Some((a, b, radius)),
        }
    }

    pub(super) fn translated(&self, delta: Point) -> Self {
        let shift = |p: Point| [p[0] + delta[0], p[1] + delta[1]];
        Self {
            ring: self.ring.iter().map(|p| shift(*p)).collect(),
            bounds: self.bounds.map(|b| Bounds {
                min: shift(b.min),
                max: shift(b.max),
            }),
            slack: self.slack,
            spine: self.spine.map(|(a, b, r)| (shift(a), shift(b), r)),
        }
    }

    /// The measured boundary gap to `other` (0 when they overlap or touch),
    /// or some value of at least `limit` once their bounds alone prove the
    /// gap is that large. Infinite when either has no copper.
    pub fn gap(&self, other: &CopperRing, limit: f64) -> f64 {
        let (Some(a), Some(b)) = (self.bounds, other.bounds) else {
            return f64::INFINITY;
        };
        let dx = (a.min[0] - b.max[0]).max(b.min[0] - a.max[0]).max(0.0);
        let dy = (a.min[1] - b.max[1]).max(b.min[1] - a.max[1]).max(0.0);
        let lower = dx.hypot(dy);
        if lower >= limit {
            return lower;
        }
        match (self.spine, other.spine) {
            (Some((a, b, r)), Some((c, d, s))) => {
                return (segments_gap(a, b, c, d) - r - s).max(0.0);
            }
            (Some((a, b, r)), None) => {
                return (polyline_polygon_distance(&[a, b], &other.ring) - r).max(0.0);
            }
            (None, Some((c, d, s))) => {
                return (polyline_polygon_distance(&[c, d], &self.ring) - s).max(0.0);
            }
            (None, None) => {}
        }
        if polygons_overlap(&self.ring, &other.ring) {
            return 0.0;
        }
        let mut closed = self.ring.clone();
        if let Some(first) = self.ring.first() {
            closed.push(*first);
        }
        polyline_polygon_distance(&closed, &other.ring)
    }

    /// Whether the two outlines are closer than the default profile's
    /// copper clearance (an exact boundary passes; curved edges are judged
    /// conservatively). The board checks take the project's rules through
    /// [`Self::closer_than`] instead.
    pub fn violates_clearance(&self, other: &CopperRing) -> bool {
        self.closer_than(other, CLEARANCE_MM)
    }

    /// Whether the two outlines are closer than `limit` mm (an exact
    /// boundary passes; curved edges are judged conservatively).
    pub fn closer_than(&self, other: &CopperRing, limit: f64) -> bool {
        let slack = self.slack + other.slack;
        self.gap(other, limit + slack) - slack < limit - EPS
    }

    /// Whether the whole outline lies on the board, by its exact bounds.
    pub fn inside(&self, board: Point) -> bool {
        self.bounds.is_none_or(|b| {
            b.min[0] >= -EPS
                && b.min[1] >= -EPS
                && b.max[0] <= board[0] + EPS
                && b.max[1] <= board[1] + EPS
        })
    }
}

/// Whether a pad shape's outline has curved edges.
fn curved(shape: &PadShape) -> bool {
    match shape {
        PadShape::Rect { .. } => false,
        PadShape::RoundRect { .. } => shape.corner_radius() > 0.0,
        PadShape::Circle { .. } | PadShape::Oblong { .. } => true,
        PadShape::Custom { outline } => outline
            .segments
            .iter()
            .any(|s| matches!(s, Segment::Arc { bulge, .. } if bulge.abs() > 1e-12)),
    }
}

/// The connected pads and the vias of a board with their copper outlines,
/// built once per document revision, for the clearance and edge checks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoardCopper {
    pads: Vec<WorldPad>,
    rings: Vec<CopperRing>,
    vias: Vec<Via>,
    via_rings: Vec<CopperRing>,
}

/// One copper shape the checks compare: a pad, a via or a track.
struct Copper<'a> {
    kind: CopperObject,
    net: Id,
    layer: CopperLayer,
    through: bool,
    ring: &'a CopperRing,
    object: ObjectRef,
}

impl BoardCopper {
    /// Every connected pad and every via of `project` with its exact
    /// outline.
    pub fn of(project: &Project) -> Self {
        let pads = super::pads(project);
        let rings = pads.iter().map(CopperRing::of_pad).collect();
        let vias = project.vias.to_vec();
        let via_rings = vias.iter().map(CopperRing::of_via).collect();
        Self {
            pads,
            rings,
            vias,
            via_rings,
        }
    }

    /// The board's vias, in document order.
    pub fn vias(&self) -> &[Via] {
        &self.vias
    }

    /// The annular-pad outline of the via at `index` in [`Self::vias`].
    pub fn via_ring(&self, index: usize) -> Option<&CopperRing> {
        self.via_rings.get(index)
    }

    /// The connected pads, in placement then pad order.
    pub fn pads(&self) -> &[WorldPad] {
        &self.pads
    }

    /// The copper outline of the pad at `index` in [`Self::pads`].
    pub fn ring(&self, index: usize) -> Option<&CopperRing> {
        self.rings.get(index)
    }

    fn shapes<'a>(&'a self, tracks: &[Track], track_rings: &'a [CopperRing]) -> Vec<Copper<'a>> {
        let pads = self.pads.iter().zip(&self.rings).map(|(pad, ring)| Copper {
            kind: CopperObject::Pad,
            net: pad.net,
            layer: pad.layer.into(),
            through: pad.through,
            ring,
            object: ObjectRef::Pad {
                instance: pad.instance,
                pad: pad.pad,
            },
        });
        // A through via has copper on every layer, as a plated barrel pad.
        let vias = self
            .vias
            .iter()
            .zip(&self.via_rings)
            .map(|(v, ring)| Copper {
                kind: CopperObject::Via,
                net: v.net,
                layer: CopperLayer::Top,
                through: true,
                ring,
                object: ObjectRef::Via { id: v.id },
            });
        let tracks = tracks.iter().zip(track_rings).map(|(t, ring)| Copper {
            kind: CopperObject::Track,
            net: t.net,
            layer: t.layer,
            through: false,
            ring,
            object: ObjectRef::Track { id: t.id },
        });
        pads.chain(vias).chain(tracks).collect()
    }

    /// The board checks: placement, edge and clearance from these outlines,
    /// then the connectivity findings of `graph` (the same revision's
    /// copper graph), the via, pad hole and courtyard checks.
    pub fn checks(&self, project: &Project, graph: &CopperGraph) -> Vec<Finding> {
        let mut issues = Vec::new();
        for id in sync_preview(project) {
            let reference = project
                .instance(id)
                .map(|i| i.designator.as_str())
                .unwrap_or("component");
            issues.push(Finding::new(
                "board.unplaced",
                format!("{reference} is not placed on the board"),
                vec![ObjectRef::Instance { id }],
            ));
        }
        let labels = pad_labels(project);
        let label = |shape: &Copper| match shape.object {
            ObjectRef::Pad { instance, pad } => format!(
                "{} pad {}",
                project
                    .instance(instance)
                    .map(|i| i.designator.as_str())
                    .unwrap_or("component"),
                labels
                    .get(&(instance, pad))
                    .cloned()
                    .unwrap_or_else(|| pad.to_string())
            ),
            ObjectRef::Track { .. } => format!("a track on {}", project.net_name(shape.net)),
            ObjectRef::Via { .. } => format!("a via on {}", project.net_name(shape.net)),
            _ => String::new(),
        };
        let track_rings: Vec<CopperRing> =
            project.tracks.iter().map(CopperRing::of_track).collect();
        let shapes = self.shapes(&project.tracks, &track_rings);
        let limits = BoardRules::of(project);
        let reach = check_reach(&limits);
        let grid = BoundsGrid::new(shapes.iter().map(|s| s.ring.bounds), reach);
        let mut near = Vec::new();
        let mut pairs = 0usize;
        let edges = BoardEdges::of(project);
        for (i, a) in shapes.iter().enumerate() {
            let edge = || limits.clearance(a.kind, Some(a.net), CopperObject::BoardEdge, None);
            match edges.verdict(a.ring, edge()) {
                EdgeVerdict::Clear => {}
                EdgeVerdict::Outside => issues.push(Finding::new(
                    "board.outside_outline",
                    format!("{} extends beyond board outline", label(a)),
                    vec![a.object],
                )),
                EdgeVerdict::Inside(kind) => issues.push(Finding::new(
                    "board.outside_outline",
                    format!(
                        "{} extends into {}",
                        label(a),
                        super::board_edge::EdgeLoopKind::Cutout(kind).describe()
                    ),
                    vec![a.object],
                )),
                EdgeVerdict::TooClose { gap, near } => {
                    let need =
                        limits.clearance_need(a.kind, Some(a.net), CopperObject::BoardEdge, None);
                    issues.push(Finding::new(
                        "board.edge_clearance",
                        format!(
                            "{} is {} mm from {}; {}",
                            crate::library::names::capitalised(&label(a)),
                            mm(gap),
                            near.describe(),
                            need.text()
                        ),
                        vec![a.object],
                    ));
                }
            }
            let Some(bounds) = a.ring.bounds else {
                continue;
            };
            grid.near(&bounds, reach, &mut near);
            for b in near
                .iter()
                .filter(|j| **j > i)
                .filter_map(|j| shapes.get(*j))
            {
                pairs += 1;
                if pairs > MAX_CHECK_PAIRS {
                    issues.push(limit_finding());
                    return issues;
                }
                if a.layer != b.layer && !a.through && !b.through {
                    continue;
                }
                let (an, bn) = class_nets(a, b);
                if a.net != b.net
                    && a.ring
                        .closer_than(b.ring, limits.clearance(a.kind, an, b.kind, bn))
                {
                    let need = limits.clearance_need(a.kind, an, b.kind, bn);
                    issues.push(Finding::new(
                        "board.clearance",
                        format!(
                            "Clearance < {} mm: {} / {} ({})",
                            mm(need.mm),
                            label(a),
                            label(b),
                            need.cite()
                        ),
                        vec![a.object, b.object],
                    ));
                }
            }
        }
        issues.extend(graph.findings(project));
        issues.extend(super::via::findings(self, project));
        issues.extend(super::pad_hole::findings(self, project, graph));
        issues.extend(courtyard::findings(project));
        issues
    }
}

/// The nets whose classes set the clearance between `a` and `b`: none
/// for two pads of one placed footprint, whose spacing the footprint
/// fixes (a QFN's pitch cannot meet a power class's clearance to its own
/// exposed pad), so they are held to the profile's pad-to-pad clearance
/// (and its overrides) only.
fn class_nets(a: &Copper<'_>, b: &Copper<'_>) -> (Option<Id>, Option<Id>) {
    match (a.object, b.object) {
        (ObjectRef::Pad { instance: x, .. }, ObjectRef::Pad { instance: y, .. }) if x == y => {
            (None, None)
        }
        _ => (Some(a.net), Some(b.net)),
    }
}

/// The finding that says the copper checks stopped at their pair limit, so
/// the geometry after it is unchecked. It names the board, so it locates.
pub(crate) fn limit_finding() -> Finding {
    Finding::new(
        "board.check_limit",
        "Board checks incomplete: copper past the check limit is unchecked".into(),
        vec![ObjectRef::Board],
    )
}
