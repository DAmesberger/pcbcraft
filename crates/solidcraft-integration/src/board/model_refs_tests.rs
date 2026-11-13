#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Slice S3: faces, edges and vertices of a part file traced in the board
//! plane, with hand-worked expected values. The part is a 4 × 2 × 1 mm box
//! with its corner at (−2, −1, 0) and a 1 mm hole through it at (0.5, 0).
use super::*;
use serde_json::json;
use solidcraft_engine::Session;
use sparksmith_core::library::package_model::PartFileModel;
use sparksmith_core::model::dimension::{BodyName, FaceName, ModelEdgeName};

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The box part: (bytes, the body's name).
pub(super) fn boxed() -> (Vec<u8>, String) {
    let mut s = Session::default();
    s.execute("file.new", &json!({"name": "part"})).unwrap();
    let made = s
        .execute(
            "solid.box",
            &json!({"corner": [-2, -1, 0], "length": 4, "width": 2, "height": 1, "name": "Assembly"}),
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
        model: TraceModel::PartFile { part: &part, bytes },
        frame,
        feature,
        picked_here: true,
        words: WORDS,
    })
}

/// How the tests' references read to a person.
const WORDS: &str = "U1 · top face";

/// A refusal a person reads names the reference in their words, never by a
/// persistent name (`F3:-x`, `F6:blend(…)`): only what follows its first
/// sentence, for an agent, may list names.
fn readable(why: &str) {
    let first = why.split_once(". ").map_or(why, |(first, _)| first);
    assert!(first.contains(WORDS), "{why}");
    let bytes = first.as_bytes();
    let named = (0..bytes.len()).any(|i| {
        bytes[i] == b'F'
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
            && bytes[i + 1..]
                .iter()
                .position(|b| !b.is_ascii_digit())
                .is_some_and(|n| n > 0 && bytes.get(i + 1 + n) == Some(&b':'))
    });
    assert!(!named, "a persistent name in {first:?}");
    assert!(!first.contains('‘') && !first.contains('|'), "{first}");
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
    readable(&why);
    let (top, pick) = face_with(&part, &body, plane([0.0, 0.0, 1.0]));
    let flat = run(&bytes, &face_ref(&body, &top, pick), Transform3::IDENTITY);
    let Err(Error::Unsupported(why)) = flat else {
        panic!("{flat:?}")
    };
    assert!(why.contains("parallel to the board"), "{why}");
    readable(&why);
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
    readable(&why);
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

/// Demo run 3: the upright edge at the box's front-right corner is not
/// parallel to the board, so it is refused, and the refusal names it in a
/// designer's words; a name that is gone is refused the same way.
#[test]
fn an_upright_or_missing_edge_is_refused_in_words() -> Fallible {
    let (bytes, body) = boxed();
    let part = evaluated(&bytes);
    let b = part.state().bodies.iter().find(|b| b.name == body).unwrap();
    let mesh = b.mesh();
    let names = naming::edge_names(b);
    let i = (0..names.len())
        .find(|i| {
            let e = &mesh.edges[*i];
            e.iter()
                .all(|p| (p.x - 2.0).abs() < 1e-9 && (p.y + 1.0).abs() < 1e-9)
        })
        .unwrap();
    let upright = |name: &str| FeatureRef::Edge {
        placement: 1,
        edge: ModelEdgeRef {
            body: BodyName(body.clone()),
            edge: ModelEdgeName(name.into()),
            pick: arr(mesh.edges[i][0]),
        },
    };
    let refused = run(&bytes, &upright(&names[i]), Transform3::IDENTITY);
    let Err(Error::Unsupported(why)) = refused else {
        panic!("{refused:?}")
    };
    assert!(why.contains("not parallel to the board"), "{why}");
    readable(&why);
    let gone = run(&bytes, &upright("F99:-x|F98:+y"), Transform3::IDENTITY);
    let Err(Error::NotFound(why)) = gone else {
        panic!("{gone:?}")
    };
    readable(&why);
    Ok(())
}

fn picked(bytes: &[u8], at: [f64; 3], reach: f64, frame: Transform3) -> Result<ModelPicked> {
    let part = PartFileModel::default();
    pick(&PickRequest {
        model: TraceModel::PartFile { part: &part, bytes },
        frame,
        at,
        reach,
        body: None,
    })
}

/// Slice S5: a point picked on the box names the face holding it, by the
/// name the trace re-finds; within reach of one of that face's edges it
/// names the edge, picked at its nearest point; off the model, nothing.
/// The pick comes in the part frame and goes back through the model
/// placement.
#[test]
fn a_pick_names_the_face_or_the_edge_under_it() -> Fallible {
    let (bytes, body) = boxed();
    let part = evaluated(&bytes);
    let (name, _) = face_with(&part, &body, plane([1.0, 0.0, 0.0]));
    // The +x face's middle, well away from its edges.
    let face = picked(&bytes, [2.0, 0.0, 0.5], 0.1, Transform3::IDENTITY)?;
    assert_eq!(
        face,
        ModelPicked::Face(ModelFaceRef {
            body: BodyName(body.clone()),
            face: FaceName(name.clone()),
            pick: [2.0, 0.0, 0.5],
        })
    );
    let ModelPicked::Face(f) = face else {
        return Err("a face".into());
    };
    let traced = run(
        &bytes,
        &face_ref(&body, &f.face.0, f.pick),
        Transform3::IDENTITY,
    )?;
    assert_eq!(traced.status, ReferenceStatus::Resolved);
    // 0.05 mm below the top edge of that face: the edge, at z = 1.
    let ModelPicked::Edge(edge) = picked(&bytes, [2.0, 0.2, 0.95], 0.1, Transform3::IDENTITY)?
    else {
        return Err("an edge".into());
    };
    assert!((edge.pick[2] - 1.0).abs() < 1e-9, "{edge:?}");
    assert!((edge.pick[0] - 2.0).abs() < 1e-9 && (edge.pick[1] - 0.2).abs() < 1e-9);
    let traced = run(
        &bytes,
        &FeatureRef::Edge {
            placement: 1,
            edge: edge.clone(),
        },
        Transform3::IDENTITY,
    )?;
    assert!(
        matches!(traced.geometry, FeatureGeometry::Line { a, b }
        if (a[0] - 2.0).abs() < 1e-9 && (b[0] - 2.0).abs() < 1e-9),
        "{traced:?}"
    );
    // The model turned a quarter turn about z by its placement: the part
    // frame's (0, 2, 0.5) is the model's +x face.
    let turned = Transform3::rot_z(90.0);
    let again = picked(&bytes, [0.0, 2.0, 0.5], 0.1, turned)?;
    assert!(
        matches!(&again, ModelPicked::Face(f) if f.face.0 == name
        && (f.pick[0] - 2.0).abs() < 1e-9 && f.pick[1].abs() < 1e-9),
        "{again:?}"
    );
    assert!(matches!(
        picked(&bytes, [9.0, 9.0, 9.0], 0.1, Transform3::IDENTITY),
        Err(Error::NotFound(_))
    ));
    Ok(())
}

/// The Layout view's body traces (`model_faces`): the box's four sides
/// and its hole, never its top or bottom. Each is listed with the trace a
/// trace of it gives, a pick at its point names it, and its heights span
/// the box (0 to 1 mm). Turned a quarter by the model placement, the
/// traces turn with it.
#[test]
fn faces_lists_every_traceable_face_as_a_pick_and_a_trace_give_it() -> Fallible {
    let (bytes, body) = boxed();
    let part = PartFileModel::default();
    for frame in [Transform3::IDENTITY, Transform3::rot_z(90.0)] {
        let listed = faces(&FacesRequest {
            model: TraceModel::PartFile {
                part: &part,
                bytes: &bytes,
            },
            frame,
        })?;
        let lines = listed
            .iter()
            .filter(|f| matches!(f.trace, FeatureGeometry::Line { .. }))
            .count();
        let points = listed
            .iter()
            .filter(|f| matches!(f.trace, FeatureGeometry::Point { .. }))
            .count();
        assert_eq!((lines, points), (4, 1), "{listed:?}");
        for f in &listed {
            assert_eq!(f.face.body.0, body);
            assert!(f.z[0].abs() < 1e-9 && (f.z[1] - 1.0).abs() < 1e-9, "{f:?}");
            let traced = run(&bytes, &face_ref(&body, &f.face.face.0, f.face.pick), frame)?;
            assert_eq!(traced.status, ReferenceStatus::Resolved, "{}", traced.note);
            assert_eq!(traced.geometry, f.trace);
            let named = picked(&bytes, f.at, 0.0, frame)?;
            let ModelPicked::Face(named) = named else {
                panic!("{named:?}")
            };
            assert_eq!(named.face, f.face.face, "{f:?}");
        }
    }
    Ok(())
}
