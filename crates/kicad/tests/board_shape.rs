//! The board outline and stackup of Rust-built synthetic boards, without a
//! sketch engine (the CLI's tests import the outline as a sketch).
mod support;

use sparksmith_core::DielectricKind;
use sparksmith_kicad::board;
use sparksmith_kicad::items::{EdgeItem, edge_bounds};
use sparksmith_core::library::footprint_source::SketchEdit;
use sparksmith_kicad::report::{Case, Report};
use support::{Board, Footprint, Pad, import};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// A 30 × 20 mm board at (100, 50)–(130, 70) in KiCad's frame, its
/// lower-left corner rounded by a 2 mm arc, with a 3 mm round cutout.
const OUTLINE: &str = "
  (gr_line (start 102 70) (end 130 70) (stroke (width 0.1) (type default)) (layer \"Edge.Cuts\"))
  (gr_line (start 130 70) (end 130 50) (stroke (width 0.1) (type default)) (layer \"Edge.Cuts\"))
  (gr_line (start 130 50) (end 100 50) (stroke (width 0.1) (type default)) (layer \"Edge.Cuts\"))
  (gr_line (start 100 50) (end 100 68) (stroke (width 0.1) (type default)) (layer \"Edge.Cuts\"))
  (gr_arc (start 100 68) (mid 100.585786 69.414214) (end 102 70) (stroke (width 0.1) (type default)) (layer \"Edge.Cuts\"))
  (gr_circle (center 120 60) (end 121.5 60) (stroke (width 0.1) (type default)) (layer \"Edge.Cuts\"))
";

const STACKUP: &str = "(stackup
      (layer \"F.SilkS\" (type \"Top Silk Screen\"))
      (layer \"F.Mask\" (type \"Top Solder Mask\") (thickness 0.02))
      (layer \"F.Cu\" (type \"copper\") (thickness 0.035))
      (layer \"dielectric 1\" (type \"prepreg\") (thickness 0.2) (material \"FR4\") (epsilon_r 4.1) (loss_tangent 0.02))
      (layer \"In1.Cu\" (type \"copper\") (thickness 0.0175))
      (layer \"dielectric 2\" (type \"core\") (thickness 0.6) (material \"FR4\") (epsilon_r 4.5) (loss_tangent 0.02)
        addsublayer (thickness 0.4) (material \"FR4\") (epsilon_r 4.5) (loss_tangent 0.02))
      (layer \"In2.Cu\" (type \"copper\") (thickness 0.0175))
      (layer \"dielectric 3\" (type \"prepreg\") (thickness 0.2) (material \"FR4\") (epsilon_r 4.1) (loss_tangent 0.02))
      (layer \"B.Cu\" (type \"copper\") (thickness 0.035))
      (layer \"B.Mask\" (type \"Bottom Solder Mask\") (thickness 0.02))
      (copper_finish \"None\") (dielectric_constraints no))";

fn outlined() -> Board {
    Board {
        copper: 4,
        origin: Some([95.0, 75.0]),
        setup: format!("(pad_to_mask_clearance 0.05) {STACKUP}"),
        footprints: vec![Footprint::new(
            "R1",
            [110.0, 60.0, 0.0],
            false,
            vec![
                Pad::smd("1", "rect", [-1.0, 0.0, 0.0], [0.8, 0.6]),
                Pad::smd("2", "rect", [1.0, 0.0, 0.0], [0.8, 0.6]),
            ],
        )],
        items: OUTLINE.into(),
    }
}

/// Equal to 10 nm (an arc's centre is fitted through points written to
/// the nanometre).
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn the_outline_box_sets_the_origin_and_arcs_bound_by_their_sweep() -> Fallible<()> {
    let text = outlined().text();
    let board = board::read(text.as_bytes(), &mut Report::default())?;
    assert_eq!(board.items.edges.len(), 6);
    assert!(matches!(board.items.edges[4], EdgeItem::Arc { .. }));
    // The quarter arc stays inside the corner it rounds: the box is the
    // board's, not the arc's full circle.
    let [min, max] = edge_bounds(&board.items.edges).ok_or("no box")?;
    assert_eq!(min, [100_000_000, 50_000_000]);
    assert_eq!(max, [130_000_000, 70_000_000]);
    // Our origin is the lower-left corner: KiCad's (100, 70).
    assert_eq!(board.origin, [100_000_000, 70_000_000]);
    Ok(())
}

#[test]
fn outline_drawings_become_counter_clockwise_sketch_curves() -> Fallible<()> {
    let text = outlined().text();
    let board = board::read(text.as_bytes(), &mut Report::default())?;
    let edits = sparksmith_kicad::shape::outline_edits(&board, &mut Report::default());
    assert_eq!(edits.len(), 6);
    // In our frame the arc runs from (0, 2) through (0.59, 0.59) to (2, 0)
    // about (2, 2): from 180° through 225° to 270°, counter-clockwise.
    let SketchEdit::AddArc {
        center, start, end, ..
    } = &edits[4]
    else {
        return Err("no arc".into());
    };
    assert!(close(center[0], 2.0) && close(center[1], 2.0), "{center:?}");
    assert!(close(start[0], 0.0) && close(start[1], 2.0), "{start:?}");
    assert!(close(end[0], 2.0) && close(end[1], 0.0), "{end:?}");
    let SketchEdit::AddCircle { center, radius, .. } = &edits[5] else {
        return Err("no circle".into());
    };
    assert!(close(center[0], 20.0) && close(center[1], 10.0) && close(*radius, 1.5));
    Ok(())
}

#[test]
fn without_a_sketch_engine_the_outline_is_its_box_and_parts_follow_the_origin() -> Fallible<()>
{
    let imported = import(outlined().text().as_bytes())?;
    let project = imported.engine.project();
    assert_eq!(imported.report.count(Case::OutlineRectangle), 1);
    assert_eq!(project.board_size, [30.0, 20.0]);
    // R1 at KiCad (110, 60): 10 right of and 10 above the corner (100, 70).
    let placement = project.placements.first().ok_or("R1")?;
    assert!(close(placement.position[0], 10.0) && close(placement.position[1], 10.0));
    // KiCad's plot about the auxiliary origin (95, 75) moves onto ours by
    // (95 − 100, 70 − 75) mm.
    assert_eq!(imported.plot_shift_nm(), [-5_000_000, -5_000_000]);
    Ok(())
}

#[test]
fn the_stackup_takes_the_files_thicknesses_kinds_and_dk() -> Fallible<()> {
    let imported = import(outlined().text().as_bytes())?;
    let stackup = &imported.engine.project().stackup;
    assert_eq!(imported.report.count(Case::StackupImported), 1);
    let copper: Vec<f64> = stackup.copper.iter().map(|c| c.thickness_um).collect();
    assert!(
        copper
            .iter()
            .zip([35.0, 17.5, 17.5, 35.0])
            .all(|(a, b)| close(*a, b)),
        "{copper:?}"
    );
    let kinds: Vec<DielectricKind> = stackup.dielectrics.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        [
            DielectricKind::Prepreg,
            DielectricKind::Core,
            DielectricKind::Prepreg
        ]
    );
    // The core's two sublayers add up; the first Dk stands.
    assert!(close(stackup.dielectrics[1].thickness_mm, 1.0));
    assert_eq!(imported.report.count(Case::StackupSublayers), 1);
    assert_eq!(stackup.dielectrics[0].dk, Some(4.1));
    assert_eq!(stackup.dielectrics[1].dk, Some(4.5));
    assert!(close(stackup.mask_um, 20.0));
    Ok(())
}
