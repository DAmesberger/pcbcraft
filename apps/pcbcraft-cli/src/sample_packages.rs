//! Slice B3 (contract C0): `sample-packages NEW_DIR: write the starter package-row library folder (about 150 cited rows)`.
//! Owned by B3; `main.rs` dispatches here and lists [`USAGE`] when it
//! is not empty.
use super::Result;

/// The usage line(s); empty until B3, so the command is not listed.
pub const USAGE: &str = "";

/// Run the subcommand with the arguments after its name. Contract stub.
pub fn run(_args: &[String]) -> Result<()> {
    Err("the starter package table is not available yet (slice B3)".into())
}
