//! Sketch edits and pad templates on Solidcraft's public `Sketch` API.
//! Solidcraft's own builders are private engine code, so these are
//! Pcbcraft's. New ids come from the sketch's counters and are reported by
//! diffing the entity ids before and after, never invented by the caller.

use std::collections::BTreeSet;

use pcbcraft_core::geometry::Point;
use pcbcraft_core::library::footprint_source::{
    DimKind, DimValue, GeomConstraint, PadTemplate, PadTemplateParams, SketchConstraintId,
    SketchCurveId, SketchEdit, SketchOutcome, SketchPointId, SketchRef,
};
use solidcraft_engine::geom::Vec2;
use solidcraft_engine::sketch::{
    ConstraintKind, CurveKind, Sketch, SketchError, measure_dimension,
};

use super::check::MAX_COORD_MM;
use super::params::Values;
use super::{Outcome, problem};

const EDIT: &str = "footprint.sketch_edit";

fn sketch_error(e: SketchError) -> super::Problem {
    problem(EDIT, e.to_string())
}

pub(crate) fn vec2(p: Point) -> Outcome<Vec2> {
    if p.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD_MM) {
        Ok(Vec2::new(p[0], p[1]))
    } else {
        Err(problem(EDIT, format!("point {p:?} is out of range")))
    }
}

fn point_index(sk: &Sketch, id: &str) -> Outcome<usize> {
    sk.point_index(id)
        .ok_or_else(|| problem(EDIT, format!("no sketch point `{id}`")))
}

fn curve_index(sk: &Sketch, id: &str) -> Outcome<usize> {
    sk.curve_index(id)
        .ok_or_else(|| problem(EDIT, format!("no sketch curve `{id}`")))
}

fn is_line(sk: &Sketch, c: usize) -> bool {
    matches!(
        sk.curves.get(c).map(|c| &c.kind),
        Some(CurveKind::Line { .. })
    )
}

/// Every entity id, in sketch order (points, curves, constraints).
fn ids(sk: &Sketch) -> Vec<SketchRef> {
    sk.points
        .iter()
        .map(|p| SketchRef::Point(SketchPointId(p.id.clone())))
        .chain(
            sk.curves
                .iter()
                .map(|c| SketchRef::Curve(SketchCurveId(c.id.clone()))),
        )
        .chain(
            sk.constraints
                .iter()
                .map(|k| SketchRef::Constraint(SketchConstraintId(k.id.clone()))),
        )
        .collect()
}

/// What changed between two sketches, in sketch order.
pub(crate) fn diff(before: &Sketch, after: &Sketch, sketch: String) -> SketchOutcome {
    let old: BTreeSet<SketchRef> = ids(before).into_iter().collect();
    let new_ids = ids(after);
    let new: BTreeSet<&SketchRef> = new_ids.iter().collect();
    SketchOutcome {
        sketch: pcbcraft_core::library::footprint_source::SketchBlob(sketch),
        created: new_ids
            .iter()
            .filter(|r| !old.contains(*r))
            .cloned()
            .collect(),
        deleted: ids(before)
            .into_iter()
            .filter(|r| !new.contains(r))
            .collect(),
    }
}

enum Ref {
    P(usize),
    C(usize),
}

fn resolve(sk: &Sketch, refs: &[SketchRef]) -> Outcome<Vec<Ref>> {
    if refs.len() > 3 {
        return Err(problem(EDIT, "a constraint takes at most three entities"));
    }
    refs.iter()
        .map(|r| match r {
            SketchRef::Point(p) => point_index(sk, &p.0).map(Ref::P),
            SketchRef::Curve(c) => curve_index(sk, &c.0).map(Ref::C),
            SketchRef::Constraint(k) => Err(problem(
                EDIT,
                format!("constraint `{}` cannot be constrained", k.0),
            )),
        })
        .collect()
}

fn geom_constraint(sk: &Sketch, kind: GeomConstraint, refs: &[Ref]) -> Outcome<ConstraintKind> {
    use ConstraintKind as K;
    use GeomConstraint as G;
    Ok(match (kind, refs) {
        (G::Coincident, [Ref::P(p), Ref::P(q)]) => K::Coincident { p: *p, q: *q },
        (G::Coincident | G::PointOnCurve, [Ref::P(p), Ref::C(c)]) => {
            K::PointOnCurve { p: *p, c: *c }
        }
        (G::Horizontal, [Ref::C(l)]) => K::Horizontal { l: *l },
        (G::Horizontal, [Ref::P(p), Ref::P(q)]) => K::HorizontalPoints { p: *p, q: *q },
        (G::Vertical, [Ref::C(l)]) => K::Vertical { l: *l },
        (G::Vertical, [Ref::P(p), Ref::P(q)]) => K::VerticalPoints { p: *p, q: *q },
        (G::Parallel, [Ref::C(a), Ref::C(b)]) => K::Parallel { a: *a, b: *b },
        (G::Perpendicular, [Ref::C(a), Ref::C(b)]) => K::Perpendicular { a: *a, b: *b },
        (G::Equal, [Ref::C(a), Ref::C(b)]) => K::Equal { a: *a, b: *b },
        (G::Tangent, [Ref::C(a), Ref::C(b)]) => K::Tangent { a: *a, b: *b },
        (G::Concentric, [Ref::C(a), Ref::C(b)]) => K::Concentric { a: *a, b: *b },
        (G::Midpoint, [Ref::P(p), Ref::C(l)]) => K::Midpoint { p: *p, l: *l },
        (G::Symmetric, [Ref::P(p), Ref::P(q), Ref::C(l)]) => K::Symmetric {
            p: *p,
            q: *q,
            l: *l,
        },
        (G::Fixed, [Ref::P(p)]) => K::Fix { p: *p },
        _ => {
            return Err(problem(
                EDIT,
                format!("{kind:?} does not take these {} entities", refs.len()),
            ));
        }
    })
    .and_then(|k| sk.validate(&k).map(|()| k).map_err(sketch_error))
}

fn dimension(sk: &Sketch, kind: DimKind, refs: &[Ref]) -> Outcome<ConstraintKind> {
    use ConstraintKind as K;
    let ends = |l: usize| match sk.curves.get(l).map(|c| &c.kind) {
        Some(CurveKind::Line { a, b }) => Ok((*a, *b)),
        _ => Err(problem(EDIT, "the dimension needs a line")),
    };
    let k = match (kind, refs) {
        (DimKind::Distance, [Ref::P(p), Ref::P(q)]) => K::Distance {
            p: *p,
            q: *q,
            value: 0.0,
        },
        (DimKind::Distance, [Ref::P(p), Ref::C(l)]) => K::PointLineDistance {
            p: *p,
            l: *l,
            value: 0.0,
        },
        (DimKind::Distance, [Ref::C(l)]) => K::Length { l: *l, value: 0.0 },
        (DimKind::HorizontalDistance, [Ref::P(p), Ref::P(q)]) => K::DistanceX {
            p: *p,
            q: *q,
            value: 0.0,
        },
        (DimKind::HorizontalDistance, [Ref::C(l)]) => {
            let (p, q) = ends(*l)?;
            K::DistanceX { p, q, value: 0.0 }
        }
        (DimKind::VerticalDistance, [Ref::P(p), Ref::P(q)]) => K::DistanceY {
            p: *p,
            q: *q,
            value: 0.0,
        },
        (DimKind::VerticalDistance, [Ref::C(l)]) => {
            let (p, q) = ends(*l)?;
            K::DistanceY { p, q, value: 0.0 }
        }
        (DimKind::Radius, [Ref::C(c)]) => K::Radius { c: *c, value: 0.0 },
        (DimKind::Diameter, [Ref::C(c)]) => K::Diameter { c: *c, value: 0.0 },
        (DimKind::Angle, [Ref::C(a), Ref::C(b)]) => K::Angle {
            a: *a,
            b: *b,
            value: 0.0,
            flip: false,
        },
        (DimKind::Angle, [Ref::C(l)]) if is_line(sk, *l) => K::LineAngle { l: *l, value: 0.0 },
        _ => {
            return Err(problem(
                EDIT,
                format!(
                    "a {kind:?} dimension does not take these {} entities",
                    refs.len()
                ),
            ));
        }
    };
    sk.validate(&k).map_err(sketch_error)?;
    Ok(k)
}

/// Set a dimension's value: a fixed number (mm, or degrees for angles) or
/// a parameter binding (the current measurement stays until evaluation).
fn set_dimension(
    sk: &Sketch,
    kind: &mut ConstraintKind,
    value: &DimValue,
) -> Outcome<Option<String>> {
    match value {
        DimValue::Fixed(v) => {
            let v = if kind.is_angle() { v.to_radians() } else { *v };
            if !v.is_finite() || v.abs() > MAX_COORD_MM {
                return Err(problem(
                    EDIT,
                    format!("dimension value {v} is out of range"),
                ));
            }
            kind.set_value(v);
            Ok(None)
        }
        DimValue::Param(name) => {
            super::params::check_name(&name.0)?;
            if let Some(v) = measure_dimension(sk, kind) {
                kind.set_value(v);
            }
            Ok(Some(name.0.clone()))
        }
    }
}

fn movable(sk: &Sketch, p: usize) -> Outcome<()> {
    match sk.points.get(p) {
        Some(point) if p != 0 && !point.fixed && point.exprs.is_none() => Ok(()),
        Some(point) => Err(problem(
            EDIT,
            format!("point `{}` is fixed and cannot move", point.id),
        )),
        None => Err(problem(EDIT, "no such point")),
    }
}

/// Apply one sketch edit, returning the edited sketch.
pub(crate) fn apply(stored: &Sketch, edit: &SketchEdit) -> Outcome<Sketch> {
    let mut sk = stored.clone();
    match edit {
        SketchEdit::AddPoint { at } => {
            sk.add_point(vec2(*at)?, None).map_err(sketch_error)?;
        }
        SketchEdit::AddLine { a, b, construction } => {
            let l = sk
                .add_line(vec2(*a)?, vec2(*b)?, None, None, None)
                .map_err(sketch_error)?;
            set_construction(&mut sk, &[l], *construction);
        }
        SketchEdit::AddArc {
            center,
            start,
            end,
            construction,
        } => {
            let a = sk
                .add_arc(vec2(*center)?, vec2(*start)?, vec2(*end)?, [None; 3], None)
                .map_err(sketch_error)?;
            set_construction(&mut sk, &[a], *construction);
        }
        SketchEdit::AddCircle {
            center,
            radius,
            construction,
        } => {
            let c = sk
                .add_circle(vec2(*center)?, *radius, None, None)
                .map_err(sketch_error)?;
            set_construction(&mut sk, &[c], *construction);
        }
        SketchEdit::AddRect { a, b, construction } => {
            let (a, b) = (vec2(*a)?, vec2(*b)?);
            let corners = [
                Vec2::new(a.x, a.y),
                Vec2::new(b.x, a.y),
                Vec2::new(b.x, b.y),
                Vec2::new(a.x, b.y),
            ];
            let lines = closed_polygon(&mut sk, &corners)?;
            set_construction(&mut sk, &lines, *construction);
            for (k, l) in lines.iter().enumerate() {
                let kind = if k % 2 == 0 {
                    ConstraintKind::Horizontal { l: *l }
                } else {
                    ConstraintKind::Vertical { l: *l }
                };
                sk.add_constraint(kind, None).map_err(sketch_error)?;
            }
        }
        SketchEdit::AddPolyline {
            points,
            closed,
            construction,
        } => {
            if points.len() < 2 || points.len() > super::check::MAX_CURVES {
                return Err(problem(EDIT, "a polyline needs 2–4096 points"));
            }
            let at: Vec<Vec2> = points.iter().map(|p| vec2(*p)).collect::<Outcome<_>>()?;
            let lines = if *closed {
                if at.len() < 3 {
                    return Err(problem(EDIT, "a closed polyline needs three points"));
                }
                closed_polygon(&mut sk, &at)?
            } else {
                open_polyline(&mut sk, &at)?
            };
            set_construction(&mut sk, &lines, *construction);
        }
        SketchEdit::AddConstraint { kind, refs } => {
            let refs = resolve(&sk, refs)?;
            let k = geom_constraint(&sk, *kind, &refs)?;
            sk.add_constraint(k, None).map_err(sketch_error)?;
        }
        SketchEdit::AddDimension { kind, refs, value } => {
            let refs = resolve(&sk, refs)?;
            let mut k = dimension(&sk, *kind, &refs)?;
            if let Some(v) = measure_dimension(&sk, &k) {
                k.set_value(v);
            }
            let param = set_dimension(&sk, &mut k, value)?;
            sk.add_constraint(k, param).map_err(sketch_error)?;
        }
        SketchEdit::SetDimension { constraint, value } => {
            let i = sk
                .constraints
                .iter()
                .position(|k| k.id == constraint.0)
                .ok_or_else(|| problem(EDIT, format!("no constraint `{}`", constraint.0)))?;
            let mut kind = sk
                .constraints
                .get(i)
                .map(|k| k.kind.clone())
                .ok_or_else(|| problem(EDIT, "no such constraint"))?;
            if !kind.is_dimension() {
                return Err(problem(
                    EDIT,
                    format!("`{}` is not a dimension", constraint.0),
                ));
            }
            let param = set_dimension(&sk, &mut kind, value)?;
            if let Some(k) = sk.constraints.get_mut(i) {
                k.kind = kind;
                k.param = param;
            }
        }
        SketchEdit::Drag { point, to } => {
            let p = point_index(&sk, &point.0)?;
            movable(&sk, p)?;
            let to = vec2(*to)?;
            if let Some(point) = sk.points.get_mut(p) {
                point.pos = to;
            }
        }
        SketchEdit::Move { ids, delta } => {
            let d = vec2(*delta)?;
            let mut points = BTreeSet::new();
            for r in ids {
                match r {
                    SketchRef::Point(p) => {
                        points.insert(point_index(&sk, &p.0)?);
                    }
                    SketchRef::Curve(c) => {
                        let c = curve_index(&sk, &c.0)?;
                        if let Some(curve) = sk.curves.get(c) {
                            points.extend(curve.kind.point_ids());
                        }
                    }
                    SketchRef::Constraint(k) => {
                        return Err(problem(EDIT, format!("constraint `{}` cannot move", k.0)));
                    }
                }
            }
            for p in points {
                movable(&sk, p)?;
                if let Some(point) = sk.points.get_mut(p) {
                    let at = point.pos + d;
                    if !(at.is_finite() && at.x.abs() <= MAX_COORD_MM && at.y.abs() <= MAX_COORD_MM)
                    {
                        return Err(problem(EDIT, "the move leaves the coordinate range"));
                    }
                    point.pos = at;
                }
            }
        }
        SketchEdit::Delete { ids } => {
            let mut points = Vec::new();
            let mut curves = Vec::new();
            let mut constraints = BTreeSet::new();
            for r in ids {
                match r {
                    SketchRef::Point(p) => {
                        let i = point_index(&sk, &p.0)?;
                        if i == 0 {
                            return Err(problem(EDIT, "the sketch origin cannot be deleted"));
                        }
                        points.push(i);
                    }
                    SketchRef::Curve(c) => curves.push(curve_index(&sk, &c.0)?),
                    SketchRef::Constraint(k) => {
                        if !sk.constraints.iter().any(|c| c.id == k.0) {
                            return Err(problem(EDIT, format!("no constraint `{}`", k.0)));
                        }
                        constraints.insert(k.0.clone());
                    }
                }
            }
            sk.constraints.retain(|k| !constraints.contains(&k.id));
            // Curves first (by index), then points by id: removing curves
            // renumbers points.
            let point_ids: Vec<String> = points
                .iter()
                .filter_map(|&i| sk.points.get(i).map(|p| p.id.clone()))
                .collect();
            sk.remove_curves(&curves);
            let points: Vec<usize> = point_ids
                .iter()
                .filter_map(|id| sk.point_index(id))
                .collect();
            sk.remove_points(&points);
        }
    }
    Ok(sk)
}

fn set_construction(sk: &mut Sketch, curves: &[usize], construction: bool) {
    for &c in curves {
        if let Some(curve) = sk.curves.get_mut(c) {
            curve.construction = construction;
        }
    }
}

fn closed_polygon(sk: &mut Sketch, at: &[Vec2]) -> Outcome<Vec<usize>> {
    let pts: Vec<usize> = at
        .iter()
        .map(|p| sk.add_point(*p, None).map_err(sketch_error))
        .collect::<Outcome<_>>()?;
    let mut lines = Vec::with_capacity(pts.len());
    for (k, &a) in pts.iter().enumerate() {
        let b = pts.get((k + 1) % pts.len()).copied().unwrap_or(a);
        lines.push(sk.add_line_pts(a, b, None).map_err(sketch_error)?);
    }
    Ok(lines)
}

fn open_polyline(sk: &mut Sketch, at: &[Vec2]) -> Outcome<Vec<usize>> {
    let pts: Vec<usize> = at
        .iter()
        .map(|p| sk.add_point(*p, None).map_err(sketch_error))
        .collect::<Outcome<_>>()?;
    pts.windows(2)
        .map(|w| match w {
            [a, b] => sk.add_line_pts(*a, *b, None).map_err(sketch_error),
            _ => Err(problem(EDIT, "polyline")),
        })
        .collect()
}

/// A constrained pad template drawn at its evaluated size: the anchor,
/// the curves it owns (copper outline and construction helpers) and the
/// sketch. Size dimensions bind to the template expressions through
/// `Constraint.param`; the anchor's own location is left to the author.
pub(crate) struct Drawn {
    pub sketch: Sketch,
    pub anchor: String,
    pub owned: Vec<String>,
}

fn paren(e: &str) -> String {
    format!("({e})")
}

pub(crate) fn template_pad(
    stored: &Sketch,
    values: &Values,
    template: PadTemplate,
    at: Point,
    params: &PadTemplateParams,
) -> Outcome<Drawn> {
    let mut sk = stored.clone();
    let at = vec2(at)?;
    let curves_before = sk.curves.len();
    let anchor = sk.add_point(at, None).map_err(sketch_error)?;
    let w = values.positive(&params.width.0)?;
    let h = values.positive(&params.height.0)?;
    let k = |sk: &mut Sketch, kind: ConstraintKind, param: Option<String>| {
        sk.add_constraint(kind, param)
            .map(|_| ())
            .map_err(sketch_error)
    };
    match template {
        PadTemplate::Rect => {
            let corners = rect_corners(at, w * 0.5, h * 0.5);
            let pts: Vec<usize> = corners
                .iter()
                .map(|p| sk.add_point(*p, None).map_err(sketch_error))
                .collect::<Outcome<_>>()?;
            let [p0, p1, p2, p3] = four(&pts)?;
            let sides = [
                sk.add_line_pts(p0, p1, None).map_err(sketch_error)?,
                sk.add_line_pts(p1, p2, None).map_err(sketch_error)?,
                sk.add_line_pts(p2, p3, None).map_err(sketch_error)?,
                sk.add_line_pts(p3, p0, None).map_err(sketch_error)?,
            ];
            let diagonal = sk.add_line_pts(p0, p2, None).map_err(sketch_error)?;
            set_construction(&mut sk, &[diagonal], true);
            for (i, l) in sides.iter().enumerate() {
                let kind = if i % 2 == 0 {
                    ConstraintKind::Horizontal { l: *l }
                } else {
                    ConstraintKind::Vertical { l: *l }
                };
                k(&mut sk, kind, None)?;
            }
            k(
                &mut sk,
                ConstraintKind::DistanceX {
                    p: p0,
                    q: p1,
                    value: w,
                },
                Some(params.width.0.clone()),
            )?;
            k(
                &mut sk,
                ConstraintKind::DistanceY {
                    p: p1,
                    q: p2,
                    value: h,
                },
                Some(params.height.0.clone()),
            )?;
            k(
                &mut sk,
                ConstraintKind::Midpoint {
                    p: anchor,
                    l: diagonal,
                },
                None,
            )?;
        }
        PadTemplate::Circle => {
            let c = sk
                .add_circle(at, w * 0.5, Some(anchor), None)
                .map_err(sketch_error)?;
            k(
                &mut sk,
                ConstraintKind::Diameter { c, value: w },
                Some(params.width.0.clone()),
            )?;
        }
        PadTemplate::Oblong => {
            // Caps on the long axis; the overall length and cap diameter
            // are the two size dimensions.
            let along_x = w >= h;
            let (long, short, long_e, short_e) = if along_x {
                (w, h, &params.width.0, &params.height.0)
            } else {
                (h, w, &params.height.0, &params.width.0)
            };
            let r = short * 0.5;
            let half = (long * 0.5 - r).max(0.0);
            if half <= 1e-6 {
                return Err(problem(
                    EDIT,
                    "an oblong pad needs a long side longer than its short side (use a circle)",
                ));
            }
            let axis = |t: f64, n: f64| {
                if along_x {
                    Vec2::new(at.x + t, at.y + n)
                } else {
                    Vec2::new(at.x - n, at.y + t)
                }
            };
            let c1 = sk.add_point(axis(-half, 0.0), None).map_err(sketch_error)?;
            let c2 = sk.add_point(axis(half, 0.0), None).map_err(sketch_error)?;
            let t1 = sk.add_point(axis(-half, -r), None).map_err(sketch_error)?;
            let t2 = sk.add_point(axis(half, -r), None).map_err(sketch_error)?;
            let t3 = sk.add_point(axis(half, r), None).map_err(sketch_error)?;
            let t4 = sk.add_point(axis(-half, r), None).map_err(sketch_error)?;
            let lower = sk.add_line_pts(t1, t2, None).map_err(sketch_error)?;
            let arc2 = add_arc_pts(&mut sk, c2, t2, t3)?;
            let upper = sk.add_line_pts(t3, t4, None).map_err(sketch_error)?;
            let arc1 = add_arc_pts(&mut sk, c1, t4, t1)?;
            let spine = sk.add_line_pts(c1, c2, None).map_err(sketch_error)?;
            set_construction(&mut sk, &[spine], true);
            let straight = |l| {
                if along_x {
                    ConstraintKind::Horizontal { l }
                } else {
                    ConstraintKind::Vertical { l }
                }
            };
            // The spine, the equal caps and the anchor's place follow from
            // the two straight sides and four tangencies; constraining them
            // too would be redundant.
            k(&mut sk, straight(lower), None)?;
            k(&mut sk, straight(upper), None)?;
            for (l, a) in [(lower, arc1), (lower, arc2), (upper, arc1), (upper, arc2)] {
                k(&mut sk, ConstraintKind::Tangent { a: l, b: a }, None)?;
            }
            k(
                &mut sk,
                ConstraintKind::SlotOverallLength {
                    a: arc1,
                    b: arc2,
                    value: long,
                },
                Some(long_e.clone()),
            )?;
            k(
                &mut sk,
                ConstraintKind::Diameter {
                    c: arc1,
                    value: short,
                },
                Some(short_e.clone()),
            )?;
            k(
                &mut sk,
                ConstraintKind::Midpoint {
                    p: anchor,
                    l: spine,
                },
                None,
            )?;
        }
        PadTemplate::RoundRect => {
            let ratio_e = params
                .radius_ratio
                .as_ref()
                .ok_or_else(|| problem(EDIT, "a rounded rectangle needs a radius ratio"))?;
            let ratio = values.number(&ratio_e.0)?;
            if !(ratio > 0.0 && ratio < 0.5) {
                return Err(problem(
                    EDIT,
                    format!("radius ratio {ratio} must be above 0 and below 0.5"),
                ));
            }
            let r = ratio * w.min(h);
            let (hx, hy) = (w * 0.5 - r, h * 0.5 - r);
            // Corner centres counter-clockwise from bottom left; each
            // corner arc runs counter-clockwise between two side lines.
            let centres = rect_corners(at, hx, hy);
            let mut cs = Vec::new();
            for c in centres {
                cs.push(sk.add_point(c, None).map_err(sketch_error)?);
            }
            let [c0, c1, c2, c3] = four(&cs)?;
            let off = |c: Vec2, dx: f64, dy: f64| Vec2::new(c.x + dx, c.y + dy);
            let [q0, q1, q2, q3] = centres;
            // Side points: bottom (q0→q1), right (q1→q2), top (q2→q3), left (q3→q0).
            let spots = [
                off(q0, 0.0, -r),
                off(q1, 0.0, -r),
                off(q1, r, 0.0),
                off(q2, r, 0.0),
                off(q2, 0.0, r),
                off(q3, 0.0, r),
                off(q3, -r, 0.0),
                off(q0, -r, 0.0),
            ];
            let mut s = Vec::new();
            for p in spots {
                s.push(sk.add_point(p, None).map_err(sketch_error)?);
            }
            let pick = |i: usize| s.get(i).copied().ok_or_else(|| problem(EDIT, "template"));
            let bottom = sk
                .add_line_pts(pick(0)?, pick(1)?, None)
                .map_err(sketch_error)?;
            let a1 = add_arc_pts(&mut sk, c1, pick(1)?, pick(2)?)?;
            let right = sk
                .add_line_pts(pick(2)?, pick(3)?, None)
                .map_err(sketch_error)?;
            let a2 = add_arc_pts(&mut sk, c2, pick(3)?, pick(4)?)?;
            let top = sk
                .add_line_pts(pick(4)?, pick(5)?, None)
                .map_err(sketch_error)?;
            let a3 = add_arc_pts(&mut sk, c3, pick(5)?, pick(6)?)?;
            let left = sk
                .add_line_pts(pick(6)?, pick(7)?, None)
                .map_err(sketch_error)?;
            let a0 = add_arc_pts(&mut sk, c0, pick(7)?, pick(0)?)?;
            let diagonal = sk.add_line_pts(c0, c2, None).map_err(sketch_error)?;
            set_construction(&mut sk, &[diagonal], true);
            k(&mut sk, ConstraintKind::Horizontal { l: bottom }, None)?;
            k(&mut sk, ConstraintKind::Horizontal { l: top }, None)?;
            k(&mut sk, ConstraintKind::Vertical { l: left }, None)?;
            k(&mut sk, ConstraintKind::Vertical { l: right }, None)?;
            for (l, a) in [
                (bottom, a0),
                (bottom, a1),
                (right, a1),
                (right, a2),
                (top, a2),
                (top, a3),
                (left, a3),
                (left, a0),
            ] {
                k(&mut sk, ConstraintKind::Tangent { a: l, b: a }, None)?;
            }
            for a in [a1, a2, a3] {
                k(&mut sk, ConstraintKind::Equal { a: a0, b: a }, None)?;
            }
            k(
                &mut sk,
                ConstraintKind::DistanceX {
                    p: pick(6)?,
                    q: pick(2)?,
                    value: w,
                },
                Some(params.width.0.clone()),
            )?;
            k(
                &mut sk,
                ConstraintKind::DistanceY {
                    p: pick(0)?,
                    q: pick(4)?,
                    value: h,
                },
                Some(params.height.0.clone()),
            )?;
            k(
                &mut sk,
                ConstraintKind::Radius { c: a0, value: r },
                Some(format!(
                    "{} * min({}, {})",
                    paren(&ratio_e.0),
                    paren(&params.width.0),
                    paren(&params.height.0)
                )),
            )?;
            k(
                &mut sk,
                ConstraintKind::Midpoint {
                    p: anchor,
                    l: diagonal,
                },
                None,
            )?;
        }
    }
    let anchor_id = sk
        .points
        .get(anchor)
        .map(|p| p.id.clone())
        .ok_or_else(|| problem(EDIT, "template anchor"))?;
    let owned = sk
        .curves
        .iter()
        .skip(curves_before)
        .map(|c| c.id.clone())
        .collect();
    Ok(Drawn {
        sketch: sk,
        anchor: anchor_id,
        owned,
    })
}

fn four(v: &[usize]) -> Outcome<[usize; 4]> {
    match v {
        [a, b, c, d] => Ok([*a, *b, *c, *d]),
        _ => Err(problem(EDIT, "template corners")),
    }
}

/// Counter-clockwise from the bottom-left corner.
fn rect_corners(c: Vec2, hx: f64, hy: f64) -> [Vec2; 4] {
    [
        Vec2::new(c.x - hx, c.y - hy),
        Vec2::new(c.x + hx, c.y - hy),
        Vec2::new(c.x + hx, c.y + hy),
        Vec2::new(c.x - hx, c.y + hy),
    ]
}

fn add_arc_pts(sk: &mut Sketch, c: usize, a: usize, b: usize) -> Outcome<usize> {
    let (Some(pc), Some(pa), Some(pb)) = (sk.point(c), sk.point(a), sk.point(b)) else {
        return Err(problem(EDIT, "arc points"));
    };
    sk.add_arc(pc, pa, pb, [Some(c), Some(a), Some(b)], None)
        .map_err(sketch_error)
}
