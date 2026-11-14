//! `sparksmith import-kicad` on an original synthetic board written here
//! (no KiCad fixture files): the outline imports as a board sketch with
//! its cutout, nets, tracks, vias and a zone come in, and the project is
//! saved with the report's detail beside it.
use serde_json::Value;
use std::process::Command;

type Fallible = Result<(), Box<dyn std::error::Error>>;

/// A 30 × 20 mm two-layer board with a rounded corner, a round cutout, two
/// resistors joined by a track, a via and a bottom track, and a ground
/// zone on both sides.
const BOARD: &str = r#"(kicad_pcb (version 20240108) (generator "sparksmith_test") (generator_version "8.0")
  (general (thickness 1.6))
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (34 "B.Paste" user) (35 "F.Paste" user)
    (38 "B.Mask" user) (39 "F.Mask" user) (44 "Edge.Cuts" user))
  (setup (pad_to_mask_clearance 0.05))
  (net 0 "") (net 1 "SIG") (net 2 "GND")
  (footprint "Synthetic:R" (layer "F.Cu") (at 105 60)
    (property "Reference" "R1" (at 0 -2 0) (layer "F.SilkS"))
    (pad "1" smd rect (at -1 0) (size 0.8 0.6) (layers "F.Cu" "F.Paste" "F.Mask") (net 2 "GND"))
    (pad "2" smd rect (at 1 0) (size 0.8 0.6) (layers "F.Cu" "F.Paste" "F.Mask") (net 1 "SIG")))
  (footprint "Synthetic:R" (layer "F.Cu") (at 125 60)
    (property "Reference" "R2" (at 0 -2 0) (layer "F.SilkS"))
    (pad "1" smd rect (at -1 0) (size 0.8 0.6) (layers "F.Cu" "F.Paste" "F.Mask") (net 1 "SIG"))
    (pad "2" smd rect (at 1 0) (size 0.8 0.6) (layers "F.Cu" "F.Paste" "F.Mask") (net 2 "GND")))
  (gr_line (start 102 70) (end 130 70) (stroke (width 0.1) (type default)) (layer "Edge.Cuts"))
  (gr_line (start 130 70) (end 130 50) (stroke (width 0.1) (type default)) (layer "Edge.Cuts"))
  (gr_line (start 130 50) (end 100 50) (stroke (width 0.1) (type default)) (layer "Edge.Cuts"))
  (gr_line (start 100 50) (end 100 68) (stroke (width 0.1) (type default)) (layer "Edge.Cuts"))
  (gr_arc (start 100 68) (mid 100.585786 69.414214) (end 102 70) (stroke (width 0.1) (type default)) (layer "Edge.Cuts"))
  (gr_circle (center 115 65) (end 116.5 65) (stroke (width 0.1) (type default)) (layer "Edge.Cuts"))
  (segment (start 106 60) (end 115 60) (width 0.25) (layer "F.Cu") (net 1))
  (via (at 115 60) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 1))
  (segment (start 115 60) (end 124 60) (width 0.25) (layer "B.Cu") (net 1))
  (zone (net 2) (net_name "GND") (layers "F&B.Cu") (hatch edge 0.5)
    (connect_pads yes (clearance 0.3)) (min_thickness 0.25)
    (fill yes (thermal_gap 0.5) (thermal_bridge_width 0.5))
    (polygon (pts (xy 101 51) (xy 129 51) (xy 129 55) (xy 101 55))))
)
"#;

fn count(rows: &Value, case: &str) -> u64 {
    rows.as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["case"] == case)
        .filter_map(|r| r["count"].as_u64())
        .sum()
}

#[test]
fn a_board_imports_with_its_outline_sketch_copper_and_zone() -> Fallible {
    let dir = tempfile::tempdir()?;
    let config = tempfile::tempdir()?;
    let board = dir.path().join("synthetic.kicad_pcb");
    std::fs::write(&board, BOARD)?;
    let target = dir.path().join("imported.sparksmith");
    let out = Command::new(env!("CARGO_BIN_EXE_sparksmith"))
        .env("SPARKSMITH_CONFIG_DIR", config.path())
        .env("SPARKSMITH_CACHE_DIR", config.path())
        // A folder with one board names it.
        .args(["import-kicad"])
        .arg(dir.path())
        .arg(&target)
        .output()?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reply: Value = serde_json::from_slice(&out.stdout)?;
    let rows = &reply["rows"];
    assert_eq!(count(rows, "outline_sketch"), 1, "{reply:#}");
    assert_eq!(reply["counts"]["cutouts"], 1);
    assert_eq!(reply["counts"]["nets"], 2);
    assert_eq!(reply["counts"]["tracks"], 2);
    assert_eq!(reply["counts"]["vias"], 1);
    assert_eq!(reply["counts"]["fills"], 2);
    assert_eq!(count(rows, "track_refused") + count(rows, "via_refused"), 0);
    assert!(target.is_file());
    // The detail stays beside the new file.
    assert!(dir.path().join("imported.kicad-import.json").is_file());
    Ok(())
}
