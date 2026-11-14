//! Step 1 to 2: the units migration and fab profiles as files, for files
//! written as version 1 (formerly `cargo xtask convert-v1`).
//!
//! Version 1 covered several incompatible changes, so a version 1 file may
//! be in any of their states. Stored f64 millimetres become integer
//! nanometres and degrees whole millidegrees, rounded half away from zero
//! as the app's one crossing does: U1 converts the schematic
//! ([`schematic`]), U2 library geometry wherever a definition is stored
//! ([`library`]). A value already in the new form is left alone. A
//! project's remembered revision and design hashes are recomputed after
//! the chain ([`identity`]).
//!
//! Fab profiles as files ([`rules`]): the old profile snapshot becomes
//! `--profile FILE.json`'s profile, or, for one the user entered from a
//! fabricator, the same values in the new form.
//!
//! Library folders (a directory holding `sparksmith-library.json`),
//! JSON-lines replays (`*.jsonl`, [`replay`]) and old profile library
//! files (`FILE.json`) have formats of their own without a version step;
//! they take the same units and profile conversion in place, idempotently,
//! after the original is copied to `FILE.pre-nm` (a folder to
//! `DIR.pre-nm`), never overwritten.
use crate::convert::{Options, Result};
use std::path::Path;

pub mod container;
mod folder;
mod footprints;
mod forms;
pub mod identity;
mod json;
mod library;
mod marking;
mod models;
mod replay;
mod rules;
mod schematic;
mod symbols;

/// A footprint or package-row body converted to integer units; the number
/// of values changed. Public for the footprint test that solves with the
/// real evaluator (`xtask`), which this crate cannot depend on.
#[doc(hidden)]
pub fn footprint_definition(
    kind: &str,
    body: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<usize> {
    footprints::definition(kind, body)
}

/// Converts a library folder, a replay or an old profile library file in
/// place; what was done, one line each.
pub fn convert_other(path: &Path) -> Result<Vec<String>> {
    let backup = container::sibling(path, ".pre-nm");
    let shown = path.display();
    let kept = backup.display();
    if path.is_dir() {
        let done = folder::convert(path, &backup)?;
        return Ok(vec![if done.records == 0 {
            format!("{shown}: already current")
        } else {
            format!(
                "{shown}: converted {} values in {} revision files (original kept as {kept})",
                done.values, done.records
            )
        }]);
    }
    let (done, what) = if path.extension().is_some_and(|e| e == "json") {
        (convert_profile(path, &backup)?, "")
    } else {
        (
            replay::convert(path, &backup)?,
            " in requests that carry definitions",
        )
    };
    Ok(vec![if done == 0 {
        format!("{shown}: already current")
    } else {
        format!("{shown}: converted {done} values{what} (original kept as {kept})")
    }])
}

/// What converting a project container did, one line each.
pub fn convert_project(path: &Path, options: &Options) -> Result<Vec<String>> {
    let shown = path.display();
    let done = container::convert(path, options)?;
    let Some(backup) = &done.backup else {
        return Ok(vec![format!(
            "{shown}: already version {}",
            sparksmith_core::format::FILE_VERSION
        )]);
    };
    let mut lines = vec![format!(
        "{shown}: converted from version {} to {}: {} values, recomputed {} origin hashes, {} design keys and {} fill keys (fill copper moved at most {} nm; original kept as {})",
        done.from,
        sparksmith_core::format::FILE_VERSION,
        done.values,
        done.identity.origins,
        done.identity.design_keys,
        done.identity.fill_keys,
        done.identity.fill_moved_nm,
        backup.display()
    )];
    for note in &done.notes {
        lines.push(format!("{shown}: {note}"));
    }
    for fill in &done.identity.stale_fills {
        lines.push(format!(
            "{shown}: fill {fill} would refill to other copper, so it was already stale; refill it in the app"
        ));
    }
    for lineage in &done.identity.project_file_lineages {
        lines.push(format!(
            "{shown}: {lineage} came from another project file; importing that file again brings it in as a new lineage"
        ));
    }
    Ok(lines)
}

/// The `fab_profile` of a current fab profile file.
pub fn read_profile(path: &Path) -> Result<serde_json::Value> {
    let file: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let profile = file
        .get("fab_profile")
        .filter(|p| file["format"] == 1 && p.get("source").is_some() && p.get("rules").is_some())
        .ok_or_else(|| {
            format!(
                "{} is not a format 1 fab profile file with a source",
                path.display()
            )
        })?;
    Ok(profile.clone())
}

/// Converts an old profile library file in place, keeping the original at
/// `backup`.
fn convert_profile(path: &Path, backup: &Path) -> Result<usize> {
    let original = std::fs::read(path)?;
    let mut file: serde_json::Value = serde_json::from_slice(&original)?;
    let changed = rules::convert_profile_file(&mut file)?;
    if changed > 0 {
        let mut text = String::new();
        pretty(&file, 0, &mut text)?;
        text.push('\n');
        if !backup.exists() {
            std::fs::write(backup, &original)?;
        }
        crate::persistence::atomic_write(path, text.as_bytes())?;
    }
    Ok(changed)
}

const PROFILE_KEY_ORDER: &[&str] = &[
    "format",
    "fab_profile",
    "id",
    "name",
    "publisher",
    "document",
    "url",
    "date",
    "notes",
    "kind",
    "pth_allowance",
    "value",
    "formula",
    "source",
    "drill_files",
    "rules",
];

/// `value` as `serde_json::to_string_pretty` writes it, but with object
/// keys in [`PROFILE_KEY_ORDER`], so a converted profile file keeps the
/// app's own layout.
fn pretty(value: &serde_json::Value, depth: usize, out: &mut String) -> Result<()> {
    let pad = |n: usize| "  ".repeat(n);
    match value {
        serde_json::Value::Object(map) if !map.is_empty() => {
            let rank = |k: &str| {
                PROFILE_KEY_ORDER
                    .iter()
                    .position(|o| *o == k)
                    .unwrap_or(PROFILE_KEY_ORDER.len())
            };
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
            out.push_str("{\n");
            for (i, key) in keys.iter().enumerate() {
                out.push_str(&pad(depth + 1));
                out.push_str(&serde_json::to_string(key)?);
                out.push_str(": ");
                pretty(&map[key.as_str()], depth + 1, out)?;
                out.push_str(if i + 1 < keys.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(depth));
            out.push('}');
        }
        serde_json::Value::Array(items) if !items.is_empty() => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&pad(depth + 1));
                pretty(item, depth + 1, out)?;
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(depth));
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other)?),
    }
    Ok(())
}
