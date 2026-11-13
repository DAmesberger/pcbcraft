//! Outlines of laid-out marking elements in the area's 2D frame (mm,
//! origin at the area centre, +x along the area's u axis): text from
//! Solidcraft's glyph outlines (`solidcraft_sketch::text`, the built-in
//! Liberation Sans, SIL OFL 1.1), polarity marks and colour bands as plain
//! shapes. SVG artwork is read by [`super::paint`].
use crate::geom::Vec2;
use crate::{Error, Result};
use solidcraft_engine::sketch::{LinkGeom, TextAlign, TextStyle, text_geometry_styled};
use sparksmith_core::Point;
use sparksmith_core::library::marking::{LaidText, PolarityMark, TEXT_ADVANCE};

/// Chord tolerance of flattened glyph and artwork curves, mm.
pub const CHORD_MM: f64 = 0.002;
/// Characters one laid-out text line may have.
pub const MAX_TEXT_CHARS: usize = 64;
/// Accepted text heights, mm.
pub const TEXT_HEIGHT_MM: std::ops::RangeInclusive<f64> = 0.05..=50.0;
/// Largest element size, mm.
pub const MAX_ELEMENT_MM: f64 = 200.0;
/// Segments of a polarity dot.
const DOT_SEGMENTS: usize = 32;
/// Segments one flattened curve gives at most.
const MAX_CURVE_SEGMENTS: usize = 64;
/// Points closer than this continue one loop, mm.
const JOIN_MM: f64 = 1e-7;

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(format!("marking decal: {}", message.into()))
}

fn finite(values: &[f64]) -> bool {
    values.iter().all(|v| v.is_finite())
}

/// The glyph loops of one text line, wound as the font draws them (the
/// counters against their outlines). `origin` is the bottom-left corner
/// of the line's box as the core lays it out (`chars × TEXT_ADVANCE ×
/// height_mm` wide, `height_mm` tall): the line is centred in the box, its
/// capitals spanning its height, and shrunk about the box's centre when the
/// font sets it wider or taller than `fit` (the area) allows. The core's
/// box assumes [`TEXT_ADVANCE`]; the font's capitals run wider, so a long
/// line may reach past its box (never past the area).
pub fn text(line: &LaidText, fit: [f64; 2]) -> Result<Vec<Vec<Point>>> {
    let h = line.height_mm;
    if !TEXT_HEIGHT_MM.contains(&h) {
        return Err(invalid(format!(
            "text height {h} mm is outside {}–{} mm",
            TEXT_HEIGHT_MM.start(),
            TEXT_HEIGHT_MM.end()
        )));
    }
    if line.text.chars().count() > MAX_TEXT_CHARS {
        return Err(invalid(format!(
            "a text line has more than {MAX_TEXT_CHARS} characters"
        )));
    }
    let [x0, y0] = line.origin;
    if !(finite(&line.origin) && x0.abs() <= MAX_ELEMENT_MM && y0.abs() <= MAX_ELEMENT_MM) {
        return Err(invalid("a text origin is out of range"));
    }
    let box_w = line.text.chars().count() as f64 * TEXT_ADVANCE * h;
    let (ox, oy) = (x0 + box_w / 2.0, y0 + h / 2.0);
    if line.text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let style = TextStyle {
        align: TextAlign::Center,
        ..TextStyle::default()
    };
    let geometry = text_geometry_styled(
        None,
        &line.text,
        Vec2::new(ox, oy - h / 2.0),
        h,
        0.0,
        &style,
        None,
    )
    .map_err(|e| invalid(format!("text: {e}")))?;
    let mut loops = chain(&geometry);
    // Shrink to the area when the font sets the line wider (or taller)
    // than the layout allowed for.
    let bounds = loops.iter().flatten().fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |b, p| {
            [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ]
        },
    );
    let (w, t) = (bounds[2] - bounds[0], bounds[3] - bounds[1]);
    if w > 0.0 && t > 0.0 && fit[0] > 0.0 && fit[1] > 0.0 {
        let k = (fit[0] / w).min(fit[1] / t).min(1.0);
        if k < 1.0 {
            for p in loops.iter_mut().flatten() {
                *p = [ox + (p[0] - ox) * k, oy + (p[1] - oy) * k];
            }
        }
    }
    Ok(loops)
}

/// Glyph curves → closed loops: a curve that starts where the last one
/// ended continues its loop.
pub(super) fn chain(geometry: &[LinkGeom]) -> Vec<Vec<Point>> {
    let mut loops: Vec<Vec<Point>> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let close = |current: &mut Vec<Point>, loops: &mut Vec<Vec<Point>>| {
        if let (Some(first), Some(last)) = (current.first(), current.last())
            && dist(*first, *last) <= JOIN_MM
        {
            current.pop();
        }
        if current.len() >= 3 {
            loops.push(std::mem::take(current));
        } else {
            current.clear();
        }
    };
    for g in geometry {
        let points = flatten(g);
        let Some(first) = points.first() else {
            continue;
        };
        let whole = matches!(g, LinkGeom::Circle(..) | LinkGeom::Ellipse { .. });
        let continues = current
            .last()
            .is_some_and(|last| dist(*last, *first) <= JOIN_MM);
        if whole || !continues {
            close(&mut current, &mut loops);
        }
        if whole {
            let mut ring = points;
            close(&mut ring, &mut loops);
            continue;
        }
        if current.is_empty() {
            current.extend(points);
        } else {
            current.extend(points.into_iter().skip(1));
        }
    }
    close(&mut current, &mut loops);
    loops
}

fn dist(a: Point, b: Point) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn pt(v: Vec2) -> Point {
    [v.x, v.y]
}

fn segments(deviation: f64) -> usize {
    if !(deviation.is_finite() && deviation > 0.0) {
        return 1;
    }
    let n = (deviation / CHORD_MM).sqrt().ceil();
    if n >= MAX_CURVE_SEGMENTS as f64 {
        MAX_CURVE_SEGMENTS
    } else {
        (n as usize).max(1)
    }
}

/// One curve as points from its start to its end, within [`CHORD_MM`].
fn flatten(g: &LinkGeom) -> Vec<Point> {
    let bezier = |p: &[Point], t: f64| -> Point {
        // De Casteljau.
        let mut q: Vec<Point> = p.to_vec();
        while q.len() > 1 {
            q = q
                .windows(2)
                .map(|w| {
                    [
                        w[0][0] + (w[1][0] - w[0][0]) * t,
                        w[0][1] + (w[1][1] - w[0][1]) * t,
                    ]
                })
                .collect();
        }
        q.first().copied().unwrap_or([0.0, 0.0])
    };
    let sample = |p: &[Point], n: usize| -> Vec<Point> {
        (0..=n).map(|k| bezier(p, k as f64 / n as f64)).collect()
    };
    let second =
        |a: Point, b: Point, c: Point| (a[0] - 2.0 * b[0] + c[0]).hypot(a[1] - 2.0 * b[1] + c[1]);
    match g {
        LinkGeom::Point(_) => Vec::new(),
        LinkGeom::Line(a, b) => vec![pt(*a), pt(*b)],
        LinkGeom::Conic { a, apex, b, rho } if (*rho - 0.5).abs() < 1e-9 => {
            let p = [pt(*a), pt(*apex), pt(*b)];
            // A quadratic's chord error is |a − 2c + b| / (4 n²).
            sample(&p, segments(second(p[0], p[1], p[2]) / 4.0))
        }
        LinkGeom::Conic { a, apex, b, rho } => {
            // A rational quadratic with weight rho / (1 − rho) at the apex.
            let w = rho / (1.0 - rho);
            let (a, c, b) = (pt(*a), pt(*apex), pt(*b));
            let n = segments(second(a, c, b) / 4.0 * w.max(1.0));
            (0..=n)
                .map(|k| {
                    let t = k as f64 / n as f64;
                    let (b0, b1, b2) = ((1.0 - t) * (1.0 - t), 2.0 * t * (1.0 - t) * w, t * t);
                    let d = b0 + b1 + b2;
                    [
                        (b0 * a[0] + b1 * c[0] + b2 * b[0]) / d,
                        (b0 * a[1] + b1 * c[1] + b2 * b[1]) / d,
                    ]
                })
                .collect()
        }
        LinkGeom::Spline {
            pts,
            control: true,
            degree: 3,
            periodic: false,
            nurbs: None,
        } if pts.len() == 4 => {
            let p: Vec<Point> = pts.iter().map(|v| pt(*v)).collect();
            let m = second(p[0], p[1], p[2]).max(second(p[1], p[2], p[3]));
            // A cubic's chord error is at most 3 m / (4 n²).
            sample(&p, segments(0.75 * m))
        }
        LinkGeom::Spline { pts, .. } => pts.iter().map(|v| pt(*v)).collect(),
        LinkGeom::Circle(c, r) => circle(pt(*c), *r, DOT_SEGMENTS),
        LinkGeom::Arc { c, a, b } => {
            let (c, a, b) = (pt(*c), pt(*a), pt(*b));
            let r = dist(c, a);
            let a0 = (a[1] - c[1]).atan2(a[0] - c[0]);
            let mut a1 = (b[1] - c[1]).atan2(b[0] - c[0]);
            while a1 <= a0 {
                a1 += std::f64::consts::TAU;
            }
            let n = segments(r * (1.0 - ((a1 - a0) / 2.0).cos()));
            (0..=n)
                .map(|k| {
                    let t = a0 + (a1 - a0) * k as f64 / n as f64;
                    [c[0] + r * t.cos(), c[1] + r * t.sin()]
                })
                .collect()
        }
        LinkGeom::Ellipse { c, major, minor } => {
            let (c, m) = (pt(*c), pt(*major));
            let (ux, uy) = (m[0] - c[0], m[1] - c[1]);
            let r = ux.hypot(uy);
            if !(r.is_finite() && r > 0.0) {
                return Vec::new();
            }
            let (vx, vy) = (-uy / r * minor, ux / r * minor);
            (0..DOT_SEGMENTS)
                .map(|k| {
                    let t = std::f64::consts::TAU * k as f64 / DOT_SEGMENTS as f64;
                    let (s, co) = t.sin_cos();
                    [c[0] + ux * co + vx * s, c[1] + uy * co + vy * s]
                })
                .collect()
        }
    }
}

/// A counter-clockwise circle.
pub fn circle(c: Point, r: f64, n: usize) -> Vec<Point> {
    (0..n)
        .map(|k| {
            let t = std::f64::consts::TAU * k as f64 / n as f64;
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        })
        .collect()
}

/// A counter-clockwise rectangle centred on `c`.
pub fn rect(c: Point, size: [f64; 2]) -> Vec<Point> {
    let (hx, hy) = (size[0] / 2.0, size[1] / 2.0);
    vec![
        [c[0] - hx, c[1] - hy],
        [c[0] + hx, c[1] - hy],
        [c[0] + hx, c[1] + hy],
        [c[0] - hx, c[1] + hy],
    ]
}

fn checked_box(origin: [f64; 2], size: [f64; 2]) -> Result<()> {
    let ok = finite(&origin)
        && finite(&size)
        && origin.iter().all(|v| v.abs() <= MAX_ELEMENT_MM)
        && size.iter().all(|v| *v > 0.0 && *v <= MAX_ELEMENT_MM);
    if ok {
        Ok(())
    } else {
        Err(invalid("an element's place or size is out of range"))
    }
}

/// A polarity mark: a dot as wide as the smaller side, or a bar or band
/// filling its box.
pub fn polarity(mark: PolarityMark, origin: [f64; 2], size: [f64; 2]) -> Result<Vec<Point>> {
    checked_box(origin, size)?;
    Ok(match mark {
        PolarityMark::Dot => circle(origin, size[0].min(size[1]) / 2.0, DOT_SEGMENTS),
        PolarityMark::Bar | PolarityMark::Band | PolarityMark::Stripe => rect(origin, size),
    })
}

/// Colour bands across an area `area` wide and high: band `i` is centred
/// `pitch · (i + 1)` from the −x end and half a pitch wide, kept inside
/// the area. One rectangle per band, in order; `None` for a band that
/// falls outside.
pub fn bands(count: usize, pitch: f64, area: [f64; 2]) -> Result<Vec<Option<Vec<Point>>>> {
    if !(pitch.is_finite() && pitch > 0.0 && pitch <= MAX_ELEMENT_MM) {
        return Err(invalid("the band pitch is out of range"));
    }
    if count > 16 {
        return Err(invalid("more than 16 colour bands"));
    }
    let half = area[0] / 2.0;
    Ok((0..count)
        .map(|i| {
            let centre = -half + pitch * (i as f64 + 1.0);
            let (x0, x1) = (
                (centre - pitch / 4.0).max(-half),
                (centre + pitch / 4.0).min(half),
            );
            (x1 - x0 > 1e-6).then(|| rect([(x0 + x1) / 2.0, 0.0], [x1 - x0, area[1]]))
        })
        .collect())
}
