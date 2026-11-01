//! Slice C7: what the staged JSON scenarios share (the session's
//! `pinout_tests`, `firmware_tests`, `templates_tests`, `models_tests`
//! and the MCP tests): the EXM-48 fixture and the stub staging.
//!
//! The slices that implement the scenario's requests (B1, C1–C4, D1) land
//! in parallel with C7. Every step is asserted strictly; a step answered
//! by a C0 contract stub ([`STUBS`]) ends the scenario early, after the
//! caller proved the refusal changed nothing. Slice I sets
//! [`REQUIRE_COMPLETE`] once those slices are merged; from then on a stub
//! answer fails the test.
use serde_json::Value;

pub(crate) type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The EXM-48 pinout document, exactly as an agent would send it.
pub(crate) const EXM48: &str = include_str!("exm48.pinout.json");

/// Set by slice I once C1–C4 are merged: then a contract stub refusal is a
/// failure, not the end of the reachable scenario.
pub(crate) const REQUIRE_COMPLETE: bool = false;

/// The rule names of the C0 contract stubs, which slice I removes once no
/// slice emits them. A refusal or finding carrying one means "not built
/// yet", never a behaviour this test accepts.
pub(crate) const STUBS: &[&str] = &[
    "pinout.unsupported",
    "rule.unsupported",
    "pin_config.unsupported",
    "pins.unsupported",
    "functions.suggest_unsupported",
    "firmware.unsupported",
    "template.unsupported",
    "model.unsupported",
];

/// A scenario step answered by a C0 stub.
#[derive(Debug)]
pub(crate) struct Stub {
    pub step: String,
    pub rule: String,
}

impl std::fmt::Display for Stub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: contract stub {}", self.step, self.rule)
    }
}

impl std::error::Error for Stub {}

/// The stub a refusal or a finding list names, if any.
pub(crate) fn stub_in(value: &Value) -> Option<&'static str> {
    let text = value.to_string();
    STUBS.iter().copied().find(|stub| text.contains(stub))
}

/// End a staged scenario: a stub stop passes until [`REQUIRE_COMPLETE`].
pub(crate) fn finish(result: Fallible<()>) -> Fallible<()> {
    match result {
        Err(error) if !REQUIRE_COMPLETE => match error.downcast::<Stub>() {
            Ok(stub) => {
                eprintln!("scenario reached {stub}; the rest waits for its slice");
                Ok(())
            }
            Err(error) => Err(error),
        },
        other => other,
    }
}
