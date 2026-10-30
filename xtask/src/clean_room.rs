//! `cargo xtask clean-room`: no third-party EDA library data in the repository.
//!
//! Pcbcraft does not copy KiCad libraries (CC-BY-SA) or library data of other EDA tools
//! (see `skills/pcbcraft-libraries/SKILL.md#clean-room-sources`). Fixtures are original
//! synthetic data. The gate lists tracked and untracked-but-not-ignored files and fails on
//! EDA library file extensions, or on file contents that start like an EDA library or
//! design file. It cannot recognise data that was retyped by hand; review still applies.
use crate::{Result, root};
use std::fs::File;
use std::io::Read;
use std::process::Command;

/// Library and design file extensions of other EDA tools, compared case-insensitively.
const EXTENSIONS: &[&str] = &[
    "kicad_sym",
    "kicad_mod",
    "kicad_pcb",
    "kicad_sch",
    "kicad_pro",
    "kicad_dru",
    "schlib",
    "pcblib",
    "intlib",
    "schdoc",
    "pcbdoc",
    "lbr",
    "olb",
    "dcm",
];

/// Leading markers of EDA library and design files. Spelled in two parts so this source
/// file does not match itself.
const MARKERS: &[(&str, &str)] = &[
    ("(kicad_", "symbol_lib"),
    ("(kicad_", "pcb"),
    ("(kicad_", "sch"),
    ("EESchema-", "LIBRARY"),
    ("PCBNEW-", "LibModule"),
    ("(foot", "print \""),
    ("(mod", "ule \""),
];

/// Only the start of each file is inspected; the markers above are file headers.
const HEAD_BYTES: usize = 256;
const MAX_FILES: usize = 100_000;

pub fn check() -> Result<()> {
    let root = root()?;
    let listed = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(&root)
        .output()?;
    if !listed.status.success() {
        return Err(String::from_utf8_lossy(&listed.stderr).into_owned().into());
    }
    let markers: Vec<String> = MARKERS.iter().map(|(a, b)| format!("{a}{b}")).collect();
    let mut violations = Vec::new();
    let mut count = 0usize;
    for raw in listed.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        count += 1;
        if count > MAX_FILES {
            return Err("clean-room: too many files to check".into());
        }
        let path = String::from_utf8_lossy(raw).into_owned();
        let lower = path.to_ascii_lowercase();
        if let Some(ext) = EXTENSIONS
            .iter()
            .find(|e| lower.ends_with(&format!(".{e}")))
        {
            violations.push(format!("{path}: EDA library/design file type `.{ext}`"));
            continue;
        }
        let full = root.join(&path);
        if !full.is_file() {
            continue;
        }
        let mut head = Vec::with_capacity(HEAD_BYTES);
        File::open(&full)?
            .take(HEAD_BYTES as u64)
            .read_to_end(&mut head)?;
        let text = String::from_utf8_lossy(&head);
        let start = text.trim_start();
        if let Some(marker) = markers.iter().find(|m| start.starts_with(m.as_str())) {
            violations.push(format!("{path}: starts like EDA library data (`{marker}`)"));
        }
    }
    if violations.is_empty() {
        println!("clean-room: {count} files contain no EDA library data");
        return Ok(());
    }
    Err(format!(
        "third-party EDA library data is not allowed (skills/pcbcraft-libraries/SKILL.md#clean-room-sources):\n  {}",
        violations.join("\n  ")
    )
    .into())
}
