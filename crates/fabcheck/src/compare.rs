//! Two images of one layer compared exactly: D = A xor B, its area, the
//! width of its widest piece (twice the radius of the largest disc that
//! fits inside D, found by shrinking D until it is empty) and the pieces
//! wider than a tolerance, with their boxes.
use crate::polygons::{self, Shapes};
use crate::{Fallible, Point};
use i_overlay::core::overlay_rule::OverlayRule;

/// Two layers compared.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerDiff {
    pub area_a_mm2: f64,
    pub area_b_mm2: f64,
    /// Area dark in exactly one of them, mm².
    pub difference_mm2: f64,
    /// Area dark in A only, and in B only, mm².
    pub only_a_mm2: f64,
    pub only_b_mm2: f64,
    /// Width of the widest piece of the difference, µm (0 when equal).
    pub worst_width_um: f64,
    /// The tolerance compared against, µm.
    pub tolerance_um: f64,
    /// The pieces wider than the tolerance.
    pub pieces: Vec<Piece>,
}

impl LayerDiff {
    /// Whether no piece of the difference is wider than the tolerance.
    pub fn passes(&self) -> bool {
        self.worst_width_um <= self.tolerance_um
    }
}

/// A piece of the difference wider than the tolerance: what is left of it
/// after shrinking by half the tolerance (its box and outer ring, mm) and
/// the piece's own width.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub min: Point,
    pub max: Point,
    /// Area of the shrunk piece, mm².
    pub area_mm2: f64,
    /// Width of the piece, µm (to 1 nm, from the shrunk piece).
    pub width_um: f64,
    /// The shrunk piece's outer ring, mm.
    pub outline: Vec<Point>,
    /// Dark in A only (else in B only).
    pub only_in_a: bool,
}

impl Piece {
    pub fn center(&self) -> Point {
        [
            (self.min[0] + self.max[0]) / 2.0,
            (self.min[1] + self.max[1]) / 2.0,
        ]
    }
}

/// Whether anything with area is left of `d` shrunk by `by` nm.
fn survives(d: &Shapes, by: i32) -> Fallible<bool> {
    Ok(polygons::offset(d, -by)?
        .iter()
        .any(|s| s.iter().map(|r| polygons::area2(r)).sum::<i128>() > 0))
}

/// Width of the widest piece of `d`, nm: twice the largest shrink that
/// leaves something, to 1 nm.
pub fn worst_width_nm(d: &Shapes) -> Fallible<i64> {
    if d.is_empty() || !survives(d, 0)? {
        return Ok(0);
    }
    let (mut lo, mut hi) = (0i64, 1i64);
    while survives(d, i32::try_from(hi)?)? {
        lo = hi;
        hi = hi.saturating_mul(2);
        if hi > i64::from(i32::MAX / 2) {
            return Err("compare: a difference piece wider than 1 m".into());
        }
    }
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if survives(d, i32::try_from(mid)?)? {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(2 * lo)
}

/// A against B with pieces wider than `tolerance_nm` listed.
pub fn compare(a: &Shapes, b: &Shapes, tolerance_nm: i32) -> Fallible<LayerDiff> {
    let d = polygons::boolean(a, b, OverlayRule::Xor);
    let only_a = polygons::boolean(a, b, OverlayRule::Difference);
    let only_b = polygons::boolean(b, a, OverlayRule::Difference);
    let worst = worst_width_nm(&d)?;
    let mut pieces = Vec::new();
    // The two sides of the difference touch only along edges, so their
    // pieces are the difference's pieces.
    for (side, only_in_a) in [(&only_a, true), (&only_b, false)] {
        if worst <= i64::from(tolerance_nm) {
            break;
        }
        for shape in polygons::offset(side, -(tolerance_nm / 2))? {
            let area: i128 = shape.iter().map(|r| polygons::area2(r)).sum();
            if area <= 0 {
                continue;
            }
            let Some(outer) = shape.first() else {
                continue;
            };
            let mut min = [f64::INFINITY; 2];
            let mut max = [f64::NEG_INFINITY; 2];
            let outline: Vec<Point> = outer
                .iter()
                .map(|p| [f64::from(p.x) / 1e6, f64::from(p.y) / 1e6])
                .collect();
            for q in &outline {
                for k in 0..2 {
                    min[k] = min[k].min(q[k]);
                    max[k] = max[k].max(q[k]);
                }
            }
            let inner = worst_width_nm(&vec![shape.clone()])?;
            pieces.push(Piece {
                min,
                max,
                area_mm2: area as f64 / 2.0 / 1e12,
                width_um: (inner + i64::from(tolerance_nm / 2) * 2) as f64 / 1e3,
                outline,
                only_in_a,
            });
        }
    }
    Ok(LayerDiff {
        area_a_mm2: polygons::area_mm2(a),
        area_b_mm2: polygons::area_mm2(b),
        difference_mm2: polygons::area_mm2(&d),
        only_a_mm2: polygons::area_mm2(&only_a),
        only_b_mm2: polygons::area_mm2(&only_b),
        worst_width_um: worst as f64 / 1e3,
        tolerance_um: f64::from(tolerance_nm) / 1e3,
        pieces,
    })
}
