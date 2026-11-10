//! The ESP32-S31 dev board's 3D model follows J1 (the USB-C receptacle,
//! held by its shell's front face a part dimension from the bottom edge)
//! when that dimension changes, and when the board widens. Moved up, J1's
//! shell pad opening covers most of a small top pour island the stale
//! pour still has (pours are refilled after the edit); the island's mask
//! cap, the island less that opening, must still be a closed solid.
use pcbcraft_automation::session::hosted;
use pcbcraft_core::layout::dimensions::DimensionCommand;
use pcbcraft_core::layout::outline::{BoardCommand, BoardEdit};
use pcbcraft_core::library::footprint_source::{
    DimValue, Expr, FootprintParam, ParamName, ParamUnit,
};
use pcbcraft_core::model::dimension::{BoardRef, FeatureRef};
use pcbcraft_core::{Engine, Operation};
use pcbcraft_solidcraft::models::imported::SourceCache;
use pcbcraft_solidcraft::project;
use std::path::Path;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn the_model_follows_j1_moved_by_its_dimension_or_the_board_width() -> Fallible {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    let mut engine = Engine::from_file(pcbcraft_io::read_project(&path)?)?;
    let sources = SourceCache::default();
    let mut model = Some(project::build_with(engine.project(), &sources, None)?);
    let j1 = engine
        .project()
        .instance_by_designator("J1")
        .ok_or("J1")?
        .id;
    // The dimension from J1's shell face to the board edge.
    let held = engine
        .project()
        .part_dimensions
        .iter()
        .find(|d| {
            d.refs.iter().any(|r| {
                matches!(r, BoardRef::Part(FeatureRef::Face { placement, .. }) if *placement == j1)
            })
        })
        .cloned()
        .ok_or("no dimension on J1's face")?;
    let y = |engine: &Engine| engine.project().placement_of(j1).map(|p| p.position[1]);
    let at = y(&engine).ok_or("J1 unplaced")?;
    let mut edits: Vec<(String, Operation)> = [0.5, 1.0, 2.0]
        .into_iter()
        .map(|mm| {
            let mut dimension = held.clone();
            dimension.value = Some(DimValue::Fixed(mm));
            (
                format!("J1 {mm} mm from the edge"),
                Operation::Dimension(DimensionCommand::Edit { dimension }),
            )
        })
        .collect();
    edits.push((
        "the board 50 mm wide".into(),
        Operation::Board(BoardCommand::Edit {
            edits: vec![BoardEdit::SetParameter {
                param: FootprintParam {
                    name: ParamName("width".into()),
                    expr: Expr("50 mm".into()),
                    unit: ParamUnit::Mm,
                    comment: String::new(),
                },
            }],
        }),
    ));
    for (what, operation) in edits {
        let revision = engine.revision();
        engine.apply_hosted(revision, operation, hosted())?;
        let previous = model.take().ok_or("no model")?;
        let (next, how) = project::update(previous, engine.project(), &sources, None)
            .map_err(|e| format!("{what}: {e}"))?;
        println!("{what}: J1 y {at} -> {:?}, {how:?}", y(&engine));
        // The next edit starts from the example again; its update starts
        // from this model.
        engine.undo()?;
        model = Some(next);
    }
    Ok(())
}
