//! `library.edit_package` and `library.fit_leads` sent as JSON behave as
//! through the engine: a quick action is one undo step, a fit with
//! `confirm: false` only reports, `confirm: true` stores the placement, and
//! the MCP instructions and generated schemas name both commands.
use super::*;
use sparksmith_core::library::{Definition, ModelPlacement};
use sparksmith_core::{DefinitionId, Engine};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

/// The demo's open package draft and its placement.
fn package(session: &Session) -> Option<(DefinitionId, ModelPlacement)> {
    session
        .engine()
        .drafts()
        .items
        .iter()
        .find_map(|d| match &d.definition {
            Definition::Package(p) => Some((d.id, p.placement)),
            _ => None,
        })
}

fn library(command: &str, params: Value, revision: u64) -> Value {
    json!({"command": "project.apply", "params": {
        "expected_revision": revision,
        "operation": {"editor": "Library", "operation": {"command": command, "params": params}}
    }})
}

#[test]
fn package_placement_commands_through_json_report_store_and_undo() -> TestResult {
    let mut session = Session::new(Engine::demo()?);
    let (draft, start) = package(&session).ok_or("no package draft")?;
    let reply = session.dispatch(library(
        "library.edit_package",
        json!({"draft": draft, "edits": [
            {"edit": "apply_quick", "value": {"rotate": {"axis": "z", "quarter_turns": 1}}}
        ]}),
        session.engine().revision(),
    ));
    assert_eq!(reply["ok"], true, "{reply}");
    let turned = package(&session).ok_or("no package")?.1;
    assert_eq!(turned.rotation_deg(), [0.0, 0.0, 90.0]);
    // Report only: the residuals of a fit, nothing stored.
    let fit = |confirm: bool, revision: u64| {
        library(
            "library.fit_leads",
            json!({"draft": draft, "pairs": [], "rotation": "quarter", "confirm": confirm}),
            revision,
        )
    };
    let reply = session.dispatch(fit(false, session.engine().revision()));
    assert_eq!(reply["ok"], true, "{reply}");
    let outcome = &reply["result"]["outcome"];
    assert_eq!(outcome["applied"], false, "{outcome}");
    assert!(outcome["fit"]["rms_mm"].as_f64().is_some_and(|r| r < 1e-9));
    assert_eq!(package(&session).ok_or("no package")?.1, turned);
    let reply = session.dispatch(fit(true, session.engine().revision()));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"]["outcome"]["applied"], true);
    assert_eq!(
        package(&session).ok_or("no package")?.1.rotation_deg(),
        [0.0; 3]
    );
    // Unknown pair IDs are refused and change nothing.
    let revision = session.engine().revision();
    let bad = session.dispatch(library(
        "library.fit_leads",
        json!({"draft": draft, "rotation": "free", "confirm": true, "pairs": [
            {"terminal": 99, "pad": 1, "lead": [0.0, 0.0, 0.0], "pad_centre": [0.0, 0.0]}
        ]}),
        revision,
    ));
    assert_eq!(bad["ok"], false, "{bad}");
    assert_eq!(session.engine().revision(), revision);
    // Each stored change is one undo step.
    for expected in [turned, start] {
        let undo = session.dispatch(json!({"command": "project.undo", "params": {"expected_revision": session.engine().revision()}}));
        assert_eq!(undo["ok"], true, "{undo}");
        assert_eq!(package(&session).ok_or("no package")?.1, expected);
    }
    let schemas = schemas().to_string();
    assert!(schemas.contains("library.edit_package") && schemas.contains("library.fit_leads"));
    let instructions = crate::mcp::INSTRUCTIONS;
    assert!(instructions.contains("library.edit_package"));
    assert!(instructions.contains("library.fit_leads"));
    Ok(())
}
