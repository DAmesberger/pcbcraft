//! Exact pad, hole and opening contours for the native copper and the
//! display layers: every [`PadShape`] and [`HoleShape`] as lines and bulge
//! arcs (or one circle), in the pad frame, grown or shrunk on request. The
//! native model draws them exactly; display layers flatten them at an
//! explicit chord tolerance.
use crate::{Error, Result};
use sparksmith_core::Point;
use sparksmith_core::geometry::Shape;
use sparksmith_core::library::{
    Hole, HoleShape, Outline, Pad, PadDefaults, PadShape, Segment,
};

/// Plated barrel wall: copper plated onto a drilled hole's wall, 25 µm,
/// IPC-6012's least average hole-wall copper for class 3 (class 2 asks
/// for 20 µm). Neither the stackup nor the fab profile records a plating
/// thickness, so the 3D view draws this one. A hole's size is its finished
/// size (the open bore); the barrel grows outward from it, so the hole
/// is drilled larger by the wall on every side.
pub const PLATING_MM: f64 = 0.025;
/// Chord tolerance of flattened display outlines (the core's display
/// tolerance for pad arcs).
pub const FLATTEN_TOLERANCE_MM: f64 = sparksmith_core::library::PAD_ARC_TOLERANCE_MM;
/// Points of one flattened loop.
pub const MAX_FLAT_POINTS: usize = 8192;
/// Chords of one flattened arc.
const MAX_ARC_CHORDS: usize = 256;
/// Below this a size or length is treated as zero.
const EPS: f64 = 1e-9;

/// A closed contour: a circle (drawn natively as one), or an outline of
/// lines and bulge arcs.
#[derive(Clone, Debug, PartialEq)]
pub enum Contour {
    Circle { center: Point, radius: f64 },
    Outline(Outline),
}

impl Contour {
    /// The image under a rigid map of the plane (`mirror` when it reverses
    /// orientation, which reverses each arc's turn).
    pub fn mapped(&self, map: impl Fn(Point) -> Point, mirror: bool) -> Self {
        match self {
            Self::Circle { center, radius } => Self::Circle {
                center: map(*center),
                radius: *radius,
            },
            Self::Outline(outline) => Self::Outline(Outline {
                start: map(outline.start),
                segments: outline
                    .segments
                    .iter()
                    .map(|s| match *s {
                        Segment::Line { to } => Segment::Line { to: map(to) },
                        Segment::Arc { to, bulge } => Segment::Arc {
                            to: map(to),
                            bulge: if mirror { -bulge } else { bulge },
                        },
                    })
                    .collect(),
            }),
        }
    }

    /// Moved by `by`.
    pub fn translated(&self, by: Point) -> Self {
        self.mapped(|p| [p[0] + by[0], p[1] + by[1]], false)
    }

    /// Turned counter-clockwise by `deg` about the origin.
    pub fn rotated(&self, deg: f64) -> Self {
        let (c, s) = sparksmith_core::geometry::cos_sin(deg);
        self.mapped(|p| [c * p[0] - s * p[1], s * p[0] + c * p[1]], false)
    }

    /// The polygon through the contour at `tolerance`, in drawing order
    /// (not closed: the last point is not repeated).
    pub fn flatten(&self, tolerance: f64) -> Result<Vec<Point>> {
        let mut out = Vec::new();
        match self {
            Self::Circle { center, radius } => {
                let n = chords(*radius, std::f64::consts::TAU, tolerance).max(8);
                for k in 0..n {
                    let a = std::f64::consts::TAU * k as f64 / n as f64;
                    let (s, c) = a.sin_cos();
                    out.push([center[0] + radius * c, center[1] + radius * s]);
                }
            }
            Self::Outline(outline) => {
                let mut from = outline.start;
                for segment in &outline.segments {
                    out.push(from);
                    if let Segment::Arc { to, bulge } = *segment
                        && let Some(arc) = Arc::of(from, to, bulge)
                    {
                        let n = chords(arc.radius, arc.sweep.abs(), tolerance);
                        for k in 1..n {
                            let a = arc.start + arc.sweep * k as f64 / n as f64;
                            let (s, c) = a.sin_cos();
                            out.push([
                                arc.center[0] + arc.radius * c,
                                arc.center[1] + arc.radius * s,
                            ]);
                        }
                    }
                    from = segment.to();
                    if out.len() > MAX_FLAT_POINTS {
                        return Err(Error::Invalid(format!(
                            "a pad outline needs more than {MAX_FLAT_POINTS} display points"
                        )));
                    }
                }
            }
        }
        out.dedup_by(|a, b| distance(*a, *b) < EPS);
        if out.len() > 1
            && let (Some(first), Some(last)) = (out.first(), out.last())
            && distance(*first, *last) < EPS
        {
            out.pop();
        }
        Ok(out)
    }
}

impl Contour {
    /// The enclosed area, mm², exact: a circle's, or an outline's polygon
    /// through its segment ends with each arc's circular segment added
    /// (bulging out) or taken away (bulging in).
    pub fn area(&self) -> f64 {
        match self {
            Self::Circle { radius, .. } => std::f64::consts::PI * radius * radius,
            Self::Outline(outline) => {
                let mut twice = 0.0;
                let mut arcs = 0.0;
                let mut from = outline.start;
                for segment in &outline.segments {
                    let to = segment.to();
                    twice += from[0] * to[1] - to[0] * from[1];
                    if let Segment::Arc { bulge, .. } = *segment
                        && let Some(arc) = Arc::of(from, to, bulge)
                    {
                        arcs += arc.radius * arc.radius / 2.0 * (arc.sweep - arc.sweep.sin());
                    }
                    from = to;
                }
                // An outline that does not end where it starts closes.
                let start = outline.start;
                twice += from[0] * start[1] - start[0] * from[1];
                (twice / 2.0 + arcs).abs()
            }
        }
    }

    /// The box round the contour, low and high corner; arcs bounded by
    /// their points at `tolerance`.
    pub fn bounds(&self, tolerance: f64) -> Result<(Point, Point)> {
        if let Self::Circle { center, radius } = self {
            return Ok((
                [center[0] - radius, center[1] - radius],
                [center[0] + radius, center[1] + radius],
            ));
        }
        let points = self.flatten(tolerance)?;
        Ok(points.iter().fold(
            ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]),
            |(lo, hi), p| {
                (
                    [lo[0].min(p[0]), lo[1].min(p[1])],
                    [hi[0].max(p[0]), hi[1].max(p[1])],
                )
            },
        ))
    }
}

/// A circular arc from a bulge segment.
pub struct Arc {
    pub center: Point,
    pub radius: f64,
    /// Start angle and signed sweep, radians.
    pub start: f64,
    pub sweep: f64,
}

impl Arc {
    /// `None` for a straight segment (zero bulge or zero chord).
    pub fn of(from: Point, to: Point, bulge: f64) -> Option<Self> {
        match sparksmith_core::library::bulge_arc(from, to, bulge)? {
            Shape::Arc {
                center,
                radius,
                start_deg,
                sweep_deg,
            } => Some(Self {
                center,
                radius,
                start: start_deg.to_radians(),
                sweep: sweep_deg.to_radians(),
            }),
            _ => None,
        }
    }
}

/// The arc's point half way along a bulge segment (what a three-point arc
/// passes through).
pub fn bulge_midpoint(from: Point, to: Point, bulge: f64) -> Point {
    let d = [to[0] - from[0], to[1] - from[1]];
    let chord = d[0].hypot(d[1]);
    let mid = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
    if chord < EPS {
        return mid;
    }
    // Sagitta, to the right of the chord for a counter-clockwise arc.
    let s = bulge * chord / 2.0;
    [mid[0] + s * d[1] / chord, mid[1] - s * d[0] / chord]
}

fn chords(radius: f64, sweep: f64, tolerance: f64) -> usize {
    let tolerance = tolerance.clamp(1e-6, radius.max(1e-6));
    let step = 2.0 * (1.0 - tolerance / radius.max(1e-9)).clamp(-1.0, 1.0).acos();
    if !step.is_finite() || step <= 0.0 {
        return MAX_ARC_CHORDS;
    }
    ((sweep / step).ceil() as usize).clamp(1, MAX_ARC_CHORDS)
}

fn distance(a: Point, b: Point) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// A pad shape grown by `grow` on every side (negative shrinks), in the pad
/// frame about the pad origin. `None` when nothing is left.
pub fn shape_contour(shape: &PadShape, grow: f64) -> Option<Contour> {
    match shape {
        PadShape::Rect { size } => rounded(size.to_mm_f64(), grow, [0.0; 4]),
        PadShape::RoundRect { size, corners, .. } => {
            let r = shape.corner_radius();
            let pick = |on: bool| if on { r } else { 0.0 };
            // Counter-clockwise from the bottom right.
            rounded(
                size.to_mm_f64(),
                grow,
                [
                    pick(corners.bottom_right),
                    pick(corners.top_right),
                    pick(corners.top_left),
                    pick(corners.bottom_left),
                ],
            )
        }
        PadShape::Circle { diameter_nm } => {
            let radius = diameter_nm.to_mm_f64() / 2.0 + grow;
            (radius > EPS).then_some(Contour::Circle {
                center: [0.0, 0.0],
                radius,
            })
        }
        PadShape::Oblong { size } => {
            let size = size.to_mm_f64();
            let r = size[0].min(size[1]) / 2.0;
            rounded(size, grow, [r; 4])
        }
        PadShape::Custom { outline } => {
            if grow.abs() < EPS {
                return Some(Contour::Outline(outline.clone()));
            }
            // The core's true offset, so the native opening is the one
            // checks, 2D and fabrication output use.
            let grown = shape.grown(grow, FLATTEN_TOLERANCE_MM)?.flatten(FLATTEN_TOLERANCE_MM);
            let (first, rest) = grown.split_first()?;
            let mut segments: Vec<Segment> =
                rest.iter().map(|p| Segment::Line { to: *p }).collect();
            segments.push(Segment::Line { to: *first });
            Some(Contour::Outline(Outline {
                start: *first,
                segments,
            }))
        }
    }
}

/// A `size` rectangle grown by `grow`, its corners (bottom right, top right,
/// top left, bottom left) rounded by `radii` (each grown with the side,
/// capped at half the short side). Growing is a true offset, as in the
/// core's `PadShape::grown`: a sharp corner grows round with radius `grow`.
/// A rectangle whose ends are wholly round is a stadium; a square one a
/// circle.
fn rounded(size: Point, grow: f64, radii: [f64; 4]) -> Option<Contour> {
    let (w, h) = (size[0] + 2.0 * grow, size[1] + 2.0 * grow);
    if w <= EPS || h <= EPS {
        return None;
    }
    let cap = w.min(h) / 2.0;
    let r = radii.map(|r| {
        if r > EPS {
            (r + grow).clamp(0.0, cap)
        } else {
            grow.clamp(0.0, cap)
        }
    });
    if r.iter().all(|r| (r - cap).abs() < EPS) && (w - h).abs() < EPS {
        return Some(Contour::Circle {
            center: [0.0, 0.0],
            radius: cap,
        });
    }
    let (hw, hh) = (w / 2.0, h / 2.0);
    // A quarter turn: bulge tan(90° / 4).
    let quarter = (std::f64::consts::PI / 8.0).tan();
    let [br, tr, tl, bl] = r;
    let corners = [
        // (corner radius, line end before the arc, arc end)
        (br, [hw - br, -hh], [hw, -hh + br]),
        (tr, [hw, hh - tr], [hw - tr, hh]),
        (tl, [-hw + tl, hh], [-hw, hh - tl]),
        (bl, [-hw, -hh + bl], [-hw + bl, -hh]),
    ];
    let start = [-hw + bl, -hh];
    let mut segments = Vec::with_capacity(8);
    let mut at = start;
    for (radius, line_end, arc_end) in corners {
        if distance(at, line_end) > EPS {
            segments.push(Segment::Line { to: line_end });
        }
        at = line_end;
        if radius > EPS {
            segments.push(Segment::Arc {
                to: arc_end,
                bulge: quarter,
            });
            at = arc_end;
        }
    }
    if distance(at, start) > EPS {
        segments.push(Segment::Line { to: start });
    }
    // Merge consecutive quarter arcs of one circle (a stadium's ends) into
    // half arcs, so a sketch draws one arc per end.
    let mut merged: Vec<Segment> = Vec::with_capacity(segments.len());
    for segment in segments {
        if let (Some(Segment::Arc { bulge: b0, .. }), Segment::Arc { to, bulge }) =
            (merged.last().copied(), segment)
            && (b0 - quarter).abs() < EPS
            && (bulge - quarter).abs() < EPS
        {
            merged.pop();
            merged.push(Segment::Arc { to, bulge: 1.0 });
            continue;
        }
        merged.push(segment);
    }
    Some(Contour::Outline(Outline {
        start,
        segments: merged,
    }))
}

/// A hole's contour in the pad frame, grown by `grow` (negative shrinks).
pub fn hole_contour(hole: &Hole, grow: f64) -> Option<Contour> {
    let contour = match hole.shape {
        HoleShape::Round { diameter_nm } => {
            let radius = diameter_nm.to_mm_f64() / 2.0 + grow;
            (radius > EPS).then_some(Contour::Circle {
                center: [0.0, 0.0],
                radius,
            })?
        }
        HoleShape::Slot {
            width_nm,
            length_nm,
            angle_mdeg,
        } => {
            let (width, length) = (width_nm.to_mm_f64(), length_nm.to_mm_f64());
            let width = width.min(length.max(width));
            let length = length.max(width);
            let r = width / 2.0;
            rounded([length, width], grow, [r; 4])?.rotated(angle_mdeg.to_degrees_f64())
        }
    };
    Some(contour.translated(hole.offset()))
}

/// The finished bore of a hole: its stated (finished) size.
pub fn bore(hole: &Hole) -> Option<Contour> {
    hole_contour(hole, 0.0)
}

/// The drilled wall of a hole: a plated hole is drilled larger than its
/// finished size by the barrel wall, an unplated one at its size.
pub fn drilled(hole: &Hole) -> Option<Contour> {
    hole_contour(hole, if hole.plated { PLATING_MM } else { 0.0 })
}

/// The drilled radius of a plated hole of `finished` diameter (a via's).
pub fn plated_drill_radius(finished: f64) -> f64 {
    finished / 2.0 + PLATING_MM
}

/// The solder-mask opening of a pad in the pad frame, or `None` when the
/// pad is tented.
pub fn mask_opening(pad: &Pad, defaults: &PadDefaults) -> Option<Contour> {
    shape_contour(&pad.shape, pad.mask_expansion(defaults)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_units::{deg, nm, pt};
    use sparksmith_core::library::CornerMask;

    /// The native contour and the core's `PadShape::grown` (the one pad
    /// geometry 2D, checks and the update preview use) agree to within
    /// flattening for every primitive shape and grow value.
    #[test]
    fn native_contours_match_the_core_pad_geometry() -> Result<()> {
        let d_shape = CornerMask {
            top_left: true,
            top_right: false,
            bottom_right: false,
            bottom_left: true,
        };
        let shapes = [
            PadShape::Rect {
                size: pt([1.2, 0.6]),
            },
            PadShape::RoundRect {
                size: pt([1.2, 0.6]),
                radius_ratio: 0.25,
                max_radius_nm: None,
                corners: CornerMask::ALL,
            },
            PadShape::RoundRect {
                size: pt([1.6, 0.8]),
                radius_ratio: 0.5,
                max_radius_nm: Some(nm(0.3)),
                corners: d_shape,
            },
            PadShape::Circle {
                diameter_nm: nm(0.9),
            },
            PadShape::Oblong {
                size: pt([1.5, 0.7]),
            },
            PadShape::Oblong {
                size: pt([0.6, 1.4]),
            },
        ];
        const TOL: f64 = 1e-4;
        for shape in &shapes {
            for grow in [0.0, 0.05, -0.1] {
                let native = shape_contour(shape, grow)
                    .ok_or_else(|| Error::Invalid(format!("{shape:?} {grow}")))?
                    .flatten(TOL)?;
                let core = shape
                    .grown(grow, TOL)
                    .ok_or_else(|| Error::Invalid(format!("core {shape:?} {grow}")))?;
                let ring = Shape::Polygon {
                    points: native.clone(),
                    radii: Vec::new(),
                };
                for p in &native {
                    let d = core.distance(*p, false);
                    assert!(
                        d < 1e-3,
                        "{shape:?} grow {grow}: native point {p:?} off by {d}"
                    );
                }
                for q in core.flatten(TOL) {
                    let d = ring.distance(q, false);
                    assert!(
                        d < 1e-3,
                        "{shape:?} grow {grow}: core point {q:?} off by {d}"
                    );
                }
            }
        }
        Ok(())
    }

    fn area(points: &[Point]) -> f64 {
        let n = points.len();
        (0..n)
            .map(|i| {
                let (a, b) = (points[i], points[(i + 1) % n]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f64>()
            / 2.0
    }

    fn flat_area(contour: &Contour) -> f64 {
        area(&contour.flatten(1e-5).unwrap_or_default())
    }

    fn close(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() < tol, "{a} != {b}");
    }

    #[test]
    fn every_shape_has_its_exact_area_counter_clockwise() -> Result<()> {
        let pi = std::f64::consts::PI;
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
                pi,
            ),
            (
                PadShape::Oblong {
                    size: pt([3.0, 1.0]),
                },
                2.0 + pi / 4.0,
            ),
            (
                PadShape::Oblong {
                    size: pt([1.0, 3.0]),
                },
                2.0 + pi / 4.0,
            ),
            (
                PadShape::RoundRect {
                    size: pt([2.0, 1.0]),
                    radius_ratio: 0.25,
                    max_radius_nm: None,
                    corners: CornerMask::ALL,
                },
                2.0 - (4.0 - pi) * 0.0625,
            ),
            (
                // A D shape: two corners at half the short side.
                PadShape::RoundRect {
                    size: pt([2.0, 1.0]),
                    radius_ratio: 0.5,
                    max_radius_nm: None,
                    corners: CornerMask {
                        top_left: false,
                        top_right: true,
                        bottom_right: true,
                        bottom_left: false,
                    },
                },
                2.0 - (1.0 - pi / 4.0) * 0.5,
            ),
            (
                PadShape::RoundRect {
                    size: pt([2.0, 1.0]),
                    radius_ratio: 0.5,
                    max_radius_nm: Some(nm(0.1)),
                    corners: CornerMask::ALL,
                },
                2.0 - (4.0 - pi) * 0.01,
            ),
            (
                PadShape::Custom {
                    outline: Outline {
                        start: [0.0, 0.0],
                        segments: vec![
                            Segment::Line { to: [2.0, 0.0] },
                            Segment::Arc {
                                to: [0.0, 0.0],
                                bulge: 1.0,
                            },
                        ],
                    },
                },
                pi / 2.0,
            ),
        ];
        for (shape, expected) in cases {
            let contour = shape_contour(&shape, 0.0)
                .ok_or_else(|| Error::Verification(format!("{shape:?}")))?;
            close(flat_area(&contour), expected, 1e-3);
        }
        Ok(())
    }

    #[test]
    fn stadium_ends_are_single_half_arcs_and_square_stadia_are_circles() -> Result<()> {
        let Some(Contour::Outline(o)) = shape_contour(
            &PadShape::Oblong {
                size: pt([3.0, 1.0]),
            },
            0.0,
        ) else {
            return Err(Error::Verification("oblong is an outline".into()));
        };
        let arcs: Vec<f64> = o
            .segments
            .iter()
            .filter_map(|s| match s {
                Segment::Arc { bulge, .. } => Some(*bulge),
                Segment::Line { .. } => None,
            })
            .collect();
        assert_eq!(arcs, vec![1.0, 1.0]);
        assert_eq!(o.segments.len(), 4);
        assert!(matches!(
            shape_contour(&PadShape::Oblong { size: pt([1.0, 1.0]) }, 0.0),
            Some(Contour::Circle { radius, .. }) if (radius - 0.5).abs() < 1e-12
        ));
        Ok(())
    }

    #[test]
    fn growing_and_shrinking_offsets_every_side() -> Result<()> {
        let rect = shape_contour(
            &PadShape::Rect {
                size: pt([2.0, 1.0]),
            },
            0.05,
        );
        // A true offset: the sides move out and the corners round by the
        // growth (A + P·g + π·g²).
        close(
            flat_area(&rect.ok_or_else(|| Error::Verification("none".into()))?),
            2.0 + 6.0 * 0.05 + std::f64::consts::PI * 0.05 * 0.05,
            1e-4,
        );
        let circle = shape_contour(
            &PadShape::Circle {
                diameter_nm: nm(1.0),
            },
            -0.1,
        );
        close(
            flat_area(&circle.ok_or_else(|| Error::Verification("none".into()))?),
            std::f64::consts::PI * 0.16,
            1e-3,
        );
        assert!(
            shape_contour(
                &PadShape::Rect {
                    size: pt([2.0, 1.0])
                },
                -0.5
            )
            .is_none()
        );
        let square = PadShape::Custom {
            outline: Outline {
                start: [0.0, 0.0],
                segments: vec![
                    Segment::Line { to: [0.0, 2.0] },
                    Segment::Line { to: [2.0, 2.0] },
                    Segment::Line { to: [2.0, 0.0] },
                    Segment::Line { to: [0.0, 0.0] },
                ],
            },
        };
        // Clockwise as authored, still grown outward.
        let grown =
            shape_contour(&square, 0.1).ok_or_else(|| Error::Verification("none".into()))?;
        close(
            flat_area(&grown).abs(),
            4.0 + 8.0 * 0.1 + std::f64::consts::PI * 0.01,
            1e-3,
        );
        Ok(())
    }

    #[test]
    fn slots_follow_their_angle_and_bores_lose_the_plating() -> Result<()> {
        let slot = Hole {
            shape: HoleShape::Slot {
                width_nm: nm(1.0),
                length_nm: nm(3.0),
                angle_mdeg: deg(90.0),
            },
            plated: true,
            offset: pt([0.5, 0.0]),
        };
        let points = hole_contour(&slot, 0.0)
            .ok_or_else(|| Error::Verification("none".into()))?
            .flatten(1e-5)
            .unwrap_or_default();
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for p in &points {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        close(lo[0], 0.0, 1e-4);
        close(hi[0], 1.0, 1e-4);
        close(lo[1], -1.5, 1e-4);
        close(hi[1], 1.5, 1e-4);
        let round = Hole {
            shape: HoleShape::Round {
                diameter_nm: nm(1.0),
            },
            plated: true,
            offset: pt([0.0, 0.0]),
        };
        assert!(matches!(
            bore(&round),
            Some(Contour::Circle { radius, .. }) if (radius - 0.5).abs() < 1e-12
        ));
        assert!(matches!(
            drilled(&round),
            Some(Contour::Circle { radius, .. }) if (radius - (0.5 + PLATING_MM)).abs() < 1e-12
        ));
        Ok(())
    }

    #[test]
    fn mirroring_reverses_arcs_and_the_midpoint_is_on_the_arc() -> Result<()> {
        let from = [1.0, 0.0];
        let to = [-1.0, 0.0];
        let mid = bulge_midpoint(from, to, 1.0);
        close(mid[0], 0.0, 1e-12);
        close(mid[1], 1.0, 1e-12);
        let arc = Arc::of(from, to, (std::f64::consts::PI / 8.0).tan())
            .ok_or_else(|| Error::Verification("none".into()))?;
        close(arc.radius, 2f64.sqrt(), 1e-9);
        let c = Contour::Outline(Outline {
            start: from,
            segments: vec![Segment::Arc { to, bulge: 1.0 }, Segment::Line { to: from }],
        });
        let m = c.mapped(|p| [p[0], -p[1]], true);
        close(flat_area(&m), -flat_area(&c), 1e-9);
        Ok(())
    }
}
