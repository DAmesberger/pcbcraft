//! The oracle: synthetic boards plotted by an installed `kicad-cli` and
//! compared with our import's own fabrication output, layer by layer, to
//! 1 µm. Opt-in (`SPARKSMITH_KICAD_ORACLE=1`): CI has no KiCad. Run with
//! `cargo test -p sparksmith-kicad --test oracle -- --ignored`.
mod support;

use sparksmith_core::fabrication::{self, FabricationOptions};
use sparksmith_fabcheck::reference;
use sparksmith_fabcheck::sets::{
    Arc, File, Judge, PadBox, PieceKind, Scope, attribute, compare_sets,
};
use std::path::PathBuf;

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

fn run_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/kicad-compare/oracle")
        .join(name)
}

#[test]
#[ignore = "needs kicad-cli; set SPARKSMITH_KICAD_ORACLE=1"]
fn synthetic_footprints_match_kicad_within_a_micron() -> Fallible<()> {
    if std::env::var_os("SPARKSMITH_KICAD_ORACLE").is_none() {
        return Ok(());
    }
    let board = support::showcase();
    let dir = run_dir("showcase");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let source = dir.join("synthetic.kicad_pcb");
    let text = board.text();
    std::fs::write(&source, &text)?;
    let theirs = reference::plot(&source, None, board.copper, &dir)?;
    let imported = support::import(text.as_bytes())?;
    let set = fabrication::generate(imported.engine.project(), &FabricationOptions::default())?;
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
    // KiCad approximates round custom-pad primitives within its default
    // maximum error of 5 µm: pieces on those arcs may be 6 µm wide.
    let judge = Judge {
        shift_nm: imported.plot_shift_nm(),
        // Both drill files are written to 1 µm.
        drill_resolution_nm: 1415,
        tolerance_nm: 1000,
        arc_tolerance_nm: 6000,
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
    let diff = compare_sets(&ours, &theirs, Scope::All, &judge)?;
    let pads: Vec<PadBox> = imported
        .pads
        .iter()
        .map(|p| PadBox {
            min: p.world.min,
            max: p.world.max,
            class: p.class.clone(),
        })
        .collect();
    let mut failures = Vec::new();
    for layer in &diff.layers {
        eprintln!(
            "{:<16} ours {:>9.4} mm², kicad {:>9.4} mm², diff {:.6} mm², worst {:>8.3} µm; {} arc pieces to {:.3} µm",
            layer.function,
            layer.diff.area_a_mm2,
            layer.diff.area_b_mm2,
            layer.diff.difference_mm2,
            layer.worst_um,
            layer.arc_pieces,
            layer.arc_worst_um
        );
        // Pieces off arcs by the class of their pad. One approximation
        // the import reports is allowed on paste: a chamfered round rect's
        // paste (KiCad rebuilds the shrunk pad from its ratios rather than
        // offsetting it).
        for row in attribute(layer, &judge, &pads) {
            eprintln!(
                "    {:?} {:>4} pieces, widest {:.3} µm at {:?}: {}",
                row.kind, row.pieces, row.widest_um, row.at, row.class
            );
            let allowed = layer.function.starts_with("Paste,") && row.class.contains("chamfer");
            let limit = match row.kind {
                PieceKind::Arc => 6.0,
                PieceKind::Other => 1.0,
            };
            if !allowed && row.widest_um > limit {
                failures.push(format!("{} {}", layer.function, row.class));
            }
        }
    }
    for drill in &diff.drills {
        eprintln!(
            "{:<16} ours {} kicad {} matched {} worst {:.3} µm (dia {:.3} µm)",
            drill.function,
            drill.diff.hits_a,
            drill.diff.hits_b,
            drill.diff.matched,
            drill.diff.worst_position_um,
            drill.diff.worst_diameter_um
        );
        for hit in drill
            .diff
            .unmatched_a
            .iter()
            .chain(&drill.diff.unmatched_b)
            .take(8)
        {
            eprintln!("    unmatched {hit:?}");
        }
        if !drill.diff.passes() {
            failures.push(drill.function.clone());
        }
    }
    eprintln!("{}", imported.report.summary());
    assert!(failures.is_empty(), "layers over 1 µm: {failures:?}");
    Ok(())
}
