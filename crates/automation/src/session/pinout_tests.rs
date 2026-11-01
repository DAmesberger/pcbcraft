//! Slice C7 (contract C0): JSON-only scenario tests of pinout import (C3). The
//! contract test pins the request surface and the explicit refusal of
//! the stubs; C7 replaces it with the scenario (import EXM-48 → review →
//! apply → assign → report → export).
use super::*;

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The diagnostic code of a refused reply.
fn refused(reply: &Value) -> Option<String> {
    (reply["ok"] == false).then(|| {
        reply["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    })
}

#[test]
fn the_pinout_schema_is_served_and_the_preview_refuses_until_c3() -> Fallible<()> {
    let mut session = Session::new(Engine::demo()?);
    let reply = session.dispatch(json!({"command": "library.pinout_schema", "params": {}}));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"]["format"], 1);
    assert!(reply["result"]["schema"].is_object());
    let document = json!({"format": 1,
        "device": {"name": "EXM-48 controller", "reference": {"title": "EXM datasheet"}},
        "variants": [{"name": "QFN-48", "terminal_count": 49}],
        "pins": []});
    let reply = session.dispatch(json!({"command": "project.query",
        "params": {"query": "library.pinout_preview", "params": {
            "target": {"target": "new", "name": "EXM-48"}, "document": document}}}));
    assert_eq!(refused(&reply).as_deref(), Some("unsupported"), "{reply}");
    Ok(())
}
