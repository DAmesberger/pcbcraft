//! A bent strip's side profile: a centreline polyline thickened to a
//! uniform thickness, each corner rounded by a circular bend, as formed
//! leads are. The outline is exact lines and arcs, so the native body has
//! true cylindrical bend faces.
use crate::pad_outline::Contour;
use crate::{Error, Result};
use pcbcraft_core::library::{Outline, Segment};

type P = [f64; 2];

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1]]
}
fn scale(a: P, k: f64) -> P {
    [a[0] * k, a[1] * k]
}
fn len(a: P) -> f64 {
    a[0].hypot(a[1])
}
/// The left normal of a unit direction.
fn left(d: P) -> P {
    [-d[1], d[0]]
}

/// One rounded corner of the centreline.
struct Bend {
    /// Signed turn (counter-clockwise positive), radians.
    turn: f64,
    /// Tangent points on the centreline, before and after the corner.
    from: P,
    to: P,
    din: P,
    dout: P,
}

/// The closed outline of a strip `thickness` thick along `points`, each
/// interior corner bent with centreline radius `radius` (reduced where a
/// segment is too short to hold its bends). Ends are cut square.
pub fn bent_strip(points: &[P], thickness: f64, radius: f64) -> Result<Contour> {
    let invalid = |what: &str| Error::Invalid(format!("model.form: lead profile {what}"));
    if points.len() < 2 || thickness.is_nan() || thickness <= 0.0 || radius.is_nan() || radius < 0.0
    {
        return Err(invalid(
            "needs two points, a positive thickness and a radius",
        ));
    }
    let h = thickness / 2.0;
    let dirs: Vec<(P, f64)> = points
        .windows(2)
        .map(|w| {
            let d = sub(w[1], w[0]);
            let l = len(d);
            (scale(d, 1.0 / l), l)
        })
        .collect();
    if dirs.iter().any(|(_, l)| !(l.is_finite() && *l > 1e-9)) {
        return Err(invalid("has a zero-length segment"));
    }
    let mut bends = Vec::new();
    for (i, pair) in dirs.windows(2).enumerate() {
        let ((din, lin), (dout, lout)) = (pair[0], pair[1]);
        let cross = din[0] * dout[1] - din[1] * dout[0];
        let dot = din[0] * dout[0] + din[1] * dout[1];
        let turn = cross.atan2(dot);
        if turn.abs() < 1e-9 {
            continue;
        }
        if turn.abs() > 0.95 * std::f64::consts::PI {
            return Err(invalid("folds back on itself"));
        }
        let half = (turn.abs() / 2.0).tan();
        // Each segment gives at most 45 % of its length to one bend, and the
        // inner face keeps a positive radius.
        let fit = 0.45 * lin.min(lout) / half;
        let r = radius.min(fit).max(h * 1.0001);
        let tangent = r * half;
        if tangent > 0.5 * lin.min(lout) {
            return Err(invalid("segments are too short for the lead thickness"));
        }
        let corner = points[i + 1];
        bends.push(Bend {
            turn,
            from: sub(corner, scale(din, tangent)),
            to: add(corner, scale(dout, tangent)),
            din,
            dout,
        });
    }
    let (first, last) = match (dirs.first(), dirs.last(), points.first(), points.last()) {
        (Some((d0, _)), Some((dn, _)), Some(p0), Some(pn)) => ((*p0, *d0), (*pn, *dn)),
        _ => return Err(invalid("is empty")),
    };
    let side = |p: P, d: P, s: f64| add(p, scale(left(d), s * h));
    let start = side(first.0, first.1, 1.0);
    let mut segments = Vec::new();
    for b in &bends {
        segments.push(Segment::Line {
            to: side(b.from, b.din, 1.0),
        });
        segments.push(Segment::Arc {
            to: side(b.to, b.dout, 1.0),
            bulge: (b.turn / 4.0).tan(),
        });
    }
    segments.push(Segment::Line {
        to: side(last.0, last.1, 1.0),
    });
    segments.push(Segment::Line {
        to: side(last.0, last.1, -1.0),
    });
    for b in bends.iter().rev() {
        segments.push(Segment::Line {
            to: side(b.to, b.dout, -1.0),
        });
        segments.push(Segment::Arc {
            to: side(b.from, b.din, -1.0),
            bulge: -(b.turn / 4.0).tan(),
        });
    }
    segments.push(Segment::Line {
        to: side(first.0, first.1, -1.0),
    });
    segments.push(Segment::Line { to: start });
    Ok(Contour::Outline(Outline { start, segments }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gull_wing_strip_has_two_bends_and_uniform_thickness() -> Result<()> {
        let c = bent_strip(&[[0.0, 1.0], [0.6, 1.0], [1.2, 0.1], [2.0, 0.1]], 0.2, 0.2)?;
        let Contour::Outline(o) = &c else {
            return Err(Error::Verification("not an outline".into()));
        };
        let arcs = o
            .segments
            .iter()
            .filter(|s| matches!(s, Segment::Arc { .. }))
            .count();
        assert_eq!(arcs, 4);
        // Closed, and its flattened area is length × thickness to within
        // the bends' difference.
        assert_eq!(o.segments.last().map(Segment::to), Some(o.start));
        let poly = c.flatten(0.001)?;
        let area: f64 = poly
            .iter()
            .zip(poly.iter().cycle().skip(1))
            .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
            .sum::<f64>()
            .abs()
            / 2.0;
        let centre = 0.6 + (0.6f64.hypot(0.9)) + 0.8;
        assert!((area - centre * 0.2).abs() < 0.02, "{area}");
        Ok(())
    }

    #[test]
    fn degenerate_profiles_are_refused() {
        assert!(bent_strip(&[[0.0, 0.0]], 0.2, 0.2).is_err());
        assert!(bent_strip(&[[0.0, 0.0], [0.0, 0.0]], 0.2, 0.2).is_err());
        assert!(bent_strip(&[[0.0, 0.0], [1.0, 0.0], [0.0, 0.0]], 0.2, 0.2).is_err());
        assert!(bent_strip(&[[0.0, 0.0], [1.0, 0.0]], -0.2, 0.2).is_err());
    }
}
