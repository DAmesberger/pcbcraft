//! The ESP32 board's memory and rebuild tail over board-parameter edits,
//! as the app runs them with the PCB 3D view open: the engine applies the
//! edit, the fill worker refills the fills it made stale, and the model
//! worker makes each new board from the last one. By hand (`cargo test
//! --profile ci -p sparksmith --bin sparksmith-desktop memory_over -- --ignored
//! --nocapture`), macOS only: the process's resident size from `ps` and
//! its physical footprint and live heap from `heap`.
use super::edit_timing_tests::{esp32, frame, settle};
use super::*;
use crate::fill_worker::FillWorker;
use sparksmith_core::Operation;
use sparksmith_core::layout::outline::{BoardCommand, BoardEdit};
use sparksmith_core::library::footprint_source::Expr;
use std::time::Instant;

type Fallible<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// This process's resident size (`ps`) and live heap (`heap`), as one
/// line.
pub(super) fn memory() -> String {
    let pid = std::process::id().to_string();
    let rss = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map_or("?".to_owned(), |kb| format!("{:.0} MB", kb / 1024.0));
    let heap = std::process::Command::new("heap")
        .args(["-s", &pid])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    // "All zones: 6084313 nodes (1857709328 bytes)": the live heap.
    let live = heap
        .lines()
        .filter_map(|l| l.trim().strip_prefix("All zones: "))
        .find_map(|l| {
            let (nodes, rest) = l.split_once(" nodes (")?;
            let bytes: f64 = rest.strip_suffix(" bytes)")?.parse().ok()?;
            Some(format!(
                "live heap {:.0} MB in {nodes} blocks",
                bytes / 1048576.0
            ))
        })
        .unwrap_or_default();
    format!("rss {rss}; {live}")
}

/// The board sketch's parameter `name` set to its value plus `by` mm, as
/// the board card's field sets it.
pub(super) fn bump(engine: &Engine, name: &str, by: f64) -> Fallible<Operation> {
    let sparksmith_core::model::board::BoardShape::Sketch(sketch) = &engine.project().board_shape
    else {
        return Err("not a sketched board".into());
    };
    let mut param = sketch
        .parameters
        .iter()
        .find(|p| p.name.0 == name)
        .cloned()
        .ok_or("no such parameter")?;
    let now: f64 = param
        .expr
        .0
        .trim_end_matches("mm")
        .trim()
        .parse()
        .map_err(|_| format!("{name} is not a plain length: {}", param.expr.0))?;
    param.expr = Expr(format!("{} mm", now + by));
    Ok(Operation::Board(BoardCommand::Edit {
        edits: vec![BoardEdit::SetParameter { param }],
    }))
}

pub(super) fn apply(engine: &mut Engine, operation: Operation) -> Fallible {
    engine.apply_hosted(
        engine.revision(),
        operation,
        sparksmith_core::Evaluators {
            footprint: Some(&sparksmith_solidcraft::footprint::SketchEvaluator),
            board: crate::pcb_editor::board_shape::host_evaluator(),
        },
    )?;
    Ok(())
}

/// The edits of round 7's endurance run: width and height up and down.
const EDITS: [(&str, f64); 12] = [
    ("width", 1.0),
    ("height", 1.0),
    ("width", 1.0),
    ("height", -1.0),
    ("width", -1.0),
    ("height", 1.0),
    ("width", -1.0),
    ("height", -1.0),
    ("width", 1.0),
    ("height", 1.0),
    ("width", -1.0),
    ("height", -1.0),
];

#[test]
#[ignore = "memory; run by hand"]
fn esp32_memory_over_board_edits() -> Fallible {
    let mut engine = esp32()?;
    println!("file read: {}", memory());
    let mut scene = SceneController::new(
        egui::Context::default(),
        crate::viewport::Viewport::detached(),
    );
    let mut fills = FillWorker::start(egui::Context::default());
    settle(&mut scene, &mut fills, &mut engine, Instant::now())?;
    println!("opened in 3D: {}", memory());
    let edits: usize = std::env::var("SPARKSMITH_MEMORY_EDITS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(EDITS.len());
    for (k, (name, by)) in EDITS.iter().take(edits).enumerate() {
        let edit = bump(&engine, name, *by)?;
        apply(&mut engine, edit)?;
        let started = Instant::now();
        settle(&mut scene, &mut fills, &mut engine, started)?;
        println!(
            "edit {} ({name} {by:+}) settled in {:.1} s: {}",
            k + 1,
            started.elapsed().as_secs_f64(),
            memory()
        );
    }
    for k in 0..4 {
        engine.undo()?;
        let started = Instant::now();
        settle(&mut scene, &mut fills, &mut engine, started)?;
        println!(
            "undo {} settled in {:.1} s: {}",
            k + 1,
            started.elapsed().as_secs_f64(),
            memory()
        );
    }
    drop(scene);
    drop(fills);
    std::thread::sleep(Duration::from_secs(2));
    println!("workers dropped: {}", memory());
    drop(engine);
    println!("engine dropped: {}", memory());
    Ok(())
}

/// Twenty edits 1.5 s apart, frames running as the app runs them, then
/// how long the workers stay busy after the last one.
#[test]
#[ignore = "timing; run by hand"]
fn esp32_edit_batch_tail() -> Fallible {
    let mut engine = esp32()?;
    let mut scene = SceneController::new(
        egui::Context::default(),
        crate::viewport::Viewport::detached(),
    );
    let mut fills = FillWorker::start(egui::Context::default());
    settle(&mut scene, &mut fills, &mut engine, Instant::now())?;
    let received = scene.received;
    let batch = Instant::now();
    for k in 0..20 {
        let (name, by) = EDITS[k % EDITS.len()];
        let edit = bump(&engine, name, by)?;
        apply(&mut engine, edit)?;
        let next = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < next {
            frame(&mut scene, &mut fills, &mut engine)?;
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let last = Instant::now();
    settle(&mut scene, &mut fills, &mut engine, last)?;
    // `settle` waits a second of quiet frames after the board is current.
    let tail = last.elapsed().as_secs_f64() - 1.0;
    println!(
        "20 edits in {:.1} s; settled {tail:.1} s after the last; {} board models made",
        last.duration_since(batch).as_secs_f64(),
        scene.received - received
    );
    Ok(())
}
