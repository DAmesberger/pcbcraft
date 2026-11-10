//! `cargo xtask convert-v1 PATH…`: a one-off converter for files written
//! before a units-migration slice changed the file format. Pcbcraft is
//! pre-release and keeps one file version with no legacy readers
//! (AGENTS.md), so this lives here, not in the app.
//!
//! Each PATH is a `.pcbcraft` project container, a library folder (a
//! directory holding `pcbcraft-library.json`) or a JSON-lines replay
//! (`*.jsonl`, [`replay`]). Stored f64 millimetres
//! become integer nanometres and degrees whole millidegrees, rounded half
//! away from zero as the app's one crossing does: U1 converts the
//! schematic ([`schematic`]), U2 library geometry wherever a definition is
//! stored ([`library`]). The result is read back as the app reads it before
//! anything is written, and a project's remembered revision and design
//! hashes are recomputed ([`identity`]).
//!
//! A value already converted is left alone, so running it twice changes
//! nothing. A file is replaced atomically after the original is copied to
//! `FILE.pre-nm` (a folder to `DIR.pre-nm`), never overwritten.
use crate::Result;
use std::path::Path;

mod container;
mod folder;
mod footprints;
mod forms;
mod identity;
mod json;
mod library;
mod marking;
mod models;
mod replay;
mod schematic;
mod symbols;

pub fn run(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Err(
            "usage: cargo xtask convert-v1 FILE.pcbcraft|LIBRARY_FOLDER|REPLAY.jsonl…".into(),
        );
    }
    for path in paths {
        let path = Path::new(path);
        let backup = container::sibling(path, ".pre-nm");
        let shown = path.display();
        if path.is_dir() {
            let done = folder::convert(path, &backup).map_err(|e| format!("{shown}: {e}"))?;
            if done.records == 0 {
                println!("{shown}: already current");
            } else {
                println!(
                    "{shown}: converted {} values in {} revision files (original kept as {})",
                    done.values,
                    done.records,
                    backup.display()
                );
            }
            continue;
        }
        if path.extension().is_some_and(|e| e == "jsonl") {
            let done = replay::convert(path, &backup).map_err(|e| format!("{shown}: {e}"))?;
            if done == 0 {
                println!("{shown}: already current");
            } else {
                println!(
                    "{shown}: converted {done} values in requests that carry definitions (original kept as {})",
                    backup.display()
                );
            }
            continue;
        }
        let done = container::convert(path, &backup).map_err(|e| format!("{shown}: {e}"))?;
        if done.values + done.identity.changed() == 0 {
            println!("{shown}: already current");
        } else {
            println!(
                "{shown}: converted {} values, recomputed {} origin hashes and {} design keys (original kept as {})",
                done.values,
                done.identity.origins,
                done.identity.design_keys,
                backup.display()
            );
        }
        for lineage in &done.identity.project_file_lineages {
            println!(
                "{shown}: {lineage} came from another project file; importing that file again brings it in as a new lineage"
            );
        }
    }
    Ok(())
}
