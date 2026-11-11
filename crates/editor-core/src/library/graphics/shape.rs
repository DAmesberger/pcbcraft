//! [`ShapeNm`]: a drawn shape as library definitions store it, in whole
//! nanometres. Every algorithm (flattening, fillets, bounds, hit tests,
//! transforms) stays on the computing [`Shape`] in millimetres; a stored
//! shape crosses with [`ShapeNm::to_mm`] and, back, only through
//! [`ShapeNm::from_mm_rounded`], the one rounding step.
//!
//! An arc is stored as its start and end points around a centre with a
//! direction (the end-plus-centre form of [`ArcNm`], with the start it
//! needs standing alone). Its radius is derived. A full turn has its end
//! equal to its start.
use crate::diagnostics::{Error, Result};
use crate::geometry::Shape;
use crate::units::{self, ArcDirection, ArcNm, Nm, PointNm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One drawn primitive in a local frame, in nanometres. The stored form of
/// [`Shape`]; see it for what each variant draws.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, units::Params)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ShapeNm {
    Line {
        a: PointNm,
        b: PointNm,
    },
    /// An open path of at least two points. `radii_nm` rounds interior
    /// corners: empty, or one fillet radius per point (0 keeps the corner
    /// sharp; the open ends cannot be rounded).
    Polyline {
        points: Vec<PointNm>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        radii_nm: Vec<Nm>,
    },
    /// Axis-aligned in its local frame, between two opposite corners, with
    /// every corner rounded by `radius_nm` (0 for square corners).
    Rect {
        a: PointNm,
        b: PointNm,
        #[serde(default, skip_serializing_if = "is_zero")]
        radius_nm: Nm,
    },
    Circle {
        center: PointNm,
        radius_nm: Nm,
    },
    /// From `start` around `centre` to `end`, turning `direction`; `end`
    /// equal to `start` is a full turn. The radius is the start's distance
    /// from the centre; the end's may differ by rounding only
    /// ([`ArcNm::check`]).
    Arc {
        start: PointNm,
        end: PointNm,
        centre: PointNm,
        direction: ArcDirection,
    },
    /// A closed outline of at least three points; the last joins the first.
    /// `radii_nm` rounds corners as for a polyline, every vertex included.
    Polygon {
        points: Vec<PointNm>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        radii_nm: Vec<Nm>,
    },
}

fn is_zero(v: &Nm) -> bool {
    *v == Nm::ZERO
}

fn mm(points: &[PointNm]) -> Vec<[f64; 2]> {
    points.iter().map(|p| p.to_mm_f64()).collect()
}

fn mm_radii(radii: &[Nm]) -> Vec<f64> {
    radii.iter().map(|r| r.to_mm_f64()).collect()
}

fn rounded(points: &[[f64; 2]]) -> Result<Vec<PointNm>> {
    points
        .iter()
        .map(|p| PointNm::from_mm_f64_rounded(*p))
        .collect()
}

fn rounded_radii(radii: &[f64]) -> Result<Vec<Nm>> {
    radii.iter().map(|r| Nm::from_mm_f64_rounded(*r)).collect()
}

/// The angle of `p` seen from `c`, in degrees.
fn bearing(c: [f64; 2], p: [f64; 2]) -> f64 {
    (p[1] - c[1]).atan2(p[0] - c[0]).to_degrees()
}

impl ShapeNm {
    /// The shape in millimetres, for computing. An arc's radius is its
    /// start's distance from the centre, its start angle that point's
    /// bearing, and its sweep the turn to the end's bearing in its
    /// direction (a whole turn when the end is the start).
    pub fn to_mm(&self) -> Shape {
        match self {
            Self::Line { a, b } => Shape::Line {
                a: a.to_mm_f64(),
                b: b.to_mm_f64(),
            },
            Self::Polyline { points, radii_nm } => Shape::Polyline {
                points: mm(points),
                radii: mm_radii(radii_nm),
            },
            Self::Polygon { points, radii_nm } => Shape::Polygon {
                points: mm(points),
                radii: mm_radii(radii_nm),
            },
            Self::Rect { a, b, radius_nm } => Shape::Rect {
                a: a.to_mm_f64(),
                b: b.to_mm_f64(),
                radius: radius_nm.to_mm_f64(),
            },
            Self::Circle { center, radius_nm } => Shape::Circle {
                center: center.to_mm_f64(),
                radius: radius_nm.to_mm_f64(),
            },
            Self::Arc {
                start,
                end,
                centre,
                direction,
            } => {
                let c = centre.to_mm_f64();
                let s = start.to_mm_f64();
                let start_deg = bearing(c, s);
                let turn = if start == end {
                    360.0
                } else {
                    let delta = bearing(c, end.to_mm_f64()) - start_deg;
                    match direction {
                        ArcDirection::CounterClockwise => delta.rem_euclid(360.0),
                        ArcDirection::Clockwise => (-delta).rem_euclid(360.0),
                    }
                };
                Shape::Arc {
                    center: c,
                    radius: crate::geometry::distance(c, s),
                    start_deg,
                    sweep_deg: match direction {
                        ArcDirection::CounterClockwise => turn,
                        ArcDirection::Clockwise => -turn,
                    },
                }
            }
        }
    }

    /// A computed shape stored: every point and radius rounded to whole
    /// nanometres ([`Nm::from_mm_f64_rounded`]). An arc stores its two end
    /// points (`Shape::arc_ends`) rounded, turning counter-clockwise for a
    /// positive sweep; a sweep of a whole turn (|sweep| ≥ 360°) stores its
    /// end as its start. Refused: a value out of bounds, and an arc that is
    /// not one ([`ArcNm::check`]: no radius left), or whose ends round to
    /// one point although it is not a whole turn.
    pub fn from_mm_rounded(shape: &Shape) -> Result<Self> {
        Ok(match shape {
            Shape::Line { a, b } => Self::Line {
                a: PointNm::from_mm_f64_rounded(*a)?,
                b: PointNm::from_mm_f64_rounded(*b)?,
            },
            Shape::Polyline { points, radii } => Self::Polyline {
                points: rounded(points)?,
                radii_nm: rounded_radii(radii)?,
            },
            Shape::Polygon { points, radii } => Self::Polygon {
                points: rounded(points)?,
                radii_nm: rounded_radii(radii)?,
            },
            Shape::Rect { a, b, radius } => Self::Rect {
                a: PointNm::from_mm_f64_rounded(*a)?,
                b: PointNm::from_mm_f64_rounded(*b)?,
                radius_nm: Nm::from_mm_f64_rounded(*radius)?,
            },
            Shape::Circle { center, radius } => Self::Circle {
                center: PointNm::from_mm_f64_rounded(*center)?,
                radius_nm: Nm::from_mm_f64_rounded(*radius)?,
            },
            Shape::Arc {
                center, sweep_deg, ..
            } => {
                let [s, e] = shape
                    .arc_ends()
                    .ok_or_else(|| Error::Invalid("an arc has no ends".into()))?;
                if !sweep_deg.is_finite() {
                    return Err(Error::Invalid("an arc's sweep must be finite".into()));
                }
                let full = sweep_deg.abs() >= 360.0;
                let start = PointNm::from_mm_f64_rounded(s)?;
                let end = if full {
                    start
                } else {
                    PointNm::from_mm_f64_rounded(e)?
                };
                if !full && end == start {
                    return Err(Error::Invalid(
                        "an arc's ends round to one point; it would be a whole turn".into(),
                    ));
                }
                let arc = Self::Arc {
                    start,
                    end,
                    centre: PointNm::from_mm_f64_rounded(*center)?,
                    direction: if *sweep_deg < 0.0 {
                        ArcDirection::Clockwise
                    } else {
                        ArcDirection::CounterClockwise
                    },
                };
                arc.check_arc()?;
                arc
            }
        })
    }

    fn check_arc(&self) -> Result<()> {
        match self {
            Self::Arc {
                start,
                end,
                centre,
                direction,
            } => ArcNm {
                end: *end,
                centre: *centre,
                direction: *direction,
            }
            .check(*start),
            _ => Ok(()),
        }
    }

    /// A square-cornered rectangle.
    pub fn rect(a: PointNm, b: PointNm) -> Self {
        Self::Rect {
            a,
            b,
            radius_nm: Nm::ZERO,
        }
    }

    /// Polygons, rectangles and circles enclose an area.
    pub fn is_closed(&self) -> bool {
        matches!(
            self,
            Self::Rect { .. } | Self::Circle { .. } | Self::Polygon { .. }
        )
    }

    /// The points the shape stores; a bound on encoded size. Corner radii
    /// are bounded by the same count.
    pub fn stored_points(&self) -> usize {
        match self {
            Self::Line { .. } | Self::Rect { .. } => 2,
            Self::Circle { .. } => 1,
            Self::Arc { .. } => 3,
            Self::Polyline { points, .. } | Self::Polygon { points, .. } => points.len(),
        }
    }

    /// Radii are not negative; corner radii are absent or one per point;
    /// an arc is well formed ([`ArcNm::check`]). Every value is within the
    /// coordinate bound by its type.
    pub fn valid(&self) -> bool {
        let radius_ok = |r: &Nm| r.get() >= 0;
        match self {
            Self::Line { .. } => true,
            Self::Rect { radius_nm, .. } => radius_ok(radius_nm),
            Self::Polyline { points, radii_nm } | Self::Polygon { points, radii_nm } => {
                (radii_nm.is_empty() || radii_nm.len() == points.len())
                    && radii_nm.iter().all(radius_ok)
            }
            Self::Circle { radius_nm, .. } => radius_ok(radius_nm),
            Self::Arc { .. } => self.check_arc().is_ok(),
        }
    }

    /// The shape moved by `delta`, exactly; an error when a point would
    /// leave the coordinate bound.
    pub fn translated(&self, delta: PointNm) -> Result<Self> {
        let mv = |p: &PointNm| -> Result<PointNm> {
            Ok(PointNm::new(
                p.x_nm.checked_add(delta.x_nm)?,
                p.y_nm.checked_add(delta.y_nm)?,
            ))
        };
        let all = |points: &[PointNm]| points.iter().map(mv).collect::<Result<Vec<_>>>();
        Ok(match self {
            Self::Line { a, b } => Self::Line {
                a: mv(a)?,
                b: mv(b)?,
            },
            Self::Polyline { points, radii_nm } => Self::Polyline {
                points: all(points)?,
                radii_nm: radii_nm.clone(),
            },
            Self::Polygon { points, radii_nm } => Self::Polygon {
                points: all(points)?,
                radii_nm: radii_nm.clone(),
            },
            Self::Rect { a, b, radius_nm } => Self::Rect {
                a: mv(a)?,
                b: mv(b)?,
                radius_nm: *radius_nm,
            },
            Self::Circle { center, radius_nm } => Self::Circle {
                center: mv(center)?,
                radius_nm: *radius_nm,
            },
            Self::Arc {
                start,
                end,
                centre,
                direction,
            } => Self::Arc {
                start: mv(start)?,
                end: mv(end)?,
                centre: mv(centre)?,
                direction: *direction,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn p(x: i64, y: i64) -> PointNm {
        PointNm::new(Nm::new(x).unwrap(), Nm::new(y).unwrap())
    }

    #[test]
    fn arcs_store_their_ends_and_compute_their_sweep() -> Result<()> {
        let quarter = Shape::Arc {
            center: [1.0, 0.0],
            radius: 2.0,
            start_deg: 90.0,
            sweep_deg: -90.0,
        };
        let stored = ShapeNm::from_mm_rounded(&quarter).unwrap();
        assert_eq!(
            stored,
            ShapeNm::Arc {
                start: p(1_000_000, 2_000_000),
                end: p(3_000_000, 0),
                centre: p(1_000_000, 0),
                direction: ArcDirection::Clockwise,
            }
        );
        assert_eq!(stored.to_mm(), quarter);

        let full = Shape::Arc {
            center: [0.0, 0.0],
            radius: 1.5,
            start_deg: 0.0,
            sweep_deg: 360.0,
        };
        let stored = ShapeNm::from_mm_rounded(&full).unwrap();
        assert!(matches!(stored, ShapeNm::Arc { start, end, .. } if start == end));
        assert_eq!(stored.to_mm(), full);

        // An oblique arc rounds its ends to whole nanometres.
        let oblique = Shape::Arc {
            center: [0.0, 0.0],
            radius: 1.0,
            start_deg: 45.0,
            sweep_deg: 90.0,
        };
        let ShapeNm::Arc { start, end, .. } = ShapeNm::from_mm_rounded(&oblique)? else {
            return Err(Error::Invalid("an arc".into()));
        };
        assert_eq!(start, p(707_107, 707_107));
        assert_eq!(end, p(-707_107, 707_107));
        Ok(())
    }

    #[test]
    fn degenerate_arcs_are_refused() {
        let flat = Shape::Arc {
            center: [0.0, 0.0],
            radius: 1.0,
            start_deg: 10.0,
            sweep_deg: 1e-9,
        };
        assert!(ShapeNm::from_mm_rounded(&flat).is_err());
        let point = Shape::Arc {
            center: [0.0, 0.0],
            radius: 0.0,
            start_deg: 0.0,
            sweep_deg: 360.0,
        };
        assert!(ShapeNm::from_mm_rounded(&point).is_err());
    }

    #[test]
    fn shapes_round_trip_and_carry_millimetres_on_the_wire() {
        let rect = Shape::Rect {
            a: [-1.25, 0.5],
            b: [2.0, 3.000_000_4],
            radius: 0.25,
        };
        let stored = ShapeNm::from_mm_rounded(&rect).unwrap();
        assert_eq!(
            serde_json::to_value(&stored).unwrap(),
            serde_json::json!({"rect": {
                "a": {"x_nm": -1_250_000, "y_nm": 500_000},
                "b": {"x_nm": 2_000_000, "y_nm": 3_000_000},
                "radius_nm": 250_000
            }})
        );
        let wire = serde_json::to_value(ShapeNmParams::from(stored.clone())).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"rect": {"a": [-1.25, 0.5], "b": [2.0, 3.0], "radius": 0.25}})
        );
        let back: ShapeNmParams = serde_json::from_value(wire).unwrap();
        assert_eq!(ShapeNm::from(back), stored);
        assert_eq!(
            stored.translated(p(1, -1)).unwrap(),
            ShapeNm::Rect {
                a: p(-1_249_999, 499_999),
                b: p(2_000_001, 2_999_999),
                radius_nm: Nm::new(250_000).unwrap(),
            }
        );
    }
}
