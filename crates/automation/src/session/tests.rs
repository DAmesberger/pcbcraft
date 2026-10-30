use super::*;
use pcbcraft_core::{Engine, Operation, schematic::SchematicCommand};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn edit(reference: &str, revision: u64) -> Value {
    serde_json::json!({"command": "project.apply", "params": {
        "expected_revision": revision,
        "operation": serde_json::to_value(Operation::Schematic(SchematicCommand::Edit {
            instance: 2,
            reference: reference.into(),
            value: "22k".into(),
        })).unwrap_or_default()
    }})
}

#[test]
fn json_requests_reach_the_engine_and_report_typed_errors() -> TestResult {
    let mut session = Session::new(Engine::demo()?);
    let revision = session.engine().revision();
    let reply = session.dispatch(edit("R10", revision));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(
        reply["result"]["project"]["instances"][0]["reference"],
        "R10"
    );
    assert!(reply["result"]["erc"].is_array());

    let stale = session.dispatch(edit("R11", revision));
    assert_eq!(stale["ok"], false);
    assert_eq!(stale["error"]["code"], "stale_revision");
    assert_eq!(stale["error"]["actual_revision"], revision + 1);

    let unknown = session.dispatch(json!({"command": "project.unknown", "params": {}}));
    assert_eq!(unknown["error"]["code"], "malformed_request");
    let extra = session.dispatch(json!({"command": "project.undo", "params": {"x": 1}}));
    assert_eq!(extra["error"]["code"], "malformed_request");

    assert_eq!(
        session.dispatch(json!({"command": "project.undo", "params": {}}))["ok"],
        true
    );
    let empty = session.dispatch(json!({"command": "project.undo", "params": {}}));
    assert_eq!(empty["error"]["code"], "history_empty");
    Ok(())
}

#[test]
fn pad_generator_is_available_through_the_shared_transport_contract() -> TestResult {
    let mut session = Session::new(Engine::demo()?);
    let original = session.engine().document().clone();
    let revision = session.engine().revision();
    let operation = json!({
        "editor": "Part", "operation": {
            "command": "part.generate_pads", "params": {"script": {
                "api_version": 1,
                "source": "for i in 0..2 { footprint.pad(i + 1, i * pitch, q(0.0, \"mm\"), pitch, pitch); }",
                "units": [{"symbol": "mm", "dimension": [1,0,0,0,0,0,0], "scale": 0.001}],
                "inputs": {"pitch": {"type": "quantity", "value": 2.0, "unit": "mm"}}
            }}
        }
    });
    let reply = session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": revision, "operation": operation,
    }}));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(
        reply["result"]["draft"]["footprint"]["pads"][1]["position"],
        json!([2.0, 0.0])
    );
    assert_eq!(session.engine().project(), original.project.as_ref());
    assert_eq!(
        session.dispatch(json!({"command": "project.undo", "params": {}}))["ok"],
        true
    );
    assert_eq!(session.engine().document(), &original);
    let catalog = serde_json::to_string(&Request::catalog())?;
    assert!(catalog.contains("part.generate_pads"));
    Ok(())
}

#[test]
fn save_and_open_are_file_effects_outside_undo_history() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("session.pcbcraft");
    let path_text = path.display().to_string();
    let mut session = Session::new(Engine::demo()?);
    let revision = session.engine().revision();
    assert_eq!(session.dispatch(edit("R10", revision))["ok"], true);
    let before = session.engine().to_file();
    let revision = session.engine().revision();

    let saved = session.dispatch(json!({"command": "project.save", "params": {"path": path_text}}));
    assert_eq!(saved["result"]["saved_path"], path_text.as_str());
    assert_eq!(
        session.engine().revision(),
        revision,
        "saving is not an edit"
    );
    assert!(session.engine().can_undo());
    // Save without a path reuses the session's file.
    assert_eq!(
        session.dispatch(json!({"command": "project.save", "params": {}}))["ok"],
        true
    );

    let mut other = Session::default();
    let refused = other.dispatch(json!({"command": "project.save", "params": {}}));
    assert_eq!(refused["error"]["code"], "invalid_input");
    let opened = other.dispatch(json!({"command": "project.open", "params": {"path": path_text}}));
    assert_eq!(opened["ok"], true);
    assert_eq!(other.engine().to_file(), before);
    assert!(!other.engine().can_undo());

    std::fs::write(&path, b"broken")?;
    let failed = other.dispatch(json!({"command": "project.open", "params": {"path": path_text}}));
    assert_eq!(failed["ok"], false);
    assert_eq!(
        other.engine().to_file(),
        before,
        "a failed open changes nothing"
    );
    Ok(())
}

#[test]
fn native_export_writes_a_new_solidcraft_file_only() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("board.solidcraft").display().to_string();
    let mut session = Session::new(Engine::demo()?);
    let request = json!({"command": "project.export_native", "params": {"path": path}});
    let first = session.dispatch(request.clone());
    assert_eq!(first["ok"], true, "{first}");
    assert!(std::path::Path::new(&path).exists());
    let second = session.dispatch(request);
    assert_eq!(second["ok"], false, "existing files are never overwritten");
    Ok(())
}

#[test]
fn catalog_matches_the_request_type() -> TestResult {
    let catalog = Request::catalog();
    assert_eq!(catalog.len(), 10);
    let schema = serde_json::to_string(&schemas())?;
    for command in &catalog {
        assert!(
            schema.contains(command.id),
            "{} is in the schema",
            command.id
        );
        assert!(command.params.is_object());
    }
    Ok(())
}
