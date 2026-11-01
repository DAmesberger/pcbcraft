//! Slice C7 (contract C0): JSON-only scenario tests of firmware export (C4). The
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
fn firmware_export_refuses_until_c4() -> Fallible<()> {
    let mut session = Session::new(Engine::demo()?);
    let instance = session
        .engine()
        .project()
        .instances
        .first()
        .map(|i| i.id)
        .ok_or("instance")?;
    let reply = session.dispatch(json!({"command": "firmware.export_preview",
        "params": {"instance": instance, "emitter": {"emitter": "c_header"}}}));
    assert_eq!(refused(&reply).as_deref(), Some("unsupported"), "{reply}");
    Ok(())
}
