//! SVG artwork → painted decal regions: what a marking draws of a user's
//! SVG, in paint order, each region with its colour and fill rule.
//!
//! Unlike [`crate::svg::convert`] (which makes editable library graphics
//! and must outline filled shapes with holes), a decal is filled
//! triangles, so this reader keeps every element's subpaths together and
//! fills them by the element's `fill-rule`: counters, holes and even-odd
//! stars come out as drawn. It is an independent reading of the SVG 1.1 /
//! SVG 2 specifications (paths, basic shapes, transforms, the painting
//! properties), written for decals; nothing is taken from another SVG
//! renderer.
//!
//! What is drawn:
//! - `path` (every command, relative and absolute, arcs by the
//!   specification's endpoint-to-centre conversion), `rect` (with corner
//!   radii), `circle`, `ellipse`, `line`, `polyline`, `polygon`, in the
//!   directions the specification gives them, so the nonzero rule sees
//!   the winding the author drew;
//! - `transform` on any element and group (`matrix`, `translate`,
//!   `scale`, `rotate` with or without a centre, `skewX`, `skewY`), nested;
//! - `fill`, `stroke`, `stroke-width`, `fill-rule`, `color`, `opacity`,
//!   `fill-opacity` and `stroke-opacity` from attributes and `style`,
//!   inherited through groups. Colours are `#rgb`, `#rrggbb`, `rgb()`
//!   (numbers or percentages) and the basic named colours; `currentColor`
//!   without a `color` is the finish's own colour (`None`), so monochrome
//!   artwork can follow the marking's ink. A paint at less than half
//!   opacity is not drawn (a decal has no transparency);
//! - strokes as the union of round-capped segments (round joins), the
//!   width scaled by the transform's area scale;
//! - gradients (`url(#id)` naming a `linearGradient` or `radialGradient`,
//!   `href` followed): the mean of their stop colours, a cheap stand-in
//!   for the blend;
//! - `text` (its own characters and those of its `tspan`s, at its first
//!   `x`/`y`, `font-size` and `text-anchor`) as outlines in Solidcraft's
//!   built-in Liberation Sans (SIL OFL 1.1), whatever `font-family` asks;
//! - `image` with a `data:image/png;base64,` URI: decoded (the `png`
//!   crate, MIT OR Apache-2.0), averaged down to at most
//!   [`IMAGE_CELLS`] cells a side, quantised to [`IMAGE_LEVELS`] levels a
//!   channel and drawn as one region per colour of row runs; cells less
//!   than half opaque are left out. JPEG and other images are skipped.
//!
//! Skipped, and said so in the notes: `use`, other image types, clip
//! paths, masks, filters and patterns. The root's `viewBox`, `width` and
//! `height` set only the aspect ratio of user units (`preserveAspectRatio
//! ="none"` stretches); the artwork is fitted to the decal by its painted
//! bounds, as library artwork is.
//!
//! Every input is bounded ([`MAX_SVG_BYTES`], [`MAX_ELEMENTS`],
//! [`MAX_POINTS`], [`MAX_LOOPS`], [`MAX_TEXT_CHARS`], [`MAX_IMAGES`],
//! [`MAX_IMAGE_PIXELS`]); past a bound the artwork is refused whole.
use super::shapes;
use crate::{Error, Result};
use pcbcraft_core::Point;
use quick_xml::events::{BytesStart, Event};
use std::collections::BTreeMap;

pub use super::fill::FillRule;

/// Largest SVG read, bytes (the library artwork limit).
pub const MAX_SVG_BYTES: usize = crate::svg::MAX_SVG_BYTES;
/// Drawn elements one artwork may have.
pub const MAX_ELEMENTS: usize = crate::svg::MAX_SVG_ELEMENTS;
/// Outline points in all.
pub const MAX_POINTS: usize = 262_144;
/// Outline loops in all.
pub const MAX_LOOPS: usize = 16_384;
/// Characters of text in all.
pub const MAX_TEXT_CHARS: usize = 512;
/// Embedded images one artwork may have.
pub const MAX_IMAGES: usize = 4;
/// Pixels of one decoded image.
pub const MAX_IMAGE_PIXELS: u64 = 4096 * 4096;
/// Cells an image is averaged down to, at most, along each side.
pub const IMAGE_CELLS: u32 = 96;
/// Levels per colour channel an image is quantised to.
pub const IMAGE_LEVELS: u32 = 8;
/// Cap height of the built-in font as a share of its em (Liberation
/// Sans: 1409 / 2048 units).
pub const FONT_CAP_EM: f64 = 1409.0 / 2048.0;
/// Chord tolerance of flattened curves on the decal, mm.
pub const CHORD_MM: f64 = shapes::CHORD_MM;
const MAX_DEPTH: usize = 256;
const MAX_CURVE_STEPS: usize = 256;
const CAP_STEPS: usize = 6;
const MAX_HREF_HOPS: usize = 8;

/// One painted region of the artwork, filled on its own: later regions
/// paint over earlier ones.
#[derive(Clone, Debug, PartialEq)]
pub struct Painted {
    /// sRGB; `None` paints in the finish's own colour.
    pub colour: Option<[u8; 3]>,
    /// Closed loops (implicitly closed), each in the winding the artwork
    /// gave it.
    pub loops: Vec<Vec<Point>>,
    pub rule: FillRule,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(format!("marking artwork: {}", message.into()))
}

/// SVG artwork scaled to fit `size` (aspect kept), centred on `origin`
/// (decal mm, +y up), in paint order.
pub fn artwork(text: &str, origin: [f64; 2], size: [f64; 2]) -> Result<Vec<Painted>> {
    artwork_with_notes(text, origin, size).map(|(painted, _)| painted)
}

/// [`artwork`] and one note per kind of thing skipped.
pub fn artwork_with_notes(
    text: &str,
    origin: [f64; 2],
    size: [f64; 2],
) -> Result<(Vec<Painted>, Vec<String>)> {
    if text.len() > MAX_SVG_BYTES {
        return Err(invalid(format!(
            "the SVG is {} bytes; at most {} KiB is read",
            text.len(),
            MAX_SVG_BYTES >> 10
        )));
    }
    let ok = |v: &[f64]| v.iter().all(|x| x.is_finite());
    if !(ok(&origin) && ok(&size) && size.iter().all(|s| *s > 0.0)) {
        return Err(invalid("the artwork's place and size must be finite"));
    }
    let gradients = gradients(text)?;
    // A first pass finds the painted bounds, a second flattens curves
    // within the chord tolerance at the fitted scale.
    let first = Walk::run(text, &gradients, None)?;
    let Some(bounds) = bounds(&first.out) else {
        return Ok((Vec::new(), first.notes()));
    };
    let (w, h) = (bounds[2] - bounds[0], bounds[3] - bounds[1]);
    let k = [size[0] / w, size[1] / h]
        .into_iter()
        .filter(|k| k.is_finite() && *k > 0.0)
        .fold(f64::INFINITY, f64::min);
    if !k.is_finite() {
        return Ok((Vec::new(), first.notes()));
    }
    let second = Walk::run(text, &gradients, Some(CHORD_MM / k))?;
    let Some(bounds) = bounds_of(&second.out) else {
        return Ok((Vec::new(), second.notes()));
    };
    // Fit again to the flattened curves (the first pass saw only chords).
    let (w, h) = (bounds[2] - bounds[0], bounds[3] - bounds[1]);
    let k = [size[0] / w, size[1] / h]
        .into_iter()
        .filter(|k| k.is_finite() && *k > 0.0)
        .fold(f64::INFINITY, f64::min);
    if !k.is_finite() {
        return Ok((Vec::new(), second.notes()));
    }
    let (cx, cy) = ((bounds[0] + bounds[2]) / 2.0, (bounds[1] + bounds[3]) / 2.0);
    let notes = second.notes();
    let mut out = second.out;
    // Document units are y-down: flip into the decal's y-up frame.
    for p in out.iter_mut().flat_map(|g| g.loops.iter_mut()).flatten() {
        *p = [origin[0] + k * (p[0] - cx), origin[1] - k * (p[1] - cy)];
    }
    Ok((out, notes))
}

fn bounds(groups: &[Painted]) -> Option<[f64; 4]> {
    bounds_of(groups)
}

fn bounds_of(groups: &[Painted]) -> Option<[f64; 4]> {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in groups.iter().flat_map(|g| g.loops.iter()).flatten() {
        b = [
            b[0].min(p[0]),
            b[1].min(p[1]),
            b[2].max(p[0]),
            b[3].max(p[1]),
        ];
    }
    (b[0] <= b[2] && b[1] <= b[3]).then_some(b)
}

// ---------------------------------------------------------------- affine

/// `x' = a x + c y + e`, `y' = b x + d y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine([f64; 6]);

impl Affine {
    const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    fn apply(&self, p: Point) -> Point {
        let [a, b, c, d, e, f] = self.0;
        [a * p[0] + c * p[1] + e, b * p[0] + d * p[1] + f]
    }

    /// `self` after `inner`: `self(inner(p))`.
    fn then(&self, inner: &Self) -> Self {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = inner.0;
        Self([
            a * a2 + c * b2,
            b * a2 + d * b2,
            a * c2 + c * d2,
            b * c2 + d * d2,
            a * e2 + c * f2 + e,
            b * e2 + d * f2 + f,
        ])
    }

    /// The factor a length scales by (the square root of the area scale).
    fn length_scale(&self) -> f64 {
        let [a, b, c, d, ..] = self.0;
        (a * d - b * c).abs().sqrt()
    }
}

/// An SVG `transform` list; `None` when malformed.
fn transform_list(s: &str) -> Option<Affine> {
    let mut m = Affine::IDENTITY;
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let close = rest.find(')')?;
        if close < open {
            return None;
        }
        let name = rest.get(..open)?.trim().trim_start_matches(',').trim();
        let args = numbers(rest.get(open + 1..close)?)?;
        let t = match (name, args.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => Affine([*a, *b, *c, *d, *e, *f]),
            ("translate", [x]) => Affine([1.0, 0.0, 0.0, 1.0, *x, 0.0]),
            ("translate", [x, y]) => Affine([1.0, 0.0, 0.0, 1.0, *x, *y]),
            ("scale", [s]) => Affine([*s, 0.0, 0.0, *s, 0.0, 0.0]),
            ("scale", [x, y]) => Affine([*x, 0.0, 0.0, *y, 0.0, 0.0]),
            ("rotate", [a]) => rotation(*a),
            ("rotate", [a, x, y]) => Affine([1.0, 0.0, 0.0, 1.0, *x, *y])
                .then(&rotation(*a))
                .then(&Affine([1.0, 0.0, 0.0, 1.0, -x, -y])),
            ("skewX", [a]) => Affine([1.0, 0.0, a.to_radians().tan(), 1.0, 0.0, 0.0]),
            ("skewY", [a]) => Affine([1.0, a.to_radians().tan(), 0.0, 1.0, 0.0, 0.0]),
            _ => return None,
        };
        m = m.then(&t);
        rest = rest.get(close + 1..)?.trim().trim_start_matches(',').trim();
    }
    m.0.iter().all(|v| v.is_finite()).then_some(m)
}

fn rotation(deg: f64) -> Affine {
    let (s, c) = deg.to_radians().sin_cos();
    Affine([c, s, -s, c, 0.0, 0.0])
}

/// Numbers separated by spaces and commas.
fn numbers(s: &str) -> Option<Vec<f64>> {
    let mut scan = Scan::new(s);
    let mut out = Vec::new();
    while scan.more() {
        out.push(scan.number()?);
    }
    Some(out)
}

// ---------------------------------------------------------------- scanner

/// A number scanner over SVG list and path data.
struct Scan<'a> {
    s: &'a [u8],
    at: usize,
}

impl<'a> Scan<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            s: s.as_bytes(),
            at: 0,
        }
    }
    fn skip(&mut self) {
        while self
            .s
            .get(self.at)
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b',')
        {
            self.at += 1;
        }
    }
    fn more(&mut self) -> bool {
        self.skip();
        self.at < self.s.len()
    }
    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.s.get(self.at).copied()
    }
    fn number(&mut self) -> Option<f64> {
        self.skip();
        let start = self.at;
        let digits = |scan: &mut Self| {
            let from = scan.at;
            while scan.s.get(scan.at).is_some_and(u8::is_ascii_digit) {
                scan.at += 1;
            }
            scan.at > from
        };
        if matches!(self.s.get(self.at), Some(b'-' | b'+')) {
            self.at += 1;
        }
        let whole = digits(self);
        let mut frac = false;
        if self.s.get(self.at) == Some(&b'.') {
            self.at += 1;
            frac = digits(self);
        }
        if !(whole || frac) {
            self.at = start;
            return None;
        }
        if matches!(self.s.get(self.at), Some(b'e' | b'E')) {
            let mark = self.at;
            self.at += 1;
            if matches!(self.s.get(self.at), Some(b'-' | b'+')) {
                self.at += 1;
            }
            if !digits(self) {
                self.at = mark;
            }
        }
        std::str::from_utf8(self.s.get(start..self.at)?)
            .ok()?
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
    }
    /// An arc flag: one `0` or `1`, separators optional.
    fn flag(&mut self) -> Option<bool> {
        let b = self.peek()?;
        self.at += 1;
        match b {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------- outlines

/// One subpath: points in path order and whether it was closed.
#[derive(Clone, Debug, Default)]
struct Sub {
    points: Vec<Point>,
    closed: bool,
}

/// Steps of a uniformly sampled curve whose chord error is `k / n²`.
fn steps(k: f64, tol: f64) -> usize {
    let n = (k / tol).sqrt().ceil();
    if n.is_finite() && n >= 1.0 {
        (n as usize).min(MAX_CURVE_STEPS)
    } else {
        1
    }
}

fn arc_steps(radius: f64, sweep: f64, tol: f64) -> usize {
    if radius.partial_cmp(&tol) != Some(std::cmp::Ordering::Greater) {
        return 4;
    }
    let step = 2.0 * (1.0 - tol / radius).acos();
    let n = (sweep.abs() / step).ceil();
    if n.is_finite() && n >= 1.0 {
        (n as usize).clamp(4, MAX_CURVE_STEPS)
    } else {
        4
    }
}

fn mid(a: Point, b: Point, t: f64) -> Point {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn len(a: Point) -> f64 {
    a[0].hypot(a[1])
}

fn sub2(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

/// Path data → subpaths, curves flattened within `tol` (user units).
fn path(d: &str, tol: f64) -> Option<Vec<Sub>> {
    let mut scan = Scan::new(d);
    let mut out: Vec<Sub> = Vec::new();
    let mut cur = Sub::default();
    let mut pos: Point = [0.0, 0.0];
    let mut start: Point = [0.0, 0.0];
    // The last control point of a cubic or quadratic, for S and T.
    let mut last_cubic: Option<Point> = None;
    let mut last_quad: Option<Point> = None;
    let mut command = None::<u8>;
    let flush = |cur: &mut Sub, out: &mut Vec<Sub>| {
        if cur.points.len() >= 2 {
            out.push(std::mem::take(cur));
        } else {
            cur.points.clear();
            cur.closed = false;
        }
    };
    while scan.more() {
        let c = match scan.peek() {
            Some(b) if b.is_ascii_alphabetic() => {
                scan.at += 1;
                b
            }
            Some(_) => match command {
                // Coordinates after a moveto are implicit linetos.
                Some(b'M') => b'L',
                Some(b'm') => b'l',
                Some(c) if !matches!(c, b'Z' | b'z') => c,
                _ => return None,
            },
            None => break,
        };
        command = Some(c);
        let rel = c.is_ascii_lowercase();
        let at = |p: Point, pos: Point| {
            if rel {
                [pos[0] + p[0], pos[1] + p[1]]
            } else {
                p
            }
        };
        let pair =
            |scan: &mut Scan<'_>| -> Option<Point> { Some([scan.number()?, scan.number()?]) };
        let mut cubic_ctrl = None;
        let mut quad_ctrl = None;
        match c.to_ascii_uppercase() {
            b'M' => {
                flush(&mut cur, &mut out);
                pos = at(pair(&mut scan)?, pos);
                start = pos;
                cur.points.push(pos);
            }
            b'L' => {
                pos = at(pair(&mut scan)?, pos);
                cur.points.push(pos);
            }
            b'H' => {
                let x = scan.number()?;
                pos = [if rel { pos[0] + x } else { x }, pos[1]];
                cur.points.push(pos);
            }
            b'V' => {
                let y = scan.number()?;
                pos = [pos[0], if rel { pos[1] + y } else { y }];
                cur.points.push(pos);
            }
            b'C' | b'S' => {
                let p1 = if c.eq_ignore_ascii_case(&b'C') {
                    at(pair(&mut scan)?, pos)
                } else {
                    last_cubic.map_or(pos, |q| [2.0 * pos[0] - q[0], 2.0 * pos[1] - q[1]])
                };
                let p2 = at(pair(&mut scan)?, pos);
                let p3 = at(pair(&mut scan)?, pos);
                let d1 = len([pos[0] - 2.0 * p1[0] + p2[0], pos[1] - 2.0 * p1[1] + p2[1]]);
                let d2 = len([p1[0] - 2.0 * p2[0] + p3[0], p1[1] - 2.0 * p2[1] + p3[1]]);
                let n = steps(0.75 * d1.max(d2), tol);
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let u = 1.0 - t;
                    let (a, b, cc, dd) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    cur.points.push([
                        a * pos[0] + b * p1[0] + cc * p2[0] + dd * p3[0],
                        a * pos[1] + b * p1[1] + cc * p2[1] + dd * p3[1],
                    ]);
                }
                cubic_ctrl = Some(p2);
                pos = p3;
            }
            b'Q' | b'T' => {
                let p1 = if c.eq_ignore_ascii_case(&b'Q') {
                    at(pair(&mut scan)?, pos)
                } else {
                    last_quad.map_or(pos, |q| [2.0 * pos[0] - q[0], 2.0 * pos[1] - q[1]])
                };
                let p2 = at(pair(&mut scan)?, pos);
                let d = len([pos[0] - 2.0 * p1[0] + p2[0], pos[1] - 2.0 * p1[1] + p2[1]]);
                let n = steps(0.25 * d, tol);
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let a = mid(pos, p1, t);
                    let b = mid(p1, p2, t);
                    cur.points.push(mid(a, b, t));
                }
                quad_ctrl = Some(p1);
                pos = p2;
            }
            b'A' => {
                let rx = scan.number()?;
                let ry = scan.number()?;
                let phi = scan.number()?;
                let large = scan.flag()?;
                let sweep = scan.flag()?;
                let to = at(pair(&mut scan)?, pos);
                arc(&mut cur.points, pos, [rx, ry], phi, large, sweep, to, tol);
                pos = to;
            }
            b'Z' => {
                cur.closed = true;
                pos = start;
                flush(&mut cur, &mut out);
                // A path may go on from the closed subpath's start.
                cur.points.push(start);
            }
            _ => return None,
        }
        last_cubic = cubic_ctrl;
        last_quad = quad_ctrl;
    }
    flush(&mut cur, &mut out);
    Some(out)
}

/// An elliptical arc from `from` to `to` (SVG 1.1 implementation notes
/// F.6.5, endpoint to centre), appended without its start point.
#[allow(clippy::too_many_arguments)]
fn arc(
    out: &mut Vec<Point>,
    from: Point,
    r: [f64; 2],
    phi_deg: f64,
    large: bool,
    sweep: bool,
    to: Point,
    tol: f64,
) {
    let (mut rx, mut ry) = (r[0].abs(), r[1].abs());
    if len(sub2(to, from)) <= 1e-12 {
        return;
    }
    if rx <= 1e-12 || ry <= 1e-12 {
        out.push(to);
        return;
    }
    let (s, c) = phi_deg.to_radians().sin_cos();
    let (dx, dy) = ((from[0] - to[0]) / 2.0, (from[1] - to[1]) / 2.0);
    let x1 = c * dx + s * dy;
    let y1 = -s * dx + c * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let k = lambda.sqrt();
        rx *= k;
        ry *= k;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = if den > 0.0 {
        (num / den).max(0.0).sqrt()
    } else {
        0.0
    };
    if large == sweep {
        coef = -coef;
    }
    let (cxp, cyp) = (coef * rx * y1 / ry, -coef * ry * x1 / rx);
    let cx = c * cxp - s * cyp + (from[0] + to[0]) / 2.0;
    let cy = s * cxp + c * cyp + (from[1] + to[1]) / 2.0;
    let angle = |u: Point, v: Point| {
        let a = (u[0] * v[1] - u[1] * v[0]).atan2(u[0] * v[0] + u[1] * v[1]);
        if a.is_finite() { a } else { 0.0 }
    };
    let u = [(x1 - cxp) / rx, (y1 - cyp) / ry];
    let v = [(-x1 - cxp) / rx, (-y1 - cyp) / ry];
    let theta = angle([1.0, 0.0], u);
    let mut delta = angle(u, v);
    let tau = std::f64::consts::TAU;
    if !sweep && delta > 0.0 {
        delta -= tau;
    } else if sweep && delta < 0.0 {
        delta += tau;
    }
    let n = arc_steps(rx.max(ry), delta, tol);
    for i in 1..n {
        let t = theta + delta * i as f64 / n as f64;
        let (st, ct) = t.sin_cos();
        out.push([
            cx + rx * ct * c - ry * st * s,
            cy + rx * ct * s + ry * st * c,
        ]);
    }
    out.push(to);
}

/// An ellipse from `(cx + rx, cy)` the way of increasing angle (SVG 2's
/// direction for `circle` and `ellipse`).
fn ellipse(c: Point, rx: f64, ry: f64, tol: f64) -> Sub {
    let n = arc_steps(rx.max(ry), std::f64::consts::TAU, tol).max(8);
    Sub {
        points: (0..n)
            .map(|i| {
                let (s, co) = (std::f64::consts::TAU * i as f64 / n as f64).sin_cos();
                [c[0] + rx * co, c[1] + ry * s]
            })
            .collect(),
        closed: true,
    }
}

/// A `rect` (SVG 1.1 §9.2: corner radii resolved and clamped), from
/// `(x + rx, y)` along +x.
fn rect(attrs: &[(String, String)], tol: f64) -> Option<Sub> {
    let get = |k: &str| attr(attrs, k).and_then(length);
    let (x, y) = (get("x").unwrap_or(0.0), get("y").unwrap_or(0.0));
    let (w, h) = (get("width")?, get("height")?);
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    let (rx, ry) = match (
        get("rx").filter(|v| *v >= 0.0),
        get("ry").filter(|v| *v >= 0.0),
    ) {
        (Some(rx), Some(ry)) => (rx, ry),
        (Some(r), None) | (None, Some(r)) => (r, r),
        (None, None) => (0.0, 0.0),
    };
    let (rx, ry) = (rx.min(w / 2.0), ry.min(h / 2.0));
    let mut points = Vec::new();
    if rx <= 0.0 || ry <= 0.0 {
        points.extend([[x, y], [x + w, y], [x + w, y + h], [x, y + h]]);
    } else {
        let n = arc_steps(rx.max(ry), std::f64::consts::FRAC_PI_2, tol);
        let corner = |points: &mut Vec<Point>, c: Point, from: f64| {
            for i in 0..=n {
                let t = from + std::f64::consts::FRAC_PI_2 * i as f64 / n as f64;
                points.push([c[0] + rx * t.cos(), c[1] + ry * t.sin()]);
            }
        };
        let pi = std::f64::consts::PI;
        corner(&mut points, [x + w - rx, y + ry], -pi / 2.0);
        corner(&mut points, [x + w - rx, y + h - ry], 0.0);
        corner(&mut points, [x + rx, y + h - ry], pi / 2.0);
        corner(&mut points, [x + rx, y + ry], pi);
    }
    Some(Sub {
        points,
        closed: true,
    })
}

fn points_list(attrs: &[(String, String)], closed: bool) -> Option<Sub> {
    let values = numbers(attr(attrs, "points")?)?;
    let points: Vec<Point> = values.chunks_exact(2).map(|p| [p[0], p[1]]).collect();
    (points.len() >= 2).then_some(Sub { points, closed })
}

// ---------------------------------------------------------------- style

#[derive(Clone, Debug, PartialEq)]
enum Paint {
    None,
    /// `None`: the finish's colour (`currentColor` without a `color`).
    Colour(Option<[u8; 3]>),
    Url(String),
}

#[derive(Clone, Debug)]
struct Style {
    fill: Paint,
    stroke: Paint,
    stroke_width: f64,
    rule: FillRule,
    fill_opacity: f64,
    stroke_opacity: f64,
    /// The product of the group opacities down to here.
    opacity: f64,
    color: Option<[u8; 3]>,
    font_size: f64,
    anchor: TextAnchor,
}

/// SVG's `text-anchor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextAnchor {
    Start,
    Middle,
    End,
}

impl Default for Style {
    fn default() -> Self {
        // SVG's initial values: a black fill, no stroke, width 1.
        Self {
            fill: Paint::Colour(Some([0, 0, 0])),
            stroke: Paint::None,
            stroke_width: 1.0,
            rule: FillRule::NonZero,
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            opacity: 1.0,
            color: None,
            font_size: 16.0,
            anchor: TextAnchor::Start,
        }
    }
}

fn attributes(e: &BytesStart) -> Vec<(String, String)> {
    e.attributes()
        .flatten()
        .map(|a| {
            let key = String::from_utf8_lossy(a.key.local_name().as_ref()).to_string();
            let value = a
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map(|v| v.to_string())
                .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).to_string());
            (key, value)
        })
        .collect()
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// A presentation property: the `style` declaration over the attribute.
fn declared(attrs: &[(String, String)], key: &str) -> Option<String> {
    let from_style = attr(attrs, "style").and_then(|style| {
        style.split(';').rev().find_map(|decl| {
            let (k, v) = decl.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_string())
        })
    });
    from_style.or_else(|| attr(attrs, key).map(|v| v.trim().to_string()))
}

/// A length in user units (`px` and no unit; other units are taken as
/// user units, which only the aspect of the artwork depends on).
fn length(s: &str) -> Option<f64> {
    let s = s.trim();
    let end = s
        .find(|c: char| c.is_ascii_alphabetic() || c == '%')
        .filter(|i| {
            // An exponent is part of the number.
            !matches!(s.as_bytes().get(*i), Some(b'e' | b'E'))
                || !s
                    .as_bytes()
                    .get(i + 1)
                    .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+'))
        })
        .unwrap_or(s.len());
    s.get(..end)?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

/// A length in millimetres, for the root's size.
fn length_mm(s: &str) -> Option<f64> {
    let v = length(s)?;
    let s = s.trim();
    let unit = if s.ends_with("mm") {
        1.0
    } else if s.ends_with("cm") {
        10.0
    } else if s.ends_with("in") {
        25.4
    } else if s.ends_with("pt") {
        25.4 / 72.0
    } else if s.ends_with("pc") {
        25.4 / 6.0
    } else if s.ends_with('%') {
        return None;
    } else {
        25.4 / 96.0
    };
    Some(v * unit)
}

const NAMED: [(&str, [u8; 3]); 20] = [
    ("black", [0, 0, 0]),
    ("white", [255, 255, 255]),
    ("red", [255, 0, 0]),
    ("green", [0, 128, 0]),
    ("lime", [0, 255, 0]),
    ("blue", [0, 0, 255]),
    ("yellow", [255, 255, 0]),
    ("cyan", [0, 255, 255]),
    ("aqua", [0, 255, 255]),
    ("magenta", [255, 0, 255]),
    ("fuchsia", [255, 0, 255]),
    ("gray", [128, 128, 128]),
    ("grey", [128, 128, 128]),
    ("silver", [192, 192, 192]),
    ("maroon", [128, 0, 0]),
    ("olive", [128, 128, 0]),
    ("teal", [0, 128, 128]),
    ("navy", [0, 0, 128]),
    ("purple", [128, 0, 128]),
    ("orange", [255, 165, 0]),
];

/// An sRGB colour: `#rgb`, `#rrggbb`, `rgb(r, g, b)` (numbers or
/// percentages) or a basic name.
fn colour(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let digit = |i: usize| -> Option<u8> {
            hex.get(i..i + 1)
                .and_then(|d| u8::from_str_radix(d, 16).ok())
        };
        return match hex.len() {
            3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
            6 => {
                let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
                Some([byte(0)?, byte(2)?, byte(4)?])
            }
            _ => None,
        };
    }
    if let Some(inner) = s
        .strip_prefix("rgb(")
        .or_else(|| s.strip_prefix("rgba("))
        .and_then(|r| r.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        let channel = |p: &str| -> Option<u8> {
            let v = match p.strip_suffix('%') {
                Some(pc) => pc.trim().parse::<f64>().ok()? / 100.0 * 255.0,
                None => p.parse::<f64>().ok()?,
            };
            v.is_finite().then(|| v.round().clamp(0.0, 255.0) as u8)
        };
        return match parts.as_slice() {
            [r, g, b] | [r, g, b, _] => Some([channel(r)?, channel(g)?, channel(b)?]),
            _ => None,
        };
    }
    let lower = s.to_ascii_lowercase();
    NAMED.iter().find(|(n, _)| *n == lower).map(|(_, c)| *c)
}

fn paint(value: &str, color: Option<[u8; 3]>) -> Option<Paint> {
    let v = value.trim();
    if matches!(v, "none" | "transparent") {
        return Some(Paint::None);
    }
    if v == "currentColor" {
        return Some(Paint::Colour(color));
    }
    if let Some(rest) = v.strip_prefix("url(") {
        let (inside, _) = rest.split_once(')')?;
        let id = inside
            .trim()
            .trim_matches(['"', '\''])
            .trim_start_matches('#');
        return Some(Paint::Url(id.to_string()));
    }
    colour(v).map(|c| Paint::Colour(Some(c)))
}

fn opacity(value: &str) -> Option<f64> {
    let v = value.trim();
    let n = match v.strip_suffix('%') {
        Some(p) => p.trim().parse::<f64>().ok()? / 100.0,
        None => v.parse::<f64>().ok()?,
    };
    n.is_finite().then(|| n.clamp(0.0, 1.0))
}

fn style(attrs: &[(String, String)], inherited: &Style) -> Style {
    let mut s = inherited.clone();
    if let Some(c) = declared(attrs, "color").and_then(|c| colour(&c)) {
        s.color = Some(c);
    }
    let paint_of = |key: &str, now: &Paint, color| match declared(attrs, key) {
        Some(v) if v != "inherit" => paint(&v, color).unwrap_or_else(|| now.clone()),
        _ => now.clone(),
    };
    s.fill = paint_of("fill", &s.fill, s.color);
    s.stroke = paint_of("stroke", &s.stroke, s.color);
    if let Some(w) = declared(attrs, "stroke-width").and_then(|w| length(&w)) {
        s.stroke_width = w.max(0.0);
    }
    match declared(attrs, "fill-rule").as_deref() {
        Some("evenodd") => s.rule = FillRule::EvenOdd,
        Some("nonzero") => s.rule = FillRule::NonZero,
        _ => {}
    }
    if let Some(o) = declared(attrs, "fill-opacity").and_then(|o| opacity(&o)) {
        s.fill_opacity = o;
    }
    if let Some(o) = declared(attrs, "stroke-opacity").and_then(|o| opacity(&o)) {
        s.stroke_opacity = o;
    }
    if let Some(o) = declared(attrs, "opacity").and_then(|o| opacity(&o)) {
        s.opacity *= o;
    }
    if let Some(f) = declared(attrs, "font-size").and_then(|f| length(&f))
        && f > 0.0
    {
        s.font_size = f;
    }
    match declared(attrs, "text-anchor").as_deref() {
        Some("middle") => s.anchor = TextAnchor::Middle,
        Some("end") => s.anchor = TextAnchor::End,
        Some("start") => s.anchor = TextAnchor::Start,
        _ => {}
    }
    s
}

// ---------------------------------------------------------------- gradients

/// Each gradient's stop colours' mean, by id, following `href`.
fn gradients(text: &str) -> Result<BTreeMap<String, Option<[u8; 3]>>> {
    struct Raw {
        stops: Vec<[u8; 3]>,
        href: Option<String>,
    }
    let mut raw: BTreeMap<String, Raw> = BTreeMap::new();
    let mut reader = quick_xml::Reader::from_str(text);
    let mut current: Option<String> = None;
    loop {
        let event = reader
            .read_event()
            .map_err(|e| invalid(format!("the SVG is not well-formed XML: {e}")))?;
        let (e, empty) = match &event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(e) => {
                let name = e.local_name();
                if matches!(name.as_ref(), b"linearGradient" | b"radialGradient") {
                    current = None;
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let name = e.local_name();
        let attrs = attributes(e);
        match name.as_ref() {
            b"linearGradient" | b"radialGradient" => {
                let Some(id) = attr(&attrs, "id") else {
                    continue;
                };
                let href = attr(&attrs, "href").map(|h| h.trim_start_matches('#').to_string());
                raw.insert(
                    id.to_string(),
                    Raw {
                        stops: Vec::new(),
                        href,
                    },
                );
                if !empty {
                    current = Some(id.to_string());
                }
            }
            b"stop" => {
                let visible = declared(&attrs, "stop-opacity")
                    .and_then(|o| opacity(&o))
                    .is_none_or(|o| o >= 0.5);
                let c = declared(&attrs, "stop-color").map_or(Some([0, 0, 0]), |c| colour(&c));
                if let (Some(id), Some(c), true) = (&current, c, visible)
                    && let Some(g) = raw.get_mut(id)
                    && g.stops.len() < 256
                {
                    g.stops.push(c);
                }
            }
            _ => {}
        }
    }
    let mut out = BTreeMap::new();
    for id in raw.keys() {
        let mut at = id.clone();
        let mut stops: &[[u8; 3]] = &[];
        for _ in 0..MAX_HREF_HOPS {
            let Some(g) = raw.get(&at) else {
                break;
            };
            if !g.stops.is_empty() {
                stops = &g.stops;
                break;
            }
            match &g.href {
                Some(next) => at = next.clone(),
                None => break,
            }
        }
        let mean = (!stops.is_empty()).then(|| {
            let n = stops.len() as f64;
            std::array::from_fn(|k| {
                (stops.iter().map(|s| f64::from(s[k])).sum::<f64>() / n).round() as u8
            })
        });
        out.insert(id.clone(), mean);
    }
    Ok(out)
}

// ---------------------------------------------------------------- walk

#[derive(Clone, Debug)]
struct Frame {
    m: Affine,
    style: Style,
}

/// A `text` element being read: its frame, origin and characters.
struct PendingText {
    frame: Frame,
    at: Point,
    text: String,
    depth: usize,
}

struct Walk<'a> {
    gradients: &'a BTreeMap<String, Option<[u8; 3]>>,
    /// Chord tolerance in document units; `None` on the bounds pass.
    tol: Option<f64>,
    root: Option<Affine>,
    stack: Vec<Frame>,
    skip: usize,
    elements: usize,
    points: usize,
    loops: usize,
    chars: usize,
    images: usize,
    text: Option<PendingText>,
    out: Vec<Painted>,
    notes: BTreeMap<String, usize>,
}

const NOT_RENDERED: [&str; 16] = [
    "defs",
    "clipPath",
    "mask",
    "pattern",
    "marker",
    "symbol",
    "metadata",
    "title",
    "desc",
    "style",
    "script",
    "foreignObject",
    "linearGradient",
    "radialGradient",
    "filter",
    "use",
];

impl<'a> Walk<'a> {
    fn run(
        text: &str,
        gradients: &'a BTreeMap<String, Option<[u8; 3]>>,
        tol: Option<f64>,
    ) -> Result<Self> {
        let mut walk = Self {
            gradients,
            tol,
            root: None,
            stack: Vec::new(),
            skip: 0,
            elements: 0,
            points: 0,
            loops: 0,
            chars: 0,
            images: 0,
            text: None,
            out: Vec::new(),
            notes: BTreeMap::new(),
        };
        let mut reader = quick_xml::Reader::from_str(text);
        loop {
            let event = reader
                .read_event()
                .map_err(|e| invalid(format!("the SVG is not well-formed XML: {e}")))?;
            match event {
                Event::Start(e) => walk.open(&e, false)?,
                Event::Empty(e) => walk.open(&e, true)?,
                Event::Text(t) => {
                    if walk.skip == 0
                        && let Some(pending) = &mut walk.text
                    {
                        let s = t
                            .xml_content(quick_xml::XmlVersion::Implicit1_0)
                            .map(|c| c.to_string())
                            .unwrap_or_default();
                        pending.text.push_str(&s);
                    }
                }
                Event::End(_) => walk.close()?,
                Event::Eof => break,
                _ => {}
            }
        }
        if walk.root.is_none() {
            return Err(invalid("the file has no <svg> root element"));
        }
        Ok(walk)
    }

    fn notes(&self) -> Vec<String> {
        self.notes
            .iter()
            .map(|(what, n)| format!("{what} skipped ({n})"))
            .collect()
    }

    fn note(&mut self, what: &str) {
        *self.notes.entry(what.to_string()).or_default() += 1;
    }

    fn close(&mut self) -> Result<()> {
        if self.skip > 0 {
            self.skip -= 1;
            return Ok(());
        }
        if self
            .text
            .as_ref()
            .is_some_and(|t| t.depth == self.stack.len())
            && let Some(pending) = self.text.take()
        {
            self.emit_text(pending)?;
        }
        self.stack.pop();
        Ok(())
    }

    fn open(&mut self, e: &BytesStart, empty: bool) -> Result<()> {
        let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
        if self.skip > 0 {
            if !empty {
                self.skip += 1;
            }
            return Ok(());
        }
        let attrs = attributes(e);
        let parent = match (self.stack.last(), &self.root) {
            (Some(f), _) => f.clone(),
            (None, Some(root)) => Frame {
                m: *root,
                style: Style::default(),
            },
            (None, None) => {
                if name != "svg" {
                    return Err(invalid("the file's root element is not <svg>"));
                }
                let root = root_affine(&attrs);
                self.root = Some(root);
                Frame {
                    m: root,
                    style: Style::default(),
                }
            }
        };
        let hidden = matches!(declared(&attrs, "display").as_deref(), Some("none"))
            || matches!(
                declared(&attrs, "visibility").as_deref(),
                Some("hidden" | "collapse")
            );
        if hidden || NOT_RENDERED.contains(&name.as_str()) {
            if name == "use" {
                self.note("use references (expand them first)");
            }
            if !empty {
                self.skip = 1;
            }
            return Ok(());
        }
        for (key, what) in [
            ("clip-path", "clip paths"),
            ("mask", "masks"),
            ("filter", "filters"),
        ] {
            if declared(&attrs, key).is_some_and(|v| v != "none") {
                self.note(what);
            }
        }
        let mut frame = parent;
        if let Some(t) = attr(&attrs, "transform") {
            match transform_list(t) {
                Some(t) => frame.m = frame.m.then(&t),
                None => {
                    self.note("elements with a malformed transform");
                    if !empty {
                        self.skip = 1;
                    }
                    return Ok(());
                }
            }
        }
        frame.style = style(&attrs, &frame.style);
        let drawn = matches!(
            name.as_str(),
            "path" | "line" | "rect" | "circle" | "ellipse" | "polyline" | "polygon" | "image"
        ) || (name == "text" && self.text.is_none());
        if drawn {
            self.elements += 1;
            if self.elements > MAX_ELEMENTS {
                return Err(invalid(format!(
                    "the SVG has more than {MAX_ELEMENTS} drawn elements"
                )));
            }
        }
        match name.as_str() {
            "text" if self.text.is_none() => {
                let first = |k: &str| {
                    attr(&attrs, k)
                        .and_then(|v| v.split([' ', ',']).find(|s| !s.is_empty()))
                        .and_then(length)
                        .unwrap_or(0.0)
                };
                let pending = PendingText {
                    frame: frame.clone(),
                    at: [first("x"), first("y")],
                    text: String::new(),
                    depth: self.stack.len() + 1,
                };
                if empty {
                    self.emit_text(pending)?;
                } else {
                    self.text = Some(pending);
                }
            }
            "image" => self.image(&attrs, &frame)?,
            "path" | "line" | "rect" | "circle" | "ellipse" | "polyline" | "polygon" => {
                self.shape(&name, &attrs, &frame)?;
            }
            _ => {}
        }
        if !empty {
            self.stack.push(frame);
            if self.stack.len() > MAX_DEPTH {
                return Err(invalid("the SVG is nested too deeply"));
            }
        }
        Ok(())
    }

    /// The chord tolerance in an element's user units.
    fn user_tol(&self, m: &Affine) -> f64 {
        let k = m.length_scale();
        match self.tol {
            Some(t) if k > 1e-12 => (t / k).max(1e-9),
            // The bounds pass: a coarse tolerance relative to nothing is
            // enough, since curves stay within their control hulls.
            _ => f64::INFINITY,
        }
    }

    fn resolve(&mut self, p: &Paint, opacity: f64) -> Option<Option<[u8; 3]>> {
        if opacity < 0.5 {
            return None;
        }
        match p {
            Paint::None => None,
            Paint::Colour(c) => Some(*c),
            Paint::Url(id) => match self.gradients.get(id) {
                Some(Some(mean)) => Some(Some(*mean)),
                _ => {
                    self.note("paint servers other than gradients (patterns, missing ids)");
                    None
                }
            },
        }
    }

    fn shape(&mut self, name: &str, attrs: &[(String, String)], frame: &Frame) -> Result<()> {
        let tol = self.user_tol(&frame.m).min(1e6);
        let num = |k: &str| attr(attrs, k).and_then(length);
        let subs: Vec<Sub> = match name {
            "path" => match attr(attrs, "d").map(|d| path(d, tol)) {
                Some(Some(subs)) => subs,
                Some(None) => {
                    self.note("paths with malformed data");
                    return Ok(());
                }
                None => Vec::new(),
            },
            "rect" => rect(attrs, tol).into_iter().collect(),
            "circle" => match num("r").filter(|r| *r > 0.0) {
                Some(r) => vec![ellipse(
                    [num("cx").unwrap_or(0.0), num("cy").unwrap_or(0.0)],
                    r,
                    r,
                    tol,
                )],
                None => Vec::new(),
            },
            "ellipse" => match (num("rx"), num("ry")) {
                (Some(rx), Some(ry)) if rx > 0.0 && ry > 0.0 => vec![ellipse(
                    [num("cx").unwrap_or(0.0), num("cy").unwrap_or(0.0)],
                    rx,
                    ry,
                    tol,
                )],
                _ => Vec::new(),
            },
            "line" => vec![Sub {
                points: vec![
                    [num("x1").unwrap_or(0.0), num("y1").unwrap_or(0.0)],
                    [num("x2").unwrap_or(0.0), num("y2").unwrap_or(0.0)],
                ],
                closed: false,
            }],
            "polyline" => points_list(attrs, false).into_iter().collect(),
            "polygon" => points_list(attrs, true).into_iter().collect(),
            _ => Vec::new(),
        };
        if subs.is_empty() {
            return Ok(());
        }
        let s = &frame.style;
        let fill = if name == "line" {
            None
        } else {
            self.resolve(&s.fill.clone(), s.fill_opacity * s.opacity)
        };
        let stroke = self.resolve(&s.stroke.clone(), s.stroke_opacity * s.opacity);
        if let Some(colour) = fill {
            let loops: Vec<Vec<Point>> = subs
                .iter()
                .filter(|sub| sub.points.len() >= 3)
                .map(|sub| sub.points.iter().map(|p| frame.m.apply(*p)).collect())
                .collect();
            self.push(Painted {
                colour,
                loops,
                rule: s.rule,
            })?;
        }
        let width = s.stroke_width * frame.m.length_scale();
        if let Some(colour) = stroke
            && width > 0.0
            && width.is_finite()
        {
            let mut loops = Vec::new();
            for sub in &subs {
                let pts: Vec<Point> = sub.points.iter().map(|p| frame.m.apply(*p)).collect();
                stroke_loops(&mut loops, &pts, sub.closed, width / 2.0);
                if loops.len() > MAX_LOOPS {
                    return Err(invalid(format!(
                        "the artwork has more than {MAX_LOOPS} outlines"
                    )));
                }
            }
            self.push(Painted {
                colour,
                loops,
                rule: FillRule::NonZero,
            })?;
        }
        Ok(())
    }

    fn push(&mut self, painted: Painted) -> Result<()> {
        if painted.loops.is_empty() {
            return Ok(());
        }
        self.loops = self.loops.saturating_add(painted.loops.len());
        self.points = self
            .points
            .saturating_add(painted.loops.iter().map(Vec::len).sum());
        if self.loops > MAX_LOOPS {
            return Err(invalid(format!(
                "the artwork has more than {MAX_LOOPS} outlines"
            )));
        }
        if self.points > MAX_POINTS {
            return Err(invalid(format!(
                "the artwork has more than {MAX_POINTS} outline points"
            )));
        }
        if painted
            .loops
            .iter()
            .flatten()
            .flatten()
            .any(|v| !v.is_finite())
        {
            return Err(invalid("the artwork reaches beyond finite coordinates"));
        }
        self.out.push(painted);
        Ok(())
    }

    fn emit_text(&mut self, pending: PendingText) -> Result<()> {
        let text: String = pending
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            return Ok(());
        }
        self.chars = self.chars.saturating_add(text.chars().count());
        if self.chars > MAX_TEXT_CHARS {
            return Err(invalid(format!(
                "the artwork has more than {MAX_TEXT_CHARS} characters of text"
            )));
        }
        let s = &pending.frame.style;
        let Some(colour) = self.resolve(&s.fill.clone(), s.fill_opacity * s.opacity) else {
            return Ok(());
        };
        use solidcraft_engine::sketch::{TextAlign, TextStyle, text_geometry_styled};
        let style = TextStyle {
            align: match s.anchor {
                TextAnchor::Start => TextAlign::Left,
                TextAnchor::Middle => TextAlign::Center,
                TextAnchor::End => TextAlign::Right,
            },
            ..TextStyle::default()
        };
        // Glyphs at a capital height of 1, then scaled to the font size.
        let geometry = text_geometry_styled(
            None,
            &text,
            crate::geom::Vec2::new(0.0, 0.0),
            1.0,
            0.0,
            &style,
            None,
        )
        .map_err(|e| invalid(format!("text: {e}")))?;
        let cap = FONT_CAP_EM * s.font_size;
        let [x, y] = pending.at;
        let m = pending.frame.m;
        // The font is y-up; user units are y-down.
        let loops: Vec<Vec<Point>> = shapes::chain(&geometry)
            .into_iter()
            .map(|l| {
                l.into_iter()
                    .map(|g| m.apply([x + g[0] * cap, y - g[1] * cap]))
                    .collect()
            })
            .collect();
        self.push(Painted {
            colour,
            loops,
            rule: FillRule::NonZero,
        })
    }

    fn image(&mut self, attrs: &[(String, String)], frame: &Frame) -> Result<()> {
        let Some(href) = attr(attrs, "href") else {
            return Ok(());
        };
        let Some(data) = href.trim().strip_prefix("data:image/png;base64,") else {
            self.note("images other than embedded PNG");
            return Ok(());
        };
        self.images += 1;
        if self.images > MAX_IMAGES {
            return Err(invalid(format!(
                "the artwork has more than {MAX_IMAGES} embedded images"
            )));
        }
        let num = |k: &str| attr(attrs, k).and_then(length);
        let (Some(w), Some(h)) = (num("width"), num("height")) else {
            self.note("images without a width and height");
            return Ok(());
        };
        if !(w > 0.0 && h > 0.0) {
            return Ok(());
        }
        let s = &frame.style;
        if s.opacity < 0.5 {
            return Ok(());
        }
        let cells = cells(data)?;
        let (x, y) = (num("x").unwrap_or(0.0), num("y").unwrap_or(0.0));
        // preserveAspectRatio: xMidYMid meet unless "none".
        let stretch = attr(attrs, "preserveAspectRatio").is_some_and(|v| v.trim() == "none");
        let (cw, ch) = (f64::from(cells.width), f64::from(cells.height));
        let (sx, sy) = if stretch {
            (w / cw, h / ch)
        } else {
            let k = (w / cw).min(h / ch);
            (k, k)
        };
        let (ox, oy) = (x + (w - sx * cw) / 2.0, y + (h - sy * ch) / 2.0);
        // Cell edges in pixels: the last cell of a row or column may be
        // narrower.
        let f = cells.cell;
        let px = |c: u32, limit: u32| f64::from(c.saturating_mul(f).min(limit));
        for (colour, runs) in cells.runs {
            let loops: Vec<Vec<Point>> = runs
                .into_iter()
                .map(|(row, a, b)| {
                    let (x0, x1) = (ox + sx * px(a, cells.width), ox + sx * px(b, cells.width));
                    let (y0, y1) = (
                        oy + sy * px(row, cells.height),
                        oy + sy * px(row + 1, cells.height),
                    );
                    [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
                        .map(|p| frame.m.apply(p))
                        .to_vec()
                })
                .collect();
            self.push(Painted {
                colour: Some(colour),
                loops,
                rule: FillRule::NonZero,
            })?;
        }
        Ok(())
    }
}

/// The root's map from user units: the `viewBox` stretched to the
/// `width` and `height` when `preserveAspectRatio="none"`, otherwise
/// uniform (only the artwork's aspect matters; it is fitted afterwards).
fn root_affine(attrs: &[(String, String)]) -> Affine {
    let view = attr(attrs, "viewBox")
        .and_then(numbers)
        .filter(|v| v.len() == 4 && v[2] > 0.0 && v[3] > 0.0);
    let stretch = attr(attrs, "preserveAspectRatio").is_some_and(|v| v.trim() == "none");
    let size = (
        attr(attrs, "width").and_then(length_mm),
        attr(attrs, "height").and_then(length_mm),
    );
    match (view, size, stretch) {
        (Some(v), (Some(w), Some(h)), true) if w > 0.0 && h > 0.0 => {
            Affine([w / v[2], 0.0, 0.0, h / v[3], 0.0, 0.0])
        }
        _ => Affine::IDENTITY,
    }
}

/// A polyline stroked `half` either side, as counter-clockwise (in
/// document units) round-capped segments.
fn stroke_loops(out: &mut Vec<Vec<Point>>, points: &[Point], closed: bool, half: f64) {
    let n = points.len();
    if n == 0 {
        return;
    }
    if n == 1 {
        out.push(
            (0..4 * CAP_STEPS)
                .map(|k| {
                    let t = std::f64::consts::TAU * k as f64 / (4 * CAP_STEPS) as f64;
                    [points[0][0] + half * t.cos(), points[0][1] + half * t.sin()]
                })
                .collect(),
        );
        return;
    }
    let count = if closed { n } else { n - 1 };
    for i in 0..count {
        let (a, b) = (points[i], points[(i + 1) % n]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let base = if dx.hypot(dy) > 1e-12 {
            dy.atan2(dx)
        } else {
            0.0
        };
        let mut ring = Vec::with_capacity(2 * CAP_STEPS + 2);
        for (c, from) in [(b, -0.5), (a, 0.5)] {
            for k in 0..=CAP_STEPS {
                let t = base + std::f64::consts::PI * (from + k as f64 / CAP_STEPS as f64);
                ring.push([c[0] + half * t.cos(), c[1] + half * t.sin()]);
            }
        }
        out.push(ring);
    }
}

// ---------------------------------------------------------------- images

/// An image (`width` × `height` pixels) averaged down into cells: each quantised colour's row runs
/// `(row, first column, end column)`, rows from the top, colours in order
/// of first appearance.
/// One colour's row runs `(row, first column, end column)`.
type ColourRuns = ([u8; 3], Vec<(u32, u32, u32)>);

struct Cells {
    width: u32,
    height: u32,
    /// Pixels a cell spans along each side (the last ones fewer).
    cell: u32,
    runs: Vec<ColourRuns>,
}

fn cells(data: &str) -> Result<Cells> {
    use base64::Engine as _;
    let compact: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact.as_bytes())
        .map_err(|_| invalid("an embedded image is not valid base64"))?;
    let limits = png::Limits { bytes: 64 << 20 };
    let mut decoder = png::Decoder::new_with_limits(std::io::Cursor::new(bytes), limits);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|e| invalid(format!("an embedded PNG cannot be read: {e}")))?;
    let (w, h) = {
        let info = reader.info();
        (info.width, info.height)
    };
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS {
        return Err(invalid(format!(
            "an embedded image is {w} × {h} pixels; at most {MAX_IMAGE_PIXELS} pixels are read"
        )));
    }
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| invalid("an embedded PNG is too large"))?;
    let mut buf = vec![0u8; size];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| invalid(format!("an embedded PNG cannot be decoded: {e}")))?;
    let channels = match frame.color_type {
        png::ColorType::Grayscale => 1usize,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => {
            return Err(invalid("an embedded PNG's palette was not expanded"));
        }
    };
    let line = frame.line_size;
    let pixel = |x: u32, y: u32| -> [f64; 4] {
        let at = y as usize * line + x as usize * channels;
        let g = |k: usize| buf.get(at + k).copied().map_or(0.0, f64::from);
        match channels {
            1 => [g(0), g(0), g(0), 255.0],
            2 => [g(0), g(0), g(0), g(1)],
            3 => [g(0), g(1), g(2), 255.0],
            _ => [g(0), g(1), g(2), g(3)],
        }
    };
    // Cells of `f` × `f` pixels, at most IMAGE_CELLS a side.
    let f = w.max(h).div_ceil(IMAGE_CELLS).max(1);
    let (cw, ch) = (w.div_ceil(f), h.div_ceil(f));
    let level = |v: f64| -> u8 {
        let steps = f64::from(IMAGE_LEVELS - 1);
        ((v / 255.0 * steps).round() / steps * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    let mut runs: Vec<ColourRuns> = Vec::new();
    for row in 0..ch {
        let mut current: Option<([u8; 3], u32)> = None;
        let end_run = |runs: &mut Vec<ColourRuns>, run: Option<([u8; 3], u32)>, end: u32| {
            if let Some((c, start)) = run {
                match runs.iter_mut().find(|(k, _)| *k == c) {
                    Some((_, list)) => list.push((row, start, end)),
                    None => runs.push((c, vec![(row, start, end)])),
                }
            }
        };
        for col in 0..cw {
            // Alpha-weighted mean of the cell.
            let mut sum = [0.0f64; 4];
            let mut count = 0.0;
            for y in row * f..((row + 1) * f).min(h) {
                for x in col * f..((col + 1) * f).min(w) {
                    let p = pixel(x, y);
                    let a = p[3] / 255.0;
                    for k in 0..3 {
                        sum[k] += p[k] * a;
                    }
                    sum[3] += p[3];
                    count += 1.0;
                }
            }
            let alpha = if count > 0.0 { sum[3] / count } else { 0.0 };
            let colour = (alpha >= 128.0).then(|| {
                let weight = sum[3] / 255.0;
                [
                    level(sum[0] / weight),
                    level(sum[1] / weight),
                    level(sum[2] / weight),
                ]
            });
            match (current, colour) {
                (Some((c, _)), Some(n)) if c == n => {}
                (run, next) => {
                    end_run(&mut runs, run, col);
                    current = next.map(|n| (n, col));
                }
            }
        }
        end_run(&mut runs, current, cw);
    }
    Ok(Cells {
        width: w,
        height: h,
        cell: f,
        runs,
    })
}

#[cfg(test)]
#[path = "paint_tests.rs"]
mod tests;
