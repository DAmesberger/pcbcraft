//! What a view needs to draw and pick part dimensions (slice S5): each
//! reference's geometry in the board, the board curve under a point, and
//! the distance a dimension shows between two references. Read from the
//! document only (the stored board solve and model traces); nothing here
//! traces a model or solves.
use super::{FeatureGeometry, board_sketch, features, model, reference_status};
use crate::Layer;
use crate::geometry::{Point, Transform2};
use crate::layout::outline::BoardShapeEvaluator;
use crate::library::footprint_source::{SketchCurveId, SketchRef};
use crate::library::{Outline, Segment};
use crate::model::board::ConstraintTarget;
use crate::model::dimension::{BoardRef, ReferenceStatus};
use crate::model::{Placement, Project};

/// A placement's part frame → board: its position, turn and (on the
/// bottom) mirror.
pub fn part_transform(placement: &Placement) -> Transform2 {
    Transform2 {
        translate: placement.position,
        rotation_deg: placement.rotation_deg,
        mirror: placement.layer == Layer::Bottom,
    }
}

/// `g` moved by `t`; a mirror turns an arc the other way.
pub fn transformed(g: FeatureGeometry, t: &Transform2) -> FeatureGeometry {
    match g {
        FeatureGeometry::Point { at } => FeatureGeometry::Point { at: t.apply(at) },
        FeatureGeometry::Line { a, b } => FeatureGeometry::Line {
            a: t.apply(a),
            b: t.apply(b),
        },
        FeatureGeometry::Arc { a, b, bulge } => FeatureGeometry::Arc {
            a: t.apply(a),
            b: t.apply(b),
            bulge: if t.mirror { -bulge } else { bulge },
        },
        FeatureGeometry::Circle { center, radius } => FeatureGeometry::Circle {
            center: t.apply(center),
            radius,
        },
    }
}

/// A target's frame → board.
fn target_transform(project: &Project, target: ConstraintTarget) -> Option<Transform2> {
    match target {
        ConstraintTarget::Placement(id) => project.placement_of(id).map(part_transform),
        // A fill's or keepout's features are in the board already.
        ConstraintTarget::Fill(_) | ConstraintTarget::Keepout(_) => Some(Transform2::IDENTITY),
    }
}

/// One segment of an outline as geometry, by index.
fn segment(outline: &Outline, index: usize) -> Option<FeatureGeometry> {
    let a = match index.checked_sub(1) {
        None => outline.start,
        Some(i) => outline.segments.get(i)?.to(),
    };
    Some(match *outline.segments.get(index)? {
        Segment::Line { to } => FeatureGeometry::Line { a, b: to },
        Segment::Arc { to, bulge: 0.0 } => FeatureGeometry::Line { a, b: to },
        Segment::Arc { to, bulge } => FeatureGeometry::Arc { a, b: to, bulge },
    })
}

/// Every board curve's solved pieces: (curve, piece in the board).
fn board_pieces(
    project: &Project,
    evaluator: Option<&dyn BoardShapeEvaluator>,
) -> Vec<(SketchCurveId, FeatureGeometry)> {
    let solved = project.board();
    let mut out = Vec::new();
    let mut add = |outline: &Outline, curves: &[SketchCurveId]| {
        for (i, curve) in curves.iter().enumerate() {
            if let Some(g) = segment(outline, i) {
                out.push((curve.clone(), g));
            }
        }
    };
    add(&solved.outer, &solved.outer_curves);
    for c in &solved.cutouts {
        add(&c.outline, &c.curves);
    }
    if out.is_empty() {
        // A plain rectangle: its named edges, by the curves of the sketch
        // the parts solve holds it as.
        if let Some(board) = evaluator.and_then(|ev| board_sketch(project, ev).ok()) {
            for edge in &solved.edges {
                if let Some(named) = board.edges.iter().find(|e| e.name == edge.name) {
                    let g = if edge.bulge == 0.0 {
                        FeatureGeometry::Line {
                            a: edge.a,
                            b: edge.b,
                        }
                    } else {
                        FeatureGeometry::Arc {
                            a: edge.a,
                            b: edge.b,
                            bulge: edge.bulge,
                        }
                    };
                    out.push((named.curve.clone(), g));
                }
            }
        }
    }
    out
}

/// The board curve nearest `at` (board mm) within `reach`, with the piece
/// of it there.
pub fn board_curve_at(
    project: &Project,
    evaluator: Option<&dyn BoardShapeEvaluator>,
    at: Point,
    reach: f64,
) -> Option<(SketchCurveId, FeatureGeometry)> {
    board_pieces(project, evaluator)
        .into_iter()
        .filter_map(|(c, g)| {
            let d = distance_to(&g, at)?;
            (d <= reach).then_some((d, c, g))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, c, g)| (c, g))
}

/// A reference's geometry in the board and how it resolves. A lost or
/// inexact 3D model reference gives the geometry it had last; a lost one
/// with none, `None`.
pub fn reference_geometry(
    project: &Project,
    evaluator: Option<&dyn BoardShapeEvaluator>,
    r: &BoardRef,
) -> Option<(FeatureGeometry, ReferenceStatus)> {
    let ids = super::board_ids(project);
    let status = reference_status(project, ids.as_ref(), r);
    let geometry = match r {
        BoardRef::Board(SketchRef::Curve(curve)) => board_pieces(project, evaluator)
            .into_iter()
            .find(|(c, _)| c == curve)
            .map(|(_, g)| g)?,
        BoardRef::Board(_) | BoardRef::Housing(_) => return None,
        BoardRef::Part(f) => {
            let local = if f.is_model() {
                model::find(project, f).and_then(|t| t.geometry)?
            } else {
                features::geometry(project, f).ok()?
            };
            transformed(local, &target_transform(project, f.target())?)
        }
    };
    Some((geometry, status))
}

/// A dimension as a view draws it: from a point of the first reference
/// to a point of the second, measured along the unit `along`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub from: Point,
    pub to: Point,
    pub along: Point,
    /// The distance, mm.
    pub value: f64,
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn unit(v: Point) -> Option<Point> {
    let len = v[0].hypot(v[1]);
    (len.is_finite() && len > 1e-12).then(|| [v[0] / len, v[1] / len])
}

/// An arc's centre and radius, from its ends and bulge.
fn arc_circle(a: Point, b: Point, bulge: f64) -> Option<(Point, f64)> {
    let chord = sub(b, a);
    let half = chord[0].hypot(chord[1]) / 2.0;
    if half.is_nan() || half <= 1e-12 || bulge == 0.0 || !bulge.is_finite() {
        return None;
    }
    // Sagitta over half chord is the bulge; the radius follows.
    let radius = half * (1.0 + bulge * bulge) / (2.0 * bulge.abs());
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let n = [-chord[1] / (2.0 * half), chord[0] / (2.0 * half)];
    // The centre lies on the left of a → b for a counter-clockwise arc
    // under half a turn.
    let off = (radius * radius - half * half).max(0.0).sqrt() * bulge.signum();
    let off = if bulge.abs() > 1.0 { -off } else { off };
    Some(([mid[0] + n[0] * off, mid[1] + n[1] * off], radius))
}

/// The point of a reference a distance is measured from: a point itself,
/// a circle's centre, a line's middle.
fn anchor(g: &FeatureGeometry) -> Point {
    match *g {
        FeatureGeometry::Point { at } => at,
        FeatureGeometry::Circle { center, .. } => center,
        FeatureGeometry::Line { a, b } | FeatureGeometry::Arc { a, b, .. } => {
            [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
        }
    }
}

/// The nearest point of `g`'s line, arc or circle (as a whole curve) to
/// `p`, and the direction from `p` to it when they meet.
fn foot(g: &FeatureGeometry, p: Point) -> Option<(Point, Option<Point>)> {
    match *g {
        FeatureGeometry::Point { at } => Some((at, None)),
        FeatureGeometry::Line { a, b } => {
            let u = unit(sub(b, a))?;
            let s = dot(sub(p, a), u);
            Some(([a[0] + u[0] * s, a[1] + u[1] * s], Some([-u[1], u[0]])))
        }
        FeatureGeometry::Arc { a, b, bulge } => {
            let (c, r) = arc_circle(a, b, bulge)?;
            let d = unit(sub(p, c))?;
            Some(([c[0] + d[0] * r, c[1] + d[1] * r], Some(d)))
        }
        FeatureGeometry::Circle { center, .. } => Some((center, None)),
    }
}

/// How far `p` is from `g` (a line or arc as its piece, a point or circle
/// as its centre).
fn distance_to(g: &FeatureGeometry, p: Point) -> Option<f64> {
    let d = |q: Point| (p[0] - q[0]).hypot(p[1] - q[1]);
    match *g {
        FeatureGeometry::Line { a, b } => {
            let v = sub(b, a);
            let len2 = dot(v, v);
            let t = if len2 > 0.0 {
                (dot(sub(p, a), v) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            Some(d([a[0] + v[0] * t, a[1] + v[1] * t]))
        }
        FeatureGeometry::Arc { a, b, bulge } => {
            // Off its circle: close enough for picking a board's arc edge.
            let (c, r) = arc_circle(a, b, bulge)?;
            Some((d(c) - r).abs())
        }
        FeatureGeometry::Point { at } => Some(d(at)),
        FeatureGeometry::Circle { center, .. } => Some(d(center)),
    }
}

/// The distance a dimension between `first` and `second` shows: from the
/// first reference's point (a line's middle, a circle's centre) to the
/// nearest point of the second (across a line, along the radius of an
/// arc), or the other way round when the first is a line or arc and the
/// second a point. Lines are measured square to the second.
pub fn shown(first: &FeatureGeometry, second: &FeatureGeometry) -> Option<Shown> {
    let point_like = |g: &FeatureGeometry| {
        matches!(
            g,
            FeatureGeometry::Point { .. } | FeatureGeometry::Circle { .. }
        )
    };
    let (a, b) = if point_like(second) && !point_like(first) {
        (second, first)
    } else {
        (first, second)
    };
    let from = anchor(a);
    let (to, normal) = foot(b, from)?;
    let along = match unit(sub(to, from)) {
        Some(u) => u,
        None => normal.and_then(unit).unwrap_or([1.0, 0.0]),
    };
    let value = dot(sub(to, from), along).abs();
    value.is_finite().then_some(Shown {
        from,
        to,
        along,
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_and_a_parallel_edge_are_measured_square_to_the_edge() {
        let face = FeatureGeometry::Line {
            a: [16.0, 1.0],
            b: [24.0, 1.0],
        };
        let edge = FeatureGeometry::Line {
            a: [0.0, 0.0],
            b: [40.0, 0.0],
        };
        let s = shown(&face, &edge).unwrap_or(Shown {
            from: [0.0; 2],
            to: [0.0; 2],
            along: [0.0; 2],
            value: -1.0,
        });
        assert!((s.value - 1.0).abs() < 1e-12, "{s:?}");
        assert_eq!(s.from, [20.0, 1.0]);
        assert_eq!(s.to, [20.0, 0.0]);
        // Flush: no length, measured along the edge's normal.
        let flush = FeatureGeometry::Line {
            a: [16.0, 0.0],
            b: [24.0, 0.0],
        };
        let s = shown(&flush, &edge);
        assert!(s.is_some_and(|s| s.value == 0.0 && s.along[0] == 0.0));
    }

    #[test]
    fn a_point_to_an_arc_is_measured_along_the_radius() {
        // A quarter arc of radius 10 about the origin, from (10, 0).
        let bulge = (std::f64::consts::FRAC_PI_2 / 4.0).tan();
        let arc = FeatureGeometry::Arc {
            a: [10.0, 0.0],
            b: [0.0, 10.0],
            bulge,
        };
        let p = FeatureGeometry::Point { at: [3.0, 4.0] };
        let s = shown(&p, &arc);
        assert!(s.is_some_and(|s| (s.value - 5.0).abs() < 1e-9), "{s:?}");
        let s = shown(&arc, &p);
        assert!(s.is_some_and(|s| (s.value - 5.0).abs() < 1e-9), "{s:?}");
    }
}
