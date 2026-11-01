//! Slice C7: the `library.models` query on the wire (D1). The EXM-48
//! scenario's model step, ranking the generated model for the imported
//! package, is in `templates_tests::build_footprint`.
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
