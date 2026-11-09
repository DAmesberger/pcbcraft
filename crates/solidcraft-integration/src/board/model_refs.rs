//! Faces, edges and vertices of a part file as dimension references
//! (docs/board-sketch-references.md §5.4, §5.5; slice S3): the host side of
//! `layout::dimensions::model`.
//!
//! A reference names a face or edge by its persistent Solidcraft name in
//! the part file's own design, on a named body, with a pick point in the
//! model frame. The part is evaluated through the one part-file cache the
//! 3D build uses ([`part_file::evaluate`]), and the name is re-found with
//! Solidcraft's own lookup (`naming::locate_face_on_body`,
//! `naming::locate_edge`). At our pin the lookup reports whether the name
//! follows exact history (`verified`) and whether the stored pick still
//! lies on the face or edge (`on_point`); both must hold for `Resolved`,
//! anything less is `Inexact` (upstream's `face_on_body` `inexact` flag is
//! not at our pin).
//!
//! The part sits on the board with only x, y and the rotation free, so a
//! face square to the board traces an exact line in the board plane, a
//! vertical cylinder or cone (a hole or a boss) its axis point, an edge
//! parallel to the board its line or arc, and a vertex its point. A sloped
//! face, a face parallel to the board and a free-form face have no single
//! trace and are refused ("pick an edge of it").
use crate::geom::Vec3;
use crate::models::part_file::{self, EvaluatedPart};
use pcbcraft_core::diagnostics::{Error, Result};
use pcbcraft_core::geometry::Transform3;
use pcbcraft_core::layout::dimensions::FeatureGeometry;
use pcbcraft_core::layout::dimensions::model::{TraceRequest, Traced};
use pcbcraft_core::model::dimension::{
    End, FeatureRef, ModelEdgeRef, ModelFaceRef, ReferenceStatus,
};
use solidcraft_engine::doc::naming;
use solidcraft_engine::doc::{ModelBody, ModelState};
use solidcraft_engine::kernel::{FaceSurface, edge_circle, face_surface};
use std::f64::consts::TAU;

/// How far from square to the board (the sine of the angle) a direction
/// may be and still count as square.
const SQUARE: f64 = 1e-6;
/// How far from flat an edge's points may lie (mm) and still count as
/// parallel to the board, or as a straight edge.
const FLAT_MM: f64 = 1e-6;
/// Faces a refusal lists for an agent to pick from.
const LISTED: usize = 12;
/// Triangles of one face read for its extent.
const MAX_FACE_TRIANGLES: usize = 2_000_000;

fn arr(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

fn vec(p: [f64; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], p[2])
}

fn mm(v: f64) -> String {
    pcbcraft_core::layout::limits::mm(v)
}

fn point_words(p: [f64; 3]) -> String {
    format!("({}, {}, {})", mm(p[0]), mm(p[1]), mm(p[2]))
}

/// Trace one reference of a part file (the evaluator's `trace_model`).
pub fn trace(request: &TraceRequest<'_>) -> Result<Traced> {
    let part = part_file::evaluate(request.bytes, &request.part.values)
        .map_err(|e| Error::Invalid(format!("part_file.read: {e}")))?;
    match request.feature {
        FeatureRef::Face { face, .. } => trace_face(&part, face, &request.frame),
        FeatureRef::Edge { edge, .. } => trace_edge(&part, edge, None, &request.frame),
        FeatureRef::Vertex { edge, end, .. } => trace_edge(&part, edge, Some(*end), &request.frame),
        _ => Err(Error::Invalid(
            "layout.dimension: only faces, edges and vertices are traced on a 3D model".into(),
        )),
    }
}

/// The solid body of the part called `name`.
fn body<'a>(state: &'a ModelState, name: &str) -> Result<&'a ModelBody> {
    state
        .bodies
        .iter()
        .find(|b| b.name == name && !b.body.is_mesh())
        .ok_or_else(|| {
            let names: Vec<&str> = state
                .bodies
                .iter()
                .filter(|b| !b.body.is_mesh())
                .take(LISTED)
                .map(|b| b.name.as_str())
                .collect();
            Error::NotFound(format!(
                "layout.dimension_reference: the 3D model has no body ‘{name}’; it has {}",
                names.join(", ")
            ))
        })
}

/// The status of a found face or edge, and why it is not resolved.
fn status(
    verified: bool,
    on_point: bool,
    pick: [f64; 3],
    spot: Option<[f64; 3]>,
) -> (ReferenceStatus, String) {
    match (verified, on_point) {
        (true, true) => (ReferenceStatus::Resolved, String::new()),
        (true, false) => (
            ReferenceStatus::Inexact,
            match spot {
                Some(spot) => format!(
                    "the pick {} is no longer on it; a point on it is {}",
                    point_words(pick),
                    point_words(spot)
                ),
                None => format!("the pick {} is no longer on it", point_words(pick)),
            },
        ),
        (false, _) => (
            ReferenceStatus::Inexact,
            "its name was worked out from the geometry, not from the part's history".into(),
        ),
    }
}

/// Faces of `body` square to the board in the part frame, by name with a
/// point on each: what a refusal offers an agent to pick from.
fn square_faces(body: &ModelBody, frame: &Transform3) -> String {
    let names = naming::face_names(body);
    let spots = naming::face_spots(body);
    let listed: Vec<String> = names
        .iter()
        .zip(&spots)
        .enumerate()
        .filter_map(|(i, (name, spot))| {
            let spot = spot.as_ref()?;
            match face_surface(&body.body, i)? {
                FaceSurface::Plane { normal, .. } => {
                    let n = frame.apply_vector(arr(normal.vec()));
                    (n[2].abs() <= SQUARE && !name.is_empty())
                        .then(|| format!("‘{name}’ at {}", point_words(arr(spot.point))))
                }
                _ => None,
            }
        })
        .take(LISTED)
        .collect();
    if listed.is_empty() {
        "it has no face square to the board".into()
    } else {
        format!("faces square to the board: {}", listed.join(", "))
    }
}

fn trace_face(part: &EvaluatedPart, face: &ModelFaceRef, frame: &Transform3) -> Result<Traced> {
    let state = part.state();
    let named = body(state, &face.body.0)?;
    let pick = vec(face.pick);
    let hit = naming::locate_face_on_body(state, Some(&face.body.0), &face.face.0, pick)
        .ok_or_else(|| {
            Error::NotFound(format!(
                "layout.dimension_reference: body ‘{}’ of the 3D model has no face ‘{}’ ({})",
                face.body.0,
                face.face.0,
                square_faces(named, frame)
            ))
        })?;
    let b = state.bodies.get(hit.body).ok_or_else(|| {
        Error::NotFound("layout.dimension_reference: the face's body is gone".into())
    })?;
    let sloped = || {
        Error::Unsupported(format!(
            "layout.dimension_sloped: face ‘{}’ has no single trace in the board plane; pick an \
             edge of it",
            face.face.0
        ))
    };
    let geometry = match face_surface(&b.body, hit.face) {
        Some(FaceSurface::Plane { normal, point }) => {
            let n = frame.apply_vector(arr(normal.vec()));
            if (n[2].abs() - 1.0).abs() <= SQUARE {
                return Err(Error::Unsupported(format!(
                    "layout.dimension_sloped: face ‘{}’ is parallel to the board; pick an edge of \
                     it",
                    face.face.0
                )));
            }
            if n[2].abs() > SQUARE {
                return Err(sloped());
            }
            plane_line(b, hit.face, n, frame.apply(arr(point)), frame).ok_or_else(sloped)?
        }
        Some(FaceSurface::Cylinder { origin, axis, .. }) => {
            axis_point(frame, arr(origin), arr(axis.vec())).ok_or_else(sloped)?
        }
        Some(FaceSurface::Cone { apex, axis, .. }) => {
            axis_point(frame, arr(apex), arr(axis.vec())).ok_or_else(sloped)?
        }
        _ => return Err(sloped()),
    };
    let spot = naming::face_spot(b, hit.face).map(|s| arr(s.point));
    let (status, note) = status(hit.verified, hit.on_point, face.pick, spot);
    Ok(Traced {
        status,
        geometry,
        note,
    })
}

/// The centre of a cylinder or cone standing on the board: its axis's
/// point in the board plane. `None` for an axis that is not vertical.
fn axis_point(frame: &Transform3, origin: [f64; 3], axis: [f64; 3]) -> Option<FeatureGeometry> {
    let a = frame.apply_vector(axis);
    if a[0].hypot(a[1]) > SQUARE {
        return None;
    }
    let o = frame.apply(origin);
    Some(FeatureGeometry::Point { at: [o[0], o[1]] })
}

/// A planar face square to the board as its line in the board plane: the
/// face's horizontal direction, counter-clockwise round the part seen from
/// above (outward normal on the right), over the face's extent.
fn plane_line(
    b: &ModelBody,
    face: usize,
    n: [f64; 3],
    on: [f64; 3],
    frame: &Transform3,
) -> Option<FeatureGeometry> {
    let flat = n[0].hypot(n[1]);
    if flat <= SQUARE {
        return None;
    }
    let t = [-n[1] / flat, n[0] / flat];
    let mesh = b.mesh();
    if mesh.triangles.len() > MAX_FACE_TRIANGLES {
        return None;
    }
    let mut span: Option<(f64, f64)> = None;
    for (tri, f) in mesh.triangles.iter().zip(&mesh.tri_face) {
        if *f as usize != face {
            continue;
        }
        for corner in mesh.tri(tri)? {
            let p = frame.apply(arr(corner));
            let s = (p[0] - on[0]) * t[0] + (p[1] - on[1]) * t[1];
            span = Some(span.map_or((s, s), |(lo, hi)| (lo.min(s), hi.max(s))));
        }
    }
    let (lo, hi) = span?;
    (hi - lo > FLAT_MM).then(|| FeatureGeometry::Line {
        a: [on[0] + t[0] * lo, on[1] + t[1] * lo],
        b: [on[0] + t[0] * hi, on[1] + t[1] * hi],
    })
}

fn trace_edge(
    part: &EvaluatedPart,
    edge: &ModelEdgeRef,
    end: Option<End>,
    frame: &Transform3,
) -> Result<Traced> {
    let state = part.state();
    let b = body(state, &edge.body.0)?;
    let hit = naming::locate_edge(b, &edge.edge.0, vec(edge.pick)).map_err(|why| {
        Error::NotFound(format!(
            "layout.dimension_reference: body ‘{}’ of the 3D model has no edge ‘{}’: {why}",
            edge.body.0, edge.edge.0
        ))
    })?;
    let mesh = b.mesh();
    let points: Vec<[f64; 3]> = mesh
        .edges
        .get(hit.edge)
        .ok_or_else(|| Error::NotFound("layout.dimension_reference: the edge is gone".into()))?
        .iter()
        .map(|p| frame.apply(arr(*p)))
        .collect();
    let (Some(first), Some(last)) = (points.first().copied(), points.last().copied()) else {
        return Err(Error::NotFound(
            "layout.dimension_reference: the edge has no points".into(),
        ));
    };
    let flat = points.iter().all(|p| (p[2] - first[2]).abs() <= FLAT_MM);
    let geometry = match end {
        Some(End::Start) => FeatureGeometry::Point {
            at: [first[0], first[1]],
        },
        Some(End::End) => FeatureGeometry::Point {
            at: [last[0], last[1]],
        },
        None if !flat => {
            return Err(Error::Unsupported(format!(
                "layout.dimension_sloped: edge ‘{}’ is not parallel to the board; pick a vertex \
                 of it",
                edge.edge.0
            )));
        }
        None => edge_trace(b, hit.edge, &points, frame).ok_or_else(|| {
            Error::Unsupported(format!(
                "layout.dimension_sloped: edge ‘{}’ is neither straight nor circular; pick a \
                 vertex of it",
                edge.edge.0
            ))
        })?,
    };
    let (status, note) = status(hit.verified, hit.on_point, edge.pick, Some(arr(hit.point)));
    Ok(Traced {
        status,
        geometry,
        note,
    })
}

/// A flat edge's trace: a circle, an arc, or a line.
fn edge_trace(
    b: &ModelBody,
    edge: usize,
    points: &[[f64; 3]],
    frame: &Transform3,
) -> Option<FeatureGeometry> {
    if let Some(c) = edge_circle(&b.body, edge) {
        let center = frame.apply(arr(c.center));
        if (c.angle - TAU).abs() <= 1e-9 {
            return Some(FeatureGeometry::Circle {
                center: [center[0], center[1]],
                radius: c.radius,
            });
        }
        let (s, e) = (frame.apply(arr(c.start)), frame.apply(arr(c.end)));
        // Counter-clockwise about its axis; seen from above when the axis
        // points up.
        let up = frame.apply_vector(arr(c.axis.vec()))[2];
        let bulge = (c.angle / 4.0).tan() * up.signum();
        return Some(FeatureGeometry::Arc {
            a: [s[0], s[1]],
            b: [e[0], e[1]],
            bulge,
        });
    }
    let (a, z) = (*points.first()?, *points.last()?);
    let d = [z[0] - a[0], z[1] - a[1]];
    let len = d[0].hypot(d[1]);
    if len <= FLAT_MM {
        return None;
    }
    let straight = points.iter().all(|p| {
        let off = ((p[0] - a[0]) * d[1] - (p[1] - a[1]) * d[0]) / len;
        off.abs() <= FLAT_MM.max(len * 1e-9)
    });
    straight.then_some(FeatureGeometry::Line {
        a: [a[0], a[1]],
        b: [z[0], z[1]],
    })
}

#[cfg(test)]
#[path = "model_refs_engine_tests.rs"]
mod engine_tests;
#[cfg(test)]
#[path = "model_refs_tests.rs"]
mod tests;
