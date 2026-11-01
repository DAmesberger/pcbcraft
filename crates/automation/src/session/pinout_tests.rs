//! Slice C7 (contract C0): JSON-only scenario tests of pinout import (C3). The
//! C3 test pins the request surface, preview, apply and the stale-base
//! refusal; C7 replaces it with the scenario (import EXM-48 → review →
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
fn the_pinout_schema_is_served_and_exm48_previews_and_applies() -> Fallible<()> {
    let mut session = Session::new(Engine::demo()?);
    let reply = session.dispatch(json!({"command": "library.pinout_schema", "params": {}}));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"]["format"], 1);
    assert!(reply["result"]["schema"].is_object());
    let document: Value = serde_json::from_str(pcbcraft_core::library::pinout::EXM48)?;
    let target = json!({"target": "new", "name": "EXM-48"});
    let reply = session.dispatch(json!({"command": "project.query",
        "params": {"query": "library.pinout_preview", "params": {
            "target": target, "document": document}}}));
    assert_eq!(reply["ok"], true, "{reply}");
    let plan = &reply["result"]["result"];
    assert_eq!(plan["review"][0]["row"], "r47", "{plan}");
    assert_eq!(plan["review"][0]["reason"], "uncertain", "{plan}");
    let base = plan["base"].clone();
    let apply = |base: Value, revision: u64| {
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": "Library", "operation": {
                "command": "library.pinout_apply",
                "params": {"target": target, "document": document, "base": base}}}}})
    };
    let reply = session.dispatch(apply(base, session.engine().revision()));
    assert_eq!(reply["ok"], true, "{reply}");
    let report = &reply["result"]["outcome"]["report"];
    assert_eq!(
        report["drafts"].as_array().map(Vec::len),
        Some(6),
        "{report}"
    );
    assert_eq!(report["unreviewed"].as_array().map(Vec::len), Some(49));
    // A plan for an existing draft goes stale when the draft changes.
    let id = report["drafts"][0].clone();
    let stale = json!({"target": "draft", "id": id});
    let reply = session.dispatch(json!({"command": "project.apply", "params": {
        "expected_revision": session.engine().revision(),
        "operation": {"editor": "Library", "operation": {
            "command": "library.pinout_apply",
            "params": {"target": stale, "document": document, "base": 0}}}}}));
    assert_eq!(refused(&reply).as_deref(), Some("conflict"), "{reply}");
    Ok(())
}
