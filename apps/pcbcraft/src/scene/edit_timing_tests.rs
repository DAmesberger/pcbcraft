//! The ESP32 board's J1 dimension edit (0 → 1 mm) as the app runs it in
//! the PCB 3D view: the engine applies the edit, the fill worker refills
//! the fills it made stale and stores their copper (no undo step), and the
//! model worker makes each new board from the last one. Measured from the
//! edit to the frame that shows its model (`cargo test --profile ci -p
//! pcbcraft --bin pcbcraft edit_timing -- --ignored --nocapture`, with
//! `PCBCRAFT_TIMINGS=1` for the worker's own lines).
use super::*;
use crate::fill_worker::FillWorker;
use pcbcraft_core::Operation;
use pcbcraft_core::layout::dimensions::DimensionCommand;
use pcbcraft_core::library::footprint_source::DimValue;
use pcbcraft_core::model::dimension::{BoardRef, FeatureRef};
use std::time::Instant;

type Fallible<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub(super) fn esp32() -> Fallible<Engine> {
    let file = pcbcraft_io::read_project(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft"
    )))?;
    let mut engine = Engine::default();
    engine.replace_document(file)?;
    Ok(engine)
}

/// J1's dimension to the bottom edge, at `mm`.
fn j1_at(engine: &Engine, mm: f64) -> Fallible<Operation> {
    let j1 = engine
        .project()
        .instance_by_designator("J1")
        .ok_or("J1")?
        .id;
    let mut dimension = engine
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
    dimension.value = Some(DimValue::Fixed(mm));
    Ok(Operation::Dimension(DimensionCommand::Edit { dimension }))
}

/// One app frame for the 3D view: store finished fill copper, ask for the
/// current board's model, take finished models. Returns whether the
/// current board is shown.
pub(super) fn frame(
    scene: &mut SceneController,
    fills: &mut FillWorker,
    engine: &mut Engine,
) -> Fallible<bool> {
    fills.poll(engine)?;
    let key = ModelKey::Project(Arc::clone(engine.project_arc()));
    scene.request(key.clone(), None, || Ok(None));
    scene.receive();
    if let Status::Failed(error) = scene.status() {
        return Err(error.clone().into());
    }
    Ok(scene.display(&key) == Display::Current)
}

/// Run frames until the current board is shown and the fills are settled
/// (nothing stored for a while): each model shown since `since`, as the
/// seconds it came at, its request, and whether it was the current board.
pub(super) fn settle(
    scene: &mut SceneController,
    fills: &mut FillWorker,
    engine: &mut Engine,
    since: Instant,
) -> Fallible<Vec<(f64, u64, bool)>> {
    let mut shown = Vec::new();
    let mut last_shown = scene.shown_ticket;
    let mut quiet = 0;
    for _ in 0..60_000 {
        let current = frame(scene, fills, engine)?;
        if scene.shown_ticket != last_shown {
            last_shown = scene.shown_ticket;
            shown.push((since.elapsed().as_secs_f64(), last_shown, current));
        }
        quiet = if current { quiet + 1 } else { 0 };
        // A second of frames with the current board shown and no refill.
        if quiet > 200 {
            return Ok(shown);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Err("the board never settled".into())
}

#[test]
#[ignore = "timing; run by hand"]
fn esp32_j1_dimension_edit_in_the_app() -> Fallible {
    let mut engine = esp32()?;
    let mut scene = SceneController::new(
        egui::Context::default(),
        crate::viewport::Viewport::detached(),
    );
    let mut fills = FillWorker::start(egui::Context::default());
    let opened = Instant::now();
    let shown = settle(&mut scene, &mut fills, &mut engine, opened)?;
    println!("opened: shown at {shown:?} (s, request, current)");
    let copper = |engine: &Engine| -> Vec<String> {
        engine
            .project()
            .fills
            .iter()
            .map(|f| serde_json::to_string(&f.computed).unwrap_or_default())
            .collect()
    };
    let fills_before = copper(&engine);
    let edit = j1_at(&engine, 1.0)?;
    let started = Instant::now();
    // As the app applies it.
    engine.apply_hosted(
        engine.revision(),
        edit,
        pcbcraft_core::Evaluators {
            footprint: Some(&pcbcraft_solidcraft::footprint::SketchEvaluator),
            board: crate::pcb_editor::board_shape::host_evaluator(),
        },
    )?;
    let shown = settle(&mut scene, &mut fills, &mut engine, started)?;
    let refilled = copper(&engine)
        .iter()
        .zip(&fills_before)
        .filter(|(after, before)| after != before)
        .count();
    println!("J1 0 -> 1 mm: shown at {shown:?} (s, request, current); {refilled} fills refilled");
    let regions = |engine: &Engine| -> Vec<usize> {
        engine
            .project()
            .fills
            .iter()
            .map(|f| f.computed.as_ref().map_or(0, |c| c.regions.len()))
            .collect()
    };
    println!("regions per fill after: {:?}", regions(&engine));
    engine.undo()?;
    println!("regions per fill before: {:?}", regions(&engine));
    Ok(())
}
