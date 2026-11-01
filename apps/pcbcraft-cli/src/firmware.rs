//! Slice C4 (contract C0): `firmware FILE.pcbcraft DESIGNATOR EMITTER NEW_FILE: export one instance's firmware pin configuration`.
//! Owned by C4; `main.rs` dispatches here and lists [`USAGE`] when it
//! is not empty.
use super::Result;

/// The usage line(s); empty until C4, so the command is not listed.
pub const USAGE: &str = "";

/// Run the subcommand with the arguments after its name. Contract stub.
pub fn run(_args: &[String]) -> Result<()> {
    Err("firmware export is not available yet (slice C4)".into())
}
