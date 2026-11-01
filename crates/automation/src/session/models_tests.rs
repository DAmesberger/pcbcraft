//! Slice C7 (contract C0): JSON-only scenario tests of the 3D model library (D1, D2). D1
//! pins the `library.models` query on the wire; C7 adds the scenario
//! (import EXM-48 → review → apply → assign → report → export).
use super::*;

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn the_model_query_answers_on_the_wire() -> Fallible<()> {
    let mut session = Session::new(Engine::demo()?);
    let reply = session.dispatch(json!({"command": "project.query",
        "params": {"query": "library.models", "params": {"query": "kind:model mount:smd"}}}));
    assert_eq!(reply["ok"], true, "{reply}");
    let text = reply.to_string();
    assert!(text.contains("Generated · chip"), "{reply}");
    let reply = session.dispatch(json!({"command": "project.query",
        "params": {"query": "library.models", "params": {"query": "kind:part"}}}));
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["error"]["code"], "invalid_input", "{reply}");
    Ok(())
}
