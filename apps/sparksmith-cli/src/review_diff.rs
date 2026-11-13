//! `review-diff`: two projects of drafts (two datasheet-assistant runs, or
//! a run and a reference) compared pin by pin, rule by rule, pad by pad
//! and by package form and marking, as Markdown on stdout
//! (`sparksmith_assist::pins::diff`). Read-only.
use super::{Result, open};
use sparksmith_core::product::SHORT_NAME as CLI;
use std::io::Write;

pub const USAGE: &str = "review-diff A.sparksmith B.sparksmith [NAME_A NAME_B]
                                   compare two projects of part drafts pin by pin (Markdown)";

pub fn run(args: &[String]) -> Result<()> {
    let (a, b, names) = match args {
        [a, b] => (a, b, ("A", "B")),
        [a, b, na, nb] => (a, b, (na.as_str(), nb.as_str())),
        _ => return Err(format!("usage: {CLI} {USAGE}").into()),
    };
    let first = open(Some(a))?.snapshot();
    let second = open(Some(b))?.snapshot();
    let text = sparksmith_assist::pins::diff(&first, &second, names);
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "# Draft comparison: {} ({a}) and {} ({b})\n",
        names.0, names.1
    )?;
    out.write_all(text.as_bytes())?;
    Ok(())
}
