//! `cargo xtask kicad-compare [--design PATH]`: import every KiCad board of
//! a design (a `.kicad_pcb` file or a folder; `SPARKSMITH_KICAD_DESIGN` when
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
use sha2::{Digest, Sha256};
use sparksmith_core::fabrication::{self, FabricationOptions};
use sparksmith_fabcheck::reference;
use sparksmith_fabcheck::sets::{
    Arc, File, Judge, PadBox, PieceKind, Scope, SetDiff, attribute, compare_sets,
};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The comparison tolerance, nm.
const TOLERANCE_NM: i32 = 1000;
/// KiCad approximates round custom-pad primitives within the board's
/// maximum error (5 µm by default): such pieces may be 1 µm wider.
const ARC_TOLERANCE_NM: i32 = 6000;
/// Both drill files are written to 1 µm per axis: a hit whose place ends
/// in half a micrometre may round apart by √2 µm (beyond the tolerance).
const DRILL_RESOLUTION_NM: i32 = 1415;
/// Folders a design search skips: version control, KiCad's backups and
/// local history.
fn skipped(name: &str) -> bool {
    name.starts_with('.') || name.ends_with("-backups") || name == "target"
}

pub fn run(args: &[String]) -> Result<()> {
    let design = match args {
        [] => std::env::var_os("SPARKSMITH_KICAD_DESIGN").map(PathBuf::from),
        [flag, path] if flag == "--design" => Some(PathBuf::from(path)),
        _ => return Err("usage: cargo xtask kicad-compare [--design PATH]".into()),
    }
    .ok_or("name the design: --design PATH or SPARKSMITH_KICAD_DESIGN")?;
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
                let status = if ok { "pass" } else { "FAIL" };
                let _ = writeln!(summary, "board {}: {status}", k + 1);
                summary.push_str(&text);
                passed &= ok;
            }
            Err(e) => {
                // The full error stays in the board's local folder.
                let _ = std::fs::write(dir.join("error.txt"), e.to_string());
                // Errors name a step or a token path, never file content.
                let _ = writeln!(summary, "board {}: not compared: {}", k + 1, kind_of(&*e));
                passed = false;
            }
        }
    }
    let _ = writeln!(summary, "outputs under target/kicad-compare/ (local only)");
    Ok((summary, passed))
}

/// The leading code of an error (`kicad.version`, `kicad.refused: …`),
/// without the core's free text, which may quote a name.
fn kind_of(e: &(dyn std::error::Error + 'static)) -> String {
    // A refused step is named (a fixed command name), its reason is not.
    if let Some(sparksmith_kicad::ImportError::Refused { step, .. }) =
        e.downcast_ref::<sparksmith_kicad::ImportError>()
    {
        return format!("kicad.refused at {step}");
    }
    let text = e.to_string();
    text.split(':').next().unwrap_or("error").to_owned()
}

fn compare_board(board: &Path, dir: &Path) -> Result<(String, bool)> {
    let bytes = std::fs::read(board)?;
    let project = board.with_extension("kicad_pro");
    let project = project.is_file().then_some(project);
    let project_bytes = project.as_deref().map(std::fs::read).transpose()?;
    let schematic = sparksmith_io::kicad_files::schematic_files(board)?;
    let imported = import(&bytes, project_bytes.as_deref(), schematic.as_ref())?;
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
        shift_nm: imported.plot_shift_nm(),
        drill_resolution_nm: DRILL_RESOLUTION_NM,
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
    let diff = compare_sets(&ours, &theirs, Scope::Filled, &judge)?;
    let (mut text, ok, rows) = summarize(&imported.report, &diff, &judge, &pads);
    let (line, outline_ok) = outline_line(&imported, &theirs, judge.shift_nm)?;
    text.push_str(&line);
    text.push_str(&findings_line(imported.engine.project()));
    let ok = ok && outline_ok;
    let local = serde_json::json!({
        "kicad_cli": reference::version(),
        "board_sha256": format!("{:x}", Sha256::digest(&bytes)),
        "import": imported.report.to_json(),
        "pieces": rows,
    });
    std::fs::write(dir.join("report.json"), serde_json::to_vec_pretty(&local)?)?;
    Ok((text, ok))
}

/// The board checks' findings after the import, counted by code (fixed
/// text: no name or place from the board).
fn findings_line(project: &sparksmith_core::Project) -> String {
    let mut by_code: std::collections::BTreeMap<String, usize> = Default::default();
    for finding in sparksmith_core::layout::checks(project, &Default::default()) {
        *by_code.entry(finding.code.to_string()).or_default() += 1;
    }
    let total: usize = by_code.values().sum();
    let list: Vec<String> = by_code
        .iter()
        .map(|(code, n)| format!("{code} {n}"))
        .collect();
    format!("  findings         {total} | {}\n", list.join(", "))
}

/// Import a board onto the shipped new board and fab profile. The gates
/// never build Solidcraft, so only with the `sketch` feature is there a
/// board sketch engine (the outline imports as a sketch, as in the app and
/// the CLI); without it the outline is the rectangle around it.
fn import(
    bytes: &[u8],
    project_file: Option<&[u8]>,
    schematic: Option<&sparksmith_io::kicad_files::SchematicFiles>,
) -> Result<sparksmith_kicad::Imported> {
    let defaults = sparksmith_io::new_project::shipped()?;
    let profile = sparksmith_io::rule_profiles::for_new_board_from_env(None)?;
    Ok(sparksmith_kicad::import(
        bytes,
        &sparksmith_kicad::Setup {
            board: &defaults.board,
            presets: &defaults.presets,
            profile: Some(&profile),
            evaluators: evaluators(),
            project_file,
            schematic: schematic
                .map(|(root, files)| sparksmith_kicad::schematic::SchematicFiles { root, files }),
        },
    )?)
}

#[cfg(feature = "sketch")]
fn evaluators() -> sparksmith_core::Evaluators<'static> {
    sparksmith_core::Evaluators {
        footprint: Some(&sparksmith_solidcraft::footprint::SketchEvaluator),
        board: Some(&sparksmith_solidcraft::board::SketchBoardEvaluator),
    }
}

#[cfg(not(feature = "sketch"))]
fn evaluators() -> sparksmith_core::Evaluators<'static> {
    sparksmith_core::Evaluators::default()
}

/// Most points the outline check samples on one reference curve.
const MAX_SAMPLES: usize = 4096;
/// Sample spacing on the reference outline, mm.
const SAMPLE_MM: f64 = 0.05;

/// The board outline against KiCad's `Edge.Cuts` plot: our solved outline
/// (outer and cutouts) and the widest distance of KiCad's drawn centre
/// lines from it, with both lengths, and whether it is within the
/// tolerance. Aggregate numbers only.
fn outline_line(
    imported: &sparksmith_kicad::Imported,
    theirs: &[File],
    shift: [i64; 2],
) -> Result<(String, bool)> {
    use sparksmith_fabcheck::gerber::{self, Kind};
    let board = imported.engine.project().board().into_owned();
    let tol = 1e-4;
    let mut rings = vec![board.outer.flatten(tol)];
    rings.extend(board.cutouts.iter().map(|c| c.outline.flatten(tol)));
    let ours_len: f64 = rings
        .iter()
        .map(|r| {
            (0..r.len())
                .map(|k| sparksmith_core::geometry::distance(r[k], r[(k + 1) % r.len()]))
                .sum::<f64>()
        })
        .sum();
    let near = |p: [f64; 2]| {
        rings
            .iter()
            .flat_map(|r| {
                (0..r.len()).map(move |k| {
                    sparksmith_core::geometry::segment_distance(p, r[k], r[(k + 1) % r.len()])
                })
            })
            .fold(f64::INFINITY, f64::min)
    };
    let Some(file) = theirs.iter().find_map(|f| {
        let parsed = gerber::parse(std::str::from_utf8(&f.bytes).ok()?).ok()?;
        let function = parsed.file_attribute(".FileFunction")?;
        (function.first().map(String::as_str) == Some("Profile")).then_some(parsed)
    }) else {
        return Ok(("  outline          no KiCad Edge.Cuts plot\n".into(), true));
    };
    let d = [shift[0] as f64 / 1e6, shift[1] as f64 / 1e6];
    let moved = |p: [f64; 2]| [p[0] + d[0], p[1] + d[1]];
    let (mut worst, mut theirs_len, mut samples) = (0.0f64, 0.0, 0usize);
    for object in &file.objects {
        let points: Vec<[f64; 2]> = match &object.kind {
            Kind::Draw { from, to } => {
                let (a, b) = (moved(*from), moved(*to));
                let len = sparksmith_core::geometry::distance(a, b);
                theirs_len += len;
                let n = ((len / SAMPLE_MM).ceil() as usize).clamp(1, MAX_SAMPLES);
                (0..=n)
                    .map(|k| {
                        let t = k as f64 / n as f64;
                        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
                    })
                    .collect()
            }
            Kind::Arc {
                from,
                to,
                center,
                clockwise,
            } => {
                let (a, b, c) = (moved(*from), moved(*to), moved(*center));
                let r = sparksmith_core::geometry::distance(a, c);
                let a0 = (a[1] - c[1]).atan2(a[0] - c[0]);
                let a1 = (b[1] - c[1]).atan2(b[0] - c[0]);
                let tau = std::f64::consts::TAU;
                let mut sweep = if *clockwise {
                    -(a0 - a1).rem_euclid(tau)
                } else {
                    (a1 - a0).rem_euclid(tau)
                };
                if sweep.abs() < 1e-12 {
                    sweep = if *clockwise { -tau } else { tau };
                }
                theirs_len += r * sweep.abs();
                let n = ((r * sweep.abs() / SAMPLE_MM).ceil() as usize).clamp(1, MAX_SAMPLES);
                (0..=n)
                    .map(|k| {
                        let t = a0 + sweep * k as f64 / n as f64;
                        [c[0] + r * t.cos(), c[1] + r * t.sin()]
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        samples += points.len();
        for p in points {
            worst = worst.max(near(p));
        }
    }
    let sketch = matches!(
        imported.engine.project().board_shape,
        sparksmith_core::model::board::BoardShape::Sketch(_)
    );
    let ok = worst * 1e6 <= f64::from(TOLERANCE_NM);
    let text = format!(
        "  outline          {} {} | {} cutouts | ours {:.3} mm long, kicad {:.3} mm | {} samples, worst {:.3} µm\n",
        if ok { "pass" } else { "FAIL" },
        if sketch { "sketch" } else { "rectangle" },
        board.cutouts.len(),
        ours_len,
        theirs_len,
        samples,
        worst * 1000.0
    );
    Ok((text, ok))
}

/// The printed summary (counts, areas, widths, generic pad classes and
/// pass or fail), whether every layer passed, and the per-class rows with
/// locations for the local report.
pub fn summarize(
    report: &sparksmith_kicad::Report,
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
        let status = if layer.passes { "pass" } else { "FAIL" };
        let _ = writeln!(
            out,
            "  {:<16} {status} | kept {}/{} reference objects | ours {:.4} mm², kicad {:.4} mm², xor {:.6} mm² (ours only {:.6}, kicad only {:.6}) | worst {:.3} µm | arcs {} to {:.3} µm",
            layer.function,
            layer.kept_objects,
            layer.reference_objects,
            layer.diff.area_a_mm2,
            layer.diff.area_b_mm2,
            layer.diff.difference_mm2,
            layer.diff.only_a_mm2,
            layer.diff.only_b_mm2,
            layer.worst_um,
            layer.arc_pieces,
            layer.arc_worst_um
        );
        ok &= layer.passes;
        for row in attribute(layer, judge, pads) {
            if row.kind == PieceKind::Other {
                let _ = writeln!(
                    out,
                    "      {:>5} pieces, widest {:.3} µm, {}: {}",
                    row.pieces,
                    row.widest_um,
                    if row.ours { "ours only" } else { "kicad only" },
                    row.class
                );
            }
            rows.push(serde_json::json!({
                "layer": layer.function,
                "kind": format!("{:?}", row.kind),
                "ours": row.ours,
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
    /// silent when `SPARKSMITH_KICAD_DESIGN` is not set (CI, other machines).
    #[test]
    #[ignore = "needs the private design and kicad-cli; set SPARKSMITH_KICAD_DESIGN"]
    fn the_private_design_matches_kicad() -> Result<()> {
        let Some(design) = std::env::var_os("SPARKSMITH_KICAD_DESIGN") else {
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
        let imported = import(text.as_bytes(), None, None)?;
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
