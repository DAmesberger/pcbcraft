//! Slice C7 (contract C0): JSON-only scenario tests of the 3D model library (D1, D2). The
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
fn the_model_queries_refuse_until_d1() -> Fallible<()> {
    let mut session = Session::new(Engine::demo()?);
    let reply = session.dispatch(json!({"command": "project.query",
        "params": {"query": "library.models", "params": {"query": "kind:model pins:8"}}}));
    assert_eq!(refused(&reply).as_deref(), Some("unsupported"), "{reply}");
    Ok(())
}
