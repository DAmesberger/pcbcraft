//! Slice C3 (contract C0): `pinout preview|apply FILE.json --project P [--draft ID]: preview or apply a typed pinout document through library.pinout_preview / library.pinout_apply`.
//! Owned by C3; `main.rs` dispatches here and lists [`USAGE`] when it
//! is not empty.
use super::Result;

/// The usage line(s); empty until C3, so the command is not listed.
pub const USAGE: &str = "";

/// Run the subcommand with the arguments after its name. Contract stub.
pub fn run(_args: &[String]) -> Result<()> {
    Err("pinout import is not available yet (slice C3)".into())
}
