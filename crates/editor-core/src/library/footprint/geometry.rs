//! The one source of pad, hole and aperture outlines: every [`PadShape`]
//! and [`HoleShape`] as an exact core [`Shape`] (rounded corners keep their
//! radii; only custom bulge arcs are flattened, at an explicit chord
//! tolerance), grown or shrunk on request, plus the mask and paste openings
//! a pad's rules resolve to. Checks, the PCB view and the Footprint tab all
//! read these; the stored data stays exact.
use super::{
    Hole, HoleShape, MaskRule, Outline, Pad, PadDefaults, PadKind, PadShape, PasteRule, Segment,
};
use crate::geometry::{
    Point, Shape, Transform2, distance, polygon_contains, segment_distance, signed_area2,
};

/// Chord tolerance the publish rules flatten curves at, in mm.
pub const CHECK_CHORD_MM: f64 = 1e-3;
/// Chord error, in mm, at which custom pad arcs are flattened for display,
/// picking and display-only 3D layers.
pub const PAD_ARC_TOLERANCE_MM: f64 = 0.002;

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

/// A `size` box centred on the origin, grown by `by` on every side, with
/// `radius` on the corners `rounded` selects (counter-clockwise from the
/// bottom left). Growing is a true offset: every point within `by` of the
/// box, so a sharp corner grows round with radius `by` and a rounded one
/// grows with the sides (capped at half the short side); shrinking keeps
/// sharp corners sharp. The native mask openings follow the same rule.
/// `None` when it vanishes.
fn rounded_box(size: Point, radius: f64, rounded: [bool; 4], by: f64) -> Option<Shape> {
    let (w, h) = (size[0] + 2.0 * by, size[1] + 2.0 * by);
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    let cap = 0.5 * w.min(h);
    let round = if radius > 0.0 {
        (radius + by).clamp(0.0, cap)
    } else {
        0.0
    };
    let sharp = by.clamp(0.0, cap);
    let (x, y) = (w / 2.0, h / 2.0);
    let radii = rounded.map(|on| if on { round } else { sharp });
    Some(Shape::Polygon {
        points: vec![[-x, -y], [x, -y], [x, y], [-x, y]],
        radii: if radii.iter().all(|r| *r <= 0.0) {
            vec![]
        } else {
            radii.to_vec()
        },
    })
}

/// A polygon offset by `by` (outward for positive), with mitred corners
/// limited to four times the offset. `None` when it collapses.
pub fn offset_polygon(points: &[Point], by: f64) -> Option<Vec<Point>> {
    let n = points.len();
    if n < 3 {
        return None;
    }
    let area = signed_area2(points);
    let ccw = if area >= 0.0 { 1.0 } else { -1.0 };
    let normal = |a: Point, b: Point| {
        let d = [b[0] - a[0], b[1] - a[1]];
        let l = d[0].hypot(d[1]).max(1e-9);
        // Outward is right of a counter-clockwise edge.
        [ccw * d[1] / l, -ccw * d[0] / l]
    };
    let out: Vec<Point> = (0..n)
        .map(|i| {
            let p = points[i];
            let n0 = normal(points[(i + n - 1) % n], p);
            let n1 = normal(p, points[(i + 1) % n]);
            let k = (1.0 + n0[0] * n1[0] + n0[1] * n1[1]).max(0.125);
            [
                p[0] + by * (n0[0] + n1[0]) / k,
                p[1] + by * (n0[1] + n1[1]) / k,
            ]
        })
        .collect();
    (signed_area2(&out) * area > 1e-9).then_some(out)
}

/// A ring offset by `by` mm (negative shrinks) as a true offset: round
/// joins whose vertices lie on the true arcs, within `tol`. When the offset
/// splits a ring (a shrunk waist), the largest piece is kept; a grown ring
/// keeps its outer boundary. `None` when nothing is left.
fn true_offset(mut ring: Vec<Point>, by: f64, tol: f64) -> Option<Vec<Point>> {
    use i_overlay::mesh::float::outline::offset::OutlineOffset;
    use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};
    if ring.len() < 3 {
        return None;
    }
    if signed_area2(&ring) < 0.0 {
        ring.reverse();
    }
    let r = by.abs().max(tol);
    let step = 2.0 * (1.0 - (tol / r).min(1.0)).acos().max(0.01);
    let style = OutlineStyle::new(by).line_join(LineJoin::Round(step));
    let shapes: Vec<Vec<Vec<Point>>> = vec![vec![ring]].outline(&style);
    shapes
        .into_iter()
        .filter_map(|shape| shape.into_iter().next())
        .map(|outer| (signed_area2(&outer).abs(), outer))
        .filter(|(area, _)| area.is_finite() && *area > 1e-12)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, mut outer)| {
            if signed_area2(&outer) < 0.0 {
                outer.reverse();
            }
            outer
        })
}

/// The circular arc from `a` to `b` with `bulge = tan(sweep / 4)`
/// (positive is counter-clockwise), or `None` for a straight or degenerate
/// segment.
pub fn bulge_arc(a: Point, b: Point, bulge: f64) -> Option<Shape> {
    let chord = [b[0] - a[0], b[1] - a[1]];
    let c = chord[0].hypot(chord[1]);
    if c.is_nan() || c <= 1e-12 || !bulge.is_finite() || bulge.abs() <= 1e-12 {
        return None;
    }
    // The centre sits left of the chord for a counter-clockwise arc below
    // a half turn.
    let k = (1.0 - bulge * bulge) / (4.0 * bulge);
    let center = [
        0.5 * (a[0] + b[0]) - chord[1] * k,
        0.5 * (a[1] + b[1]) + chord[0] * k,
    ];
    Some(Shape::Arc {
        center,
        radius: c * (1.0 + bulge * bulge) / (4.0 * bulge.abs()),
        start_deg: (a[1] - center[1]).atan2(a[0] - center[0]).to_degrees(),
        sweep_deg: 4.0 * bulge.atan().to_degrees(),
    })
}

impl PadShape {
    /// The corner radius of a rounded rectangle after the ratio, the cap and
    /// the half short side are applied; 0 for other shapes.
    pub fn corner_radius(&self) -> f64 {
        match self {
            Self::RoundRect {
                size,
                radius_ratio,
                max_radius_nm,
                ..
            } => {
                let [w, h] = size.to_mm_f64();
                let short = w.min(h).max(0.0);
                let r = (radius_ratio * short).clamp(0.0, short / 2.0);
                max_radius_nm.map_or(r, |m| r.min(m.to_mm_f64().max(0.0)))
            }
            _ => 0.0,
        }
    }

    /// The shape grown by `by` mm on every side (negative shrinks) as a core
    /// shape in the pad frame, centred on the origin: a true offset, so a
    /// grown sharp corner is rounded by `by`. Rounded corners stay exact; a
    /// custom outline's arcs and grown corners are flattened within `tol`.
    /// `None` when it vanishes.
    pub fn grown(&self, by: f64, tol: f64) -> Option<Shape> {
        match self {
            Self::Rect { size } => rounded_box(size.to_mm_f64(), 0.0, [false; 4], by),
            Self::RoundRect { size, corners, .. } => rounded_box(
                size.to_mm_f64(),
                self.corner_radius(),
                [
                    corners.bottom_left,
                    corners.bottom_right,
                    corners.top_right,
                    corners.top_left,
                ],
                by,
            ),
            Self::Circle { diameter_nm } => {
                let radius = diameter_nm.to_mm_f64() / 2.0 + by;
                (radius > 0.0).then_some(Shape::Circle {
                    center: [0.0, 0.0],
                    radius,
                })
            }
            Self::Oblong { size } => {
                let size = size.to_mm_f64();
                rounded_box(size, size[0].min(size[1]) / 2.0, [true; 4], by)
            }
            Self::Custom { outline } => {
                let ring = outline.flatten(tol);
                let ring = if by == 0.0 {
                    ring
                } else {
                    true_offset(ring, by, tol)?
                };
                (ring.len() >= 3).then(|| Shape::polygon(ring))
            }
        }
    }

    /// The land outline in the pad frame (custom arcs within `tol`). A
    /// degenerate shape gives an empty polygon.
    pub fn outline(&self, tol: f64) -> Shape {
        self.grown(0.0, tol)
            .unwrap_or_else(|| Shape::polygon(Vec::new()))
    }

    /// The copper outline in the pad frame as a counter-clockwise ring
    /// within `tol` mm (curves are inscribed).
    pub fn flatten(&self, tol: f64) -> Vec<Point> {
        self.outline(tol).flatten(tol)
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
            let to = segment.to();
            if let Segment::Arc { bulge, .. } = *segment
                && let Some(arc) = bulge_arc(from, to, bulge)
            {
                for p in arc.flatten(tol).into_iter().skip(1) {
                    push(&mut out, p);
                }
                // Land exactly on the stored end point.
                if let Some(last) = out.last_mut() {
                    *last = to;
                }
            } else {
                push(&mut out, to);
            }
            from = to;
        }
        close(out)
    }
}

impl HoleShape {
    /// The drill outline centred on the hole: a circle, or a stadium of
    /// `length` overall along `angle_deg` for a slot.
    pub fn shape(&self) -> Shape {
        match *self {
            Self::Round { diameter_nm } => Shape::Circle {
                center: [0.0, 0.0],
                radius: diameter_nm.to_mm_f64() / 2.0,
            },
            Self::Slot {
                width_nm,
                length_nm,
                angle_mdeg,
            } => {
                let (width, length) = (width_nm.to_mm_f64(), length_nm.to_mm_f64());
                rounded_box([length.max(width), width], width / 2.0, [true; 4], 0.0)
                    .unwrap_or_else(|| Shape::polygon(Vec::new()))
                    .transformed(&Transform2 {
                        translate: [0.0, 0.0],
                        rotation_deg: angle_mdeg.to_degrees_f64(),
                        mirror: false,
                    })
            }
        }
    }

    /// The drill outline as a counter-clockwise ring within `tol`.
    pub fn flatten(&self, tol: f64) -> Vec<Point> {
        self.shape().flatten(tol)
    }
}

impl Hole {
    /// The drill outline in the pad frame.
    pub fn shape(&self) -> Shape {
        self.shape.shape().translated(self.offset())
    }

    /// The drill outline in the pad frame, as a ring within `tol`.
    pub fn ring(&self, tol: f64) -> Vec<Point> {
        self.shape().flatten(tol)
    }
}

/// Whether a pad kind lands a package terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalRule {
    Required,
    Forbidden,
    Optional,
}

impl PadKind {
    /// Whether this kind must, must not or may land a terminal (what
    /// `footprint.terminal_required` and `terminal_forbidden` enforce).
    pub fn terminal_rule(self) -> TerminalRule {
        match self {
            Self::Smd | Self::PlatedThrough => TerminalRule::Required,
            Self::NonPlatedHole | Self::Fiducial => TerminalRule::Forbidden,
            Self::Mechanical => TerminalRule::Optional,
        }
    }
}

impl Pad {
    /// Pad frame to footprint frame: rotate about the pad origin, then move.
    pub fn frame(&self) -> Transform2 {
        Transform2 {
            translate: self.position(),
            rotation_deg: self.rotation_deg(),
            mirror: false,
        }
    }

    /// A pad-frame point in the footprint frame.
    pub fn to_footprint(&self, p: Point) -> Point {
        self.frame().apply(p)
    }

    /// The copper outline in the footprint frame, as a ring within `tol`.
    pub fn outline(&self, tol: f64) -> Vec<Point> {
        self.shape
            .flatten(tol)
            .into_iter()
            .map(|p| self.to_footprint(p))
            .collect()
    }

    /// The land in the footprint frame (custom arcs within `tol`).
    pub fn land(&self, tol: f64) -> Shape {
        self.shape.outline(tol).transformed(&self.frame())
    }

    /// The drilled hole in the footprint frame.
    pub fn hole_shape(&self) -> Option<Shape> {
        Some(self.hole.as_ref()?.shape().transformed(&self.frame()))
    }

    /// How far the solder-mask opening grows past the land, or `None` when
    /// the pad is tented.
    pub fn mask_expansion(&self, defaults: &PadDefaults) -> Option<f64> {
        match self.mask {
            MaskRule::Inherit => Some(defaults.mask_expansion_mm()),
            MaskRule::Tented => None,
            MaskRule::Expansion(e) => Some(e.to_mm_f64()),
        }
    }

    /// The solder-mask opening in the footprint frame; `None` when tented
    /// or shrunk to nothing.
    pub fn mask_opening(&self, defaults: &PadDefaults, tol: f64) -> Option<Shape> {
        let by = self.mask_expansion(defaults)?;
        Some(self.shape.grown(by, tol)?.transformed(&self.frame()))
    }

    /// The paste openings in the footprint frame. Inherited paste is none
    /// for hole kinds and fiducials.
    pub fn paste_openings(&self, defaults: &PadDefaults, tol: f64) -> Vec<Shape> {
        let frame = self.frame();
        let shrink = |by: f64| {
            self.shape
                .grown(-by, tol)
                .map(|s| s.transformed(&frame))
                .into_iter()
                .collect()
        };
        match &self.paste {
            PasteRule::Inherit => match self.kind {
                PadKind::Smd | PadKind::Mechanical if self.hole.is_none() => {
                    shrink(defaults.paste_reduction_mm())
                }
                _ => Vec::new(),
            },
            PasteRule::None => Vec::new(),
            PasteRule::Reduction(r) => shrink(r.to_mm_f64()),
            PasteRule::Windows(windows) => windows
                .iter()
                .map(|w| w.shape.to_mm().transformed(&frame))
                .collect(),
        }
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
    use crate::tests::support::{mdeg, nm, pt};
    use std::f64::consts::PI;

    fn area(ring: &[Point]) -> f64 {
        signed_area2(ring) / 2.0
    }

    fn close_to(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    impl PadShape {
        fn flatten_grown(&self, by: f64, tol: f64) -> Vec<Point> {
            self.grown(by, tol)
                .map(|s| s.flatten(tol))
                .unwrap_or_default()
        }
    }

    #[test]
    fn flattened_shapes_are_counter_clockwise_with_their_exact_area() {
        let tol = 1e-4;
        let cases = [
            (
                PadShape::Rect {
                    size: pt([2.0, 1.0]),
                },
                2.0,
            ),
            (
                PadShape::Circle {
                    diameter_nm: nm(2.0),
                },
                PI,
            ),
            // Two semicircles of radius 0.5 and a 1 × 1 middle.
            (
                PadShape::Oblong {
                    size: pt([2.0, 1.0]),
                },
                1.0 + PI * 0.25,
            ),
            (
                PadShape::Oblong {
                    size: pt([1.0, 3.0]),
                },
                2.0 + PI * 0.25,
            ),
            // Ratio 0.25 of the 1 mm short side, every corner.
            (
                PadShape::RoundRect {
                    size: pt([2.0, 1.0]),
                    radius_ratio: 0.25,
                    max_radius_nm: None,
                    corners: CornerMask::ALL,
                },
                2.0 - (4.0 - PI) * 0.0625,
            ),
            // The cap wins over the ratio; only two corners (a D-shape).
            (
                PadShape::RoundRect {
                    size: pt([2.0, 1.0]),
                    radius_ratio: 0.5,
                    max_radius_nm: Some(nm(0.1)),
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

    /// Openings are true offsets: every point within `by` of the land. A
    /// grown sharp corner is rounded by `by` (a convex land of area A and
    /// perimeter P grows to A + P·by + π·by²); a shrunk one stays sharp.
    #[test]
    fn grown_openings_are_true_offsets_with_round_corners() {
        let tol = 1e-5;
        let by = 0.1;
        let rect = PadShape::Rect {
            size: pt([2.0, 1.0]),
        };
        let Some(Shape::Polygon { radii, .. }) = rect.grown(by, tol) else {
            return assert!(rect.grown(by, tol).is_some());
        };
        assert_eq!(radii, vec![by; 4]);
        let grown = area(&rect.grown(by, tol).map(|s| s.flatten(tol)).unwrap_or_default());
        assert!(close_to(grown, 2.0 + 6.0 * by + PI * by * by, 1e-4), "{grown}");
        let shrunk = rect.grown(-by, tol);
        assert!(
            matches!(&shrunk, Some(Shape::Polygon { radii, .. }) if radii.is_empty()),
            "{shrunk:?}"
        );
        // A D-shaped round rect: its sharp corners round by `by`, its
        // rounded ones by their radius plus `by`.
        let d = PadShape::RoundRect {
            size: pt([2.0, 1.0]),
            radius_ratio: 0.25,
            max_radius_nm: None,
            corners: CornerMask {
                top_left: true,
                top_right: false,
                bottom_right: false,
                bottom_left: true,
            },
        };
        let Some(Shape::Polygon { radii, .. }) = d.grown(by, tol) else {
            return assert!(d.grown(by, tol).is_some());
        };
        assert_eq!(radii, vec![0.35, by, by, 0.35]);
        // A custom triangle: the same area rule, its vertices round.
        let triangle = PadShape::Custom {
            outline: Outline {
                start: [0.0, 0.0],
                segments: vec![
                    Segment::Line { to: [2.0, 0.0] },
                    Segment::Line { to: [0.0, 1.0] },
                    Segment::Line { to: [0.0, 0.0] },
                ],
            },
        };
        let perimeter = 3.0 + 5f64.sqrt();
        let ring = triangle.flatten_grown(by, tol);
        assert!(
            close_to(area(&ring), 1.0 + perimeter * by + PI * by * by, 1e-3),
            "{}",
            area(&ring)
        );
        assert!(!crate::geometry::polygon_self_intersects(&ring));
        // Shrunk, a convex land's corners stay sharp: its vertices move
        // inward along the bisectors (no larger than the mitred offset).
        let inner = triangle.flatten_grown(-by, tol);
        assert!(area(&inner) > 0.0 && area(&inner) < 1.0);
    }

    #[test]
    fn the_corner_radius_clamps_to_ratio_cap_and_half_side() {
        let rr = |ratio: f64, cap: Option<f64>| PadShape::RoundRect {
            size: pt([2.0, 1.0]),
            radius_ratio: ratio,
            max_radius_nm: cap.map(nm),
            corners: CornerMask::ALL,
        };
        assert!(close_to(rr(0.25, None).corner_radius(), 0.25, 1e-12));
        assert!(close_to(rr(0.25, Some(0.1)).corner_radius(), 0.1, 1e-12));
        assert!(close_to(rr(0.9, None).corner_radius(), 0.5, 1e-12));
        assert!(close_to(rr(-1.0, None).corner_radius(), 0.0, 1e-12));
        assert_eq!(
            PadShape::Rect {
                size: pt([1.0, 1.0])
            }
            .corner_radius(),
            0.0
        );
    }

    #[test]
    fn slots_extend_along_their_angle_and_pads_rotate_about_their_origin() {
        let slot = HoleShape::Slot {
            width_nm: nm(0.5),
            length_nm: nm(2.0),
            angle_mdeg: mdeg(90.0),
        };
        let ring = slot.flatten(1e-4);
        let b = crate::geometry::Bounds::of(ring.iter().copied());
        let Some(b) = b else {
            return assert!(b.is_some());
        };
        assert!(close_to(b.size()[0], 0.5, 1e-3) && close_to(b.size()[1], 2.0, 1e-3));
        let pad = Pad {
            rotation_mdeg: mdeg(90.0),
            ..Pad::smd_rect(
                crate::ids::PadId(1),
                crate::ids::PackageTerminalId(1),
                "1",
                pt([3.0, 1.0]),
                pt([2.0, 1.0]),
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
        let land = PadShape::Circle {
            diameter_nm: nm(2.0),
        }
        .flatten(1e-3);
        let inner = HoleShape::Round {
            diameter_nm: nm(1.0),
        }
        .flatten(1e-3);
        assert!(ring_within(&inner, &land, 2e-3));
        let moved: Vec<Point> = inner.iter().map(|p| [p[0] + 0.6, p[1]]).collect();
        assert!(!ring_within(&moved, &land, 2e-3));
        let far: Vec<Point> = land.iter().map(|p| [p[0] + 2.5, p[1]]).collect();
        assert!(close_to(ring_gap(&land, &far), 0.5, 2e-3));
        assert_eq!(ring_gap(&land, &inner), 0.0, "nested rings overlap");
    }
}
