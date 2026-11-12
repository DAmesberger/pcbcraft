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

/// `layout.dimension_order` puts J1's two dimensions in the listed order
/// as one undo step, moving nothing on the board; a missing or repeated id
/// is refused and changes nothing.
#[test]
fn dimension_order_is_document_data_and_one_undo_step() -> TestResult {
    let mut session = open()?;
    let ids = |s: &Session| -> Vec<u64> {
        s.engine()
            .project()
            .part_dimensions
            .iter()
            .map(|d| d.id)
            .collect()
    };
    let before = ids(&session);
    assert!(before.len() >= 2, "{before:?}");
    let (_, at) = j1(&session)?;
    let mut flipped = before.clone();
    flipped.swap(0, 1);
    let order = |ids: &[u64]| {
        json!({"editor": "Dimension", "operation": {"command": "layout.dimension_order",
            "params": {"ids": ids}}})
    };
    let done = apply(&mut session, order(&flipped[..2]));
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(ids(&session), flipped);
    let (_, now) = j1(&session)?;
    assert!(close(at, now), "order moves nothing: {at:?} → {now:?}");
    let revision = session.engine().revision();
    for bad in [vec![before[0], 999_999], vec![before[0], before[0]]] {
        let refused = apply(&mut session, order(&bad));
        assert_eq!(refused["ok"], false, "{bad:?}: {refused}");
        assert_eq!(ids(&session), flipped, "refused: unchanged");
        assert_eq!(session.engine().revision(), revision);
    }
    let undone = session.dispatch(json!({"command": "project.undo", "params": {
        "expected_revision": revision}}));
    assert_eq!(undone["ok"], true, "{undone}");
    assert_eq!(ids(&session), before, "one undo step");
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

/// The seat of a top-side part: the top of the top copper, board mm.
fn seat(session: &Session) -> f64 {
    session
        .engine()
        .project()
        .stackup
        .copper_span(pcbcraft_core::CopperLayer::Top)
        .map_or(0.0, |s| s.1)
}

/// What the 3D Dimension tool does on a pick of `designator`'s model at
/// `at` (board mm, z above the top copper): the reference, as JSON, and
/// the distance the tool shows from it to the bottom edge (y = 0) now.
fn pick_to_bottom(session: &Session, designator: &str, at: [f64; 3]) -> TestResult<(Value, f64)> {
    use pcbcraft_core::layout::dimensions::{FeatureGeometry, model, view};
    let project = session.engine().project();
    let id = project.instance_by_designator(designator).ok_or("part")?.id;
    let placement = project.placement_of(id).ok_or("placed")?;
    let (feature, trace) = model::pick(
        project,
        &pcbcraft_solidcraft::board::SketchBoardEvaluator,
        id,
        [at[0], at[1], seat(session) + at[2]],
        0.2,
    )?;
    let on_board = view::transformed(trace, &view::part_transform(placement));
    let bottom = FeatureGeometry::Line {
        a: [0.0, 0.0],
        b: [project.board_size[0], 0.0],
    };
    let shown = view::shown(&on_board, &bottom).ok_or("shown")?;
    Ok((
        serde_json::to_value(pcbcraft_core::model::dimension::BoardRef::Part(feature))?,
        shown.value,
    ))
}

fn bottom_edge(session: &Session) -> TestResult<Value> {
    let pcbcraft_core::model::board::BoardShape::Sketch(s) =
        &session.engine().project().board_shape
    else {
        return Err("a sketch board".into());
    };
    let e = s
        .edges
        .iter()
        .find(|e| e.name.0 == "bottom")
        .ok_or("bottom")?;
    Ok(json!({"board": {"curve": e.curve.0}}))
}

fn add_distance(session: &mut Session, refs: Value, value: f64) -> Value {
    apply(
        session,
        json!({"editor": "Dimension", "operation": {"command": "layout.dimension_add", "params": {
            "dimension": {"kind": {"dim": "distance"}, "refs": refs, "value": {"fixed": value}}}}}),
    )
}

/// Second VM test drive, blocker 1: J1's shell face is already flush with
/// the bottom edge. A second dimension from the tongue's front lower edge,
/// 0.6 mm above that edge (shell face y = -3.675, tongue edge y = -3.075 in
/// the part frame), to the bottom edge at 1 mm cannot hold with it unless
/// J1 turns; a distance never turns a part, so it is refused in words that
/// name both dimensions and the part, and nothing changes. (Before, the
/// solve turned J1 by 23.7° to put the tongue edge's start 1 mm up.)
#[test]
fn a_second_distance_that_only_a_turn_would_satisfy_is_refused() -> TestResult {
    let mut session = open()?;
    let (_, at) = j1(&session)?;
    assert!(close(at, [20.0, 3.675]), "{at:?}");
    let (tongue, now) = pick_to_bottom(&session, "J1", [20.0, 0.6, 1.4])?;
    assert!(tongue.to_string().contains("Tongue"), "{tongue}");
    assert!((now - 0.6).abs() < 1e-9, "{now}");
    let before = session.engine().to_file();
    let refs = json!([tongue, bottom_edge(&session)?]);
    let refused = add_distance(&mut session, refs, 1.0);
    assert_eq!(refused["ok"], false, "{refused}");
    let text = refused.to_string();
    assert!(
        text.contains(
            "J1 · front edge to the bottom edge (1 mm) conflicts with J1 · front face to the \
             bottom edge (0 mm), which already holds J1"
        ),
        "{text}"
    );
    // No solver or model ids in the words.
    for id in ["F3:", "F1:", "k1", "l1"] {
        assert!(!text.contains(id), "{id} in {text}");
    }
    assert_eq!(session.engine().to_file(), before);
    let project = session.engine().project();
    let id = project.instance_by_designator("J1").ok_or("J1")?.id;
    assert_eq!(project.placement_of(id).ok_or("J1")?.rotation_deg, 0.0);
    Ok(())
}

/// Second VM test drive, blocker 3: R5 (a generated 0603 at (24, 11.5),
/// turned 90°) dimensioned from its body's side edge, which runs along the
/// board's y from (24.4, 11) to (24.4, 12), to the bottom edge. The tool
/// shows 11.5 mm (from the edge's middle); 11 mm moves R5 0.5 mm down to
/// y = 11, and the distance shown afterwards is the 11 mm the card holds.
/// (Before, the solve measured from the edge's start, already 11 mm up, so
/// it committed and nothing moved.) The example holds R5 at fixed offsets
/// from J1, so those two dimensions are deleted first.
#[test]
fn an_edge_dimension_moves_the_part_to_the_value_shown() -> TestResult {
    let mut session = open()?;
    let r5 = session
        .engine()
        .project()
        .instance_by_designator("R5")
        .ok_or("R5")?
        .id;
    let held: Vec<u64> = session
        .engine()
        .project()
        .part_dimensions
        .iter()
        .filter(|d| {
            d.refs.iter().any(|r| {
                matches!(r, pcbcraft_core::model::dimension::BoardRef::Part(
                    pcbcraft_core::model::dimension::FeatureRef::Origin {
                        target: pcbcraft_core::model::board::ConstraintTarget::Placement(p),
                    }) if *p == r5)
            })
        })
        .map(|d| d.id)
        .collect();
    assert_eq!(held.len(), 2, "R5 held by J1");
    for id in held {
        let deleted = apply(
            &mut session,
            json!({"editor": "Dimension", "operation": {"command": "layout.dimension_delete",
                "params": {"id": id}}}),
        );
        assert_eq!(deleted["ok"], true, "{deleted}");
    }
    let (edge, now) = pick_to_bottom(&session, "R5", [24.4, 11.5, 0.2])?;
    assert!((now - 11.5).abs() < 1e-9, "{now}");
    let refs = json!([edge, bottom_edge(&session)?]);
    let added = add_distance(&mut session, refs, 11.0);
    assert_eq!(added["ok"], true, "{added}");
    let project = session.engine().project();
    let r5 = project.instance_by_designator("R5").ok_or("R5")?.id;
    let placed = project.placement_of(r5).ok_or("R5")?;
    assert!(
        close(placed.position, [24.0, 11.0]),
        "{:?}",
        placed.position
    );
    assert_eq!(placed.rotation_deg, 90.0);
    let (_, shown) = pick_to_bottom(&session, "R5", [24.4, 11.0, 0.2])?;
    assert!((shown - 11.0).abs() < 1e-9, "{shown}");
    Ok(())
}
