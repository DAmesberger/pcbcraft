//! A Gerber file's image as exact polygons in integer nanometres, over the
//! permissive `i_overlay` crate (MIT OR Apache-2.0; integer arithmetic, so
//! results are deterministic).
//!
//! Every flash, stroke and region is expanded to closed rings: circles and
//! arcs become chords whose vertices lie on the true curve, within
//! [`ARC_TOLERANCE_NM`]; straight edges are exact. Dark and clear objects
//! are applied in file order (runs of one polarity at a time). A [`Shapes`]
//! value is i_overlay's normal form: each shape an outer ring
//! (counter-clockwise) followed by its holes (clockwise).
use crate::gerber::{Contour, GerberFile, Kind, Object, Path, Prim, Template};
use crate::{Fallible, Point};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::Overlay;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::i_float::int::angle::Angle;
use i_overlay::i_float::int::point::IntPoint;
use i_overlay::mesh::int::arc::ArcOptions;
use i_overlay::mesh::int::outline::offset::IntOutlineOffset;
use i_overlay::mesh::int::style::{IntLineJoin, IntOutlineStyle};

/// Largest distance of a chord from its arc, nm.
pub const ARC_TOLERANCE_NM: f64 = 250.0;
/// Largest coordinate magnitude, nm (1 m: far beyond any board, well
/// inside i_overlay's safe integer range).
pub const MAX_COORD_NM: f64 = 1e9;
/// Most vertices one curve is flattened to.
const MAX_CURVE_VERTICES: f64 = 100_000.0;

pub type Ring = Vec<IntPoint>;
/// An outer ring followed by its holes.
pub type Shape = Vec<Ring>;
pub type Shapes = Vec<Shape>;

/// Millimetres to whole nanometres, refusing a coordinate past
/// [`MAX_COORD_NM`].
pub fn nm(p: Point) -> Fallible<IntPoint> {
    let x = (p[0] * 1e6).round();
    let y = (p[1] * 1e6).round();
    if !(x.abs() <= MAX_COORD_NM && y.abs() <= MAX_COORD_NM) {
        return Err(format!("polygons: a coordinate past {MAX_COORD_NM} nm").into());
    }
    Ok(IntPoint::new(x as i32, y as i32))
}

/// A ring from millimetre points.
fn ring(points: &[Point]) -> Fallible<Ring> {
    points.iter().map(|p| nm(*p)).collect()
}

/// Twice the signed area of a ring, nm² (positive counter-clockwise).
pub fn area2(ring: &[IntPoint]) -> i128 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            i128::from(a.x) * i128::from(b.y) - i128::from(b.x) * i128::from(a.y)
        })
        .sum()
}

/// The area of shapes in normal form, mm².
pub fn area_mm2(shapes: &[Shape]) -> f64 {
    let twice: i128 = shapes.iter().flatten().map(|r| area2(r)).sum();
    twice as f64 / 2.0 / 1e12
}

/// Rings cleaned into normal form under `rule`.
pub fn normalise(rings: &[Ring], rule: FillRule) -> Shapes {
    let rings: Vec<&Ring> = rings.iter().filter(|r| r.len() >= 3).collect();
    if rings.is_empty() {
        return Vec::new();
    }
    let rings: Vec<Ring> = rings.into_iter().cloned().collect();
    Overlay::from_subj(&rings).overlay(OverlayRule::Subject, rule)
}

/// One boolean operation on two shape sets in normal form.
pub fn boolean(a: &[Shape], b: &[Shape], rule: OverlayRule) -> Shapes {
    if a.is_empty() && b.is_empty() {
        return Vec::new();
    }
    Overlay::from_subj_and_clip(&a.to_vec(), &b.to_vec()).overlay(rule, FillRule::NonZero)
}

/// The shapes grown by `by` nm (negative shrinks), with round joins of
/// at most 1/64 turn per step.
pub fn offset(shapes: &[Shape], by: i32) -> Fallible<Shapes> {
    if shapes.is_empty() || by == 0 {
        return Ok(shapes.to_vec());
    }
    let shapes = shapes.to_vec();
    let join = IntLineJoin::Round(ArcOptions {
        max_step: Angle::from_bits(1 << 26),
        ..ArcOptions::default()
    });
    Ok(shapes.outline(&IntOutlineStyle::new(by).line_join(join))?)
}

/// Vertices that keep the chords of an arc of `radius` (mm) within
/// [`ARC_TOLERANCE_NM`] over `sweep` radians.
fn steps(radius: f64, sweep: f64) -> usize {
    let r = radius * 1e6;
    let step = if r > ARC_TOLERANCE_NM {
        2.0 * (1.0 - ARC_TOLERANCE_NM / r).acos()
    } else {
        std::f64::consts::FRAC_PI_2
    };
    (sweep.abs() / step).ceil().clamp(1.0, MAX_CURVE_VERTICES) as usize
}

/// A full circle, counter-clockwise from angle 0.
fn circle(c: Point, r: f64) -> Vec<Point> {
    let n = steps(r, std::f64::consts::TAU).max(8);
    (0..n)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}

/// The points of an arc from `from` to `to` about `center`, `from`
/// included and `to` excluded; `from == to` is a full circle.
fn arc(from: Point, to: Point, center: Point, clockwise: bool) -> Vec<Point> {
    let r = (from[0] - center[0]).hypot(from[1] - center[1]);
    let a0 = (from[1] - center[1]).atan2(from[0] - center[0]);
    let a1 = (to[1] - center[1]).atan2(to[0] - center[0]);
    let tau = std::f64::consts::TAU;
    let same = (from[0] - to[0]).hypot(from[1] - to[1]) < 1e-9;
    let mut sweep = if clockwise { a0 - a1 } else { a1 - a0 }.rem_euclid(tau);
    if same {
        sweep = tau;
    }
    let n = steps(r, sweep);
    let sign = if clockwise { -1.0 } else { 1.0 };
    let mut out = vec![from];
    out.extend((1..n).map(|k| {
        let a = a0 + sign * sweep * k as f64 / n as f64;
        [center[0] + r * a.cos(), center[1] + r * a.sin()]
    }));
    out
}

/// Every point within `r` of the segment `a`–`b`.
fn capsule(a: Point, b: Point, r: f64) -> Vec<Point> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0].hypot(d[1]);
    if len < 1e-9 {
        return circle(a, r);
    }
    let heading = d[1].atan2(d[0]);
    let half = std::f64::consts::PI;
    let n = steps(r, half).max(2);
    let cap = |c: Point, start: f64| {
        (0..=n).map(move |k| {
            let t = start + half * k as f64 / n as f64;
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        })
    };
    let half_turn = std::f64::consts::FRAC_PI_2;
    cap(b, heading - half_turn)
        .chain(cap(a, heading + half_turn))
        .collect()
}

fn rect(c: Point, w: f64, h: f64) -> Vec<Point> {
    let (x, y) = (w / 2.0, h / 2.0);
    vec![
        [c[0] - x, c[1] - y],
        [c[0] + x, c[1] - y],
        [c[0] + x, c[1] + y],
        [c[0] - x, c[1] + y],
    ]
}

fn regular(c: Point, r: f64, n: usize, rotation: f64) -> Vec<Point> {
    let n = n.max(3);
    (0..n)
        .map(|k| {
            let a = (rotation + 360.0 * k as f64 / n as f64).to_radians();
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}

fn rotate(p: Point, deg: f64) -> Point {
    let (s, c) = deg.to_radians().sin_cos();
    [c * p[0] - s * p[1], s * p[0] + c * p[1]]
}

fn shifted(p: Point, by: Point) -> Point {
    [p[0] + by[0], p[1] + by[1]]
}

/// Rings of one simple polygon each, as shapes (the even-odd interior).
fn simple(polygons: &[Vec<Point>]) -> Fallible<Shapes> {
    let rings = polygons
        .iter()
        .map(|p| ring(p))
        .collect::<Fallible<Vec<_>>>()?;
    Ok(normalise(&rings, FillRule::EvenOdd))
}

/// Each polygon as its own even-odd shape set, together: overlapping
/// polygons add under the non-zero rule.
fn separately(polygons: &[Vec<Point>]) -> Fallible<Shapes> {
    let mut parts = Vec::with_capacity(polygons.len());
    for points in polygons {
        parts.extend(simple(std::slice::from_ref(points))?);
    }
    Ok(parts)
}

/// A macro primitive at `at`, and whether it adds.
fn primitive(prim: &Prim, at: Point) -> (bool, Vec<Point>) {
    let place = |p: Point, rot: f64| shifted(rotate(p, rot), at);
    match prim {
        Prim::Circle {
            on,
            diameter,
            center,
            rotation,
        } => (*on, circle(place(*center, *rotation), diameter / 2.0)),
        Prim::VectorLine {
            on,
            width,
            start,
            end,
            rotation,
        } => {
            let d = [end[0] - start[0], end[1] - start[1]];
            let len = d[0].hypot(d[1]).max(1e-12);
            let n = [-d[1] / len * width / 2.0, d[0] / len * width / 2.0];
            let corners = [
                [start[0] - n[0], start[1] - n[1]],
                [end[0] - n[0], end[1] - n[1]],
                [end[0] + n[0], end[1] + n[1]],
                [start[0] + n[0], start[1] + n[1]],
            ];
            (*on, corners.iter().map(|p| place(*p, *rotation)).collect())
        }
        Prim::CenterLine {
            on,
            width,
            height,
            center,
            rotation,
        } => (
            *on,
            rect(*center, *width, *height)
                .into_iter()
                .map(|p| place(p, *rotation))
                .collect(),
        ),
        Prim::Outline {
            on,
            points,
            rotation,
        } => (*on, points.iter().map(|p| place(*p, *rotation)).collect()),
        Prim::Polygon {
            on,
            vertices,
            center,
            diameter,
            rotation,
        } => (
            *on,
            regular(
                place(*center, *rotation),
                diameter / 2.0,
                *vertices,
                *rotation,
            ),
        ),
    }
}

fn with_hole(shape: Vec<Point>, at: Point, hole: Option<f64>) -> Fallible<Shapes> {
    let body = simple(&[shape])?;
    match hole {
        Some(d) if d > 0.0 => Ok(boolean(
            &body,
            &simple(&[circle(at, d / 2.0)])?,
            OverlayRule::Difference,
        )),
        _ => Ok(body),
    }
}

/// The image of an aperture flashed at `at`.
pub fn flash(template: &Template, at: Point) -> Fallible<Shapes> {
    match template {
        Template::Circle { diameter, hole } => with_hole(circle(at, diameter / 2.0), at, *hole),
        Template::Rect { w, h, hole } => with_hole(rect(at, *w, *h), at, *hole),
        Template::Obround { w, h, hole } => {
            let shape = if w >= h {
                let d = (w - h) / 2.0;
                capsule([at[0] - d, at[1]], [at[0] + d, at[1]], h / 2.0)
            } else {
                let d = (h - w) / 2.0;
                capsule([at[0], at[1] - d], [at[0], at[1] + d], w / 2.0)
            };
            with_hole(shape, at, *hole)
        }
        Template::Polygon {
            diameter,
            vertices,
            rotation,
            hole,
        } => with_hole(regular(at, diameter / 2.0, *vertices, *rotation), at, *hole),
        Template::Macro { primitives, .. } => {
            // Runs of exposure-on primitives are united at once; an
            // exposure-off primitive erases what came before it.
            let mut out: Shapes = Vec::new();
            let mut run: Vec<Vec<Point>> = Vec::new();
            for prim in primitives {
                let (on, points) = primitive(prim, at);
                if on {
                    run.push(points);
                } else {
                    out = boolean(&out, &separately(&run)?, OverlayRule::Union);
                    run.clear();
                    out = boolean(&out, &simple(&[points])?, OverlayRule::Difference);
                }
            }
            Ok(boolean(&out, &separately(&run)?, OverlayRule::Union))
        }
    }
}

/// A region contour as one ring, arcs flattened.
fn contour_points(contour: &Contour) -> Vec<Point> {
    let mut points = vec![contour.start];
    let mut at = contour.start;
    for segment in &contour.segments {
        match *segment {
            Path::Line { to } => points.push(to),
            Path::Arc {
                to,
                center,
                clockwise,
            } => {
                points.extend(arc(at, to, center, clockwise).into_iter().skip(1));
                points.push(to);
            }
        }
        at = match segment {
            Path::Line { to } | Path::Arc { to, .. } => *to,
        };
    }
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    points
}

/// The image of one graphics object.
pub fn object_shapes(file: &GerberFile, object: &Object) -> Fallible<Shapes> {
    let template = || -> Fallible<&Template> {
        let id = object
            .aperture
            .ok_or("polygons: an object without an aperture")?;
        Ok(&file
            .apertures
            .get(&id)
            .ok_or_else(|| format!("polygons: aperture D{id} is not defined"))?
            .template)
    };
    let radius = |t: &Template| -> Fallible<f64> {
        match t {
            Template::Circle { diameter, .. } => Ok(diameter / 2.0),
            _ => Err("polygons: only circular apertures may draw".into()),
        }
    };
    match &object.kind {
        Kind::Flash { at } => flash(template()?, *at),
        Kind::Draw { from, to } => simple(&[capsule(*from, *to, radius(template()?)?)]),
        Kind::Arc {
            from,
            to,
            center,
            clockwise,
        } => {
            let r = radius(template()?)?;
            let mut points = arc(*from, *to, *center, *clockwise);
            points.push(*to);
            let capsules = points
                .windows(2)
                .map(|w| capsule(w[0], w[1], r))
                .collect::<Vec<_>>();
            Ok(boolean(&separately(&capsules)?, &[], OverlayRule::Union))
        }
        Kind::Region { contour } => simple(&[contour_points(contour)]),
    }
}

/// The image of the objects of `file` that `keep` accepts, in order.
pub fn render(file: &GerberFile, keep: impl Fn(&Object) -> bool) -> Fallible<Shapes> {
    let mut image: Shapes = Vec::new();
    let mut run: Shapes = Vec::new();
    let mut dark = true;
    let flush = |image: Shapes, run: &mut Shapes, dark: bool| -> Shapes {
        if run.is_empty() {
            return image;
        }
        let rule = if dark {
            OverlayRule::Union
        } else {
            OverlayRule::Difference
        };
        let out = boolean(&image, run, rule);
        run.clear();
        out
    };
    for object in file.objects.iter().filter(|o| keep(o)) {
        if object.dark != dark {
            image = flush(image, &mut run, dark);
            dark = object.dark;
        }
        let shapes = object_shapes(file, object)?;
        if dark {
            run.extend(shapes);
        } else {
            // Clear objects are united first, then removed at once.
            run = boolean(&run, &shapes, OverlayRule::Union);
        }
    }
    Ok(flush(image, &mut run, dark))
}

/// Every vertex moved by `by` mm.
pub fn translated(shapes: &[Shape], by: Point) -> Fallible<Shapes> {
    let d = nm(by)?;
    Ok(shapes
        .iter()
        .map(|s| {
            s.iter()
                .map(|r| {
                    r.iter()
                        .map(|p| IntPoint::new(p.x.saturating_add(d.x), p.y.saturating_add(d.y)))
                        .collect()
                })
                .collect()
        })
        .collect())
}

#[cfg(test)]
mod tests;
