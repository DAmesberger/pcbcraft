//! A bitmap rasteriser for verification: pixel centres are sampled, so a
//! pixel is dark when its centre lies in the shape. Shapes are rendered row
//! by row as exact spans: discs and capsules analytically, polygons by
//! even-odd crossings of the pixel-centre line, composite (macro) shapes by
//! span set operations in primitive order. Dark objects set, clear objects
//! clear, in object order.
use crate::Fallible;
use crate::Point;
use crate::excellon::{DrillFile, Hit};
use crate::gerber::{Contour, GerberFile, Kind, Path, Prim, Template};

/// Chord tolerance arcs are flattened at for rasterising, mm.
pub const ARC_TOLERANCE_MM: f64 = 0.0005;
/// The most pixels one bitmap holds (64 MiB of bits).
const MAX_PIXELS: usize = 1 << 29;

/// A window of square pixels, one bit each.
#[derive(Clone, Debug, PartialEq)]
pub struct Bitmap {
    pub min: Point,
    pub pixel: f64,
    pub width: usize,
    pub height: usize,
    words: usize,
    bits: Vec<u64>,
}

impl Bitmap {
    /// The window from `min` to `max` (grown to whole pixels) at `pixel` mm.
    pub fn new(min: Point, max: Point, pixel: f64) -> Fallible<Self> {
        if !(pixel > 0.0 && pixel.is_finite() && max[0] > min[0] && max[1] > min[1]) {
            return Err("raster: an empty window or pixel".into());
        }
        let w = ((max[0] - min[0]) / pixel).ceil();
        let h = ((max[1] - min[1]) / pixel).ceil();
        if w * h > MAX_PIXELS as f64 || w.is_nan() || h.is_nan() {
            return Err(format!("raster: {w} × {h} pixels is too many").into());
        }
        let (width, height) = (w as usize, h as usize);
        let words = width.div_ceil(64);
        Ok(Self {
            min,
            pixel,
            width,
            height,
            words,
            bits: vec![0; words * height],
        })
    }

    /// A bitmap on the same grid, empty.
    pub fn blank(&self) -> Self {
        Self {
            bits: vec![0; self.bits.len()],
            ..self.clone()
        }
    }

    pub fn same_grid(&self, o: &Bitmap) -> bool {
        self.min == o.min
            && self.pixel == o.pixel
            && self.width == o.width
            && self.height == o.height
    }

    pub fn get(&self, i: usize, j: usize) -> bool {
        i < self.width && j < self.height && self.bits[j * self.words + i / 64] >> (i % 64) & 1 == 1
    }

    /// Whether the pixel whose centre is nearest `p` is dark.
    pub fn at(&self, p: Point) -> bool {
        let i = ((p[0] - self.min[0]) / self.pixel).floor();
        let j = ((p[1] - self.min[1]) / self.pixel).floor();
        i >= 0.0 && j >= 0.0 && self.get(i as usize, j as usize)
    }

    pub fn count(&self) -> u64 {
        self.bits.iter().map(|w| u64::from(w.count_ones())).sum()
    }

    pub fn area_mm2(&self) -> f64 {
        self.count() as f64 * self.pixel * self.pixel
    }

    /// The y of row `j`'s pixel centres.
    fn row_y(&self, j: usize) -> f64 {
        self.min[1] + (j as f64 + 0.5) * self.pixel
    }

    /// Rows whose centres lie in `[y0, y1]`.
    fn rows(&self, y0: f64, y1: f64) -> std::ops::Range<usize> {
        let first = ((y0 - self.min[1]) / self.pixel - 0.5).ceil().max(0.0);
        let last = ((y1 - self.min[1]) / self.pixel - 0.5).floor() + 1.0;
        let last = last.clamp(0.0, self.height as f64);
        let first = first.min(last);
        first as usize..last as usize
    }

    /// Set (dark) or clear the pixels of row `j` whose centres lie in
    /// `[x0, x1)`.
    fn span(&mut self, j: usize, x0: f64, x1: f64, dark: bool) {
        if j >= self.height || x1 <= x0 || x1.is_nan() || x0.is_nan() {
            return;
        }
        let a = ((x0 - self.min[0]) / self.pixel - 0.5).ceil().max(0.0);
        let b = ((x1 - self.min[0]) / self.pixel - 0.5)
            .ceil()
            .min(self.width as f64);
        if b <= a {
            return;
        }
        let (a, b) = (a as usize, b as usize);
        let row = &mut self.bits[j * self.words..(j + 1) * self.words];
        let mut i = a;
        while i < b {
            let word = i / 64;
            let lo = i % 64;
            let hi = (b - word * 64).min(64);
            let mask = if hi - lo == 64 {
                u64::MAX
            } else {
                ((1u64 << (hi - lo)) - 1) << lo
            };
            if dark {
                row[word] |= mask;
            } else {
                row[word] &= !mask;
            }
            i = word * 64 + hi;
        }
    }

    /// Render `shape` dark or clear.
    pub fn render(&mut self, shape: &Shape, dark: bool) {
        let (rows, spans) = spans_of(shape, self);
        for (k, row) in spans.iter().enumerate() {
            for (x0, x1) in row {
                self.span(rows.start + k, *x0, *x1, dark);
            }
        }
    }

    /// `self` OR `o` (same grid).
    pub fn union(&mut self, o: &Bitmap) -> Fallible<()> {
        if !self.same_grid(o) {
            return Err("raster: different grids".into());
        }
        for (a, b) in self.bits.iter_mut().zip(&o.bits) {
            *a |= b;
        }
        Ok(())
    }

    /// `self` AND NOT `o` (same grid).
    pub fn subtract(&mut self, o: &Bitmap) -> Fallible<()> {
        if !self.same_grid(o) {
            return Err("raster: different grids".into());
        }
        for (a, b) in self.bits.iter_mut().zip(&o.bits) {
            *a &= !b;
        }
        Ok(())
    }
}

/// A shape to rasterise, in mm.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Disc {
        c: Point,
        r: f64,
    },
    /// Every point within `r` of the segment `a`–`b` (round ends).
    Capsule {
        a: Point,
        b: Point,
        r: f64,
    },
    /// Closed rings filled even-odd together.
    Poly {
        contours: Vec<Vec<Point>>,
    },
    /// Parts applied in order: `true` adds, `false` removes.
    Composite {
        parts: Vec<(bool, Shape)>,
    },
}

impl Shape {
    pub fn rect(center: Point, w: f64, h: f64) -> Self {
        let (x, y) = (w / 2.0, h / 2.0);
        Self::Poly {
            contours: vec![vec![
                [center[0] - x, center[1] - y],
                [center[0] + x, center[1] - y],
                [center[0] + x, center[1] + y],
                [center[0] - x, center[1] + y],
            ]],
        }
    }

    /// `[ymin, ymax]` of the shape.
    fn y_range(&self) -> (f64, f64) {
        match self {
            Self::Disc { c, r } => (c[1] - r, c[1] + r),
            Self::Capsule { a, b, r } => (a[1].min(b[1]) - r, a[1].max(b[1]) + r),
            Self::Poly { contours } => contours
                .iter()
                .flatten()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                    (lo.min(p[1]), hi.max(p[1]))
                }),
            Self::Composite { parts } => parts
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, s)| s.y_range())
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (a, b)| {
                    (lo.min(a), hi.max(b))
                }),
        }
    }
}

type Span = (f64, f64);

/// The rows the shape touches and its spans on each, sorted and disjoint.
fn spans_of(shape: &Shape, bitmap: &Bitmap) -> (std::ops::Range<usize>, Vec<Vec<Span>>) {
    let (y0, y1) = shape.y_range();
    if y1 < y0 || y0.is_nan() || y1.is_nan() {
        return (0..0, Vec::new());
    }
    let rows = bitmap.rows(y0, y1);
    let spans = spans_on(shape, bitmap, rows.clone());
    (rows, spans)
}

fn spans_on(shape: &Shape, bitmap: &Bitmap, rows: std::ops::Range<usize>) -> Vec<Vec<Span>> {
    match shape {
        Shape::Disc { c, r } => rows
            .map(|j| disc_span(*c, *r, bitmap.row_y(j)).into_iter().collect())
            .collect(),
        Shape::Capsule { a, b, r } => rows
            .map(|j| {
                capsule_span(*a, *b, *r, bitmap.row_y(j))
                    .into_iter()
                    .collect()
            })
            .collect(),
        Shape::Poly { contours } => poly_spans(contours, bitmap, rows),
        Shape::Composite { parts } => {
            let mut out: Vec<Vec<Span>> = vec![Vec::new(); rows.len()];
            for (on, part) in parts {
                let theirs = spans_on(part, bitmap, rows.clone());
                for (mine, theirs) in out.iter_mut().zip(theirs) {
                    *mine = if *on {
                        union(mine, &theirs)
                    } else {
                        subtract(mine, &theirs)
                    };
                }
            }
            out
        }
    }
}

fn disc_span(c: Point, r: f64, y: f64) -> Option<Span> {
    let dy = y - c[1];
    (dy.abs() <= r).then(|| {
        let s = (r * r - dy * dy).max(0.0).sqrt();
        (c[0] - s, c[0] + s)
    })
}

/// The interval of x where `alpha·x + beta` lies in `[lo, hi]`.
fn linear_interval(alpha: f64, beta: f64, lo: f64, hi: f64) -> Option<Span> {
    if alpha.abs() < 1e-15 {
        return (lo..=hi)
            .contains(&beta)
            .then_some((f64::NEG_INFINITY, f64::INFINITY));
    }
    let (a, b) = ((lo - beta) / alpha, (hi - beta) / alpha);
    Some((a.min(b), a.max(b)))
}

fn capsule_span(a: Point, b: Point, r: f64, y: f64) -> Option<Span> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0].hypot(d[1]);
    let mut parts: Vec<Span> = [disc_span(a, r, y), disc_span(b, r, y)]
        .into_iter()
        .flatten()
        .collect();
    if len > 1e-12 {
        let u = [d[0] / len, d[1] / len];
        let n = [-u[1], u[0]];
        // (p − a)·u ∈ [0, len] and (p − a)·n ∈ [−r, r], p = (x, y).
        let along = linear_interval(u[0], (y - a[1]) * u[1] - a[0] * u[0], 0.0, len);
        let across = linear_interval(n[0], (y - a[1]) * n[1] - a[0] * n[0], -r, r);
        if let (Some(p), Some(q)) = (along, across) {
            let (lo, hi) = (p.0.max(q.0), p.1.min(q.1));
            if hi >= lo && lo.is_finite() && hi.is_finite() {
                parts.push((lo, hi));
            }
        }
    }
    // A capsule is convex: its row is one interval.
    parts
        .into_iter()
        .reduce(|p, q| (p.0.min(q.0), p.1.max(q.1)))
}

/// Even-odd spans of closed rings: each edge crosses the pixel-centre
/// lines of the rows within its half-open y range.
fn poly_spans(
    contours: &[Vec<Point>],
    bitmap: &Bitmap,
    rows: std::ops::Range<usize>,
) -> Vec<Vec<Span>> {
    let mut crossings: Vec<Vec<f64>> = vec![Vec::new(); rows.len()];
    for ring in contours {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        for k in 0..n {
            let (p, q) = (ring[k], ring[(k + 1) % n]);
            if p[1] == q[1] {
                continue;
            }
            let (lo, hi) = if p[1] < q[1] { (p, q) } else { (q, p) };
            // Rows with centre y in [lo.y, hi.y).
            let first = ((lo[1] - bitmap.min[1]) / bitmap.pixel - 0.5).ceil();
            let last = ((hi[1] - bitmap.min[1]) / bitmap.pixel - 0.5).ceil();
            let first = first.max(rows.start as f64);
            let last = last.min(rows.end as f64);
            let mut j = first;
            while j < last {
                let y = bitmap.min[1] + (j + 0.5) * bitmap.pixel;
                let x = lo[0] + (y - lo[1]) * (hi[0] - lo[0]) / (hi[1] - lo[1]);
                crossings[j as usize - rows.start].push(x);
                j += 1.0;
            }
        }
    }
    crossings
        .into_iter()
        .map(|mut xs| {
            xs.sort_by(f64::total_cmp);
            xs.chunks_exact(2).map(|c| (c[0], c[1])).collect()
        })
        .collect()
}

/// Sorted, disjoint union of two sorted, disjoint span lists.
fn union(a: &[Span], b: &[Span]) -> Vec<Span> {
    let mut all: Vec<Span> = a.iter().chain(b).copied().collect();
    all.sort_by(|p, q| p.0.total_cmp(&q.0));
    let mut out: Vec<Span> = Vec::with_capacity(all.len());
    for s in all {
        match out.last_mut() {
            Some(last) if s.0 <= last.1 => last.1 = last.1.max(s.1),
            _ => out.push(s),
        }
    }
    out
}

/// `a` without `b`.
fn subtract(a: &[Span], b: &[Span]) -> Vec<Span> {
    let mut out = Vec::new();
    for &(mut lo, hi) in a {
        for &(c, d) in b {
            if d <= lo || c >= hi {
                continue;
            }
            if c > lo {
                out.push((lo, c));
            }
            lo = lo.max(d);
            if lo >= hi {
                break;
            }
        }
        if lo < hi {
            out.push((lo, hi));
        }
    }
    out
}

/// The points of an arc from `from` to `to` about `center`, within
/// [`ARC_TOLERANCE_MM`]; `from == to` is a full circle.
pub fn arc_points(from: Point, to: Point, center: Point, clockwise: bool) -> Vec<Point> {
    let r = (from[0] - center[0]).hypot(from[1] - center[1]);
    let a0 = (from[1] - center[1]).atan2(from[0] - center[0]);
    let a1 = (to[1] - center[1]).atan2(to[0] - center[0]);
    let tau = std::f64::consts::TAU;
    let same = (from[0] - to[0]).hypot(from[1] - to[1]) < 1e-9;
    let mut sweep = if clockwise { a0 - a1 } else { a1 - a0 }.rem_euclid(tau);
    if same || sweep < 1e-12 {
        sweep = if same { tau } else { sweep };
    }
    let step = if r > ARC_TOLERANCE_MM {
        2.0 * (1.0 - ARC_TOLERANCE_MM / r).acos()
    } else {
        tau
    };
    let n = (sweep / step).ceil().clamp(1.0, 200_000.0) as usize;
    let sign = if clockwise { -1.0 } else { 1.0 };
    let mut out: Vec<Point> = (0..n)
        .map(|k| {
            let a = a0 + sign * sweep * k as f64 / n as f64;
            [center[0] + r * a.cos(), center[1] + r * a.sin()]
        })
        .collect();
    out.push(to);
    out
}

/// A contour as a closed ring, arcs flattened.
pub fn contour_ring(contour: &Contour) -> Vec<Point> {
    let mut ring = vec![contour.start];
    let mut at = contour.start;
    for segment in &contour.segments {
        match *segment {
            Path::Line { to } => ring.push(to),
            Path::Arc {
                to,
                center,
                clockwise,
            } => ring.extend(arc_points(at, to, center, clockwise).into_iter().skip(1)),
        }
        at = match segment {
            Path::Line { to } | Path::Arc { to, .. } => *to,
        };
    }
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    ring
}

fn rotate(p: Point, deg: f64) -> Point {
    let (s, c) = deg.to_radians().sin_cos();
    [c * p[0] - s * p[1], s * p[0] + c * p[1]]
}

fn shifted(p: Point, by: Point) -> Point {
    [p[0] + by[0], p[1] + by[1]]
}

/// A macro primitive at `at`, and whether it adds.
fn primitive(prim: &Prim, at: Point) -> (bool, Shape) {
    let place = |p: Point, rot: f64| shifted(rotate(p, rot), at);
    match prim {
        Prim::Circle {
            on,
            diameter,
            center,
            rotation,
        } => (
            *on,
            Shape::Disc {
                c: place(*center, *rotation),
                r: diameter / 2.0,
            },
        ),
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
                [start[0] + n[0], start[1] + n[1]],
                [start[0] - n[0], start[1] - n[1]],
                [end[0] - n[0], end[1] - n[1]],
                [end[0] + n[0], end[1] + n[1]],
            ];
            (
                *on,
                Shape::Poly {
                    contours: vec![corners.iter().map(|p| place(*p, *rotation)).collect()],
                },
            )
        }
        Prim::CenterLine {
            on,
            width,
            height,
            center,
            rotation,
        } => {
            let (x, y) = (width / 2.0, height / 2.0);
            let corners = [[-x, -y], [x, -y], [x, y], [-x, y]];
            (
                *on,
                Shape::Poly {
                    contours: vec![
                        corners
                            .iter()
                            .map(|p| place(shifted(*p, *center), *rotation))
                            .collect(),
                    ],
                },
            )
        }
        Prim::Outline {
            on,
            points,
            rotation,
        } => (
            *on,
            Shape::Poly {
                contours: vec![points.iter().map(|p| place(*p, *rotation)).collect()],
            },
        ),
        Prim::Polygon {
            on,
            vertices,
            center,
            diameter,
            rotation,
        } => (
            *on,
            Shape::Poly {
                contours: vec![regular(
                    place(*center, *rotation),
                    diameter / 2.0,
                    *vertices,
                    *rotation,
                )],
            },
        ),
    }
}

fn regular(c: Point, r: f64, n: usize, rotation: f64) -> Vec<Point> {
    (0..n.max(3))
        .map(|k| {
            let a = (rotation + 360.0 * k as f64 / n.max(3) as f64).to_radians();
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}

fn with_hole(shape: Shape, at: Point, hole: Option<f64>) -> Shape {
    match hole {
        Some(d) if d > 0.0 => Shape::Composite {
            parts: vec![(true, shape), (false, Shape::Disc { c: at, r: d / 2.0 })],
        },
        _ => shape,
    }
}

/// The image of an aperture flashed at `at`.
pub fn flash(template: &Template, at: Point) -> Shape {
    match template {
        Template::Circle { diameter, hole } => with_hole(
            Shape::Disc {
                c: at,
                r: diameter / 2.0,
            },
            at,
            *hole,
        ),
        Template::Rect { w, h, hole } => with_hole(Shape::rect(at, *w, *h), at, *hole),
        Template::Obround { w, h, hole } => {
            let shape = if w >= h {
                let d = (w - h) / 2.0;
                Shape::Capsule {
                    a: [at[0] - d, at[1]],
                    b: [at[0] + d, at[1]],
                    r: h / 2.0,
                }
            } else {
                let d = (h - w) / 2.0;
                Shape::Capsule {
                    a: [at[0], at[1] - d],
                    b: [at[0], at[1] + d],
                    r: w / 2.0,
                }
            };
            with_hole(shape, at, *hole)
        }
        Template::Polygon {
            diameter,
            vertices,
            rotation,
            hole,
        } => with_hole(
            Shape::Poly {
                contours: vec![regular(at, diameter / 2.0, *vertices, *rotation)],
            },
            at,
            *hole,
        ),
        Template::Macro { primitives, .. } => Shape::Composite {
            parts: primitives.iter().map(|p| primitive(p, at)).collect(),
        },
    }
}

/// Render every object of a Gerber file, in order.
pub fn render_gerber(file: &GerberFile, bitmap: &mut Bitmap) -> Fallible<()> {
    for object in &file.objects {
        let aperture = || -> Fallible<&Template> {
            let id = object
                .aperture
                .ok_or("raster: an object without an aperture")?;
            Ok(&file
                .apertures
                .get(&id)
                .ok_or_else(|| format!("raster: aperture D{id} is not defined"))?
                .template)
        };
        let round = |t: &Template| -> Fallible<f64> {
            match t {
                Template::Circle { diameter, .. } => Ok(diameter / 2.0),
                _ => Err("raster: only circular apertures may draw".into()),
            }
        };
        match &object.kind {
            Kind::Flash { at } => bitmap.render(&flash(aperture()?, *at), object.dark),
            Kind::Draw { from, to } => {
                let r = round(aperture()?)?;
                bitmap.render(
                    &Shape::Capsule {
                        a: *from,
                        b: *to,
                        r,
                    },
                    object.dark,
                );
            }
            Kind::Arc {
                from,
                to,
                center,
                clockwise,
            } => {
                let r = round(aperture()?)?;
                let points = arc_points(*from, *to, *center, *clockwise);
                for pair in points.windows(2) {
                    bitmap.render(
                        &Shape::Capsule {
                            a: pair[0],
                            b: pair[1],
                            r,
                        },
                        object.dark,
                    );
                }
            }
            Kind::Region { contour } => bitmap.render(
                &Shape::Poly {
                    contours: vec![contour_ring(contour)],
                },
                object.dark,
            ),
        }
    }
    Ok(())
}

/// Render every hit and slot of a drill file whose tool `keep` accepts.
pub fn render_drill(file: &DrillFile, bitmap: &mut Bitmap) -> Fallible<()> {
    for hit in &file.hits {
        let tool = |t: u32| -> Fallible<f64> {
            file.tools
                .get(&t)
                .copied()
                .ok_or_else(|| format!("raster: tool T{t} is not defined").into())
        };
        match *hit {
            Hit::Round { tool: t, at } => bitmap.render(
                &Shape::Disc {
                    c: at,
                    r: tool(t)? / 2.0,
                },
                true,
            ),
            Hit::Slot { tool: t, from, to } => bitmap.render(
                &Shape::Capsule {
                    a: from,
                    b: to,
                    r: tool(t)? / 2.0,
                },
                true,
            ),
        }
    }
    Ok(())
}

/// Two renderings of one layer compared.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Comparison {
    pub pixel: f64,
    pub area_a: f64,
    pub area_b: f64,
    /// Area dark in exactly one of them, mm².
    pub xor_area: f64,
    /// `xor_area` over the larger area (0 when both are empty).
    pub xor_share: f64,
}

pub fn compare(a: &Bitmap, b: &Bitmap) -> Fallible<Comparison> {
    if !a.same_grid(b) {
        return Err("raster: different grids".into());
    }
    let xor: u64 = a
        .bits
        .iter()
        .zip(&b.bits)
        .map(|(p, q)| u64::from((p ^ q).count_ones()))
        .sum();
    let px2 = a.pixel * a.pixel;
    let (area_a, area_b) = (a.area_mm2(), b.area_mm2());
    let xor_area = xor as f64 * px2;
    let larger = area_a.max(area_b);
    Ok(Comparison {
        pixel: a.pixel,
        area_a,
        area_b,
        xor_area,
        xor_share: if larger > 0.0 { xor_area / larger } else { 0.0 },
    })
}

pub fn xor_area_mm2(a: &Bitmap, b: &Bitmap) -> Fallible<f64> {
    Ok(compare(a, b)?.xor_area)
}
