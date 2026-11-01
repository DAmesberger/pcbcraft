//! Slice C7: what the JSON scenarios share (the session's `pinout_tests`,
//! `firmware_tests`, `templates_tests`, `models_tests` and the MCP tests):
//! the EXM-48 fixture. Every track is integrated, so each scenario runs to
//! its end and any unexpected refusal fails it.

pub(crate) type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The EXM-48 pinout document, exactly as an agent would send it.
pub(crate) const EXM48: &str = include_str!("exm48.pinout.json");
