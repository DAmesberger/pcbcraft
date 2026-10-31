//! Copper-profile classification (design §2.5 step 7): a solved profile
//! loop around a pad anchor becomes a typed [`PadShape`] in the pad frame,
//! with its rotation, or an exact custom outline. Holes come from their
//! role-owned construction curves.

use pcbcraft_core::library::{CornerMask, Hole, HoleShape, Outline, PadShape, Segment};
use solidcraft_engine::geom::{Loop2, Seg2, Vec2};
use solidcraft_engine::sketch::{CurveKind, Sketch};

use super::{Outcome, problem};

/// Geometric tolerance of classification, mm (the quantum is 1e-6 mm).
pub(crate) const EPS: f64 = 1e-6;
const TAU: f64 = std::f64::consts::TAU;
const PI: f64 = std::f64::consts::PI;

pub(crate) const UNCLASSIFIED: &str = "footprint.pad_shape_unclassified";

/// One merged boundary piece: a line or a circular arc (radians,
/// counter-clockwise positive).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Piece {
    Line {
        a: Vec2,
        b: Vec2,
    },
    Arc {
        c: Vec2,
        r: f64,
        start: f64,
        sweep: f64,
    },
}

fn at_angle(c: Vec2, r: f64, t: f64) -> Vec2 {
    Vec2::new(c.x + r * t.cos(), c.y + r * t.sin())
}

impl Piece {
    fn start(&self) -> Vec2 {
        match *self {
            Self::Line { a, .. } => a,
            Self::Arc { c, r, start, .. } => at_angle(c, r, start),
        }
    }
    fn end(&self) -> Vec2 {
        match *self {
            Self::Line { b, .. } => b,
            Self::Arc { c, r, start, sweep } => at_angle(c, r, start + sweep),
        }
    }
}

fn close(a: Vec2, b: Vec2) -> bool {
    a.dist(b) <= EPS
}

fn merge(a: Piece, b: Piece) -> Option<Piece> {
    match (a, b) {
        (Piece::Line { a: p, b: q }, Piece::Line { a: q2, b: r }) if close(q, q2) => {
            let (d1, d2) = (q - p, r - q2);
            let cross = d1.x * d2.y - d1.y * d2.x;
            let dot = d1.x * d2.x + d1.y * d2.y;
            (cross.abs() <= EPS * (d1.len() + d2.len()) && dot > 0.0)
                .then_some(Piece::Line { a: p, b: r })
        }
        (
            Piece::Arc { c, r, start, sweep },
            Piece::Arc {
                c: c2,
                r: r2,
                sweep: sweep2,
                ..
            },
        ) if close(c, c2) && (r - r2).abs() <= EPS && sweep.signum() == sweep2.signum() => {
            close(a.end(), b.start()).then_some(Piece::Arc {
                c,
                r,
                start,
                sweep: sweep + sweep2,
            })
        }
        _ => None,
    }
}

/// The loop's lines and arcs, with collinear lines and co-circular arcs
/// merged. Free-form segments are refused.
pub(crate) fn pieces(lp: &Loop2) -> Outcome<Vec<Piece>> {
    let mut out: Vec<Piece> = Vec::new();
    for seg in &lp.segs {
        let piece = match *seg {
            Seg2::Line { a, b } => Piece::Line { a, b },
            Seg2::Arc {
                center,
                radius,
                start,
                sweep,
            } => Piece::Arc {
                c: center,
                r: radius,
                start,
                sweep,
            },
            _ => {
                return Err(problem(
                    UNCLASSIFIED,
                    "a pad outline may have only lines and arcs",
                ));
            }
        };
        match out.last().copied().and_then(|last| merge(last, piece)) {
            Some(m) => {
                if let Some(last) = out.last_mut() {
                    *last = m;
                }
            }
            None => out.push(piece),
        }
    }
    if out.len() > 1
        && let (Some(last), Some(first)) = (out.last().copied(), out.first().copied())
        && let Some(m) = merge(last, first)
    {
        out.pop();
        if let Some(first) = out.first_mut() {
            *first = m;
        }
    }
    if out.is_empty() {
        return Err(problem(UNCLASSIFIED, "an empty outline"));
    }
    Ok(out)
}

fn rotate(v: Vec2, rad: f64) -> Vec2 {
    let (s, c) = rad.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Into the pad frame: relative to the anchor, turned by `-theta`.
fn local(p: Piece, anchor: Vec2, theta: f64) -> Piece {
    let to = |v: Vec2| rotate(v - anchor, -theta);
    match p {
        Piece::Line { a, b } => Piece::Line { a: to(a), b: to(b) },
        Piece::Arc { c, r, start, sweep } => Piece::Arc {
            c: to(c),
            r,
            start: start - theta,
            sweep,
        },
    }
}

/// A line direction in degrees in [0°, 180°), snapped so a hair below
/// 180° counts as 0°.
fn axis_deg(a: Vec2, b: Vec2) -> f64 {
    let d = (b.y - a.y).atan2(b.x - a.x).to_degrees().rem_euclid(180.0);
    if d > 180.0 - 1e-9 { 0.0 } else { d }
}

/// The shape and its rotation (degrees).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Classified {
    pub shape: PadShape,
    pub rotation_deg: f64,
}

/// Classify a closed loop around `anchor`. `orientation` (degrees) fixes
/// the pad frame; otherwise a primitive's own axis does.
pub(crate) fn classify(
    pieces: &[Piece],
    anchor: Vec2,
    orientation: Option<f64>,
) -> Outcome<Classified> {
    let natural = pieces
        .iter()
        .filter_map(|p| match *p {
            Piece::Line { a, b } => Some(axis_deg(a, b)),
            Piece::Arc { .. } => None,
        })
        .fold(None, |m: Option<f64>, d| Some(m.map_or(d, |m| m.min(d))))
        .unwrap_or(0.0);
    let theta = orientation.unwrap_or(natural);
    let rad = theta.to_radians();
    let frame: Vec<Piece> = pieces.iter().map(|p| local(*p, anchor, rad)).collect();
    if let Some(shape) = primitive(&frame) {
        return Ok(Classified {
            rotation_deg: shape.canonical_rotation(theta),
            shape,
        });
    }
    let theta = orientation.unwrap_or(0.0);
    let rad = theta.to_radians();
    let frame: Vec<Piece> = pieces.iter().map(|p| local(*p, anchor, rad)).collect();
    let shape = PadShape::Custom {
        outline: outline(&frame)?,
    };
    Ok(Classified {
        rotation_deg: shape.canonical_rotation(theta),
        shape,
    })
}

fn horizontal(a: Vec2, b: Vec2) -> bool {
    (a.y - b.y).abs() <= EPS && (a.x - b.x).abs() > EPS
}

fn vertical(a: Vec2, b: Vec2) -> bool {
    (a.x - b.x).abs() <= EPS && (a.y - b.y).abs() > EPS
}

fn about_zero(v: f64) -> bool {
    v.abs() <= EPS
}

/// A primitive centred on the pad origin in the pad frame, if the loop is
/// one.
fn primitive(frame: &[Piece]) -> Option<PadShape> {
    let lines: Vec<(Vec2, Vec2)> = frame
        .iter()
        .filter_map(|p| match *p {
            Piece::Line { a, b } => Some((a, b)),
            Piece::Arc { .. } => None,
        })
        .collect();
    let arcs: Vec<(Vec2, f64, f64)> = frame
        .iter()
        .filter_map(|p| match *p {
            Piece::Arc { c, r, sweep, .. } => Some((c, r, sweep)),
            Piece::Line { .. } => None,
        })
        .collect();
    let alternating = frame.windows(2).all(|w| {
        matches!(
            w,
            [Piece::Line { .. }, Piece::Arc { .. }] | [Piece::Arc { .. }, Piece::Line { .. }]
        )
    });
    let aligned = lines
        .iter()
        .all(|&(a, b)| horizontal(a, b) || vertical(a, b));
    let same_radius = |r0: f64| arcs.iter().all(|&(_, r, _)| (r - r0).abs() <= EPS);
    match (lines.len(), arcs.as_slice()) {
        (0, [(c, r, sweep)])
            if (sweep.abs() - TAU).abs() <= 1e-9 && about_zero(c.x) && about_zero(c.y) =>
        {
            Some(PadShape::Circle { diameter: 2.0 * r })
        }
        (4, []) if aligned => {
            let (min, max) = vertex_box(frame);
            let symmetric = about_zero(min.x + max.x) && about_zero(min.y + max.y);
            let alternate = lines
                .windows(2)
                .all(|w| matches!(w, [(a, b), (c, d)] if horizontal(*a, *b) != horizontal(*c, *d)));
            (symmetric && alternate).then_some(PadShape::Rect {
                size: [max.x - min.x, max.y - min.y],
            })
        }
        (2, [(c1, r, s1), (c2, _, s2)])
            if alternating
                && aligned
                && same_radius(*r)
                && (s1.abs() - PI).abs() <= 1e-9
                && (s2.abs() - PI).abs() <= 1e-9
                && about_zero(c1.x + c2.x)
                && about_zero(c1.y + c2.y) =>
        {
            let len = lines.first().map(|(a, b)| a.dist(*b))?;
            let along_x = lines.iter().all(|&(a, b)| horizontal(a, b));
            let along_y = lines.iter().all(|&(a, b)| vertical(a, b));
            if along_x && about_zero(c1.y) {
                Some(PadShape::Oblong {
                    size: [len + 2.0 * r, 2.0 * r],
                })
            } else if along_y && about_zero(c1.x) {
                Some(PadShape::Oblong {
                    size: [2.0 * r, len + 2.0 * r],
                })
            } else {
                None
            }
        }
        (4, [(_, r, _), ..])
            if arcs.len() == 4
                && alternating
                && aligned
                && same_radius(*r)
                && arcs
                    .iter()
                    .all(|&(_, _, s)| (s.abs() - PI / 2.0).abs() <= 1e-9) =>
        {
            let hx = arcs.iter().map(|(c, ..)| c.x.abs()).fold(0.0_f64, f64::max);
            let hy = arcs.iter().map(|(c, ..)| c.y.abs()).fold(0.0_f64, f64::max);
            let corners_ok = arcs
                .iter()
                .all(|(c, ..)| (c.x.abs() - hx).abs() <= EPS && (c.y.abs() - hy).abs() <= EPS);
            let size = [2.0 * (hx + r), 2.0 * (hy + r)];
            let short = size[0].min(size[1]);
            (corners_ok && short > 0.0).then_some(PadShape::RoundRect {
                size,
                radius_ratio: r / short,
                max_radius: None,
                corners: CornerMask::ALL,
            })
        }
        _ => None,
    }
}

fn vertex_box(frame: &[Piece]) -> (Vec2, Vec2) {
    let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in frame {
        for v in [p.start(), p.end()] {
            min = Vec2::new(min.x.min(v.x), min.y.min(v.y));
            max = Vec2::new(max.x.max(v.x), max.y.max(v.y));
        }
    }
    (min, max)
}

fn point(v: Vec2) -> [f64; 2] {
    [v.x, v.y]
}

/// An exact outline: arcs over a half turn are split so every bulge stays
/// finite.
fn outline(frame: &[Piece]) -> Outcome<Outline> {
    let first = frame
        .first()
        .ok_or_else(|| problem(UNCLASSIFIED, "an empty outline"))?;
    let mut segments = Vec::new();
    for p in frame {
        match *p {
            Piece::Line { b, .. } => segments.push(Segment::Line { to: point(b) }),
            Piece::Arc { c, r, start, sweep } => {
                let parts = if sweep.abs() > PI { 2 } else { 1 };
                for k in 1..=parts {
                    let t = start + sweep * f64::from(k) / f64::from(parts);
                    segments.push(Segment::Arc {
                        to: point(at_angle(c, r, t)),
                        bulge: (sweep / f64::from(parts) / 4.0).tan(),
                    });
                }
            }
        }
    }
    if segments.len() > pcbcraft_core::library::MAX_OUTLINE_SEGMENTS {
        return Err(problem(UNCLASSIFIED, "the outline has too many segments"));
    }
    Ok(Outline {
        start: point(first.start()),
        segments,
    })
}

/// A drill from its role-owned construction curves: one circle (round) or
/// two lines and two half arcs (slot), in the pad frame.
pub(crate) fn hole(
    sk: &Sketch,
    curves: &[usize],
    plated: bool,
    anchor: Vec2,
    rotation_deg: f64,
) -> Outcome<Hole> {
    const BAD: &str = "footprint.hole_unclassified";
    let rad = rotation_deg.to_radians();
    let to_local = |v: Vec2| rotate(v - anchor, -rad);
    let pos = |i: usize| {
        sk.point(i)
            .ok_or_else(|| problem(BAD, "a hole curve names a missing point"))
    };
    let kinds: Vec<&CurveKind> = curves
        .iter()
        .filter_map(|&c| sk.curves.get(c).map(|c| &c.kind))
        .collect();
    match kinds.as_slice() {
        [CurveKind::Circle { c, r }] => Ok(Hole {
            shape: HoleShape::Round { diameter: 2.0 * r },
            plated,
            offset: point(to_local(pos(*c)?)),
        }),
        _ => {
            let mut centres = Vec::new();
            let mut radii = Vec::new();
            let mut lines = 0;
            for k in &kinds {
                match k {
                    CurveKind::Arc { c, a, .. } => {
                        let (pc, pa) = (pos(*c)?, pos(*a)?);
                        centres.push(pc);
                        radii.push(pc.dist(pa));
                    }
                    CurveKind::Line { .. } => lines += 1,
                    _ => return Err(problem(BAD, "a hole is a circle or a slot")),
                }
            }
            match (centres.as_slice(), radii.as_slice(), lines) {
                ([c1, c2], [r1, r2], 2) if (r1 - r2).abs() <= EPS => {
                    let mid = Vec2::new((c1.x + c2.x) * 0.5, (c1.y + c2.y) * 0.5);
                    Ok(Hole {
                        shape: HoleShape::Slot {
                            width: 2.0 * r1,
                            length: c1.dist(*c2) + 2.0 * r1,
                            angle_deg: (axis_deg(*c1, *c2) - rotation_deg).rem_euclid(180.0),
                        },
                        plated,
                        offset: point(to_local(mid)),
                    })
                }
                _ => Err(problem(BAD, "a hole is a circle or a slot")),
            }
        }
    }
}
