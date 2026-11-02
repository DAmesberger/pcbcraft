//! Route postures: how one step of an interactive route reaches its target
//! from the route head. A step is a polyline of at most two segments (an
//! elbow) and, with a layer change, a via. The functions are pure: the
//! Route tool's live preview and its committed `layout.route_apply` edit set
//! come from the same [`step`], and a future push-and-shove strategy can take
//! the same proposed edit set as its input.
use super::{RouteEditSet, RouteRequest, ViaRequest};
use crate::geometry::Point;
use crate::ids::Id;
use crate::model::CopperLayer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The shortest track core accepts (`layout.route`), in mm. A shorter elbow
/// leg is dropped and its neighbours joined.
pub const MIN_SEGMENT_MM: f64 = 0.01;

/// How a route step bends on its way to the target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Posture {
    /// An orthogonal and a 45° diagonal leg.
    #[default]
    Diagonal,
    /// A horizontal and a vertical leg.
    Orthogonal,
    /// One straight segment at any angle.
    Free,
}

impl Posture {
    pub const ALL: [Posture; 3] = [Posture::Diagonal, Posture::Orthogonal, Posture::Free];

    /// The posture after this one, wrapping.
    pub fn next(self) -> Self {
        match self {
            Posture::Diagonal => Posture::Orthogonal,
            Posture::Orthogonal => Posture::Free,
            Posture::Free => Posture::Diagonal,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Posture::Diagonal => "45°",
            Posture::Orthogonal => "90°",
            Posture::Free => "Free",
        }
    }
}

/// The corner points of a step from `from` to `to`, both included. The
/// 45° elbow runs straight first (diagonal first when `flipped`); the 90°
/// elbow runs horizontally first (vertically first when `flipped`). Legs
/// shorter than [`MIN_SEGMENT_MM`] are dropped, so the result is `[from]`
/// when `to` is within reach of `from`, else two or three points ending at
/// `to`.
pub fn path(from: Point, to: Point, posture: Posture, flipped: bool) -> Vec<Point> {
    let [dx, dy] = [to[0] - from[0], to[1] - from[1]];
    let corner = match posture {
        Posture::Free => None,
        Posture::Orthogonal if flipped => Some([from[0], to[1]]),
        Posture::Orthogonal => Some([to[0], from[1]]),
        Posture::Diagonal => {
            let diagonal = dx.abs().min(dy.abs());
            let diag = [diagonal.copysign(dx), diagonal.copysign(dy)];
            if flipped {
                Some([from[0] + diag[0], from[1] + diag[1]])
            } else {
                Some([to[0] - diag[0], to[1] - diag[1]])
            }
        }
    };
    let far = |a: Point, b: Point| crate::geometry::distance(a, b) >= MIN_SEGMENT_MM;
    let mut points = vec![from];
    if let Some(corner) = corner.filter(|c| c.iter().all(|v| v.is_finite()))
        && far(from, corner)
        && far(corner, to)
    {
        points.push(corner);
    }
    if far(from, to) {
        points.push(to);
    }
    points
}

/// One step of an interactive route on `net` at `width`.
#[derive(Clone, Debug, PartialEq)]
pub struct StepRequest {
    pub net: Id,
    pub width: f64,
    pub posture: Posture,
    pub flipped: bool,
    /// Where the step starts: the route head, on `layer`.
    pub head: Point,
    pub layer: CopperLayer,
    /// A layer change on the way: the via, then routing on to the target on
    /// the second layer. The via may sit at the head (no first leg).
    pub via: Option<(ViaRequest, CopperLayer)>,
    pub to: Point,
}

impl StepRequest {
    /// The layer the step ends on.
    pub fn end_layer(&self) -> CopperLayer {
        self.via.map_or(self.layer, |(_, layer)| layer)
    }
}

/// The tracks and via one route step adds: the elbow from the head (to the
/// via, then from the via to the target on the via's layer). Empty when
/// there is nothing to add.
pub fn step(request: &StepRequest) -> RouteEditSet {
    let legs = |from: Point, to: Point, layer: CopperLayer| -> Vec<RouteRequest> {
        path(from, to, request.posture, request.flipped)
            .windows(2)
            .filter_map(|w| match w {
                [start, end] => Some(RouteRequest {
                    net: request.net,
                    start: *start,
                    end: *end,
                    width: request.width,
                    layer,
                }),
                _ => None,
            })
            .collect()
    };
    match request.via {
        Some((via, layer)) => {
            let mut add = legs(request.head, via.position, request.layer);
            add.extend(legs(via.position, request.to, layer));
            RouteEditSet {
                add,
                vias: vec![via],
                ..Default::default()
            }
        }
        None => RouteEditSet {
            add: legs(request.head, request.to, request.layer),
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        crate::geometry::distance(a, b) < 1e-9
    }

    fn angle(a: Point, b: Point) -> f64 {
        (b[1] - a[1]).atan2(b[0] - a[0]).to_degrees()
    }

    /// The 45° elbow is a straight leg and an exact diagonal; flipping
    /// puts the diagonal first. Both reach the target.
    #[test]
    fn the_45_degree_elbow_is_orthogonal_then_diagonal() {
        let points = path([0.0, 0.0], [5.0, 2.0], Posture::Diagonal, false);
        assert_eq!(points.len(), 3);
        assert!(close(points[1], [3.0, 0.0]));
        assert!(close(points[2], [5.0, 2.0]));
        assert!((angle(points[1], points[2]) - 45.0).abs() < 1e-9);
        let flipped = path([0.0, 0.0], [5.0, 2.0], Posture::Diagonal, true);
        assert!(close(flipped[1], [2.0, 2.0]));
        assert!(angle(flipped[1], flipped[2]).abs() < 1e-9);
        // Every quadrant keeps the diagonal at a multiple of 45°.
        for to in [[-4.0, 1.0], [1.0, -4.0], [-3.0, -7.0]] {
            for flip in [false, true] {
                let p = path([1.0, 1.0], to, Posture::Diagonal, flip);
                for w in p.windows(2) {
                    let a = angle(w[0], w[1]).rem_euclid(45.0);
                    assert!(a < 1e-9 || 45.0 - a < 1e-9, "{p:?}");
                }
                assert!(close(*p.last().unwrap_or(&[0.0; 2]), to));
            }
        }
    }

    /// A target on an axis or diagonal is one segment; one within reach of
    /// the head is none.
    #[test]
    fn straight_targets_need_no_corner() {
        assert_eq!(
            path([0.0; 2], [3.0, 3.0], Posture::Diagonal, false).len(),
            2
        );
        assert_eq!(
            path([0.0; 2], [0.0, 3.0], Posture::Orthogonal, false).len(),
            2
        );
        assert_eq!(path([0.0; 2], [0.005, 0.0], Posture::Free, false).len(), 1);
        let p = path([0.0; 2], [3.0, 1.0], Posture::Orthogonal, false);
        assert!(close(p[1], [3.0, 0.0]));
        let p = path([0.0; 2], [3.0, 1.0], Posture::Orthogonal, true);
        assert!(close(p[1], [0.0, 1.0]));
        assert_eq!(path([0.0; 2], [3.0, 1.0], Posture::Free, false).len(), 2);
    }

    /// A step with a layer change routes to the via on the first layer and
    /// on from it on the second.
    #[test]
    fn a_layer_change_step_routes_both_layers_through_the_via() {
        let via = ViaRequest::through(7, [4.0, 0.0]);
        let edits = step(&StepRequest {
            net: 7,
            width: 0.25,
            posture: Posture::Diagonal,
            flipped: false,
            head: [0.0, 0.0],
            layer: CopperLayer::Top,
            via: Some((via, CopperLayer::Bottom)),
            to: [6.0, 2.0],
        });
        assert_eq!(edits.vias, vec![via]);
        let layers: Vec<_> = edits.add.iter().map(|r| r.layer).collect();
        assert_eq!(layers, [CopperLayer::Top, CopperLayer::Bottom]);
        assert!(edits.add.iter().all(|r| r.width == 0.25 && r.net == 7));
        assert!(close(edits.add[0].end, [4.0, 0.0]));
        assert!(close(edits.add[1].start, [4.0, 0.0]));
    }
}
