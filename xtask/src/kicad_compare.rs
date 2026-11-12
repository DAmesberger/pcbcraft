//! `cargo xtask kicad-compare [--design PATH]`: import every KiCad board of
//! a design (a `.kicad_pcb` file or a folder; `PCBCRAFT_KICAD_DESIGN` when
//! no path is given), write our fabrication files, plot KiCad's own with
//! the installed `kicad-cli` from a temporary copy, and compare the pad
//! layers (copper pads, mask, paste) and drill hits to 1 µm.
//!
//! Privacy: the design is read in place and never written; no tool runs in
//! its folder. Everything produced goes to the ignored
//! `target/kicad-compare/<run>/`. The printed summary holds counts, areas,
//! widths and pass or fail only; names and locations stay in the local
//! `report.json` of each board. Boards are numbered, never named.
use crate::{Result, root};
use pcbcraft_core::fabrication::{self, FabricationOptions};
use pcbcraft_fabcheck::reference;
use pcbcraft_fabcheck::sets::{
    Arc, Corner, File, Judge, PadBox, PieceKind, Scope, SetDiff, attribute, compare_sets,
};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The comparison tolerance, nm.
const TOLERANCE_NM: i32 = 1000;
/// KiCad approximates round custom-pad primitives within the board's
/// maximum error (5 µm by default): such pieces may be 1 µm wider.
const ARC_TOLERANCE_NM: i32 = 6000;
/// Folders a design search skips: version control, KiCad's backups and
/// local history.
fn skipped(name: &str) -> bool {
    name.starts_with('.') || name.ends_with("-backups") || name == "target"
}

pub fn run(args: &[String]) -> Result<()> {
    let design = match args {
        [] => std::env::var_os("PCBCRAFT_KICAD_DESIGN").map(PathBuf::from),
        [flag, path] if flag == "--design" => Some(PathBuf::from(path)),
        _ => return Err("usage: cargo xtask kicad-compare [--design PATH]".into()),
    }
    .ok_or("name the design: --design PATH or PCBCRAFT_KICAD_DESIGN")?;
    let (summary, passed) = compare_design(&design)?;
    print!("{summary}");
    if passed {
        Ok(())
    } else {
        Err("kicad-compare: some pad layers or drills differ beyond the tolerance".into())
    }
}

/// The boards of a design: the file itself, or every `.kicad_pcb` under a
/// folder (not in backups or hidden folders), sorted.
fn boards(design: &Path) -> Result<Vec<PathBuf>> {
    if design.is_file() {
        return Ok(vec![design.to_path_buf()]);
    }
    let mut out = Vec::new();
    let mut stack = vec![design.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            if path.is_dir() {
                if !skipped(&name) {
                    stack.push(path);
                }
            } else if name.ends_with(".kicad_pcb") && !name.starts_with('_') {
                out.push(path);
            }
        }
        if out.len() > 64 {
            return Err("kicad-compare: more than 64 boards in the design".into());
        }
    }
    out.sort();
    Ok(out)
}

/// Compare every board of `design`; the summary and whether all passed.
pub fn compare_design(design: &Path) -> Result<(String, bool)> {
    let run_dir = root()?.join("target/kicad-compare").join(format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
    ));
    let version = reference::version().ok_or("kicad-cli cannot be run")?;
    let boards = boards(design)?;
    let mut summary = format!(
        "kicad-compare: {} board files, kicad-cli {version}, tolerance {} µm (arcs {} µm)\n",
        boards.len(),
        f64::from(TOLERANCE_NM) / 1e3,
        f64::from(ARC_TOLERANCE_NM) / 1e3
    );
    let mut passed = true;
    for (k, board) in boards.iter().enumerate() {
        let dir = run_dir.join(format!("board-{}", k + 1));
        std::fs::create_dir_all(&dir)?;
        match compare_board(board, &dir) {
            Ok((text, ok)) => {
                let status = if !ok {
                    "FAIL"
                } else if text.contains(" pass* ") {
                    "pass*"
                } else {
                    "pass"
                };
                let _ = writeln!(summary, "board {}: {status}", k + 1);
                summary.push_str(&text);
                passed &= ok;
            }
            Err(e) => {
                // Errors name a step or a token path, never file content.
                let _ = writeln!(summary, "board {}: not compared: {}", k + 1, kind_of(&*e));
                passed = false;
            }
        }
    }
    let _ = writeln!(
        summary,
        "pass*: within tolerance except mask or paste openings at sharp land corners (KiCad offsets round; ours keeps corners sharp)\noutputs under target/kicad-compare/ (local only)"
    );
    Ok((summary, passed))
}

/// The leading code of an error (`kicad.version`, `kicad.refused: …`),
/// without the core's free text, which may quote a name.
fn kind_of(e: &dyn std::error::Error) -> String {
    let text = e.to_string();
    text.split(':').next().unwrap_or("error").to_owned()
}

fn compare_board(board: &Path, dir: &Path) -> Result<(String, bool)> {
    let bytes = std::fs::read(board)?;
    let imported = pcbcraft_kicad::import(&bytes)?;
    let project = board.with_extension("kicad_pro");
    let project = project.is_file().then_some(project);
    let copper = imported.report.counts.copper_layers;
    let theirs = reference::plot(board, project.as_deref(), copper, dir)?;
    let set = fabrication::generate(
        imported.engine.project(),
        &FabricationOptions {
            base_name: Some("ours".into()),
            ..FabricationOptions::default()
        },
    )?;
    let ours: Vec<File> = set
        .files
        .iter()
        .map(|f| File {
            name: f.name.clone(),
            bytes: f.bytes.clone(),
        })
        .collect();
    let ours_dir = dir.join("ours");
    std::fs::create_dir_all(&ours_dir)?;
    for f in &ours {
        std::fs::write(ours_dir.join(&f.name), &f.bytes)?;
    }
    let judge = Judge {
        tolerance_nm: TOLERANCE_NM,
        arc_tolerance_nm: ARC_TOLERANCE_NM,
        arcs: imported
            .pads
            .iter()
            .flat_map(|p| p.arcs.iter())
            .map(|(center, radius)| Arc {
                center: *center,
                radius: *radius,
            })
            .collect(),
        corners: imported
            .pads
            .iter()
            .flat_map(|p| p.corners.iter())
            .map(|(at, reach)| Corner {
                at: *at,
                reach: *reach,
            })
            .collect(),
    };
    let pads: Vec<PadBox> = imported
        .pads
        .iter()
        .map(|p| PadBox {
            min: p.world.min,
            max: p.world.max,
            class: p.class.clone(),
        })
        .collect();
    let diff = compare_sets(&ours, &theirs, Scope::Pads, &judge)?;
    let (text, ok, rows) = summarize(&imported.report, &diff, &judge, &pads);
    let local = serde_json::json!({
        "kicad_cli": reference::version(),
        "board_sha256": format!("{:x}", Sha256::digest(&bytes)),
        "import": imported.report.to_json(),
        "pieces": rows,
    });
    std::fs::write(dir.join("report.json"), serde_json::to_vec_pretty(&local)?)?;
    Ok((text, ok))
}

/// The printed summary (counts, areas, widths, generic pad classes and
/// pass or fail), whether every layer passed, and the per-class rows with
/// locations for the local report.
pub fn summarize(
    report: &pcbcraft_kicad::Report,
    diff: &SetDiff,
    judge: &Judge,
    pads: &[PadBox],
) -> (String, bool, Vec<serde_json::Value>) {
    let mut out = String::new();
    for line in report.summary().lines() {
        let _ = writeln!(out, "  {line}");
    }
    let mut ok = true;
    let mut rows = Vec::new();
    for layer in &diff.layers {
        let status = match (layer.passes, layer.corner_pieces) {
            (false, _) => "FAIL",
            (true, 0) => "pass",
            // Within tolerance except openings at sharp land corners,
            // which KiCad offsets round and our writer keeps sharp (an
            // open decision; never counted as a pass silently).
            (true, _) => "pass*",
        };
        let _ = writeln!(
            out,
            "  {:<16} {status} | kept {}/{} reference objects | ours {:.4} mm², kicad {:.4} mm², xor {:.6} mm² | worst {:.3} µm | arcs {} to {:.3} µm | corners {} to {:.3} µm",
            layer.function,
            layer.kept_objects,
            layer.reference_objects,
            layer.diff.area_a_mm2,
            layer.diff.area_b_mm2,
            layer.diff.difference_mm2,
            layer.worst_um,
            layer.arc_pieces,
            layer.arc_worst_um,
            layer.corner_pieces,
            layer.corner_worst_um
        );
        ok &= layer.passes;
        for row in attribute(layer, judge, pads) {
            if row.kind == PieceKind::Other {
                let _ = writeln!(
                    out,
                    "      {:>5} pieces, widest {:.3} µm: {}",
                    row.pieces, row.widest_um, row.class
                );
            }
            rows.push(serde_json::json!({
                "layer": layer.function,
                "kind": format!("{:?}", row.kind),
                "class": row.class,
                "pieces": row.pieces,
                "widest_um": row.widest_um,
                "at_mm": row.at,
            }));
        }
    }
    for drill in &diff.drills {
        let _ = writeln!(
            out,
            "  {:<16} {} | ours {} hits, kicad {} (+{} via hits left out) | matched {} | worst {:.3} µm, diameter {:.3} µm",
            drill.function,
            if drill.diff.passes() { "pass" } else { "FAIL" },
            drill.diff.hits_a,
            drill.diff.hits_b,
            drill.excluded,
            drill.diff.matched,
            drill.diff.worst_position_um,
            drill.diff.worst_diameter_um
        );
        ok &= drill.diff.passes();
    }
    (out, ok, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The private design comparison: opt-in, ignored by default, and
    /// silent when `PCBCRAFT_KICAD_DESIGN` is not set (CI, other machines).
    #[test]
    #[ignore = "needs the private design and kicad-cli; set PCBCRAFT_KICAD_DESIGN"]
    fn the_private_design_matches_kicad() -> Result<()> {
        let Some(design) = std::env::var_os("PCBCRAFT_KICAD_DESIGN") else {
            return Ok(());
        };
        let (summary, passed) = compare_design(Path::new(&design))?;
        eprint!("{summary}");
        assert!(passed, "pad layers or drills differ beyond the tolerance");
        Ok(())
    }

    #[test]
    fn the_summary_names_nothing_from_the_board() -> Result<()> {
        // A board whose every name is distinctive; our set against itself.
        let text = "(kicad_pcb (version 20240108) (generator \"zqxgen\") \
            (layers (0 \"F.Cu\" signal) (31 \"B.Cu\" signal) (39 \"F.Mask\" user) (35 \"F.Paste\" user)) \
            (setup (pad_to_mask_clearance 0.05)) (net 1 \"QQSECRETNET\") \
            (footprint \"ZqxLibrary:ZqxFootprint\" (layer \"F.Cu\") (at 101.2345 77.6543 90) \
              (property \"Reference\" \"ZQX42\" (at 0 0 0) (layer \"F.SilkS\")) \
              (property \"Value\" \"QQSECRETVALUE\" (at 0 0 0) (layer \"F.Fab\")) \
              (pad \"QQ7\" smd rect (at 0.4321 0 90) (size 1.2345 0.6789) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\") \
                (net 1 \"QQSECRETNET\") (pinfunction \"QQPINFN\") (zqxtoken 1))))";
        let imported = pcbcraft_kicad::import(text.as_bytes())?;
        let set = fabrication::generate(imported.engine.project(), &FabricationOptions::default())?;
        let ours: Vec<File> = set
            .files
            .iter()
            .map(|f| File {
                name: f.name.clone(),
                bytes: f.bytes.clone(),
            })
            .collect();
        let judge = Judge::exact(TOLERANCE_NM);
        let diff = compare_sets(&ours, &ours, Scope::Pads, &judge)?;
        let (summary, _, _) = summarize(&imported.report, &diff, &judge, &[]);
        assert!(summary.contains("refused"), "{summary}");
        for secret in [
            "zqx", "ZQX", "Zqx", "QQ", "101.2345", "77.6543", "1.2345", "0.6789", "0.4321",
        ] {
            assert!(!summary.contains(secret), "{secret} in {summary}");
        }
        Ok(())
    }
}
