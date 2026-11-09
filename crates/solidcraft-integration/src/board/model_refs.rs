//! Faces, edges and vertices of a part's 3D model as dimension references
//! (docs/board-sketch-references.md §5.4, §5.5, §6.2; slices S3 and S4):
//! the host side of `layout::dimensions::model`.
//!
//! A reference names a face or edge by its persistent Solidcraft name in
//! the model's own design, on a named body, with a pick point in the model
//! frame. The design is the one the 3D build uses ([`ModelDesign`]): a
//! part file evaluated through the part-file cache ([`part_file::
//! evaluate`]); a generated model's standalone design, which the board
//! inserts, or the envelope's one-box design ([`named::package`]); the
//! design that imports a STEP model ([`named::step`]). The name is
//! re-found with Solidcraft's own lookup (`naming::locate_face_on_body`,
//! `naming::locate_edge`), which reports whether the name follows exact
//! history (`verified`) and whether the stored pick still lies on the face
//! or edge (`on_point`). The pick must lie on it for `Resolved`. In the
//! design the reference was picked on (`TraceRequest::picked_here`) every
//! name is the one picked; in another design a name worked out from
//! geometry (every STEP face's, some generated faces') is only `Inexact`
//! until the face is picked again (upstream's `face_on_body` `inexact`
//! flag is not at our pin).
//!
//! The part sits on the board with only x, y and the rotation free, so a
//! face square to the board traces an exact line in the board plane, a
//! vertical cylinder or cone (a hole or a boss) its axis point, an edge
//! parallel to the board its line or arc, and a vertex its point. A sloped
//! face, a face parallel to the board and a free-form face have no single
//! trace and are refused ("pick an edge of it").
use crate::geom::Vec3;
use crate::models::named::{self, NamedDesign};
use crate::models::part_file::{self, EvaluatedPart};
use pcbcraft_core::diagnostics::{Error, Result};
use pcbcraft_core::geometry::Transform3;
use pcbcraft_core::layout::dimensions::FeatureGeometry;
use pcbcraft_core::layout::dimensions::model::{TraceModel, TraceRequest, Traced};
use pcbcraft_core::model::dimension::{
    End, FeatureRef, ModelEdgeRef, ModelFaceRef, ReferenceStatus,
};
use solidcraft_engine::doc::naming;
use solidcraft_engine::doc::{ModelBody, ModelState};
use solidcraft_engine::kernel::{FaceSurface, edge_circle, face_surface};
use std::f64::consts::TAU;
use std::sync::Arc;

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
/// A point a note offers to pick again lies this far from the stored pick
/// at least (mm), so picking it is a new reference.
const REPICK_MM: f64 = 1e-3;

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

/// The evaluated design a model reference is traced on, and a pick in the
/// 3D view is named on (`naming::face_name_at_body` on [`Self::state`],
/// the pick in the model frame).
#[derive(Clone, Debug)]
pub enum ModelDesign {
    Part(Arc<EvaluatedPart>),
    Named(Arc<NamedDesign>),
}

impl ModelDesign {
    /// The design of `model`, from the cache the 3D build fills.
    pub fn of(model: &TraceModel<'_>) -> Result<Self> {
        let read = |e: crate::Error| Error::Invalid(format!("model.read: {e}"));
        match *model {
            TraceModel::PartFile { part, bytes } => part_file::evaluate(bytes, &part.values)
                .map(Self::Part)
                .map_err(|e| Error::Invalid(format!("part_file.read: {e}"))),
            TraceModel::Package { package } => {
                named::package(package).map(Self::Named).map_err(read)
            }
            TraceModel::Step { model, bytes } => {
                named::step(model, bytes).map(Self::Named).map_err(read)
            }
        }
    }

    /// Its evaluated bodies with their persistent face names, model frame.
    pub fn state(&self) -> &ModelState {
        match self {
            Self::Part(part) => part.state(),
            Self::Named(design) => design.state(),
        }
    }
}

/// Trace one reference of a part's 3D model (the evaluator's
/// `trace_model`).
pub fn trace(request: &TraceRequest<'_>) -> Result<Traced> {
    let design = ModelDesign::of(&request.model)?;
    let state = design.state();
    // An imported file has no history: its import numbers the faces of
    // each body in file order, so a name is exact for those bytes only
    // (§6.2). Another file is another design, and `picked_here` is false.
    let here = Trust {
        picked_here: request.picked_here,
        history: !matches!(request.model, TraceModel::Step { .. }),
    };
    match request.feature {
        FeatureRef::Face { face, .. } => trace_face(state, face, &request.frame, here),
        FeatureRef::Edge { edge, .. } => trace_edge(state, edge, None, &request.frame, here),
        FeatureRef::Vertex { edge, end, .. } => {
            trace_edge(state, edge, Some(*end), &request.frame, here)
        }
        _ => Err(Error::Invalid(
            "layout.dimension: only faces, edges and vertices are traced on a 3D model".into(),
        )),
    }
}

/// How far a found name may be trusted.
#[derive(Clone, Copy, Debug)]
struct Trust {
    /// The design is the one the reference was picked on.
    picked_here: bool,
    /// The design has history, so a verified name follows it.
    history: bool,
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

/// The status of a found face or edge, and why it is not resolved. A name
/// counts as exact when it follows the model's history or the design is
/// the one it was picked on (`picked_here`). `repick` is a point on it,
/// apart from the pick, to pick it again at.
fn status(
    exact: (bool, bool),
    on_point: bool,
    pick: [f64; 3],
    repick: Option<[f64; 3]>,
) -> (ReferenceStatus, String) {
    let (verified, picked_here) = exact;
    let again = match repick {
        Some(p) => format!("; pick it again (a point on it is {})", point_words(p)),
        None => "; pick it again".into(),
    };
    if !on_point {
        return (
            ReferenceStatus::Inexact,
            format!("the pick {} is no longer on it{again}", point_words(pick)),
        );
    }
    if verified || picked_here {
        return (ReferenceStatus::Resolved, String::new());
    }
    (
        ReferenceStatus::Inexact,
        format!(
            "the 3D model changed since it was picked, and its name was worked out from the \
             geometry, not from the model's history (a STEP model's always is){again}"
        ),
    )
}

/// A point on face `face` of `b` to pick it again at: its spot, or a
/// triangle's centre, at least [`REPICK_MM`] from `pick`.
fn face_repick(b: &ModelBody, face: usize, pick: [f64; 3]) -> Option<[f64; 3]> {
    let far = |p: &[f64; 3]| {
        let d = [p[0] - pick[0], p[1] - pick[1], p[2] - pick[2]];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() >= REPICK_MM
    };
    if let Some(spot) = naming::face_spot(b, face).map(|s| arr(s.point))
        && far(&spot)
    {
        return Some(spot);
    }
    let mesh = b.mesh();
    mesh.triangles
        .iter()
        .zip(&mesh.tri_face)
        .filter(|(_, f)| **f as usize == face)
        .filter_map(|(t, _)| mesh.tri(t))
        .map(|[a, b, c]| arr((a + b + c) / 3.0))
        .find(far)
}

/// A point on an edge (`points`, its polyline) to pick it again at: `at`,
/// or a polyline point, at least [`REPICK_MM`] from `pick`.
fn edge_repick(at: [f64; 3], points: &[Vec3], pick: [f64; 3]) -> Option<[f64; 3]> {
    let far = |p: &[f64; 3]| {
        let d = [p[0] - pick[0], p[1] - pick[1], p[2] - pick[2]];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() >= REPICK_MM
    };
    std::iter::once(at)
        .chain(points.windows(2).map(|w| arr((w[0] + w[1]) / 2.0)))
        .find(far)
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

fn trace_face(
    state: &ModelState,
    face: &ModelFaceRef,
    frame: &Transform3,
    trust: Trust,
) -> Result<Traced> {
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
    let repick = face_repick(b, hit.face, face.pick);
    let (status, note) = status(
        (hit.verified && trust.history, trust.picked_here),
        hit.on_point,
        face.pick,
        repick,
    );
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
    state: &ModelState,
    edge: &ModelEdgeRef,
    end: Option<End>,
    frame: &Transform3,
    trust: Trust,
) -> Result<Traced> {
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
    let polyline = mesh.edges.get(hit.edge).map_or(&[][..], Vec::as_slice);
    let repick = edge_repick(arr(hit.point), polyline, edge.pick);
    let (status, note) = status(
        (hit.verified && trust.history, trust.picked_here),
        hit.on_point,
        edge.pick,
        repick,
    );
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
#[path = "model_refs_kinds_tests.rs"]
mod kinds_tests;
#[cfg(test)]
#[path = "model_refs_tests.rs"]
mod tests;
