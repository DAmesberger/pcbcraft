//! Slice B1 through the JSON transport, over the ESP32-S31 dev board
//! example: the board interface's slots survive undo, redo, save and
//! reopen, and a retired slot is never used again. The mounting holes are
//! edited with the host's board evaluator, as the app and an agent edit
//! them, and J1's front face (a 3D model reference the example's
//! dimension already traces) is exported as a feature.
use super::Session;
use pcbcraft_core::Engine;
use serde_json::{Value, json};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const EXAMPLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft"
);

fn ok(session: &mut Session, request: Value) -> TestResult<Value> {
    let reply = session.dispatch(request);
    if reply["ok"] != true {
        return Err(format!("refused: {reply}").into());
    }
    Ok(reply)
}

fn open(path: &str) -> TestResult<Session> {
    let mut session = Session::new(Engine::default());
    ok(
        &mut session,
        json!({"command": "project.open", "params": {"expected_revision": 0, "path": path}}),
    )?;
    Ok(session)
}

fn apply(session: &mut Session, operation: Value) -> Value {
    let revision = session.engine().revision();
    session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": revision, "operation": operation}}))
}

fn interface(session: &mut Session, command: &str, params: Value) -> TestResult<Value> {
    let reply = apply(
        session,
        json!({"editor": "Interface", "operation": {"command": command, "params": params}}),
    );
    if reply["ok"] != true {
        return Err(format!("refused: {reply}").into());
    }
    Ok(reply)
}

fn history(session: &mut Session, command: &str) -> TestResult {
    let revision = session.engine().revision();
    ok(
        session,
        json!({"command": command, "params": {"expected_revision": revision}}),
    )?;
    Ok(())
}

fn list(session: &mut Session) -> TestResult<Value> {
    let reply = ok(
        session,
        json!({"command": "project.query", "params": {"query": "interface.list", "params": {}}}),
    )?;
    if !reply["result"]["result"]["datums"].is_array() {
        return Err(format!("no datums: {reply}").into());
    }
    Ok(reply["result"]["result"].clone())
}

/// (slot, token) of every live datum.
fn slots(list: &Value) -> Vec<(u64, String)> {
    list["datums"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| {
            (
                d["slot"].as_u64().unwrap_or(u64::MAX),
                d["token"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

fn retired(list: &Value) -> Vec<u64> {
    list["retired"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect()
}

fn remove_hole(session: &mut Session, id: u64) -> TestResult {
    let reply = apply(
        session,
        json!({"editor": "Board", "operation": {"command": "board.edit", "params": {"edits": [
            {"op": "remove_mounting_hole", "id": id}]}}}),
    );
    if reply["ok"] != true {
        return Err(format!("refused: {reply}").into());
    }
    Ok(())
}

#[test]
fn the_interface_slots_survive_undo_redo_save_and_reopen() -> TestResult {
    let mut session = open(EXAMPLE)?;
    let board = list(&mut session)?;
    let tokens: Vec<String> = slots(&board).into_iter().map(|(_, t)| t).collect();
    assert_eq!(
        tokens,
        [
            "board.frame",
            "board.top",
            "board.bottom",
            "board.thickness",
            "board.outline",
            "h1",
            "h2",
            "h3",
            "h4",
            "side.bottom",
            "side.top",
        ]
    );
    let hole = board["datums"]
        .as_array()
        .and_then(|d| d.iter().find(|d| d["token"] == "h2"))
        .ok_or("h2")?
        .clone();
    assert_eq!(hole["kind"], "axis");
    assert_eq!(hole["geometry"]["diameter"], 2.7);
    assert_eq!(hole["geometry"]["point"], json!([36.5, 3.5, 0.0]));

    // Export J1's front face, which the example's dimension references.
    let face = session
        .engine()
        .project()
        .part_dimensions
        .iter()
        .flat_map(|d| d.refs.iter())
        .find(|r| matches!(r, pcbcraft_core::model::dimension::BoardRef::Part(f) if f.is_model()))
        .cloned()
        .ok_or("a model reference")?;
    interface(
        &mut session,
        "interface.export",
        json!({"target": {"target": "feature", "feature": face}, "name": "USB opening face"}),
    )?;
    let exported = list(&mut session)?;
    let row = exported["datums"]
        .as_array()
        .and_then(|d| d.last())
        .ok_or("exported")?
        .clone();
    assert_eq!(row["slot"], 11);
    assert_eq!(row["name"], "USB opening face");
    assert_eq!(row["status"], "resolved");
    assert_eq!(row["kind"], "plane", "{row}");
    assert_eq!(slots(&exported)[..11], slots(&board)[..]);

    // A deleted hole retires its slot; undo brings it back in that slot.
    remove_hole(&mut session, 2)?;
    let removed = list(&mut session)?;
    assert_eq!(retired(&removed), [6]);
    assert!(slots(&removed).iter().all(|(_, t)| t != "h2"));
    history(&mut session, "project.undo")?;
    assert_eq!(slots(&list(&mut session)?), slots(&exported));
    assert!(retired(&list(&mut session)?).is_empty());
    history(&mut session, "project.redo")?;
    assert_eq!(retired(&list(&mut session)?), [6]);

    // Undo the export too, then export again: a new slot, the old one
    // retired.
    history(&mut session, "project.undo")?;
    history(&mut session, "project.undo")?;
    assert_eq!(retired(&list(&mut session)?), [11]);
    interface(
        &mut session,
        "interface.export",
        json!({"target": {"target": "feature", "feature": face}}),
    )?;
    let again = list(&mut session)?;
    assert_eq!(slots(&again).last().map(|(s, _)| *s), Some(12));
    assert_eq!(retired(&again), [11]);

    // Save and reopen: the same list, and the counters keep going.
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("interface.pcbcraft");
    let path = file.display().to_string();
    ok(
        &mut session,
        json!({"command": "project.save", "params": {"path": path}}),
    )?;
    let mut reopened = open(&path)?;
    assert_eq!(list(&mut reopened)?, again);
    remove_hole(&mut reopened, 3)?;
    let after = list(&mut reopened)?;
    assert_eq!(retired(&after), [7, 11]);
    interface(
        &mut reopened,
        "interface.export",
        json!({"target": {"target": "board_parameter", "name": "width"}}),
    )?;
    let last = list(&mut reopened)?;
    assert_eq!(
        slots(&last).last(),
        Some(&(13, "board.parameter.width".to_owned()))
    );
    Ok(())
}
