//! Mechanical parts through automation: list, place a built-in, reuse the
//! variant, new values move the instance, explicit lands, refusals.
use super::super::Session;
use pcbcraft_core::Engine;
use pcbcraft_core::library::PadKind;
use serde_json::{Value, json};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn call(session: &mut Session, command: &str, params: Value) -> Value {
    session.dispatch(json!({"command": command, "params": params}))
}

fn revision(session: &Session) -> u64 {
    session.engine().revision()
}

fn place(session: &mut Session, mut params: Value) -> Value {
    params["expected_revision"] = json!(revision(session));
    call(session, "layout.place_part_file", params)
}

/// The built-in used: the module body is in every build of the table.
const BODY: &str = "esp32-s31-wroom-3";

#[test]
fn part_files_lists_the_builtins_with_their_parameters() -> TestResult {
    let mut session = Session::new(Engine::default());
    let reply = call(&mut session, "library.part_files", json!({}));
    let builtin = reply["result"]["builtin"]
        .as_array()
        .ok_or(format!("{reply}"))?;
    let body = builtin
        .iter()
        .find(|b| b["name"] == BODY)
        .ok_or("the module body is listed")?;
    assert_eq!(body["kind"], "package");
    assert!(
        body["parameters"]
            .as_array()
            .is_some_and(|p| p.iter().any(|p| p["name"] == "can_height"))
    );
    Ok(())
}

#[test]
fn a_placed_builtin_reuses_its_variant_and_new_values_move_it() -> TestResult {
    let mut session = Session::new(Engine::default());
    let first = place(
        &mut session,
        json!({"builtin": BODY, "position": [30.0, 30.0], "prefix": "MP"}),
    );
    assert_eq!(first["ok"], true, "{first}");
    let outcome = &first["result"]["outcome"];
    assert_eq!(outcome["outcome"], "part_placed", "{first}");
    let instance = outcome["instance"].as_u64().ok_or(format!("{first}"))?;
    assert_eq!(outcome["created"], true);
    assert_eq!(first["result"]["part"]["ok"], true);
    let second = place(
        &mut session,
        json!({"builtin": BODY, "position": [60.0, 30.0], "prefix": "MP"}),
    );
    assert_eq!(second["ok"], true, "{second}");
    let project = session.engine().project();
    assert_eq!(project.instances.len(), 2);
    assert_eq!(project.instances[0].component, project.instances[1].component);
    assert_eq!(project.library.models.len(), 1);
    let rev = revision(&session);
    let moved = call(
        &mut session,
        "layout.set_part_file_values",
        json!({"instance": instance, "values": {"can_height": {"mm": 3.0}},
            "expected_revision": rev}),
    );
    assert_eq!(moved["ok"], true, "{moved}");
    let project = session.engine().project();
    let a = project.instance(instance).ok_or("kept")?;
    assert_eq!(a.designator, "MP1");
    assert_ne!(a.component, project.instances[1].component);
    Ok(())
}

#[test]
fn explicit_lands_make_mechanical_pads_and_refusals_change_nothing() -> TestResult {
    let mut session = Session::new(Engine::default());
    let reply = place(
        &mut session,
        json!({"builtin": BODY, "position": [30.0, 30.0], "pads": "terminals",
            "lands": [{"name": "1", "at": [-8.0, 0.0], "shape": {"circle": {"diameter": 3.0, "hole": 1.6}}},
                      {"name": "2", "at": [8.0, 0.0], "shape": {"hole": {"diameter": 2.0}}}]}),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let project = session.engine().project();
    let footprint = &project.library.footprints[0].definition;
    assert_eq!(footprint.pads[0].kind, PadKind::Mechanical);
    assert!(footprint.pads[0].terminal.is_some(), "pads: terminals");
    assert_eq!(footprint.pads[1].kind, PadKind::NonPlatedHole);
    let component = &project.library.components[0].definition;
    assert_eq!(component.terminals.len(), 1, "a hole lands no terminal");
    let before = revision(&session);
    for bad in [
        json!({"builtin": "no-such-part", "position": [1.0, 1.0]}),
        json!({"builtin": BODY, "position": [1.0, 1.0], "values": {"can_height": {"deg": 3.0}}}),
        json!({"path": "/nonexistent/x.solidcraft", "position": [1.0, 1.0], "licence": "MIT"}),
        json!({"builtin": BODY, "position": [1.0, 1.0], "prefix": "lower"}),
    ] {
        let reply = place(&mut session, bad.clone());
        assert_eq!(reply["ok"], false, "{bad}: {reply}");
    }
    assert_eq!(revision(&session), before);
    Ok(())
}
