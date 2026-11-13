//! `library.import_svg` through the JSON transport: the session reads the
//! file, Solidcraft's reader converts it, and one `library.edit_graphics`
//! step adds it.
use super::*;
use sparksmith_core::library::{
    Definition, DefinitionMeta, FootprintDefinition, PackageDefinition, SymbolDefinition, Unit,
    graphics::DrawingLayer,
};
use sparksmith_core::{PackageRef, Revision, UnitId};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

const MARK: &str = include_str!("../../../solidcraft-integration/src/svg/mark.svg");

fn new_draft(session: &mut Session, definition: Definition) -> std::result::Result<u64, String> {
    let operation = Operation::Library(LibraryCommand::NewDraft {
        meta: DefinitionMeta {
            name: "logo".into(),
            description: String::new(),
            provenance: "original test fixture".into(),
        },
        definition,
    });
    let reply = session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": session.engine().revision(),
        "operation": serde_json::to_value(operation).map_err(|e| e.to_string())?,
    }}));
    reply["result"]["outcome"]["id"]
        .as_u64()
        .ok_or_else(|| format!("no draft id in {reply}"))
}

fn import(revision: u64, draft: u64, target: Value, path: &str, scale: f64) -> Value {
    json!({"command": "library.import_svg", "params": {
        "expected_revision": revision, "draft": draft, "target": target,
        "path": path, "at": [10.0, 5.0], "scale": scale}})
}

fn symbol_graphics(session: &Session, draft: u64) -> usize {
    match session
        .engine()
        .drafts()
        .get(sparksmith_core::DefinitionId(draft))
        .map(|d| &d.definition)
    {
        Some(Definition::Symbol(s)) => s.units.iter().map(|u| u.graphics.len()).sum(),
        _ => usize::MAX,
    }
}

#[test]
fn an_svg_imports_atomically_as_one_undo_step() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("mark.svg");
    std::fs::write(&path, MARK)?;
    let path = path.to_str().ok_or("utf-8 path")?;
    let mut session = Session::default();
    let symbol = new_draft(
        &mut session,
        Definition::Symbol(SymbolDefinition {
            next_local: 1,
            units: vec![Unit {
                id: UnitId(1),
                label: String::new(),
                graphics: vec![],
                pins: vec![],
            }],
            ..SymbolDefinition::default()
        }),
    )?;
    let revision = session.engine().revision();

    let reply = session.dispatch(import(revision, symbol, json!({"unit": 1}), path, 1.0));
    assert_eq!(reply["ok"], true, "{reply}");
    let added = reply["result"]["outcome"]["added"].as_array().map(Vec::len);
    assert_eq!(added, Some(7), "{}", reply["result"]["outcome"]);
    assert_eq!(symbol_graphics(&session, symbol), 7);
    assert!(
        reply["result"]["import"]["notes"]
            .as_array()
            .is_some_and(|n| n
                .iter()
                .any(|n| n.as_str().is_some_and(|n| n.contains("holes")))),
        "{}",
        reply["result"]["import"]
    );
    assert_eq!(session.engine().revision(), revision + 1, "one step");

    // Refusals change nothing: stale revision, missing file, a scale whose
    // stroke is too wide, a courtyard target.
    let now = session.engine().revision();
    for (bad, code) in [
        (
            import(revision, symbol, json!({"unit": 1}), path, 1.0),
            "stale_revision",
        ),
        (
            import(now, symbol, json!({"unit": 1}), "/nonexistent/x.svg", 1.0),
            "io_error",
        ),
        (
            import(now, symbol, json!({"unit": 1}), path, 20.0),
            "invalid_input",
        ),
        (
            import(now, symbol, json!({"unit": 9}), path, 1.0),
            "not_found",
        ),
    ] {
        let reply = session.dispatch(bad);
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["error"]["code"], code, "{reply}");
        assert_eq!(session.engine().revision(), now);
        assert_eq!(symbol_graphics(&session, symbol), 7);
    }

    session.dispatch(json!({"command": "project.undo", "params": {"expected_revision": session.engine().revision()}}));
    assert_eq!(symbol_graphics(&session, symbol), 0, "undo removes it all");
    Ok(())
}

#[test]
fn an_svg_lands_on_footprint_silkscreen_not_the_courtyard() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("mark.svg");
    std::fs::write(&path, MARK)?;
    let path = path.to_str().ok_or("utf-8 path")?;
    let mut session = Session::default();
    let package = new_draft(
        &mut session,
        Definition::Package(PackageDefinition {
            envelope_nm: [2.0, 1.2, 0.5].map(sparksmith_core::library::package_form::typical_nm),
            ..PackageDefinition::default()
        }),
    )?;
    let footprint = new_draft(
        &mut session,
        Definition::Footprint(FootprintDefinition {
            package: PackageRef::new(sparksmith_core::DefinitionId(package), Revision::FIRST),
            next_local: 1,
            ..FootprintDefinition::default()
        }),
    )?;
    let revision = session.engine().revision();
    let courtyard = import(
        revision,
        footprint,
        json!({"footprint": {"layer": "courtyard", "side": "component"}}),
        path,
        0.2,
    );
    let reply = session.dispatch(courtyard);
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(session.engine().revision(), revision);

    let silk = json!({"footprint": {"layer": "silkscreen", "side": "component"}});
    let reply = session.dispatch(import(revision, footprint, silk, path, 0.2));
    assert_eq!(reply["ok"], true, "{reply}");
    let Some(Definition::Footprint(f)) = session
        .engine()
        .drafts()
        .get(sparksmith_core::DefinitionId(footprint))
        .map(|d| &d.definition)
    else {
        return Err("footprint draft".into());
    };
    assert_eq!(f.graphics.len(), 7);
    assert!(
        f.graphics
            .iter()
            .all(|g| g.layer == DrawingLayer::Silkscreen)
    );
    // Scaled to a fifth: the 28.4 mm badge is 5.68 mm wide around x = 10.
    let size = &reply["result"]["import"]["size"];
    assert!(
        (size[0].as_f64().unwrap_or(0.0) - 5.68).abs() < 1e-9,
        "{size}"
    );
    Ok(())
}

#[test]
fn import_svg_is_in_the_generated_catalog() -> TestResult {
    let catalog = Request::catalog();
    let info = catalog
        .iter()
        .find(|c| c.id == "library.import_svg")
        .ok_or("listed")?;
    let schema = info.params.to_string();
    for field in ["expected_revision", "draft", "target", "path", "scale"] {
        assert!(schema.contains(field), "{field} in {schema}");
    }
    assert!(
        crate::mcp::tools()
            .iter()
            .any(|t| t["name"] == crate::mcp::tool_name("library.import_svg"))
    );
    Ok(())
}
