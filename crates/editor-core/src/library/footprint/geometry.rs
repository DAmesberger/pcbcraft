//! Exact pad, hole and aperture outlines flattened at an explicit chord
//! tolerance, for checks and display. The stored data stays exact.
use super::{Hole, HoleShape, Outline, Pad, PadShape, Segment};
use crate::geometry::{Point, Shape, cos_sin, distance, polygon_contains, segment_distance};

/// Chord tolerance the publish rules flatten curves at, in mm.
pub const CHECK_CHORD_MM: f64 = 1e-3;

fn arc(center: Point, radius: f64, start_deg: f64, sweep_deg: f64, tol: f64) -> Vec<Point> {
    Shape::Arc {
        center,
        radius,
        start_deg,
        sweep_deg,
    }
    .flatten(tol)
}

fn push(out: &mut Vec<Point>, p: Point) {
    if out.last().is_none_or(|q| distance(*q, p) > 1e-12) {
        out.push(p);
    }
}

fn close(mut ring: Vec<Point>) -> Vec<Point> {
    while ring.len() > 1
        && ring
            .first()
            .zip(ring.last())
            .is_some_and(|(a, b)| distance(*a, *b) <= 1e-12)
    {
        ring.pop();
    }
    ring
}

/// A stadium of overall `size`, centred on the origin, counter-clockwise.
fn stadium(size: Point, tol: f64) -> Vec<Point> {
    let [w, h] = size;
    let mut out = Vec::new();
    if w >= h {
        let (r, c) = (h / 2.0, (w - h) / 2.0);
        for p in arc([c, 0.0], r, -90.0, 180.0, tol) {
            push(&mut out, p);
        }
        for p in arc([-c, 0.0], r, 90.0, 180.0, tol) {
            push(&mut out, p);
        }
    } else {
        let (r, c) = (w / 2.0, (h - w) / 2.0);
        for p in arc([0.0, c], r, 0.0, 180.0, tol) {
            push(&mut out, p);
        }
        for p in arc([0.0, -c], r, 180.0, 180.0, tol) {
            push(&mut out, p);
        }
    }
    close(out)
}

fn rotate(p: Point, deg: f64) -> Point {
    let (c, s) = cos_sin(deg);
    [c * p[0] - s * p[1], s * p[0] + c * p[1]]
}

impl PadShape {
    /// The corner radius of a rounded rectangle after the ratio, the cap and
    /// the half short side are applied; 0 for other shapes.
    pub fn corner_radius(&self) -> f64 {
        match self {
            Self::RoundRect {
                size,
                radius_ratio,
                max_radius,
                ..
            } => {
                let short = size[0].min(size[1]);
                let r = (radius_ratio * short).clamp(0.0, short / 2.0);
                max_radius.map_or(r, |m| r.min(m.max(0.0)))
            }
            _ => 0.0,
        }
    }

    /// The copper outline in the pad frame as a counter-clockwise ring
    /// within `tol` mm (curves are inscribed).
    pub fn flatten(&self, tol: f64) -> Vec<Point> {
        match self {
            Self::Rect { size } => {
                let [hw, hh] = [size[0] / 2.0, size[1] / 2.0];
                vec![[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]]
            }
            Self::RoundRect { size, corners, .. } => {
                let r = self.corner_radius();
                let [hw, hh] = [size[0] / 2.0, size[1] / 2.0];
                let mut out = Vec::new();
                for (rounded, corner, start) in [
                    (corners.bottom_left, [-hw, -hh], 180.0),
                    (corners.bottom_right, [hw, -hh], 270.0),
                    (corners.top_right, [hw, hh], 0.0),
                    (corners.top_left, [-hw, hh], 90.0),
                ] {
                    if rounded && r > 0.0 {
                        let centre = [
                            corner[0] - r * corner[0].signum(),
                            corner[1] - r * corner[1].signum(),
                        ];
                        for p in arc(centre, r, start, 90.0, tol) {
                            push(&mut out, p);
                        }
                    } else {
                        push(&mut out, corner);
                    }
                }
                close(out)
            }
            Self::Circle { diameter } => Shape::Circle {
                center: [0.0, 0.0],
                radius: diameter / 2.0,
            }
            .flatten(tol),
            Self::Oblong { size } => stadium(*size, tol),
            Self::Custom { outline } => outline.flatten(tol),
        }
    }
}

impl Outline {
    /// Whether the last segment ends at `start`.
    pub fn closed(&self) -> bool {
        self.segments
            .last()
            .is_some_and(|s| distance(s.to(), self.start) <= 1e-9)
    }

    /// The outline as a ring within `tol` mm, without repeating `start`.
    pub fn flatten(&self, tol: f64) -> Vec<Point> {
        let mut out = vec![self.start];
        let mut from = self.start;
        for segment in &self.segments {
            match *segment {
                Segment::Line { to } => push(&mut out, to),
                Segment::Arc { to, bulge } => {
                    let c = distance(from, to);
                    if bulge == 0.0 || c <= 1e-12 {
                        push(&mut out, to);
                    } else {
                        let sweep = 4.0 * bulge.atan();
                        let radius = c / (2.0 * (sweep / 2.0).sin().abs());
                        let mid = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
                        let normal = [-(to[1] - from[1]) / c, (to[0] - from[0]) / c];
                        let k = (c / 2.0) * (1.0 - bulge * bulge) / (2.0 * bulge);
                        let centre = [mid[0] + normal[0] * k, mid[1] + normal[1] * k];
                        let start = (from[1] - centre[1]).atan2(from[0] - centre[0]);
                        for p in arc(centre, radius, start.to_degrees(), sweep.to_degrees(), tol)
                            .into_iter()
                            .skip(1)
                        {
                            push(&mut out, p);
                        }
                        // Land exactly on the stored end point.
                        if let Some(last) = out.last_mut() {
                            *last = to;
                        }
                    }
                }
            }
            from = segment.to();
        }
        close(out)
    }
}

impl HoleShape {
    /// The drill outline centred on the hole, counter-clockwise.
    pub fn flatten(&self, tol: f64) -> Vec<Point> {
        match *self {
            Self::Round { diameter } => Shape::Circle {
                center: [0.0, 0.0],
                radius: diameter / 2.0,
            }
            .flatten(tol),
            Self::Slot {
                width,
                length,
                angle_deg,
            } => stadium([length.max(width), width], tol)
                .into_iter()
                .map(|p| rotate(p, angle_deg))
                .collect(),
        }
    }
}

impl Hole {
    /// The drill outline in the pad frame.
    pub fn ring(&self, tol: f64) -> Vec<Point> {
        self.shape
            .flatten(tol)
            .into_iter()
            .map(|p| [p[0] + self.offset[0], p[1] + self.offset[1]])
            .collect()
    }
}

impl Pad {
    /// A pad-frame point in the footprint frame.
    pub fn to_footprint(&self, p: Point) -> Point {
        let q = rotate(p, self.rotation_deg);
        [q[0] + self.position[0], q[1] + self.position[1]]
    }

    /// The copper outline in the footprint frame, within `tol` mm.
    pub fn outline(&self, tol: f64) -> Vec<Point> {
        self.shape
            .flatten(tol)
            .into_iter()
            .map(|p| self.to_footprint(p))
            .collect()
    }
}

/// Distance from `p` to a ring's boundary.
pub(super) fn ring_distance(ring: &[Point], p: Point) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| segment_distance(p, ring[i], ring[(i + 1) % n]))
        .fold(f64::INFINITY, f64::min)
}

/// Whether every point of `inner` lies in `outer` (or within `tol` of its
/// boundary) and no edges cross, so a non-convex outer ring cannot be
/// bridged.
pub(super) fn ring_within(inner: &[Point], outer: &[Point], tol: f64) -> bool {
    if outer.len() < 3 {
        return false;
    }
    let inside = inner
        .iter()
        .all(|p| polygon_contains(outer, *p) || ring_distance(outer, *p) <= tol);
    if !inside {
        return false;
    }
    let edges = |r: &[Point]| {
        let n = r.len();
        (0..n).map(|i| (r[i], r[(i + 1) % n])).collect::<Vec<_>>()
    };
    let (a, b) = (edges(inner), edges(outer));
    !a.iter().any(|(p, q)| {
        b.iter()
            .any(|(r, s)| crate::geometry::segments_cross(*p, *q, *r, *s))
    })
}

/// The smallest boundary distance between two rings, or 0 when they
/// overlap or touch.
pub(super) fn ring_gap(a: &[Point], b: &[Point]) -> f64 {
    if crate::geometry::polygons_overlap(a, b) {
        return 0.0;
    }
    let mut closed = a.to_vec();
    if let Some(first) = a.first() {
        closed.push(*first);
    }
    crate::geometry::polyline_polygon_distance(&closed, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::signed_area2;
    use crate::library::CornerMask;
    use std::f64::consts::PI;

    fn area(ring: &[Point]) -> f64 {
        signed_area2(ring) / 2.0
    }

    fn close_to(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn flattened_shapes_are_counter_clockwise_with_their_exact_area() {
        let tol = 1e-4;
        let cases = [
            (PadShape::Rect { size: [2.0, 1.0] }, 2.0),
            (PadShape::Circle { diameter: 2.0 }, PI),
            // Two semicircles of radius 0.5 and a 1 × 1 middle.
            (PadShape::Oblong { size: [2.0, 1.0] }, 1.0 + PI * 0.25),
            (PadShape::Oblong { size: [1.0, 3.0] }, 2.0 + PI * 0.25),
            // Ratio 0.25 of the 1 mm short side, every corner.
            (
                PadShape::RoundRect {
                    size: [2.0, 1.0],
                    radius_ratio: 0.25,
                    max_radius: None,
                    corners: CornerMask::ALL,
                },
                2.0 - (4.0 - PI) * 0.0625,
            ),
            // The cap wins over the ratio; only two corners (a D-shape).
            (
                PadShape::RoundRect {
                    size: [2.0, 1.0],
                    radius_ratio: 0.5,
                    max_radius: Some(0.1),
                    corners: CornerMask {
                        top_left: true,
                        top_right: false,
                        bottom_right: false,
                        bottom_left: true,
                    },
                },
                2.0 - (4.0 - PI) * 0.01 / 2.0,
            ),
            // A half disc: a line, then a bulge-1 arc back to the start.
            (
                PadShape::Custom {
                    outline: Outline {
                        start: [-1.0, 0.0],
                        segments: vec![
                            Segment::Line { to: [1.0, 0.0] },
                            Segment::Arc {
                                to: [-1.0, 0.0],
                                bulge: 1.0,
                            },
                        ],
                    },
                },
                PI / 2.0,
            ),
            // A quarter-turn arc (bulge tan 22.5°) closing a 1 mm square's
            // corner outward: square plus a circular segment.
            (
                PadShape::Custom {
                    outline: Outline {
                        start: [0.0, 0.0],
                        segments: vec![
                            Segment::Line { to: [1.0, 0.0] },
                            Segment::Arc {
                                to: [0.0, 1.0],
                                bulge: (PI / 8.0).tan(),
                            },
                            Segment::Line { to: [0.0, 0.0] },
                        ],
                    },
                },
                // Radius 1 about the origin: a quarter disc.
                PI / 4.0,
            ),
        ];
        for (shape, expected) in cases {
            let ring = shape.flatten(tol);
            assert!(
                close_to(area(&ring), expected, 1e-3),
                "{shape:?}: {} vs {expected}",
                area(&ring)
            );
            assert!(
                !crate::geometry::polygon_self_intersects(&ring),
                "{shape:?}"
            );
        }
    }

    #[test]
    fn the_corner_radius_clamps_to_ratio_cap_and_half_side() {
        let rr = |ratio: f64, cap: Option<f64>| PadShape::RoundRect {
            size: [2.0, 1.0],
            radius_ratio: ratio,
            max_radius: cap,
            corners: CornerMask::ALL,
        };
        assert!(close_to(rr(0.25, None).corner_radius(), 0.25, 1e-12));
        assert!(close_to(rr(0.25, Some(0.1)).corner_radius(), 0.1, 1e-12));
        assert!(close_to(rr(0.9, None).corner_radius(), 0.5, 1e-12));
        assert!(close_to(rr(-1.0, None).corner_radius(), 0.0, 1e-12));
        assert_eq!(PadShape::Rect { size: [1.0, 1.0] }.corner_radius(), 0.0);
    }

    #[test]
    fn slots_extend_along_their_angle_and_pads_rotate_about_their_origin() {
        let slot = HoleShape::Slot {
            width: 0.5,
            length: 2.0,
            angle_deg: 90.0,
        };
        let ring = slot.flatten(1e-4);
        let b = crate::geometry::Bounds::of(ring.iter().copied());
        let Some(b) = b else {
            return assert!(b.is_some());
        };
        assert!(close_to(b.size()[0], 0.5, 1e-3) && close_to(b.size()[1], 2.0, 1e-3));
        let pad = Pad {
            rotation_deg: 90.0,
            ..Pad::smd_rect(
                crate::ids::PadId(1),
                crate::ids::PackageTerminalId(1),
                "1",
                [3.0, 1.0],
                [2.0, 1.0],
            )
        };
        let outline = pad.outline(1e-3);
        let Some(b) = crate::geometry::Bounds::of(outline.iter().copied()) else {
            return assert!(!outline.is_empty());
        };
        assert!(close_to(b.size()[0], 1.0, 1e-9) && close_to(b.size()[1], 2.0, 1e-9));
        assert!(close_to(b.center()[0], 3.0, 1e-9) && close_to(b.center()[1], 1.0, 1e-9));
        assert_eq!(b, pad.bounds(), "matches the stored-extent bounds");
    }

    #[test]
    fn containment_and_gaps_between_rings() {
        let land = PadShape::Circle { diameter: 2.0 }.flatten(1e-3);
        let inner = HoleShape::Round { diameter: 1.0 }.flatten(1e-3);
        assert!(ring_within(&inner, &land, 2e-3));
        let moved: Vec<Point> = inner.iter().map(|p| [p[0] + 0.6, p[1]]).collect();
        assert!(!ring_within(&moved, &land, 2e-3));
        let far: Vec<Point> = land.iter().map(|p| [p[0] + 2.5, p[1]]).collect();
        assert!(close_to(ring_gap(&land, &far), 0.5, 2e-3));
        assert_eq!(ring_gap(&land, &inner), 0.0, "nested rings overlap");
    }
}
