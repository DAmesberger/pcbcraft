//! The ESP32-S31 dev board's USB-C connector J1 held at half the board's
//! width: widening the board moves J1, and the tracks on its pads come with
//! it (`layout::dimensions::carry`), so once the pours are refilled
//! nothing is left unconnected.
use sparksmith_automation::session::hosted;
use sparksmith_core::layout::dimensions::DimensionCommand;
use sparksmith_core::layout::fill::{FillCommand, RefillTarget};
use sparksmith_core::layout::outline::{BoardCommand, BoardEdit};
use sparksmith_core::layout::{self, CopperGraph};
use sparksmith_core::library::footprint_source::{
    DimKind, DimValue, Expr, FootprintParam, ParamName, ParamUnit, SketchRef,
};
use sparksmith_core::model::board::{BoardShape, ConstraintTarget};
use sparksmith_core::model::dimension::{BoardRef, FeatureRef, PartDimensionSpec, PartRelation};
use sparksmith_core::{Engine, Operation, Outcome, Project};
use std::path::Path;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn apply(engine: &mut Engine, op: Operation) -> sparksmith_core::Result<Outcome> {
    engine.apply_hosted(engine.revision(), op, hosted())
}

/// Board findings by code, and the airwires: what is left unconnected.
fn unconnected(project: &Project) -> (usize, usize) {
    let unrouted = layout::checks(project)
        .iter()
        .filter(|f| f.code == "board.unrouted")
        .count();
    (unrouted, CopperGraph::of(project).airwires().len())
}

/// Clearance findings. Printed, not asserted: J1's fan-out stretched
/// 5 mm along its own pad row is the board's to re-route.
fn errors(project: &Project) -> usize {
    layout::checks(project)
        .iter()
        .filter(|f| f.code == "board.clearance")
        .count()
}

fn set_width(engine: &mut Engine, mm: f64) -> sparksmith_core::Result<Outcome> {
    apply(
        engine,
        Operation::Board(BoardCommand::Edit {
            edits: vec![BoardEdit::SetParameter {
                param: FootprintParam {
                    name: ParamName("width".into()),
                    expr: Expr(format!("{mm} mm")),
                    unit: ParamUnit::Mm,
                    comment: String::new(),
                },
            }],
        }),
    )
}

#[test]
fn j1_held_at_half_the_width_keeps_its_tracks_when_the_board_widens() -> Fallible {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.sparksmith");
    let mut engine = Engine::from_file(sparksmith_io::read_project(&path)?)?;
    if matches!(engine.project().board_shape, BoardShape::Rectangle) {
        apply(&mut engine, Operation::Board(BoardCommand::StartSketch))?;
    }
    let j1 = engine
        .project()
        .instances
        .iter()
        .find(|i| i.designator == "J1")
        .map(|i| i.id)
        .ok_or("no J1")?;
    let BoardShape::Sketch(sketch) = &engine.project().board_shape else {
        return Err("not a sketch".into());
    };
    let left = sketch
        .edges
        .iter()
        .find(|e| e.name.0 == "left")
        .map(|e| e.curve.clone())
        .ok_or("no left edge")?;
    let width = engine.project().board_size[0];
    let x = engine
        .project()
        .placement_of(j1)
        .ok_or("J1 unplaced")?
        .position[0];
    let original = unconnected(engine.project());
    apply(
        &mut engine,
        Operation::Dimension(DimensionCommand::Add {
            dimension: PartDimensionSpec {
                kind: PartRelation::Dim(DimKind::Distance),
                refs: vec![
                    BoardRef::Part(FeatureRef::Origin {
                        target: ConstraintTarget::Placement(j1),
                    }),
                    BoardRef::Board(SketchRef::Curve(left)),
                ],
                value: Some(DimValue::Expr(Expr("width / 2".into()))),
                reference: false,
            },
        }),
    )?;
    let held = unconnected(engine.project());
    let held_errors = errors(engine.project());
    let held_x = engine.project().placement_of(j1).ok_or("J1")?.position[0];
    println!(
        "board {width} mm, J1 x {x} -> held at {held_x}; unconnected {original:?} -> {held:?}"
    );
    assert_eq!(held, original);
    let before = engine.to_file();
    set_width(&mut engine, width + 10.0)?;
    // The pours follow the outline and are stale until refilled.
    apply(
        &mut engine,
        Operation::Fill(FillCommand::Refill {
            target: RefillTarget::All,
        }),
    )?;
    let moved_x = engine.project().placement_of(j1).ok_or("J1")?.position[0];
    assert!(
        (moved_x - held_x - 5.0).abs() < 1e-6,
        "{held_x} -> {moved_x}"
    );
    let after = unconnected(engine.project());
    println!(
        "widened by 10 mm: J1 x {moved_x}; unconnected {after:?}; clearance findings {held_errors} -> {}",
        errors(engine.project())
    );
    assert_eq!(after, held);
    engine.undo()?;
    engine.undo()?;
    assert_eq!(engine.to_file(), before);
    Ok(())
}
