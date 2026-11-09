//! Slice S3 over the ESP32-S31 dev board example: the USB-C receptacle J1
//! is held by its part file, its shell's front face flush with the bottom
//! edge and its origin on the board's centre line, so it follows a change
//! of the board's width. Faces are addressed through the same JSON
//! operations an agent sends, and a face the part does not have is refused
//! with the faces it does have.
use super::Session;
use pcbcraft_core::Engine;
use pcbcraft_core::model::dimension::ReferenceStatus;
use serde_json::{Value, json};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const EXAMPLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft"
);

fn open() -> TestResult<Session> {
    let mut session = Session::new(Engine::default());
    let opened = session.dispatch(json!({"command": "project.open", "params": {
        "expected_revision": session.engine().revision(), "path": EXAMPLE}}));
    assert_eq!(opened["ok"], true, "{opened}");
    Ok(session)
}

fn apply(session: &mut Session, operation: Value) -> Value {
    let revision = session.engine().revision();
    session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": revision, "operation": operation}}))
}

fn j1(session: &Session) -> TestResult<(u64, [f64; 2])> {
    let project = session.engine().project();
    let id = project.instance_by_designator("J1").ok_or("J1")?.id;
    let at = project.placement_of(id).ok_or("J1 placed")?.position;
    Ok((id, at))
}

fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
}

/// The board 40 mm wide puts J1 at x = 20; 50 mm wide, at x = 25, its
/// front face still on the bottom edge (y = 7.35 / 2). Undo puts it back.
#[test]
fn the_usb_c_receptacle_follows_the_board_width() -> TestResult {
    let mut session = open()?;
    let traces = &session.engine().project().model_traces;
    assert_eq!(traces.len(), 1, "{traces:?}");
    assert_eq!(traces[0].status, ReferenceStatus::Resolved);
    let (_, at) = j1(&session)?;
    assert!(close(at, [20.0, 3.675]), "{at:?}");
    let widened = apply(
        &mut session,
        json!({"editor": "Board", "operation": {"command": "board.edit", "params": {"edits": [
            {"op": "set_parameter", "param": {"name": "width", "expr": "50 mm", "unit": "mm",
                "comment": ""}}]}}}),
    );
    assert_eq!(widened["ok"], true, "{widened}");
    let (_, at) = j1(&session)?;
    assert!(close(at, [25.0, 3.675]), "{at:?}");
    let revision = session.engine().revision();
    let undone = session.dispatch(json!({"command": "project.undo", "params": {
        "expected_revision": revision}}));
    assert_eq!(undone["ok"], true, "{undone}");
    let (_, at) = j1(&session)?;
    assert!(close(at, [20.0, 3.675]), "{at:?}");
    Ok(())
}

/// An agent naming a face J1's part file does not have is refused, and the
/// refusal lists the faces square to the board, with a point on each; the
/// document is unchanged.
#[test]
fn a_face_the_part_lacks_is_refused_with_the_faces_it_has() -> TestResult {
    let mut session = open()?;
    let (id, _) = j1(&session)?;
    let before = session.engine().to_file();
    let refused = apply(
        &mut session,
        json!({"editor": "Dimension", "operation": {"command": "layout.dimension_add", "params": {
            "dimension": {"kind": {"dim": "distance"}, "value": {"fixed": 1.0}, "refs": [
                {"part": {"feature": "face", "placement": id,
                    "face": {"body": "Shell", "face": "F9:nowhere", "pick": [0.0, 0.0, 0.0]}}},
                {"part": {"feature": "origin", "target": {"kind": "placement", "id": id}}}]}}}}),
    );
    assert_eq!(refused["ok"], false, "{refused}");
    let text = refused.to_string();
    assert!(text.contains("faces square to the board"), "{text}");
    assert_eq!(session.engine().to_file(), before);
    Ok(())
}
