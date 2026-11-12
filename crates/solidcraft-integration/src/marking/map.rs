//! Surface maps: chart → model. A decal is filled flat in its chart
//! (mm), then each triangle is taken onto its surface: a plane by its
//! frame, a cylinder by arc length round its axis (subdivided so the
//! chords stay within [`WRAP_CHORD_MM`] of the curve). A freeform map
//! is one more variant here: it needs only `point` and `frame` for a chart
//! point; flattening, placing and clipping stay as they are.
use crate::geom::Vec3;
use crate::geom::prelude::*;
use crate::{Error, Result};
use pcbcraft_core::Point;
use pcbcraft_core::library::marking::surface::{CylinderArea, WrapChart};
use pcbcraft_core::library::model3d::MarkingArea;

/// Largest gap between a wrapped decal's chords and its cylinder, mm.
pub const WRAP_CHORD_MM: f64 = 0.002;
/// Most slabs one triangle is cut into round a cylinder.
pub const MAX_SPLIT: usize = 4096;

/// A chart taken onto the model.
#[derive(Clone, Copy, Debug)]
pub enum SurfaceMap {
    Plane {
        origin: Vec3,
        u: Vec3,
        v: Vec3,
        n: Vec3,
    },
    Cylinder {
        cylinder: CylinderArea,
        chart: WrapChart,
    },
}

fn vec(a: [f64; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}

fn unit(v: Vec3) -> Option<Vec3> {
    let len = v.dot(v).sqrt();
    (len.is_finite() && len > 1e-9).then(|| v * (1.0 / len))
}

fn degenerate() -> Error {
    Error::Invalid("marking decal: the marking surface is degenerate".into())
}

impl SurfaceMap {
    /// A plane's map; its u axis is made square to the normal.
    pub fn plane(area: &MarkingArea) -> Result<Self> {
        let (origin, size) = (area.origin(), area.size());
        let all = origin
            .iter()
            .chain(&area.normal)
            .chain(&area.u_axis)
            .chain(&size);
        if !all.clone().all(|v| v.is_finite()) || origin.iter().any(|v| v.abs() > 1e5) {
            return Err(degenerate());
        }
        let n = unit(vec(area.normal)).ok_or_else(degenerate)?;
        let u = vec(area.u_axis);
        let u = unit(u - n * u.dot(n)).ok_or_else(degenerate)?;
        Ok(Self::Plane {
            origin: vec(area.origin()),
            u,
            v: n.cross(u),
            n,
        })
    }

    /// A cylinder's map under `chart`.
    pub fn cylinder(cylinder: CylinderArea, chart: WrapChart) -> Result<Self> {
        let finite = cylinder
            .base()
            .iter()
            .chain(&cylinder.axis)
            .chain(&cylinder.zero)
            .chain([&cylinder.radius(), &cylinder.length()])
            .all(|v| v.is_finite());
        let axis = unit(vec(cylinder.axis)).ok_or_else(degenerate)?;
        let zero = vec(cylinder.zero);
        let zero = unit(zero - axis * zero.dot(axis)).ok_or_else(degenerate)?;
        if !finite || cylinder.radius() <= 0.0 || cylinder.base().iter().any(|v| v.abs() > 1e5) {
            return Err(degenerate());
        }
        Ok(Self::Cylinder {
            cylinder: CylinderArea {
                axis: [axis.x, axis.y, axis.z],
                zero: [zero.x, zero.y, zero.z],
                ..cylinder
            },
            chart,
        })
    }

    /// The point `lift` mm off the surface at chart point `p`, the
    /// outward normal there and the chart's u and v directions.
    pub fn frame(&self, p: Point, lift: f64) -> (Vec3, Vec3, Vec3, Vec3) {
        match self {
            Self::Plane { origin, u, v, n } => {
                (*origin + *u * p[0] + *v * p[1] + *n * lift, *n, *u, *v)
            }
            Self::Cylinder { cylinder, chart } => {
                let (theta, t) = cylinder.chart_to_surface(chart, p);
                let (at, n) = cylinder.point(theta, t, lift);
                let (n, axis) = (vec(n), vec(cylinder.axis));
                let round = axis.cross(n);
                let (u, v) = if cylinder.along_u {
                    (axis, round * -1.0)
                } else {
                    (round, axis)
                };
                (vec(at), n, u, v)
            }
        }
    }

    /// The chart axis that runs round a cylinder (0 on a plane).
    pub fn round_axis(&self) -> usize {
        match self {
            Self::Plane { .. } => 0,
            Self::Cylinder { cylinder, .. } => usize::from(cylinder.along_u),
        }
    }

    /// How many pieces a triangle spanning `extent` mm of chart round a
    /// cylinder is cut into along each edge so its chords stay within
    /// [`WRAP_CHORD_MM`]: one on a plane.
    pub fn splits(&self, tri: [Point; 3]) -> usize {
        let Self::Cylinder { cylinder, .. } = self else {
            return 1;
        };
        let k = usize::from(cylinder.along_u);
        let (lo, hi) = tri
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), p| {
                (l.min(p[k]), h.max(p[k]))
            });
        let extent = hi - lo;
        // A chord of arc s sags r(1 − cos(s / 2r)) ≈ s² / 8r.
        let step = (8.0 * cylinder.radius() * WRAP_CHORD_MM).sqrt();
        if !(extent.is_finite() && step > 0.0) {
            return 1;
        }
        let n = (extent / step).ceil();
        if n >= MAX_SPLIT as f64 {
            MAX_SPLIT
        } else {
            (n as usize).max(1)
        }
    }
}

/// Clip a counter-clockwise triangle to the box `[x0, x1] × [y0, y1]`
/// (each bound `None` leaves that side open): the kept polygon, counter-
/// clockwise, empty when nothing is left.
pub fn clip(tri: [Point; 3], bounds: [Option<f64>; 4]) -> Vec<Point> {
    let mut poly: Vec<Point> = tri.to_vec();
    // (axis, limit, keep below)
    let planes = [
        (0usize, bounds[0], false),
        (0, bounds[1], true),
        (1, bounds[2], false),
        (1, bounds[3], true),
    ];
    for (axis, limit, below) in planes {
        let Some(limit) = limit else {
            continue;
        };
        let inside = |p: &Point| {
            if below {
                p[axis] <= limit
            } else {
                p[axis] >= limit
            }
        };
        let mut out = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            let (ia, ib) = (inside(&a), inside(&b));
            if ia {
                out.push(a);
            }
            if ia != ib {
                let t = (limit - a[axis]) / (b[axis] - a[axis]);
                out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            }
        }
        poly = out;
        if poly.len() < 3 {
            return Vec::new();
        }
    }
    poly
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_keeps_the_part_inside_and_its_winding() {
        let tri = [[-1.0, -1.0], [3.0, -1.0], [-1.0, 3.0]];
        let kept = clip(tri, [Some(0.0), Some(1.0), Some(0.0), Some(1.0)]);
        // The unit square lies wholly inside the triangle.
        let area: f64 = (0..kept.len())
            .map(|i| {
                let (a, b) = (kept[i], kept[(i + 1) % kept.len()]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f64>()
            / 2.0;
        assert!((area - 1.0).abs() < 1e-12, "{area}");
        assert!(clip(tri, [Some(5.0), None, None, None]).is_empty());
        assert_eq!(clip(tri, [None; 4]).len(), 3);
    }

    #[test]
    fn a_cylinder_frame_is_outward_and_right_handed() -> Result<()> {
        let cylinder = CylinderArea {
            base_nm: crate::test_units::nm3([0.0, 0.0, 0.0]),
            axis: [0.0, 0.0, 1.0],
            zero: [1.0, 0.0, 0.0],
            radius_nm: crate::test_units::nm(2.0),
            length_nm: crate::test_units::nm(4.0),
            along_u: false,
        };
        let chart = cylinder.chart(&Default::default()).ok_or_else(degenerate)?;
        let map = SurfaceMap::cylinder(cylinder, chart)?;
        let (at, n, u, v) = map.frame([0.0, 0.0], 0.01);
        // The chart centre faces −x (the default seam), lifted off it.
        assert!((at.x + 2.01).abs() < 1e-12 && (n.x + 1.0).abs() < 1e-12);
        let un = u.cross(v);
        assert!((un.dot(n) - 1.0).abs() < 1e-12);
        // About 0.18 mm steps on a 2 mm radius for 2 µm chords.
        assert_eq!(map.splits([[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]), 6);
        Ok(())
    }
}
