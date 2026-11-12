//! Point snapping and inference for symbol drawing, adapted from
//! Solidcraft's `drag_snap.rs` and sketch `refine_snap` (MIT OR Apache-2.0;
//! see ATTRIBUTION.md), without constraints: a point snaps, in this order
//! and within a pixel tolerance, to an endpoint or vertex, a pin's
//! connection end, a midpoint, a centre, the origin, onto a curve, into
//! horizontal or vertical alignment with another point (dashed guides),
//! and otherwise to the library grid. Holding Ctrl/Cmd or Alt suppresses
//! snapping. Pure: no painting, no document access.
use pcbcraft_core::Point;
use pcbcraft_core::geometry::{Bounds, Shape, distance, segment_distance};
use pcbcraft_core::library::{Item, Unit};

/// Snap distance in screen pixels.
pub const SNAP_PX: f32 = 7.0;

/// What a point snapped to, in priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Vertex,
    PinEnd,
    Midpoint,
    Center,
    Origin,
    OnCurve,
    Aligned,
    Grid,
    /// Snapping suppressed or off.
    Free,
}

impl Kind {
    /// A short marker label, as Solidcraft labels its snaps.
    pub fn label(self) -> &'static str {
        match self {
            Self::Vertex => "End",
            Self::PinEnd => "Pin",
            Self::Midpoint => "Mid",
            Self::Center => "Centre",
            Self::Origin => "Origin",
            Self::OnCurve => "On",
            Self::Aligned | Self::Grid | Self::Free => "",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snap {
    pub at: Point,
    pub kind: Kind,
    /// Dashed alignment guides, from the point lined up with to `at`.
    pub guides: Vec<[Point; 2]>,
}

/// What a point can snap to.
#[derive(Clone, Debug, Default)]
pub struct Targets {
    pub vertices: Vec<Point>,
    pub pin_ends: Vec<Point>,
    pub midpoints: Vec<Point>,
    pub centers: Vec<Point>,
    /// Shapes to snap onto, each with a cheap box that contains it.
    pub curves: Vec<(Bounds, Shape)>,
}

impl Targets {
    /// Targets from a unit's shapes and pins, skipping `skip` (the objects
    /// being moved), plus the pending points of the shape being drawn.
    pub fn of(unit: &Unit, skip: impl Fn(&super::Selected) -> bool, pending: &[Point]) -> Self {
        let mut t = Targets::default();
        for graphic in &unit.graphics {
            if skip(&super::Selected::Graphic(graphic.id)) {
                continue;
            }
            match &graphic.item {
                Item::Shape { shape, .. } => {
                    let shape = &shape.to_mm();
                    t.vertices.extend(shape.vertices());
                    match shape {
                        Shape::Circle { center, .. } | Shape::Arc { center, .. } => {
                            t.centers.push(*center);
                        }
                        Shape::Rect { a, b, .. } => {
                            t.centers.push([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
                        }
                        _ => {}
                    }
                    let ring = shape.vertices();
                    let closed = matches!(shape, Shape::Rect { .. } | Shape::Polygon { .. });
                    if !matches!(shape, Shape::Circle { .. } | Shape::Arc { .. }) {
                        let n = ring.len();
                        let edges = if closed { n } else { n.saturating_sub(1) };
                        for i in 0..edges {
                            let (a, b) = (ring[i], ring[(i + 1) % n]);
                            t.midpoints.push([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
                        }
                    }
                    t.curves.push((shape.coarse_bounds(0.0), shape.clone()));
                }
                // Hidden text is not drawn, so it offers no snap.
                Item::Text(text) if text.visible => t.vertices.push(text.at.to_mm_f64()),
                Item::Text(_) => {}
            }
        }
        for pin in &unit.pins {
            if !skip(&super::Selected::Pin(pin.terminal)) {
                t.pin_ends.push(pin.position_mm());
            }
        }
        t.vertices.extend_from_slice(pending);
        t
    }
}

fn nearest(points: &[Point], p: Point, tol: f64) -> Option<Point> {
    points
        .iter()
        .map(|q| (distance(*q, p), *q))
        .filter(|(d, _)| *d <= tol)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, q)| q)
}

/// The point of `shape` nearest `p`, through its flattened outline.
fn on_curve(shape: &Shape, p: Point, chord: f64) -> Option<Point> {
    let mut ring = shape.flatten(chord);
    if shape.is_closed()
        && let Some(first) = ring.first().copied()
    {
        ring.push(first);
    }
    ring.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let ab = [b[0] - a[0], b[1] - a[1]];
            let len2 = ab[0] * ab[0] + ab[1] * ab[1];
            let t = if len2 > 0.0 {
                (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            [a[0] + ab[0] * t, a[1] + ab[1] * t]
        })
        .min_by(|a, b| distance(*a, p).total_cmp(&distance(*b, p)))
}

/// Round to the grid of pitch `step` through the origin.
pub fn to_grid(p: Point, step: f64) -> Point {
    if step > 0.0 && step.is_finite() {
        p.map(|v| (v / step).round() * step)
    } else {
        p
    }
}

/// Snap `raw` (mm) with `tol` mm of reach, `grid` mm pitch. With
/// `enabled` false the point stays where it is.
pub fn snap(raw: Point, tol: f64, grid: f64, targets: &Targets, enabled: bool) -> Snap {
    let plain = |at, kind| Snap {
        at,
        kind,
        guides: vec![],
    };
    if !enabled {
        return plain(raw, Kind::Free);
    }
    for (points, kind) in [
        (&targets.vertices, Kind::Vertex),
        (&targets.pin_ends, Kind::PinEnd),
        (&targets.midpoints, Kind::Midpoint),
        (&targets.centers, Kind::Center),
    ] {
        if let Some(q) = nearest(points, raw, tol) {
            return plain(q, kind);
        }
    }
    if distance(raw, [0.0, 0.0]) <= tol {
        return plain([0.0, 0.0], Kind::Origin);
    }
    let curve = targets
        .curves
        .iter()
        .filter(|(b, _)| b.inflated(tol).contains(raw))
        .filter(|(_, s)| s.distance(raw, false) <= tol)
        .filter_map(|(_, s)| on_curve(s, raw, (tol / 20.0).max(1e-4)))
        .min_by(|a, b| distance(*a, raw).total_cmp(&distance(*b, raw)));
    if let Some(q) = curve {
        return plain(q, Kind::OnCurve);
    }
    // Line up with a point horizontally or vertically; the other axis
    // keeps to the grid.
    let anchors = targets
        .vertices
        .iter()
        .chain(&targets.pin_ends)
        .chain(&targets.centers);
    let mut best_x: Option<(f64, Point)> = None;
    let mut best_y: Option<(f64, Point)> = None;
    for q in anchors {
        let dx = (q[0] - raw[0]).abs();
        let dy = (q[1] - raw[1]).abs();
        if dx <= tol && best_x.is_none_or(|(d, _)| dx < d) {
            best_x = Some((dx, *q));
        }
        if dy <= tol && best_y.is_none_or(|(d, _)| dy < d) {
            best_y = Some((dy, *q));
        }
    }
    let grid_point = to_grid(raw, grid);
    if best_x.is_none() && best_y.is_none() {
        return plain(grid_point, Kind::Grid);
    }
    let at = [
        best_x.map_or(grid_point[0], |(_, q)| q[0]),
        best_y.map_or(grid_point[1], |(_, q)| q[1]),
    ];
    let guides = best_x
        .into_iter()
        .chain(best_y)
        .map(|(_, q)| [q, at])
        .filter(|[q, at]| distance(*q, *at) > 1e-9)
        .collect();
    Snap {
        at,
        kind: Kind::Aligned,
        guides,
    }
}

/// Whether segment `a`–`b` reaches into the box `min`–`max`.
pub fn segment_touches_box(a: Point, b: Point, min: Point, max: Point) -> bool {
    let inside = |p: Point| (min[0]..=max[0]).contains(&p[0]) && (min[1]..=max[1]).contains(&p[1]);
    if inside(a) || inside(b) {
        return true;
    }
    let corners = [min, [max[0], min[1]], max, [min[0], max[1]]];
    (0..4).any(|i| {
        let (c, d) = (corners[i], corners[(i + 1) % 4]);
        pcbcraft_core::geometry::segments_cross(a, b, c, d)
            || segment_distance(c, a, b) <= 1e-12
            || segment_distance(d, a, b) <= 1e-12
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets() -> Targets {
        Targets {
            vertices: vec![[0.0, 0.0], [10.0, 0.0]],
            pin_ends: vec![[-5.0, 2.5]],
            midpoints: vec![[5.0, 0.0]],
            centers: vec![[5.0, 5.0]],
            curves: vec![{
                let line = Shape::Line {
                    a: [0.0, 0.0],
                    b: [10.0, 0.0],
                };
                (line.coarse_bounds(0.0), line)
            }],
        }
    }

    #[test]
    fn snaps_follow_the_priority_order_then_alignment_then_grid() {
        let t = targets();
        let at = |p| snap(p, 0.5, 2.5, &t, true);
        assert_eq!(at([9.8, 0.2]).kind, Kind::Vertex);
        assert_eq!(at([-5.2, 2.4]).kind, Kind::PinEnd);
        assert_eq!(at([5.1, 0.1]).at, [5.0, 0.0]);
        assert_eq!(at([5.1, 0.1]).kind, Kind::Midpoint);
        assert_eq!(at([5.2, 5.2]).kind, Kind::Center);
        let on = at([7.3, 0.3]);
        assert_eq!(on.kind, Kind::OnCurve);
        assert!((on.at[1]).abs() < 1e-9 && (on.at[0] - 7.3).abs() < 1e-9);
        // Lined up under a vertex: x from the vertex, y on the grid.
        let aligned = at([10.3, 6.1]);
        assert_eq!(aligned.kind, Kind::Aligned);
        assert_eq!(aligned.at, [10.0, 5.0]);
        assert_eq!(aligned.guides, vec![[[10.0, 0.0], [10.0, 5.0]]]);
        assert_eq!(at([21.1, 13.9]).at, [20.0, 15.0]);
        assert_eq!(at([21.1, 13.9]).kind, Kind::Grid);
        // Suppressed: the raw point.
        let free = snap([9.8, 0.2], 0.5, 2.5, &t, false);
        assert_eq!((free.at, free.kind), ([9.8, 0.2], Kind::Free));
    }

    #[test]
    fn segments_touch_boxes_they_enter_or_cross() {
        let (min, max) = ([0.0, 0.0], [2.0, 2.0]);
        assert!(segment_touches_box([-1.0, 1.0], [3.0, 1.0], min, max));
        assert!(segment_touches_box([1.0, 1.0], [5.0, 5.0], min, max));
        assert!(!segment_touches_box([3.0, 0.0], [5.0, 5.0], min, max));
    }
}
