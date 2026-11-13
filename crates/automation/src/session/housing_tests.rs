//! Slice B4's gate (acceptance 6 and 7 of the design note) through the
//! JSON transport, over the ESP32-S31 dev board example (with J1's package
//! carrying its USB4105 part file's opening, as in B3's gate), with an
//! original headless SolidCraft enclosure test document: the published
//! board linked and grounded; a 2 mm wall across J1's opening with a
//! cutout projected from the opening ("USB opening"); a boss projected
//! from mounting hole 1 ("Boss"); and a far wall beyond the board's top
//! edge ("Far wall"), saved as a `.solidcraft` file.
use super::Session;
use super::interface_publish_tests::{
    TestResult, apply, enclosure, faces, instance, ok, open, publish, remove,
};
use pcbcraft_core::Operation;
use pcbcraft_core::layout::outline::{BoardCommand, BoardEdit};
use pcbcraft_core::library::footprint_source::{
    DimKind, DimValue, ParamName, SketchConstraintId, SketchCurveId, SketchEdit, SketchPointId,
    SketchRef,
};
use pcbcraft_core::model::BoardShape;
use pcbcraft_solidcraft::enclosure::Enclosure;
use serde_json::{Value, json};
use std::path::Path;

/// Where the far wall's inner face stands in the test document, mm: half
/// a millimetre beyond the board's top edge (y = 64).
const FAR_WALL_Y: f64 = 64.5;

/// The test enclosure with its far wall's inner face at `far_y`, saved at
/// `path`.
fn save_enclosure(design: &Path, opening: &str, far_y: f64, path: &Path) -> TestResult {
    let (mut housing, _) = enclosure(design, opening)?;
    housing.solid_box("Far wall", [-5.0, far_y, -2.0], [50.0, 2.0, 12.0])?;
    let _ = std::fs::remove_file(path);
    housing.save(&path.display().to_string())?;
    Ok(())
}

/// The far wall's inner face (normal -y) in a saved enclosure: its name
/// and a point on it.
fn far_face(path: &Path) -> TestResult<(String, [f64; 3])> {
    let (mut housing, newer) = Enclosure::open(&path.display().to_string())?;
    assert!(!newer);
    let face = faces(&mut housing, "Far wall")?
        .into_iter()
        .find(|f| f.normal.is_some_and(|n| n[1] < -0.99))
        .ok_or("the far wall's inner face")?;
    Ok((face.name, face.point))
}

/// `housing.status`, the document's view of the link.
fn status(session: &mut Session) -> TestResult<Value> {
    Ok(ok(
        session,
        json!({"command": "project.query", "params": {
            "query": "housing.status", "params": {}}}),
    )?["result"]
        .clone())
}

/// Whether `housing.preview` finds the enclosure changed since the board
/// read it.
fn changed(session: &mut Session, housing: &Path, design: &Path) -> TestResult<bool> {
    let preview = ok(
        session,
        json!({"command": "housing.preview", "params": files(housing, design)}),
    )?;
    preview["changed"].as_bool().ok_or_else(|| "changed".into())
}

/// The files every enclosure request names.
fn files(housing: &Path, design: &Path) -> Value {
    json!({"path": housing.display().to_string(), "design": design.display().to_string()})
}

fn with(mut params: Value, more: Value) -> Value {
    if let (Some(a), Some(b)) = (params.as_object_mut(), more.as_object()) {
        a.extend(b.clone());
    }
    params
}

/// `housing.update` with no enclosure linked: link it.
fn link(session: &mut Session, housing: &Path, design: &Path) -> TestResult<Value> {
    let revision = session.engine().revision();
    ok(
        session,
        json!({"command": "housing.update", "params": {
            "path": housing.display().to_string(),
            "design": design.display().to_string(),
            "expected_revision": revision}}),
    )
}

/// The production check's items with `code`.
fn preflight(session: &mut Session, code: &str) -> TestResult<Vec<Value>> {
    let reply = ok(
        session,
        json!({"command": "project.query", "params": {
            "query": "fabrication.preflight", "params": {}}}),
    )?;
    Ok(reply["result"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| i["code"] == code)
        .cloned()
        .collect())
}

fn undo(session: &mut Session) -> TestResult {
    let revision = session.engine().revision();
    ok(
        session,
        json!({"command": "project.undo", "params": {"expected_revision": revision}}),
    )?;
    Ok(())
}

/// The board's solved edge `name`: its two ends.
fn edge(session: &Session, name: &str) -> TestResult<([f64; 2], [f64; 2])> {
    let BoardShape::Sketch(sketch) = &session.engine().project().board_shape else {
        return Err("a sketch board".into());
    };
    let e = sketch
        .solved
        .edges
        .iter()
        .find(|e| e.name.0 == name)
        .ok_or(name.to_owned())?;
    Ok((e.a, e.b))
}

fn position(session: &Session, designator: &str) -> TestResult<[f64; 2]> {
    let id = instance(session, designator)?;
    Ok(session
        .engine()
        .project()
        .placement_of(id)
        .ok_or("placed")?
        .position)
}

/// Acceptance 6: deleting J1, the publish preview's dry run names the
/// enclosure's cutout that would fail, before anything is written. Published
/// anyway, `housing.status` reads "broken reference", the production check
/// has an unwaivable `housing.reference_lost` naming the cutout and J1, and
/// undo restores everything.
#[test]
fn b4_deleting_j1_breaks_the_cutout_and_blocks_fabrication() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let housing = dir.path().join("housing.solidcraft");
    let mut session = open(dir.path())?;
    let j1 = instance(&session, "J1")?;
    let opening = format!("i{j1}.opening");
    publish(&mut session, &design, "interface")?;
    save_enclosure(&design, &opening, FAR_WALL_Y, &housing)?;
    let linked = link(&mut session, &housing, &design)?;
    let consumers = linked["housing"]["consumers"]
        .as_array()
        .ok_or("consumers")?;
    assert!(
        consumers
            .iter()
            .any(|c| c["feature"] == "USB opening" && c["token"] == opening.as_str()),
        "the cutout uses J1's opening: {consumers:?}"
    );
    assert!(
        consumers.iter().any(|c| c["feature"] == "Boss"),
        "the boss uses mounting hole 1: {consumers:?}"
    );
    assert_eq!(status(&mut session)?["state"], "linked");
    assert!(!changed(&mut session, &housing, &design)?);
    let before = session.engine().project().clone();
    let enclosure_bytes = std::fs::read(&housing)?;

    remove(&mut session, j1)?;
    let design_bytes = std::fs::read(&design)?;
    let preview = ok(
        &mut session,
        json!({"command": "interface.publish_preview", "params": {
            "path": design.display().to_string(), "tier": "interface",
            "enclosure_path": housing.display().to_string()}}),
    )?;
    assert_eq!(
        std::fs::read(&design)?,
        design_bytes,
        "a preview writes nothing"
    );
    assert_eq!(
        std::fs::read(&housing)?,
        enclosure_bytes,
        "the enclosure is never written"
    );
    let effects = preview["dry_run"]["effects"]
        .as_array()
        .ok_or("the dry run")?;
    let fails: Vec<&Value> = effects.iter().filter(|e| e["effect"] == "fails").collect();
    let [cutout] = fails.as_slice() else {
        return Err(format!("one failing feature: {effects:?}").into());
    };
    assert_eq!(cutout["feature"], "USB opening");
    assert_eq!(
        cutout["message"],
        "The enclosure's ‘USB opening’ uses J1 opening, which this publish removes, so it would \
         fail"
    );
    assert!(cutout["datum"].is_u64(), "{cutout}");

    // Broken as soon as J1 is gone, and after publishing anyway.
    publish(&mut session, &design, "interface")?;
    let broken = status(&mut session)?;
    assert_eq!(broken["state"], "broken_reference", "{broken}");
    assert_eq!(broken["words"], "broken reference");
    let items = preflight(&mut session, "housing.reference_lost")?;
    let [item] = items.as_slice() else {
        return Err(format!("one broken reference: {items:?}").into());
    };
    assert_eq!(item["severity"], "error");
    let title = item["title"].as_str().ok_or("a title")?;
    assert!(
        title.contains("‘USB opening’") && title.contains("J1 was deleted"),
        "{title}"
    );
    // Never waived.
    let revision = session.engine().revision();
    let waive = session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": revision,
        "operation": {"editor": "Fabrication", "operation": {
            "command": "fabrication.waive",
            "params": {"keys": [item["key"]], "reason": "the enclosure is redone"}}}}}));
    assert_eq!(waive["ok"], false, "{waive}");

    // Undo the publish's record, then the deletion.
    undo(&mut session)?;
    undo(&mut session)?;
    assert_eq!(
        *session.engine().project(),
        before,
        "undo restores the document"
    );
    assert!(preflight(&mut session, "housing.reference_lost")?.is_empty());
    assert_eq!(status(&mut session)?["state"], "linked");
    Ok(())
}

/// The dry run of a move names the enclosure feature that follows, by its
/// SolidCraft name (acceptance 2's second half): J1 moved 2 mm, the
/// cutout moves 2 mm along X; nothing fails.
#[test]
fn b4_the_dry_run_names_the_feature_that_follows() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let housing = dir.path().join("housing.solidcraft");
    let mut session = open(dir.path())?;
    let opening = format!("i{}.opening", instance(&session, "J1")?);
    publish(&mut session, &design, "interface")?;
    save_enclosure(&design, &opening, FAR_WALL_Y, &housing)?;
    link(&mut session, &housing, &design)?;
    super::interface_publish_tests::move_j1(&mut session, 2.0)?;
    let preview = ok(
        &mut session,
        json!({"command": "interface.publish_preview", "params": {
            "path": design.display().to_string(), "tier": "interface",
            "enclosure_path": housing.display().to_string()}}),
    )?;
    let effects = preview["dry_run"]["effects"]
        .as_array()
        .ok_or("the dry run")?;
    let [moves] = effects.as_slice() else {
        return Err(format!("one effect: {effects:?}").into());
    };
    assert_eq!(moves["effect"], "moves");
    assert_eq!(moves["feature"], "USB opening");
    assert_eq!(
        moves["message"],
        "The enclosure's ‘USB opening’ follows J1 opening: it moves 2 mm along X"
    );
    Ok(())
}

/// Acceptance 7: the enclosure → board direction. The board's top edge
/// is dimensioned 0.5 mm from the far wall's inner face, followed as a
/// housing reference, and SW2 is dimensioned 7 mm from the top edge. The
/// wall moved 1 mm in the test document: `housing.preview` finds the
/// enclosure changed and lists the reference, the top
/// edge and SW2, each 1 mm along Y; `housing.update` at the previewed hash
/// applies it as one undo step.
#[test]
fn b4_the_board_follows_the_enclosure_wall() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let housing = dir.path().join("housing.solidcraft");
    let mut session = open(dir.path())?;
    let opening = format!("i{}.opening", instance(&session, "J1")?);
    publish(&mut session, &design, "interface")?;
    save_enclosure(&design, &opening, FAR_WALL_Y, &housing)?;
    link(&mut session, &housing, &design)?;
    let (face, pick) = far_face(&housing)?;
    let revision = session.engine().revision();
    let added = ok(
        &mut session,
        json!({"command": "housing.follow", "params": with(files(&housing, &design), json!({
            "body": "Far wall", "face": face, "pick": pick, "name": "Far wall · inner face",
            "expected_revision": revision}))}),
    )?;
    let reference = &added["housing"]["references"][0];
    assert_eq!(reference["status"], "resolved");
    let curve = reference["sketch"]["curve"]
        .as_str()
        .ok_or("the reference owns a fixed line")?
        .to_owned();
    let traced = &reference["geometry"];
    for end in ["a", "b"] {
        let y = traced[end][1].as_f64().ok_or("a line")?;
        assert!((y - FAR_WALL_Y).abs() <= 1e-9, "{traced}");
    }

    // The top edge follows the wall: the height dimension gives way to
    // 0.5 mm from the top-right corner to the wall's trace.
    let (top_a, _) = edge(&session, "top")?;
    assert!((top_a[1] - 64.0).abs() <= 1e-9);
    apply(
        &mut session,
        Operation::Board(BoardCommand::Edit {
            edits: vec![
                BoardEdit::Sketch {
                    edit: SketchEdit::Delete {
                        ids: vec![SketchRef::Constraint(SketchConstraintId("k6".into()))],
                    },
                },
                BoardEdit::RemoveParameter {
                    name: ParamName("height".into()),
                },
                BoardEdit::Sketch {
                    edit: SketchEdit::AddDimension {
                        kind: DimKind::Distance,
                        refs: vec![
                            SketchRef::Point(SketchPointId("p2".into())),
                            SketchRef::Curve(SketchCurveId(curve)),
                        ],
                        value: DimValue::Fixed(FAR_WALL_Y - 64.0),
                    },
                },
            ],
        }),
    )?;
    assert!((edge(&session, "top")?.0[1] - 64.0).abs() <= 1e-9);
    // SW2 held 7 mm below the top edge.
    let sw2 = instance(&session, "SW2")?;
    apply(
        &mut session,
        serde_json::from_value(json!({"editor": "Dimension", "operation": {
            "command": "layout.dimension_add", "params": {"dimension": {
                "kind": {"dim": "distance"},
                "refs": [
                    {"part": {"feature": "origin", "target": {"kind": "placement", "id": sw2}}},
                    {"board": {"curve": "l3"}}],
                "value": {"fixed": 7.0}}}}}))?,
    )?;
    let sw2_before = position(&session, "SW2")?;
    assert!((sw2_before[1] - 57.0).abs() <= 1e-9, "{sw2_before:?}");
    let j1_before = position(&session, "J1")?;
    assert!(!changed(&mut session, &housing, &design)?);

    // The wall moves 1 mm in the test document.
    save_enclosure(&design, &opening, FAR_WALL_Y + 1.0, &housing)?;
    assert_eq!(far_face(&housing)?.0, face, "the same face, by name");

    let before = session.engine().project().clone();
    let revision = session.engine().revision();
    let preview = ok(
        &mut session,
        json!({"command": "housing.preview", "params": files(&housing, &design)}),
    )?;
    assert_eq!(
        session.engine().revision(),
        revision,
        "a preview changes nothing"
    );
    assert!(preview["refused"].is_null(), "{preview}");
    assert_eq!(preview["changed"], true);
    let summaries: Vec<&str> = preview["objects"]
        .as_array()
        .ok_or("objects")?
        .iter()
        .filter_map(|o| o["summary"].as_str())
        .collect();
    assert!(
        summaries.contains(&"Board edge top, 1 mm along Y"),
        "{summaries:?}"
    );
    assert!(summaries.contains(&"SW2, 1 mm along Y"), "{summaries:?}");
    assert!(
        !summaries.iter().any(|s| s.starts_with("J1")),
        "J1 is held to the bottom edge: {summaries:?}"
    );
    let references = preview["references"].as_array().ok_or("references")?;
    let [row] = references.as_slice() else {
        return Err(format!("{references:?}").into());
    };
    assert_eq!(row["summary"], "Far wall · inner face, 1 mm along Y");

    // A stale hash is refused, changing nothing.
    let stale = session.dispatch(json!({"command": "housing.update", "params": with(
        files(&housing, &design), json!({
        "expect_hash": before.housing.as_ref().ok_or("linked")?.file_sha256.to_string(),
        "expected_revision": revision}))}));
    assert_eq!(stale["ok"], false, "{stale}");
    assert_eq!(session.engine().revision(), revision);

    ok(
        &mut session,
        json!({"command": "housing.update", "params": with(files(&housing, &design), json!({
            "expect_hash": preview["expect_hash"], "expected_revision": revision}))}),
    )?;
    assert_eq!(session.engine().revision(), revision + 1, "one step");
    let (top_a, top_b) = edge(&session, "top")?;
    assert!((top_a[1] - 65.0).abs() <= 1e-9 && (top_b[1] - 65.0).abs() <= 1e-9);
    let sw2_after = position(&session, "SW2")?;
    assert!((sw2_after[1] - sw2_before[1] - 1.0).abs() <= 1e-9);
    assert!((sw2_after[0] - sw2_before[0]).abs() <= 1e-9);
    assert_eq!(position(&session, "J1")?, j1_before);
    assert!(!changed(&mut session, &housing, &design)?);
    assert_eq!(status(&mut session)?["state"], "linked");

    undo(&mut session)?;
    assert_eq!(
        *session.engine().project(),
        before,
        "one undo step restores it"
    );
    Ok(())
}

/// A missing enclosure: the preview says so in the user's words, the
/// project keeps its snapshot, and a relink to its new place, previewed
/// there, links it again.
#[test]
fn b4_a_missing_enclosure_keeps_the_snapshot() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let housing = dir.path().join("housing.solidcraft");
    let mut session = open(dir.path())?;
    let opening = format!("i{}.opening", instance(&session, "J1")?);
    publish(&mut session, &design, "interface")?;
    save_enclosure(&design, &opening, FAR_WALL_Y, &housing)?;
    link(&mut session, &housing, &design)?;
    let moved = dir.path().join("moved.solidcraft");
    std::fs::rename(&housing, &moved)?;
    let missing = session.dispatch(json!({"command": "housing.preview",
        "params": files(&housing, &design)}));
    assert_eq!(missing["ok"], false);
    assert!(
        missing
            .to_string()
            .contains("The enclosure file was not found"),
        "{missing}"
    );
    // The project keeps its snapshot.
    assert_eq!(status(&mut session)?["state"], "linked");
    // Without a preview's hash nothing is applied.
    let revision = session.engine().revision();
    let unreviewed = session.dispatch(json!({"command": "housing.update", "params": with(
        files(&moved, &design), json!({"expected_revision": revision}))}));
    assert_eq!(unreviewed["ok"], false, "{unreviewed}");
    assert_eq!(session.engine().revision(), revision);
    // Relinked at its new place, as previewed there.
    let there = ok(
        &mut session,
        json!({"command": "housing.preview", "params": files(&moved, &design)}),
    )?;
    assert!(there["refused"].is_null(), "{there}");
    assert_eq!(there["changed"], false, "the same file, moved");
    ok(
        &mut session,
        json!({"command": "housing.update", "params": with(files(&moved, &design), json!({
            "expect_hash": there["expect_hash"], "expected_revision": revision}))}),
    )?;
    let linked = status(&mut session)?;
    assert_eq!(
        linked["enclosure_hint"],
        moved.display().to_string(),
        "{linked}"
    );
    assert!(!changed(&mut session, &moved, &design)?);
    Ok(())
}
