#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Slice S3: faces, edges and vertices of a part file traced in the board
//! plane, with hand-worked expected values. The part is a 4 × 2 × 1 mm box
//! with its corner at (−2, −1, 0) and a 1 mm hole through it at (0.5, 0).
use super::*;
use pcbcraft_core::library::package_model::PartFileModel;
use pcbcraft_core::model::dimension::{BodyName, FaceName, ModelEdgeName};
use serde_json::json;
use solidcraft_engine::Session;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The box part: (bytes, the body's name).
pub(super) fn boxed() -> (Vec<u8>, String) {
    let mut s = Session::default();
    s.execute("file.new", &json!({"name": "part"})).unwrap();
    let made = s
        .execute(
            "solid.box",
            &json!({"corner": [-2, -1, 0], "length": 4, "width": 2, "height": 1, "name": "Housing"}),
        )
        .unwrap();
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    s.execute(
        "solid.cylinder",
        &json!({"diameter": 1, "height": 3, "base": [0.5, 0, -1], "operation": "cut",
            "targets": [body], "name": "Hole"}),
    )
    .unwrap();
    s.execute(
        "appearance.assign",
        &json!({"bodies": [body], "name": "lcp", "color": "#202020", "opacity": 1.0}),
    )
    .unwrap();
    (solidcraft_engine::io::write_design(&s.doc), body)
}

fn evaluated(bytes: &[u8]) -> std::sync::Arc<EvaluatedPart> {
    part_file::evaluate(bytes, &Default::default()).unwrap()
}

/// The face of `body` whose outward normal is `normal` (model frame): its
/// name and a point on it.
fn face_with(
    part: &EvaluatedPart,
    body: &str,
    pred: impl Fn(&FaceSurface) -> bool,
) -> (String, [f64; 3]) {
    let b = part.state().bodies.iter().find(|b| b.name == body).unwrap();
    let names = naming::face_names(b);
    let spots = naming::face_spots(b);
    (0..names.len())
        .find(|i| face_surface(&b.body, *i).is_some_and(|s| pred(&s)))
        .map(|i| (names[i].clone(), arr(spots[i].unwrap().point)))
        .unwrap()
}

fn plane(n: [f64; 3]) -> impl Fn(&FaceSurface) -> bool {
    move |s| {
        matches!(s, FaceSurface::Plane { normal, .. } if (arr(normal.vec())[0] - n[0]).abs() < 1e-9
        && (arr(normal.vec())[1] - n[1]).abs() < 1e-9 && (arr(normal.vec())[2] - n[2]).abs() < 1e-9)
    }
}

fn face_ref(body: &str, name: &str, pick: [f64; 3]) -> FeatureRef {
    FeatureRef::Face {
        placement: 1,
        face: ModelFaceRef {
            body: BodyName(body.into()),
            face: FaceName(name.into()),
            pick,
        },
    }
}

fn run(bytes: &[u8], feature: &FeatureRef, frame: Transform3) -> Result<Traced> {
    let part = PartFileModel::default();
    trace(&TraceRequest {
        part: &part,
        bytes,
        frame,
        feature,
    })
}

fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

/// The +x face (x = 2) is square to the board: its line runs
/// counter-clockwise round the part, (2, −1) to (2, 1). Turned a quarter
/// about z by the model placement, it is the face y = 2 from (1, 2) to
/// (−1, 2).
#[test]
fn a_face_square_to_the_board_traces_its_line() -> Fallible {
    let (bytes, body) = boxed();
    let part = evaluated(&bytes);
    let (name, pick) = face_with(&part, &body, plane([1.0, 0.0, 0.0]));
    let traced = run(&bytes, &face_ref(&body, &name, pick), Transform3::IDENTITY)?;
    assert_eq!(traced.status, ReferenceStatus::Resolved, "{}", traced.note);
    let FeatureGeometry::Line { a, b } = traced.geometry else {
        panic!("{:?}", traced.geometry)
    };
    assert!(close(a, [2.0, -1.0]) && close(b, [2.0, 1.0]), "{a:?} {b:?}");
    let turned = run(
        &bytes,
        &face_ref(&body, &name, pick),
        Transform3::rot_z(90.0),
    )?;
    let FeatureGeometry::Line { a, b } = turned.geometry else {
        panic!("{:?}", turned.geometry)
    };
    assert!(close(a, [1.0, 2.0]) && close(b, [-1.0, 2.0]), "{a:?} {b:?}");
    Ok(())
}

/// The hole is a vertical cylinder: its centre (0.5, 0).
#[test]
fn a_vertical_hole_traces_its_centre() -> Fallible {
    let (bytes, body) = boxed();
    let part = evaluated(&bytes);
    let (name, pick) = face_with(&part, &body, |s| matches!(s, FaceSurface::Cylinder { .. }));
    let traced = run(&bytes, &face_ref(&body, &name, pick), Transform3::IDENTITY)?;
    assert_eq!(traced.status, ReferenceStatus::Resolved, "{}", traced.note);
    let FeatureGeometry::Point { at } = traced.geometry else {
        panic!("{:?}", traced.geometry)
    };
    assert!(close(at, [0.5, 0.0]), "{at:?}");
    Ok(())
}

/// Tilted 30° about x by the model placement, the −y face slopes: it is
/// refused. The top face is parallel to the board: refused too.
#[test]
fn sloped_and_flat_faces_are_refused() -> Fallible {
    let (bytes, body) = boxed();
    let part = evaluated(&bytes);
    let (front, pick) = face_with(&part, &body, plane([0.0, -1.0, 0.0]));
    let sloped = run(
        &bytes,
        &face_ref(&body, &front, pick),
        Transform3::rot_x(30.0),
    );
    let Err(Error::Unsupported(why)) = sloped else {
        panic!("{sloped:?}")
    };
    assert!(why.contains("pick an edge"), "{why}");
    let (top, pick) = face_with(&part, &body, plane([0.0, 0.0, 1.0]));
    let flat = run(&bytes, &face_ref(&body, &top, pick), Transform3::IDENTITY);
    let Err(Error::Unsupported(why)) = flat else {
        panic!("{flat:?}")
    };
    assert!(why.contains("parallel to the board"), "{why}");
    Ok(())
}

/// A name the model no longer has is not found, and the refusal lists the
/// faces square to the board to pick from; a pick off the face is inexact
/// and says where a point on it is.
#[test]
fn a_missing_face_is_not_found_and_a_stray_pick_is_inexact() -> Fallible {
    let (bytes, body) = boxed();
    let part = evaluated(&bytes);
    let gone = run(
        &bytes,
        &face_ref(&body, "F99:side:l1", [2.0, 0.0, 0.5]),
        Transform3::IDENTITY,
    );
    let Err(Error::NotFound(why)) = gone else {
        panic!("{gone:?}")
    };
    assert!(why.contains("faces square to the board"), "{why}");
    let (name, _) = face_with(&part, &body, plane([1.0, 0.0, 0.0]));
    let stray = run(
        &bytes,
        &face_ref(&body, &name, [9.0, 9.0, 9.0]),
        Transform3::IDENTITY,
    )?;
    assert_eq!(stray.status, ReferenceStatus::Inexact);
    assert!(stray.note.contains("no longer on it"), "{}", stray.note);
    Ok(())
}

/// The top edge of the +x face is parallel to the board: its line is
/// x = 2 from y = −1 to 1 (either way round), and its ends are vertices.
#[test]
fn an_edge_parallel_to_the_board_traces_its_line_and_ends() -> Fallible {
    let (bytes, body) = boxed();
    let part = evaluated(&bytes);
    let b = part.state().bodies.iter().find(|b| b.name == body).unwrap();
    let mesh = b.mesh();
    let names = naming::edge_names(b);
    let i = (0..names.len())
        .find(|i| {
            let e = &mesh.edges[*i];
            e.iter()
                .all(|p| (p.x - 2.0).abs() < 1e-9 && (p.z - 1.0).abs() < 1e-9)
        })
        .unwrap();
    let pick = arr(mesh.edges[i][0]);
    let edge = ModelEdgeRef {
        body: BodyName(body.clone()),
        edge: ModelEdgeName(names[i].clone()),
        pick,
    };
    let traced = run(
        &bytes,
        &FeatureRef::Edge {
            placement: 1,
            edge: edge.clone(),
        },
        Transform3::IDENTITY,
    )?;
    assert_eq!(traced.status, ReferenceStatus::Resolved, "{}", traced.note);
    let FeatureGeometry::Line { a, b } = traced.geometry else {
        panic!("{:?}", traced.geometry)
    };
    let ends = [a, b];
    assert!(
        ends.iter().any(|p| close(*p, [2.0, -1.0])) && ends.iter().any(|p| close(*p, [2.0, 1.0])),
        "{ends:?}"
    );
    let start = run(
        &bytes,
        &FeatureRef::Vertex {
            placement: 1,
            edge,
            end: End::Start,
        },
        Transform3::IDENTITY,
    )?;
    assert_eq!(start.geometry, FeatureGeometry::Point { at: a });
    Ok(())
}
