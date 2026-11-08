//! Plain 2D coordinate vocabulary shared by library definitions and the
//! project: points, drawn shapes, rigid transforms and bounds. Millimetres
//! throughout, y up. Checks here are persistence bounds, not design rules.
//!
//! The shape vocabulary follows the line/arc/circle concept of Solidcraft's
//! sketch curves (MIT OR Apache-2.0); this code is written independently.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod frame3;
pub use frame3::{Point3, Transform3};

pub type Point = [f64; 2];

/// Largest coordinate magnitude the document stores, in millimetres.
pub const COORDINATE_LIMIT_MM: f64 = 10000.0;

/// Vertices one curve flattens to at most, whatever the tolerance.
pub const MAX_FLATTEN_VERTICES: usize = 1024;

/// Finite and within ±10000 mm.
pub fn finite_within(values: &[f64]) -> bool {
    values
        .iter()
        .all(|v| v.is_finite() && v.abs() <= COORDINATE_LIMIT_MM)
}

pub fn point_valid(p: Point) -> bool {
    finite_within(&p)
}

/// A positive physical dimension: finite and 0.01–10000 mm.
pub fn size_valid(size: &[f64]) -> bool {
    size.iter()
        .all(|v| v.is_finite() && (0.01..=COORDINATE_LIMIT_MM).contains(v))
}

/// One drawn primitive in a local frame. Angles are degrees, counter-clockwise
/// positive.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Shape {
    Line {
        a: Point,
        b: Point,
    },
    /// An open path of at least two points. `radii` rounds interior
    /// corners: empty, or one fillet radius per point (0 keeps the corner
    /// sharp; the open ends cannot be rounded).
    Polyline {
        points: Vec<Point>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        radii: Vec<f64>,
    },
    /// Axis-aligned in its local frame, between two opposite corners, with
    /// every corner rounded by `radius` (0 for square corners).
    Rect {
        a: Point,
        b: Point,
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: f64,
    },
    Circle {
        center: Point,
        radius: f64,
    },
    /// From `start_deg`, turning `sweep_deg` (0 < |sweep| ≤ 360).
    Arc {
        center: Point,
        radius: f64,
        start_deg: f64,
        sweep_deg: f64,
    },
    /// A closed outline of at least three points; the last joins the first.
    /// `radii` rounds corners as for a polyline, every vertex included.
    Polygon {
        points: Vec<Point>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        radii: Vec<f64>,
    },
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// Reflect across the local x-axis (y → −y) if `mirror`, then rotate
/// counter-clockwise by `rotation_deg`, then translate.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Transform2 {
    pub translate: Point,
    pub rotation_deg: f64,
    pub mirror: bool,
}

/// An axis-aligned box, `min ≤ max` on both axes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Bounds {
    pub min: Point,
    pub max: Point,
}

/// Cosine and sine of an angle in degrees, exact at multiples of 90° so
/// grid coordinates stay on the grid after quarter turns.
pub fn cos_sin(deg: f64) -> (f64, f64) {
    let turns = deg / 90.0;
    let nearest = turns.round();
    if (turns - nearest).abs() < 1e-12 && nearest.is_finite() {
        // rem_euclid of an integral f64 is integral; 0..4 fits any int.
        match nearest.rem_euclid(4.0) as u8 {
            0 => return (1.0, 0.0),
            1 => return (0.0, 1.0),
            2 => return (-1.0, 0.0),
            _ => return (0.0, -1.0),
        }
    }
    let r = deg.to_radians();
    (r.cos(), r.sin())
}

/// Whether an angle is a whole number of quarter turns.
pub fn is_quarter_turn(deg: f64) -> bool {
    let turns = deg / 90.0;
    (turns - turns.round()).abs() < 1e-12
}

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}
fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn length(a: Point) -> f64 {
    a[0].hypot(a[1])
}
pub fn distance(a: Point, b: Point) -> f64 {
    length(sub(a, b))
}

impl Transform2 {
    pub const IDENTITY: Self = Self {
        translate: [0.0, 0.0],
        rotation_deg: 0.0,
        mirror: false,
    };

    /// A pure translation.
    pub fn translation(by: Point) -> Self {
        Self {
            translate: by,
            ..Self::IDENTITY
        }
    }

    /// Rotate and reflect only, without the translation.
    pub fn apply_vector(&self, v: Point) -> Point {
        let v = if self.mirror { [v[0], -v[1]] } else { v };
        let (c, s) = cos_sin(self.rotation_deg);
        [c * v[0] - s * v[1], s * v[0] + c * v[1]]
    }

    pub fn apply(&self, p: Point) -> Point {
        add(self.apply_vector(p), self.translate)
    }

    /// This transform followed by `o`: `self.then(o).apply(p)` equals
    /// `o.apply(self.apply(p))`.
    pub fn then(&self, o: &Transform2) -> Transform2 {
        // A reflection reverses the sense of the rotation before it.
        let rotation_deg = if o.mirror {
            o.rotation_deg - self.rotation_deg
        } else {
            o.rotation_deg + self.rotation_deg
        };
        Transform2 {
            translate: o.apply(self.translate),
            rotation_deg: rotation_deg.rem_euclid(360.0),
            mirror: self.mirror != o.mirror,
        }
    }

    /// The transform that undoes this one: `self.inverse().apply(self.apply(p))`
    /// is `p`. A reflection is its own inverse and reverses the sense of the
    /// rotation it follows.
    pub fn inverse(&self) -> Transform2 {
        let linear = Transform2 {
            translate: [0.0, 0.0],
            rotation_deg: if self.mirror {
                self.rotation_deg
            } else {
                -self.rotation_deg
            }
            .rem_euclid(360.0),
            mirror: self.mirror,
        };
        let back = linear.apply_vector(self.translate);
        Transform2 {
            translate: [-back[0], -back[1]],
            ..linear
        }
    }

    /// Map an angle measured in the local frame.
    pub fn apply_angle(&self, deg: f64) -> f64 {
        let local = if self.mirror { -deg } else { deg };
        local + self.rotation_deg
    }
}

impl Bounds {
    pub fn point(p: Point) -> Self {
        Self { min: p, max: p }
    }
    /// The smallest box holding every point; `None` for none.
    pub fn of(points: impl IntoIterator<Item = Point>) -> Option<Self> {
        points.into_iter().fold(None, |b: Option<Bounds>, p| {
            Some(b.map_or(Self::point(p), |b| b.including(p)))
        })
    }
    pub fn including(self, p: Point) -> Self {
        Self {
            min: [self.min[0].min(p[0]), self.min[1].min(p[1])],
            max: [self.max[0].max(p[0]), self.max[1].max(p[1])],
        }
    }
    pub fn union(self, o: Bounds) -> Self {
        self.including(o.min).including(o.max)
    }
    /// Grown by `by` on every side.
    pub fn inflated(self, by: f64) -> Self {
        Self {
            min: [self.min[0] - by, self.min[1] - by],
            max: [self.max[0] + by, self.max[1] + by],
        }
    }
    pub fn contains(&self, p: Point) -> bool {
        (self.min[0]..=self.max[0]).contains(&p[0]) && (self.min[1]..=self.max[1]).contains(&p[1])
    }
    pub fn center(&self) -> Point {
        [
            (self.min[0] + self.max[0]) / 2.0,
            (self.min[1] + self.max[1]) / 2.0,
        ]
    }
    pub fn size(&self) -> Point {
        [self.max[0] - self.min[0], self.max[1] - self.min[1]]
    }
    /// The four corners, counter-clockwise from `min`.
    pub fn corners(&self) -> [Point; 4] {
        [
            self.min,
            [self.max[0], self.min[1]],
            self.max,
            [self.min[0], self.max[1]],
        ]
    }
    /// Whether the boxes share any point (touching counts).
    pub fn intersects(&self, o: &Bounds) -> bool {
        self.min[0] <= o.max[0]
            && o.min[0] <= self.max[0]
            && self.min[1] <= o.max[1]
            && o.min[1] <= self.max[1]
    }
}

/// Whether `deg` lies on the arc from `start` turning `sweep`, inclusive.
fn angle_on_arc(deg: f64, start: f64, sweep: f64) -> bool {
    if sweep.abs() >= 360.0 {
        return true;
    }
    let delta = if sweep >= 0.0 {
        (deg - start).rem_euclid(360.0)
    } else {
        (start - deg).rem_euclid(360.0)
    };
    delta <= sweep.abs() + 1e-9 || delta >= 360.0 - 1e-9
}

fn polar(center: Point, radius: f64, deg: f64) -> Point {
    let (c, s) = cos_sin(deg);
    [center[0] + radius * c, center[1] + radius * s]
}

/// Segment count for a curve of `radius` spanning `sweep_deg` so no chord
/// strays more than `tol` from it, within 1..=[`MAX_FLATTEN_VERTICES`].
fn arc_segments(radius: f64, sweep_deg: f64, tol: f64) -> usize {
    let sweep = sweep_deg.abs().to_radians();
    let tol = if tol.is_finite() && tol > 0.0 {
        tol
    } else {
        0.01
    };
    let step = if tol >= radius {
        std::f64::consts::PI
    } else {
        2.0 * (1.0 - tol / radius).acos()
    };
    let wanted = if step > 0.0 && step.is_finite() {
        (sweep / step).ceil()
    } else {
        1.0
    };
    // Bounded before the cast, so it cannot truncate.
    wanted.clamp(1.0, (MAX_FLATTEN_VERTICES - 1) as f64) as usize
}

/// The largest turn at a vertex that can still be rounded, in degrees; a
/// path folding back on itself has no fillet.
pub const MAX_FILLET_TURN_DEG: f64 = 179.0;

/// Whether `p` is inside the closed outline that `parts` (lines and fillet
/// arcs, end to end) trace, by the even-odd rule. Exact and linear in the
/// parts: the polygon through the parts' start points, toggled inside each
/// arc's circular segment (between its chord and the arc), with no
/// flattening.
fn rounded_contains(parts: &[Shape], p: Point) -> bool {
    let chords: Vec<Point> = parts
        .iter()
        .filter_map(|part| match part {
            Shape::Line { a, .. } => Some(*a),
            Shape::Arc { .. } => part.arc_ends().map(|[s, _]| s),
            _ => None,
        })
        .collect();
    let mut inside = polygon_contains(&chords, p);
    for part in parts {
        if let Shape::Arc {
            center,
            radius,
            start_deg,
            sweep_deg,
        } = part
            && let Some([s, e]) = part.arc_ends()
            && distance(p, *center) <= *radius
        {
            let mid = polar(*center, *radius, start_deg + sweep_deg / 2.0);
            let side = |q: Point| cross(sub(e, s), sub(q, s));
            if side(p) * side(mid) > 0.0 {
                inside = !inside;
            }
        }
    }
    inside
}

fn radius_at(radii: &[f64], i: usize) -> f64 {
    radii
        .get(i)
        .copied()
        .filter(|r| r.is_finite())
        .unwrap_or(0.0)
        .max(0.0)
}

fn unit(v: Point) -> Option<Point> {
    let len = length(v);
    (len > 1e-12).then(|| [v[0] / len, v[1] / len])
}

/// The signed turn at `v` from the edge arriving from `prev` to the edge
/// leaving towards `next`, in degrees (counter-clockwise positive); `None`
/// when either edge has no length.
fn turn_deg(prev: Point, v: Point, next: Point) -> Option<f64> {
    let a = unit(sub(v, prev))?;
    let b = unit(sub(next, v))?;
    Some(cross(a, b).atan2(dot(a, b)).to_degrees())
}

/// `tan(|turn| / 2)`, exact for right-angle turns so rounded grid corners
/// keep their tangent points on the grid.
fn half_turn_tan(turn_deg: f64) -> f64 {
    if (turn_deg.abs() - 90.0).abs() < 1e-12 {
        1.0
    } else {
        (turn_deg.abs() / 2.0).to_radians().tan()
    }
}

/// Neighbours of vertex `i`, or `None` at the ends of an open path.
fn neighbours(points: &[Point], i: usize, closed: bool) -> Option<(Point, Point)> {
    let n = points.len();
    if n < 3 || (!closed && (i == 0 || i + 1 == n)) {
        return None;
    }
    Some((points[(i + n - 1) % n], points[(i + 1) % n]))
}

/// How far each corner's fillet reaches along its two edges, as requested.
fn wanted_trims(points: &[Point], radii: &[f64], closed: bool) -> Vec<f64> {
    (0..points.len())
        .map(|i| {
            let r = radius_at(radii, i);
            match (r > 0.0).then(|| neighbours(points, i, closed)).flatten() {
                Some((prev, next)) => match turn_deg(prev, points[i], next) {
                    Some(t) if t.abs() > 1e-9 && t.abs() <= MAX_FILLET_TURN_DEG => {
                        r * half_turn_tan(t)
                    }
                    _ => 0.0,
                },
                None => 0.0,
            }
        })
        .collect()
}

/// Edges of a path as vertex index pairs.
fn edge_indices(n: usize, closed: bool) -> impl Iterator<Item = (usize, usize)> {
    let count = if closed { n } else { n.saturating_sub(1) };
    (0..count).map(move |i| (i, (i + 1) % n.max(1)))
}

/// The trims actually drawn: where two fillets would overlap on an edge,
/// both shrink in proportion until they meet, so the outline never folds.
fn fitted_trims(points: &[Point], radii: &[f64], closed: bool) -> Vec<f64> {
    let wanted = wanted_trims(points, radii, closed);
    let mut factor = vec![1.0_f64; points.len()];
    for (i, j) in edge_indices(points.len(), closed) {
        let len = distance(points[i], points[j]);
        let sum = wanted[i] + wanted[j];
        if sum > len && sum > 0.0 {
            let f = len / sum;
            factor[i] = factor[i].min(f);
            factor[j] = factor[j].min(f);
        }
    }
    wanted.iter().zip(&factor).map(|(w, f)| w * f).collect()
}

/// Whether every requested corner radius fits: no rounded open end or
/// fold-back, and no edge shorter than the fillets reaching along it.
pub fn corner_radii_fit(points: &[Point], radii: &[f64], closed: bool) -> bool {
    let n = points.len();
    for i in 0..n {
        if radius_at(radii, i) <= 0.0 {
            continue;
        }
        let Some((prev, next)) = neighbours(points, i, closed) else {
            return false;
        };
        match turn_deg(prev, points[i], next) {
            Some(t) if t.abs() <= MAX_FILLET_TURN_DEG => {}
            _ => return false,
        }
    }
    let wanted = wanted_trims(points, radii, closed);
    edge_indices(n, closed)
        .all(|(i, j)| wanted[i] + wanted[j] <= distance(points[i], points[j]) + 1e-9)
}

/// Radii reduced, corner by corner, to what [`corner_radii_fit`] accepts:
/// open ends and fold-backs become sharp, and fillets that would overlap
/// shrink in proportion. A drawing tool uses this so what it commits is
/// always valid.
pub fn fit_corner_radii(points: &[Point], radii: &[f64], closed: bool) -> Vec<f64> {
    if radii.iter().all(|r| *r <= 0.0) {
        return vec![];
    }
    let wanted = wanted_trims(points, radii, closed);
    let fitted = fitted_trims(points, radii, closed);
    (0..points.len())
        .map(|i| {
            if wanted[i] <= 0.0 {
                0.0
            } else {
                // Shrink slightly below the limit so rounding stays inside it.
                radius_at(radii, i) * (fitted[i] / wanted[i]) * (1.0 - 1e-9)
            }
        })
        .collect()
}

/// The largest radius vertex `i` of an open path can take while its
/// neighbours' fillets stay as drawn: each of its two edges, less what the
/// fillet at the edge's other end takes of it. 0 at an end, and where the
/// edges run in line or fold back.
pub fn corner_radius_limit(points: &[Point], radii: &[f64], i: usize) -> f64 {
    let Some((prev, next)) = neighbours(points, i, false) else {
        return 0.0;
    };
    let v = points[i];
    let turn = match turn_deg(prev, v, next) {
        Some(t) if t.abs() > 1e-9 && t.abs() <= MAX_FILLET_TURN_DEG => t,
        _ => return 0.0,
    };
    let mut others = radii.to_vec();
    others.resize(points.len(), 0.0);
    others[i] = 0.0;
    let trims = fitted_trims(points, &others, false);
    let trim = |j: usize| trims.get(j).copied().unwrap_or(0.0);
    let room = (distance(prev, v) - trim(i - 1)).min(distance(v, next) - trim(i + 1));
    room.max(0.0) / half_turn_tan(turn)
}

/// Each rounded corner's arc as drawn on an open path, by vertex index:
/// where it starts, its middle, and where it ends.
pub fn fillet_arcs(points: &[Point], radii: &[f64]) -> Vec<(usize, [Point; 3])> {
    fillets(points, radii, false)
        .into_iter()
        .enumerate()
        .filter_map(|(i, fillet)| {
            let (start, end, arc) = fillet?;
            match arc {
                Shape::Arc {
                    center,
                    radius,
                    start_deg,
                    sweep_deg,
                } => Some((
                    i,
                    [
                        start,
                        polar(center, radius, start_deg + sweep_deg / 2.0),
                        end,
                    ],
                )),
                _ => None,
            }
        })
        .collect()
}

/// The straight edges and fillet arcs of a path whose corners are rounded
/// by `radii`, in path order.
pub fn rounded_path(points: &[Point], radii: &[f64], closed: bool) -> Vec<Shape> {
    let n = points.len();
    let corners = fillets(points, radii, closed);
    let mut out = Vec::new();
    for (i, j) in edge_indices(n, closed) {
        let a = corners[i].as_ref().map_or(points[i], |c| c.1);
        let b = corners[j].as_ref().map_or(points[j], |c| c.0);
        if distance(a, b) > 1e-12 {
            out.push(Shape::Line { a, b });
        }
        if let Some((_, _, arc)) = &corners[j] {
            out.push(arc.clone());
        }
    }
    out
}

/// For each vertex rounded as drawn: where its fillet starts, where it
/// ends, and its arc.
fn fillets(points: &[Point], radii: &[f64], closed: bool) -> Vec<Option<(Point, Point, Shape)>> {
    let trims = fitted_trims(points, radii, closed);
    (0..points.len())
        .map(|i| {
            let trim = trims[i];
            if trim <= 0.0 {
                return None;
            }
            let (prev, next) = neighbours(points, i, closed)?;
            let v = points[i];
            let a = unit(sub(v, prev))?;
            let b = unit(sub(next, v))?;
            let turn = turn_deg(prev, v, next)?;
            let r = trim / half_turn_tan(turn);
            let start = [v[0] - a[0] * trim, v[1] - a[1] * trim];
            let end = [v[0] + b[0] * trim, v[1] + b[1] * trim];
            let normal = if turn > 0.0 {
                [-a[1], a[0]]
            } else {
                [a[1], -a[0]]
            };
            let center = [start[0] + normal[0] * r, start[1] + normal[1] * r];
            let from = sub(start, center);
            Some((
                start,
                end,
                Shape::Arc {
                    center,
                    radius: r,
                    start_deg: from[1].atan2(from[0]).to_degrees(),
                    sweep_deg: turn,
                },
            ))
        })
        .collect()
}

impl Shape {
    /// A square-cornered rectangle.
    pub fn rect(a: Point, b: Point) -> Self {
        Self::Rect { a, b, radius: 0.0 }
    }
    /// An open path with sharp corners.
    pub fn polyline(points: Vec<Point>) -> Self {
        Self::Polyline {
            points,
            radii: vec![],
        }
    }
    /// A closed outline with sharp corners.
    pub fn polygon(points: Vec<Point>) -> Self {
        Self::Polygon {
            points,
            radii: vec![],
        }
    }

    /// Polygons, rectangles and circles enclose an area.
    pub fn is_closed(&self) -> bool {
        matches!(
            self,
            Self::Rect { .. } | Self::Circle { .. } | Self::Polygon { .. }
        )
    }

    /// The path vertices and corner radii of a shape that can round its
    /// corners: a rectangle's four corners counter-clockwise from `min`,
    /// or a polyline's or polygon's points.
    fn corner_path(&self) -> Option<(Vec<Point>, Vec<f64>, bool)> {
        match self {
            Self::Rect { a, b, radius } => Some((
                Bounds::point(*a).including(*b).corners().to_vec(),
                vec![*radius; 4],
                true,
            )),
            Self::Polyline { points, radii } => Some((points.clone(), radii.clone(), false)),
            Self::Polygon { points, radii } => Some((points.clone(), radii.clone(), true)),
            _ => None,
        }
    }

    /// Whether any corner is rounded.
    pub fn is_rounded(&self) -> bool {
        match self {
            Self::Rect { radius, .. } => *radius > 0.0,
            Self::Polyline { radii, .. } | Self::Polygon { radii, .. } => {
                radii.iter().any(|r| *r > 0.0)
            }
            _ => false,
        }
    }

    /// A rounded shape as its straight edges and fillet arcs; `None` when
    /// no corner is rounded.
    pub fn primitives(&self) -> Option<Vec<Shape>> {
        if !self.is_rounded() {
            return None;
        }
        let (points, radii, closed) = self.corner_path()?;
        Some(rounded_path(&points, &radii, closed))
    }

    /// Whether every corner radius fits the shape (see [`corner_radii_fit`]).
    pub fn corners_fit(&self) -> bool {
        match self.corner_path() {
            Some((points, radii, closed)) if self.is_rounded() => {
                corner_radii_fit(&points, &radii, closed)
            }
            _ => true,
        }
    }

    /// The points the shape stores; a bound on encoded size. Corner radii
    /// are bounded by the same count.
    pub fn stored_points(&self) -> usize {
        match self {
            Self::Line { .. } | Self::Rect { .. } => 2,
            Self::Circle { .. } | Self::Arc { .. } => 1,
            Self::Polyline { points, .. } | Self::Polygon { points, .. } => points.len(),
        }
    }

    /// Every point and scalar is finite and within ±10000 mm; radii are not
    /// negative; corner radii are absent or one per point; arc sweeps are
    /// at most one turn and angles finite.
    pub fn finite(&self) -> bool {
        let radius_ok = |r: &f64| finite_within(&[*r]) && *r >= 0.0;
        match self {
            Self::Line { a, b } => point_valid(*a) && point_valid(*b),
            Self::Rect { a, b, radius } => point_valid(*a) && point_valid(*b) && radius_ok(radius),
            Self::Polyline { points, radii } | Self::Polygon { points, radii } => {
                points.iter().all(|p| point_valid(*p))
                    && (radii.is_empty() || radii.len() == points.len())
                    && radii.iter().all(radius_ok)
            }
            Self::Circle { center, radius } => point_valid(*center) && radius_ok(radius),
            Self::Arc {
                center,
                radius,
                start_deg,
                sweep_deg,
            } => {
                point_valid(*center)
                    && radius_ok(radius)
                    && start_deg.is_finite()
                    && start_deg.abs() <= 3600.0
                    && sweep_deg.is_finite()
                    && sweep_deg.abs() <= 360.0
            }
        }
    }

    /// The points an arc starts and ends at.
    pub fn arc_ends(&self) -> Option<[Point; 2]> {
        match self {
            Self::Arc {
                center,
                radius,
                start_deg,
                sweep_deg,
            } => Some([
                polar(*center, *radius, *start_deg),
                polar(*center, *radius, start_deg + sweep_deg),
            ]),
            _ => None,
        }
    }

    /// A cheap box that contains the shape drawn with a stroke of `width`:
    /// its stored points (a circle or arc its whole circle), never
    /// building fillets. Rounded corners lie inside it, since each fillet
    /// stays within the corner it rounds. Hit tests and snapping check it
    /// before measuring a distance.
    pub fn coarse_bounds(&self, width: f64) -> Bounds {
        let half = (width.max(0.0)) / 2.0;
        let raw = match self {
            Self::Line { a, b } | Self::Rect { a, b, .. } => Bounds::point(*a).including(*b),
            Self::Polyline { points, .. } | Self::Polygon { points, .. } => {
                Bounds::of(points.iter().copied()).unwrap_or(Bounds::point([0.0, 0.0]))
            }
            Self::Circle { center, radius } | Self::Arc { center, radius, .. } => {
                Bounds::point(*center).inflated(*radius)
            }
        };
        raw.inflated(half)
    }

    /// Bounds of the shape drawn with a stroke of `width`. Arc extremes are
    /// exact: the end points plus every axis crossing the sweep passes.
    /// Rounded corners use their fillet arcs, so the box is tight.
    pub fn bounds(&self, width: f64) -> Bounds {
        let half = (width.max(0.0)) / 2.0;
        if let Some(parts) = self.primitives() {
            let raw = parts
                .iter()
                .map(|p| p.bounds(0.0))
                .reduce(Bounds::union)
                .unwrap_or(Bounds::point([0.0, 0.0]));
            return raw.inflated(half);
        }
        let raw =
            match self {
                Self::Line { a, b } | Self::Rect { a, b, .. } => Bounds::point(*a).including(*b),
                Self::Polyline { points, .. } | Self::Polygon { points, .. } => {
                    Bounds::of(points.iter().copied()).unwrap_or(Bounds::point([0.0, 0.0]))
                }
                Self::Circle { center, radius } => Bounds::point(*center).inflated(*radius),
                Self::Arc {
                    center,
                    radius,
                    start_deg,
                    sweep_deg,
                } => {
                    let mut b = Bounds::point(polar(*center, *radius, *start_deg))
                        .including(polar(*center, *radius, start_deg + sweep_deg));
                    for axis in [0.0, 90.0, 180.0, 270.0] {
                        if angle_on_arc(axis, *start_deg, *sweep_deg) {
                            b = b.including(polar(*center, *radius, axis));
                        }
                    }
                    b
                }
            };
        raw.inflated(half)
    }

    /// Distance from `p` to the drawn curve, or 0 inside a closed shape when
    /// `filled`. Rounded corners measure to their fillet arcs.
    pub fn distance(&self, p: Point, filled: bool) -> f64 {
        if let Some(parts) = self.primitives() {
            if filled && self.is_closed() && rounded_contains(&parts, p) {
                return 0.0;
            }
            return parts
                .iter()
                .map(|s| s.distance(p, false))
                .fold(f64::INFINITY, f64::min);
        }
        match self {
            Self::Line { a, b } => segment_distance(p, *a, *b),
            Self::Polyline { points, .. } => path_distance(p, points, false),
            Self::Rect { a, b, .. } => {
                let box_ = Bounds::point(*a).including(*b);
                let corners = box_.corners();
                if filled && box_.contains(p) {
                    0.0
                } else {
                    path_distance(p, &corners, true)
                }
            }
            Self::Polygon { points, .. } => {
                if filled && polygon_contains(points, p) {
                    0.0
                } else {
                    path_distance(p, points, true)
                }
            }
            Self::Circle { center, radius } => {
                let d = distance(p, *center);
                if filled && d <= *radius {
                    0.0
                } else {
                    (d - radius).abs()
                }
            }
            Self::Arc {
                center,
                radius,
                start_deg,
                sweep_deg,
            } => {
                let v = sub(p, *center);
                let deg = v[1].atan2(v[0]).to_degrees();
                if length(v) > 0.0 && angle_on_arc(deg, *start_deg, *sweep_deg) {
                    (length(v) - radius).abs()
                } else {
                    let [s, e] = self.arc_ends().unwrap_or([*center, *center]);
                    distance(p, s).min(distance(p, e))
                }
            }
        }
    }

    /// The shape as straight segments within `chord_tol_mm`. Closed shapes
    /// return their ring without repeating the first point; curves have at
    /// most [`MAX_FLATTEN_VERTICES`] vertices each, and a rounded path at
    /// most that many per corner.
    pub fn flatten(&self, chord_tol_mm: f64) -> Vec<Point> {
        if let Some(parts) = self.primitives() {
            let mut out: Vec<Point> = Vec::new();
            for part in &parts {
                for p in part.flatten(chord_tol_mm) {
                    if out.last().is_none_or(|q| distance(*q, p) > 1e-12) {
                        out.push(p);
                    }
                }
            }
            if self.is_closed()
                && out.len() > 1
                && out
                    .first()
                    .zip(out.last())
                    .is_some_and(|(a, b)| distance(*a, *b) <= 1e-12)
            {
                out.pop();
            }
            return out;
        }
        match self {
            Self::Line { a, b } => vec![*a, *b],
            Self::Polyline { points, .. } | Self::Polygon { points, .. } => points.clone(),
            Self::Rect { a, b, .. } => Bounds::point(*a).including(*b).corners().to_vec(),
            Self::Circle { center, radius } => {
                let n = arc_segments(*radius, 360.0, chord_tol_mm).max(3);
                (0..n)
                    .map(|i| polar(*center, *radius, 360.0 * i as f64 / n as f64))
                    .collect()
            }
            Self::Arc {
                center,
                radius,
                start_deg,
                sweep_deg,
            } => {
                let n = arc_segments(*radius, *sweep_deg, chord_tol_mm);
                (0..=n)
                    .map(|i| {
                        polar(
                            *center,
                            *radius,
                            start_deg + sweep_deg * i as f64 / n as f64,
                        )
                    })
                    .collect()
            }
        }
    }

    /// The vertices a user can grab or snap to: line and path points,
    /// rectangle corners and arc ends (not fillet tangent points).
    pub fn vertices(&self) -> Vec<Point> {
        match self {
            Self::Line { a, b } => vec![*a, *b],
            Self::Polyline { points, .. } | Self::Polygon { points, .. } => points.clone(),
            Self::Rect { a, b, .. } => Bounds::point(*a).including(*b).corners().to_vec(),
            Self::Circle { center, radius } => vec![
                [center[0] + radius, center[1]],
                [center[0], center[1] + radius],
                [center[0] - radius, center[1]],
                [center[0], center[1] - radius],
            ],
            Self::Arc { .. } => self.arc_ends().map(|e| e.to_vec()).unwrap_or_default(),
        }
    }

    /// The shape in another frame. A rectangle stays a rectangle under
    /// quarter turns and becomes a polygon otherwise; a reflection negates
    /// arc start and sweep before the rotation. Corner radii are kept.
    pub fn transformed(&self, t: &Transform2) -> Shape {
        let map = |points: &[Point]| points.iter().map(|p| t.apply(*p)).collect();
        match self {
            Self::Line { a, b } => Self::Line {
                a: t.apply(*a),
                b: t.apply(*b),
            },
            Self::Polyline { points, radii } => Self::Polyline {
                points: map(points),
                radii: radii.clone(),
            },
            Self::Polygon { points, radii } => Self::Polygon {
                points: map(points),
                radii: radii.clone(),
            },
            Self::Rect { a, b, radius } if is_quarter_turn(t.rotation_deg) => Self::Rect {
                a: t.apply(*a),
                b: t.apply(*b),
                radius: *radius,
            },
            Self::Rect { a, b, radius } => Self::Polygon {
                points: map(&Bounds::point(*a).including(*b).corners()),
                radii: if *radius > 0.0 {
                    vec![*radius; 4]
                } else {
                    vec![]
                },
            },
            Self::Circle { center, radius } => Self::Circle {
                center: t.apply(*center),
                radius: *radius,
            },
            Self::Arc {
                center,
                radius,
                start_deg,
                sweep_deg,
            } => Self::Arc {
                center: t.apply(*center),
                radius: *radius,
                start_deg: t.apply_angle(*start_deg).rem_euclid(360.0),
                sweep_deg: if t.mirror { -sweep_deg } else { *sweep_deg },
            },
        }
    }

    /// The shape moved by `delta`.
    pub fn translated(&self, delta: Point) -> Shape {
        self.transformed(&Transform2::translation(delta))
    }
}

fn path_distance(p: Point, points: &[Point], closed: bool) -> f64 {
    let open = points
        .windows(2)
        .map(|w| segment_distance(p, w[0], w[1]))
        .fold(f64::INFINITY, f64::min);
    let closing = match (closed, points.first(), points.last()) {
        (true, Some(first), Some(last)) => segment_distance(p, *last, *first),
        _ => f64::INFINITY,
    };
    let single = if points.len() == 1 {
        points.first().map_or(f64::INFINITY, |q| distance(p, *q))
    } else {
        f64::INFINITY
    };
    open.min(closing).min(single)
}

/// Distance from `p` to the segment `a`–`b`.
pub fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let ab = sub(b, a);
    let len2 = dot(ab, ab);
    let t = if len2 > 0.0 {
        (dot(sub(p, a), ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    distance(p, [a[0] + ab[0] * t, a[1] + ab[1] * t])
}

/// The distance between segments `a`–`b` and `c`–`d` (0 when they cross).
pub fn segments_gap(a: Point, b: Point, c: Point, d: Point) -> f64 {
    if segments_cross(a, b, c, d) {
        return 0.0;
    }
    segment_distance(a, c, d)
        .min(segment_distance(b, c, d))
        .min(segment_distance(c, a, b))
        .min(segment_distance(d, a, b))
}

/// Even-odd containment of `p` in a closed outline. Points exactly on the
/// boundary may report either way; callers needing a margin compare
/// distances.
pub fn polygon_contains(polygon: &[Point], p: Point) -> bool {
    let mut inside = false;
    let n = polygon.len();
    for i in 0..n {
        let a = polygon[i];
        let b = polygon[(i + 1) % n];
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Twice the signed area: positive for a counter-clockwise ring.
pub fn signed_area2(polygon: &[Point]) -> f64 {
    let n = polygon.len();
    (0..n)
        .map(|i| cross(polygon[i], polygon[(i + 1) % n]))
        .sum()
}

/// Whether segments `a`–`b` and `c`–`d` cross at a single interior point of
/// both (touching, end points and collinear overlap do not count).
pub fn segments_cross(a: Point, b: Point, c: Point, d: Point) -> bool {
    let d1 = cross(sub(b, a), sub(c, a));
    let d2 = cross(sub(b, a), sub(d, a));
    let d3 = cross(sub(d, c), sub(a, c));
    let d4 = cross(sub(d, c), sub(b, c));
    let scale = length(sub(b, a)).max(length(sub(d, c))).max(1.0);
    let eps = 1e-12 * scale * scale;
    ((d1 > eps && d2 < -eps) || (d1 < -eps && d2 > eps))
        && ((d3 > eps && d4 < -eps) || (d3 < -eps && d4 > eps))
}

fn edges(polygon: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    let n = polygon.len();
    (0..n).map(move |i| (polygon[i], polygon[(i + 1) % n]))
}

/// Whether a closed outline's non-adjacent edges cross or touch. Repeated
/// consecutive points (zero-length edges) are ignored rather than read as
/// the outline touching itself.
pub fn polygon_self_intersects(polygon: &[Point]) -> bool {
    let mut distinct: Vec<Point> = Vec::with_capacity(polygon.len());
    for &p in polygon {
        if distinct.last().is_none_or(|&q| length(sub(p, q)) >= 1e-9) {
            distinct.push(p);
        }
    }
    while distinct.len() > 1
        && distinct
            .first()
            .zip(distinct.last())
            .is_some_and(|(&a, &b)| length(sub(a, b)) < 1e-9)
    {
        distinct.pop();
    }
    let polygon = distinct.as_slice();
    let n = polygon.len();
    if n < 4 {
        return false;
    }
    for i in 0..n {
        let (a, b) = (polygon[i], polygon[(i + 1) % n]);
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            let (c, d) = (polygon[j], polygon[(j + 1) % n]);
            if segments_cross(a, b, c, d)
                || segment_distance(c, a, b) < 1e-9
                || segment_distance(d, a, b) < 1e-9
                || segment_distance(a, c, d) < 1e-9
                || segment_distance(b, c, d) < 1e-9
            {
                return true;
            }
        }
    }
    false
}

/// Whether two closed outlines share interior area; touching edges or
/// corners is not overlap. Exact for simple polygons up to a 1e-7 mm probe
/// offset.
pub fn polygons_overlap(a: &[Point], b: &[Point]) -> bool {
    if a.len() < 3 || b.len() < 3 {
        return false;
    }
    let (Some(ba), Some(bb)) = (Bounds::of(a.iter().copied()), Bounds::of(b.iter().copied()))
    else {
        return false;
    };
    if !ba.intersects(&bb) {
        return false;
    }
    if edges(a).any(|(p, q)| edges(b).any(|(r, s)| segments_cross(p, q, r, s))) {
        return true;
    }
    // No proper crossing: either one lies inside the other, or they only
    // touch. Probe just inside each edge midpoint of each outline.
    inside_probe(a, b) || inside_probe(b, a)
}

/// Whether a point just inside some edge of `of` lies strictly inside `other`.
fn inside_probe(of: &[Point], other: &[Point]) -> bool {
    const PROBE: f64 = 1e-7;
    let orientation = signed_area2(of).signum();
    if orientation == 0.0 {
        return false;
    }
    edges(of).any(|(p, q)| {
        let e = sub(q, p);
        let len = length(e);
        if len == 0.0 {
            return false;
        }
        // Inward normal of a counter-clockwise ring is the left normal.
        let n = [-e[1] / len * orientation, e[0] / len * orientation];
        let probe = [
            (p[0] + q[0]) / 2.0 + n[0] * PROBE,
            (p[1] + q[1]) / 2.0 + n[1] * PROBE,
        ];
        polygon_contains(other, probe) && path_distance(probe, other, true) > PROBE / 2.0
    })
}

/// The smallest distance between an open path and a closed outline's
/// boundary, or 0 when any part of the path is inside it.
pub fn polyline_polygon_distance(line: &[Point], polygon: &[Point]) -> f64 {
    if line.iter().any(|p| polygon_contains(polygon, *p)) {
        return 0.0;
    }
    let mut best = f64::INFINITY;
    for w in line.windows(2) {
        for (r, s) in edges(polygon) {
            if segments_cross(w[0], w[1], r, s) {
                return 0.0;
            }
            best = best
                .min(segment_distance(w[0], r, s))
                .min(segment_distance(w[1], r, s))
                .min(segment_distance(r, w[0], w[1]))
                .min(segment_distance(s, w[0], w[1]));
        }
    }
    if line.len() == 1
        && let Some(p) = line.first()
    {
        best = path_distance(*p, polygon, true);
    }
    best
}

/// The area a stroke of `width` covers, as overlapping closed outlines: one
/// rectangle per flattened segment and one disc per vertex (round caps and
/// joins). Filled closed shapes are covered by their interior separately.
pub fn stroke_outline(shape: &Shape, width: f64, tol: f64) -> Vec<Vec<Point>> {
    let half = width / 2.0;
    if half.is_nan() || half <= 0.0 {
        return vec![];
    }
    let mut ring = shape.flatten(tol);
    if shape.is_closed()
        && let Some(first) = ring.first().copied()
    {
        ring.push(first);
    }
    let mut out = Vec::new();
    for w in ring.windows(2) {
        let e = sub(w[1], w[0]);
        let len = length(e);
        if len == 0.0 {
            continue;
        }
        let n = [-e[1] / len * half, e[0] / len * half];
        out.push(vec![add(w[0], n), sub(w[0], n), sub(w[1], n), add(w[1], n)]);
    }
    let cap = Shape::Circle {
        center: [0.0, 0.0],
        radius: half,
    }
    .flatten(tol);
    for p in &ring {
        out.push(cap.iter().map(|c| add(*c, *p)).collect());
    }
    out
}

#[cfg(test)]
mod tests;
