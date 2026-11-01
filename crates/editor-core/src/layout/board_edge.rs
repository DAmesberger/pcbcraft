//! Copper against the board shape (`Project::board()`): the outer outline,
//! routed cutouts and milled slots. Copper must lie inside the outline and
//! outside every cutout and slot, and keep the board-edge clearance of the
//! design rules (`board edge` pairs of the clearance matrix) from each of
//! their edges.
//!
//! A rectangular board is judged on the exact bounds of each copper shape,
//! as before rules were configurable, so its verdicts are exact. A sketched
//! shape is flattened within [`CURVE_SLACK_MM`]; copper crossing an edge is
//! found exactly on the flattened rings, and gaps are judged
//! conservatively by both rings' slack (a curved gap may be flagged up to
//! the slack early, never missed). With a zero clearance, curved copper
//! within the slack of a curved edge is accepted.
use super::{CURVE_SLACK_MM, CopperRing, EPS};
use crate::geometry::{
    Bounds, Point, polygon_contains, polyline_polygon_distance, segment_distance, segments_cross,
};
use crate::library::{Outline, Segment};
use crate::model::board::CutoutKind;
use crate::model::{BoardShape, Project};

/// Which loop of the board an edge belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeLoopKind {
    Outer,
    Cutout(CutoutKind),
}

impl EdgeLoopKind {
    /// "the board edge", "a cutout", "a slot".
    pub fn describe(self) -> &'static str {
        match self {
            Self::Outer => "the board edge",
            Self::Cutout(CutoutKind::Cutout) => "a cutout",
            Self::Cutout(CutoutKind::Slot { .. }) => "a slot",
        }
    }
}

/// How one copper shape sits against the board shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EdgeVerdict {
    /// Inside the board, outside every cutout, clear of every edge.
    Clear,
    /// Partly or wholly outside the outer outline.
    Outside,
    /// Partly or wholly inside a cutout or slot.
    Inside(CutoutKind),
    /// On the board but closer than the limit to an edge of `near`.
    TooClose { gap: f64, near: EdgeLoopKind },
}

struct EdgeLoop {
    ring: Vec<Point>,
    bounds: Bounds,
    /// Each edge's bounds, edge `k` running from `ring[k]` to `ring[k+1]`.
    edges: Vec<Bounds>,
    kind: EdgeLoopKind,
    slack: f64,
}

impl EdgeLoop {
    fn of(outline: &Outline, kind: EdgeLoopKind) -> Option<Self> {
        let ring = outline.flatten(CURVE_SLACK_MM);
        if ring.len() < 3 {
            return None;
        }
        let bounds = Bounds::of(ring.iter().copied())?;
        let edges = (0..ring.len())
            .filter_map(|k| {
                let (a, b) = edge(&ring, k)?;
                Bounds::of([a, b])
            })
            .collect();
        let curved = outline
            .segments
            .iter()
            .any(|s| matches!(s, Segment::Arc { bulge, .. } if bulge.abs() > 1e-12));
        Some(Self {
            ring,
            bounds,
            edges,
            kind,
            slack: if curved { CURVE_SLACK_MM } else { 0.0 },
        })
    }
}

fn edge(ring: &[Point], k: usize) -> Option<(Point, Point)> {
    let a = *ring.get(k)?;
    let b = *ring.get((k + 1) % ring.len())?;
    Some((a, b))
}

/// The least distance from `p` to the boundary of `ring`.
fn boundary_distance(ring: &[Point], p: Point) -> f64 {
    (0..ring.len())
        .filter_map(|k| edge(ring, k))
        .map(|(a, b)| segment_distance(p, a, b))
        .fold(f64::INFINITY, f64::min)
}

/// Whether `p` lies strictly inside `ring` (not on its boundary).
fn strictly_inside(ring: &[Point], p: Point) -> bool {
    polygon_contains(ring, p) && boundary_distance(ring, p) > EPS
}

/// Whether segment `a`–`b` enters the interior of `ring` (touching its
/// boundary does not count).
fn enters(ring: &[Point], a: Point, b: Point) -> bool {
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    (0..ring.len())
        .filter_map(|k| edge(ring, k))
        .any(|(c, d)| segments_cross(a, b, c, d))
        || [a, b, mid].into_iter().any(|p| strictly_inside(ring, p))
}

/// The signed gap from copper `ring` to edge `a`–`b`: negative when the
/// edge cuts into the copper.
fn edge_gap(copper: &CopperRing, a: Point, b: Point) -> f64 {
    match copper.spine {
        Some((s, e, r)) => {
            let centre = if segments_cross(s, e, a, b) {
                0.0
            } else {
                segment_distance(s, a, b)
                    .min(segment_distance(e, a, b))
                    .min(segment_distance(a, s, e))
                    .min(segment_distance(b, s, e))
            };
            centre - r
        }
        None if enters(&copper.ring, a, b) => -1.0,
        None => polyline_polygon_distance(&[a, b], &copper.ring),
    }
}

/// The board's outline and cutouts, prepared once per check run.
pub struct BoardEdges {
    /// A plain rectangle of this size at the origin: judged on bounds.
    rect: Option<Point>,
    loops: Vec<EdgeLoop>,
}

impl BoardEdges {
    pub fn of(project: &Project) -> Self {
        if let BoardShape::Rectangle = project.board_shape {
            return Self {
                rect: Some(project.board_size),
                loops: Vec::new(),
            };
        }
        let board = project.board();
        let mut loops = Vec::new();
        loops.extend(EdgeLoop::of(&board.outer, EdgeLoopKind::Outer));
        for cutout in &board.cutouts {
            loops.extend(EdgeLoop::of(
                &cutout.outline,
                EdgeLoopKind::Cutout(cutout.kind),
            ));
        }
        Self { rect: None, loops }
    }

    /// Where `copper` sits against the board, with `limit` the least gap
    /// it must keep from every edge (0: it may touch).
    pub fn verdict(&self, copper: &CopperRing, limit: f64) -> EdgeVerdict {
        let Some(bounds) = copper.bounds else {
            return EdgeVerdict::Clear;
        };
        if let Some(size) = self.rect {
            if !copper.inside(size) {
                return EdgeVerdict::Outside;
            }
            let gap = bounds
                .min
                .into_iter()
                .chain([size[0] - bounds.max[0], size[1] - bounds.max[1]])
                .fold(f64::INFINITY, f64::min)
                .max(0.0);
            return if limit > 0.0 && gap < limit - EPS {
                EdgeVerdict::TooClose {
                    gap,
                    near: EdgeLoopKind::Outer,
                }
            } else {
                EdgeVerdict::Clear
            };
        }
        let mut nearest: Option<(f64, EdgeLoopKind, f64)> = None;
        for edges in &self.loops {
            let slack = copper.slack + edges.slack;
            let reach = limit.max(0.0) + slack + EPS;
            let reaches = edges.bounds.inflated(reach).intersects(&bounds);
            let near: Vec<(Point, Point)> = edges
                .edges
                .iter()
                .filter(|_| reaches)
                .enumerate()
                .filter(|(_, b)| b.inflated(reach).intersects(&bounds))
                .filter_map(|(k, _)| edge(&edges.ring, k))
                .collect();
            let mut gap = f64::INFINITY;
            for (a, b) in &near {
                let d = edge_gap(copper, *a, *b);
                if d < -EPS {
                    return crossing(edges.kind);
                }
                gap = gap.min(d.max(0.0));
            }
            // No edge cuts the copper, so it lies wholly on one side of
            // this loop: probe the point of it farthest from the near
            // edges.
            let probe = probe(copper, &near);
            let inside = polygon_contains(&edges.ring, probe);
            match edges.kind {
                EdgeLoopKind::Outer if !inside => return EdgeVerdict::Outside,
                EdgeLoopKind::Cutout(kind) if inside => return EdgeVerdict::Inside(kind),
                _ => {}
            }
            if gap.is_finite() && nearest.is_none_or(|(g, _, s)| gap - slack < g - s) {
                nearest = Some((gap, edges.kind, slack));
            }
        }
        match nearest {
            Some((gap, near, slack)) if limit > 0.0 && gap - slack < limit - EPS => {
                EdgeVerdict::TooClose { gap, near }
            }
            _ => EdgeVerdict::Clear,
        }
    }
}

fn crossing(kind: EdgeLoopKind) -> EdgeVerdict {
    match kind {
        EdgeLoopKind::Outer => EdgeVerdict::Outside,
        EdgeLoopKind::Cutout(kind) => EdgeVerdict::Inside(kind),
    }
}

/// A point of the copper that decides which side of a loop it is on: a
/// track's centre-line start (a half-width from any edge that does not
/// cut it), else the ring vertex farthest from the `near` edges.
fn probe(copper: &CopperRing, near: &[(Point, Point)]) -> Point {
    if let Some((start, _, _)) = copper.spine {
        return start;
    }
    let far = |p: Point| {
        near.iter()
            .map(|(a, b)| segment_distance(p, *a, *b))
            .fold(f64::INFINITY, f64::min)
    };
    copper
        .ring
        .iter()
        .copied()
        .max_by(|p, q| far(*p).total_cmp(&far(*q)))
        .unwrap_or(copper.ring.first().copied().unwrap_or_default())
}
