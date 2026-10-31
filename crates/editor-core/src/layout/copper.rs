//! Board copper as exact outlines: every connected pad is its core pad
//! outline ([`WorldPad::outline`], built on `library/footprint/geometry.rs`)
//! and every track its butt-capped rectangle. Connectivity, clearance,
//! airwires and routing checks all measure these rings. A [`BoardCopper`]
//! is built once per document revision and reused, including for the
//! placement drag preview ([`BoardCopper::moved`]).
//!
//! Curved edges are flattened within [`CURVE_SLACK_MM`]. Each ring records
//! that slack, so the checks stay exact for straight-sided copper. On
//! curved edges, touching is accepted within the slack, and clearance is
//! reported conservatively: a curved gap may be flagged up to the slack
//! before it is truly under the rule, and is never missed.
use super::{CLEARANCE_MM, EPS, MAX_CHECK_PAIRS, WorldPad, courtyard, pad_labels, sync_preview};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::{Bounds, Point, Shape, polygons_overlap, polyline_polygon_distance};
use crate::ids::Id;
use crate::library::{PAD_ARC_TOLERANCE_MM, PadShape, Segment};
use crate::model::{Layer, Project, Track};
use std::collections::{BTreeMap, BTreeSet};

/// The chord tolerance curved copper edges are flattened at, in mm, and so
/// the most a measured gap on a curved edge differs from the exact one.
pub const CURVE_SLACK_MM: f64 = PAD_ARC_TOLERANCE_MM;

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
        }
    }

    /// A placed pad's copper outline.
    pub fn of_pad(pad: &WorldPad) -> Self {
        Self::of_shape(&pad.outline(), curved(&pad.shape))
    }

    /// A track's rectangle with butt end caps.
    pub fn of_track(track: &Track) -> Self {
        let ring = super::track_polygon(track).to_vec();
        Self {
            bounds: Bounds::of(ring.iter().copied()),
            ring,
            slack: 0.0,
        }
    }

    fn translated(&self, delta: Point) -> Self {
        let shift = |p: Point| [p[0] + delta[0], p[1] + delta[1]];
        Self {
            ring: self.ring.iter().map(|p| shift(*p)).collect(),
            bounds: self.bounds.map(|b| Bounds {
                min: shift(b.min),
                max: shift(b.max),
            }),
            slack: self.slack,
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
        if polygons_overlap(&self.ring, &other.ring) {
            return 0.0;
        }
        let mut closed = self.ring.clone();
        if let Some(first) = self.ring.first() {
            closed.push(*first);
        }
        polyline_polygon_distance(&closed, &other.ring)
    }

    /// Whether the two outlines touch or overlap (same-net copper joins).
    pub fn touches(&self, other: &CopperRing) -> bool {
        let slack = self.slack + other.slack;
        self.gap(other, EPS + slack + EPS) <= EPS + slack
    }

    /// Whether the two outlines are closer than the clearance rule (an
    /// exact boundary passes; curved edges are judged conservatively).
    pub fn violates_clearance(&self, other: &CopperRing) -> bool {
        let slack = self.slack + other.slack;
        self.gap(other, CLEARANCE_MM + slack) - slack < CLEARANCE_MM - EPS
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

/// The connected pads of a board with their copper outlines, built once per
/// document revision.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoardCopper {
    pads: Vec<WorldPad>,
    rings: Vec<CopperRing>,
}

/// One copper shape the checks compare: a pad or a track.
struct Copper<'a> {
    net: Id,
    layer: Layer,
    through: bool,
    ring: &'a CopperRing,
    object: ObjectRef,
    pad: Option<usize>,
}

impl BoardCopper {
    /// Every connected pad of `project` with its exact outline.
    pub fn of(project: &Project) -> Self {
        let pads = super::pads(project);
        let rings = pads.iter().map(CopperRing::of_pad).collect();
        Self { pads, rings }
    }

    /// The connected pads, in placement then pad order.
    pub fn pads(&self) -> &[WorldPad] {
        &self.pads
    }

    /// The copper outline of the pad at `index` in [`Self::pads`].
    pub fn ring(&self, index: usize) -> Option<&CopperRing> {
        self.rings.get(index)
    }

    /// The same copper with one placement's pads moved by `delta`: what a
    /// pure move of that placement gives, without rebuilding the outlines.
    pub fn moved(&self, instance: Id, delta: Point) -> Self {
        let mut moved = self.clone();
        for (pad, ring) in moved.pads.iter_mut().zip(moved.rings.iter_mut()) {
            if pad.instance == instance {
                pad.position = [pad.position[0] + delta[0], pad.position[1] + delta[1]];
                *ring = ring.translated(delta);
            }
        }
        moved
    }

    fn shapes<'a>(&'a self, tracks: &[Track], track_rings: &'a [CopperRing]) -> Vec<Copper<'a>> {
        let pads = self
            .pads
            .iter()
            .zip(&self.rings)
            .enumerate()
            .map(|(i, (pad, ring))| Copper {
                net: pad.net,
                layer: pad.layer,
                through: pad.through,
                ring,
                object: ObjectRef::Pad {
                    instance: pad.instance,
                    pad: pad.pad,
                },
                pad: Some(i),
            });
        let tracks = tracks.iter().zip(track_rings).map(|(t, ring)| Copper {
            net: t.net,
            layer: t.layer,
            through: false,
            ring,
            object: ObjectRef::Track { id: t.id },
            pad: None,
        });
        pads.chain(tracks).collect()
    }

    /// Geometric connectivity and clearance checks. Net membership does not
    /// join separated copper: copper joins only where same-layer outlines
    /// touch, and no interconnect through a component is assumed.
    pub fn checks(&self, project: &Project) -> Vec<Finding> {
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
            ObjectRef::Track { id } => format!("track {id}"),
            _ => String::new(),
        };
        let track_rings: Vec<CopperRing> =
            project.tracks.iter().map(CopperRing::of_track).collect();
        let shapes = self.shapes(&project.tracks, &track_rings);
        let mut parents = (0..shapes.len()).collect::<Vec<_>>();
        let mut pairs = 0usize;
        for (i, a) in shapes.iter().enumerate() {
            if !a.ring.inside(project.board_size) {
                issues.push(Finding::new(
                    "board.outside_outline",
                    format!("{} extends beyond board outline", label(a)),
                    vec![a.object],
                ));
            }
            for (j, b) in shapes.iter().enumerate().skip(i + 1) {
                pairs += 1;
                if pairs > MAX_CHECK_PAIRS {
                    issues.push(Finding::new(
                        "board.check_limit",
                        "Connectivity/clearance check limit reached; remaining geometry is unchecked"
                            .into(),
                        vec![],
                    ));
                    return issues;
                }
                if a.layer != b.layer && !a.through && !b.through {
                    continue;
                }
                if a.net == b.net {
                    if a.ring.touches(b.ring) {
                        union(&mut parents, i, j);
                    }
                } else if a.ring.violates_clearance(b.ring) {
                    issues.push(Finding::new(
                        "board.clearance",
                        format!("Clearance < 0.2 mm: {} / {}", label(a), label(b)),
                        vec![a.object, b.object],
                    ));
                }
            }
        }
        let mut groups: BTreeMap<Id, BTreeSet<usize>> = BTreeMap::new();
        for (i, shape) in shapes.iter().enumerate().filter(|(_, s)| s.pad.is_some()) {
            groups
                .entry(shape.net)
                .or_default()
                .insert(find(&parents, i));
        }
        for shape in &shapes {
            if shape.pad.is_some() && groups.get(&shape.net).is_some_and(|g| g.len() > 1) {
                issues.push(Finding::new(
                    "board.unrouted",
                    format!(
                        "Unrouted: {} ({})",
                        label(shape),
                        project.net_name(shape.net)
                    ),
                    vec![shape.object, ObjectRef::Net { id: shape.net }],
                ));
            }
        }
        issues.extend(courtyard::findings(project));
        issues
    }

    /// Airwires between physically disconnected copper islands of each net,
    /// against `tracks`. The bounded scan conservatively leaves extra
    /// guides when it cannot prove connectivity.
    pub fn airwires(&self, tracks: &[Track]) -> Vec<(WorldPad, WorldPad)> {
        let track_rings: Vec<CopperRing> = tracks.iter().map(CopperRing::of_track).collect();
        let shapes = self.shapes(tracks, &track_rings);
        let mut parents = (0..shapes.len()).collect::<Vec<_>>();
        let mut pairs = 0usize;
        'scan: for (i, a) in shapes.iter().enumerate() {
            for (j, b) in shapes.iter().enumerate().skip(i + 1) {
                pairs += 1;
                if pairs > MAX_CHECK_PAIRS {
                    break 'scan;
                }
                if a.net == b.net
                    && (a.layer == b.layer || a.through || b.through)
                    && a.ring.touches(b.ring)
                {
                    union(&mut parents, i, j);
                }
            }
        }
        let mut islands: BTreeMap<Id, BTreeMap<usize, usize>> = BTreeMap::new();
        for (i, pad) in self.pads.iter().enumerate() {
            islands
                .entry(pad.net)
                .or_default()
                .entry(find(&parents, i))
                .or_insert(i);
        }
        let mut result = Vec::new();
        for representatives in islands.values() {
            let ids = representatives.values().copied().collect::<Vec<_>>();
            for pair in ids.windows(2) {
                if let (Some(a), Some(b)) = (self.pads.get(pair[0]), self.pads.get(pair[1])) {
                    result.push((a.clone(), b.clone()));
                }
            }
        }
        result
    }
}

fn find(parents: &[usize], mut at: usize) -> usize {
    while let Some(&parent) = parents.get(at)
        && parent != at
    {
        at = parent;
    }
    at
}

fn union(parents: &mut [usize], a: usize, b: usize) {
    let a = find(parents, a);
    let b = find(parents, b);
    if let Some(parent) = parents.get_mut(b) {
        *parent = a;
    }
}
