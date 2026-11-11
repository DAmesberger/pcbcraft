//! The ESP32 board's model after an edit holds what a fresh build of the
//! edited board holds: an update takes unchanged bodies' geometry from the
//! last model, and that geometry must not keep the last model's evaluated
//! bodies alive (it did through each body's lazy volume: 1,278 MB live
//! against 912 MB for a fresh build, after one width edit and its refill).
//! By hand, macOS only (live heap from `heap -s`):
//! `cargo test --profile ci -p pcbcraft-automation --test model_memory -- --ignored --nocapture`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pcbcraft_core::layout::outline::{BoardCommand, BoardEdit};
use pcbcraft_core::{Engine, Operation};
use pcbcraft_solidcraft::models::imported::SourceCache;
use pcbcraft_solidcraft::project;
use std::time::Instant;

/// This process's live heap in MB (`heap -s`), or `None` where `heap` is
/// not available.
fn live() -> Option<f64> {
    let pid = std::process::id().to_string();
    let out = std::process::Command::new("heap")
        .args(["-s", &pid])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("All zones: "))
        .find_map(|l| {
            let (_, rest) = l.split_once(" nodes (")?;
            rest.strip_suffix(" bytes)")?.parse::<f64>().ok()
        })
        .map(|bytes| bytes / 1048576.0)
}

fn path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft")
}

/// The board sketch's `name` parameter `by` mm more, as the board card sets it.
fn bump(engine: &mut Engine, name: &str, by: f64) {
    let pcbcraft_core::model::board::BoardShape::Sketch(sketch) = &engine.project().board_shape
    else {
        panic!("not a sketched board")
    };
    let mut param = sketch
        .parameters
        .iter()
        .find(|p| p.name.0 == name)
        .unwrap()
        .clone();
    let now: f64 = param.expr.0.trim_end_matches("mm").trim().parse().unwrap();
    param.expr = pcbcraft_core::library::footprint_source::Expr(format!("{} mm", now + by));
    engine
        .apply_hosted(
            engine.revision(),
            Operation::Board(BoardCommand::Edit {
                edits: vec![BoardEdit::SetParameter { param }],
            }),
            pcbcraft_automation::session::hosted(),
        )
        .unwrap();
}

#[test]
#[ignore = "memory; run by hand"]
fn esp32_an_updated_model_holds_what_a_fresh_build_holds() {
    let Some(base) = live() else {
        eprintln!("no `heap` here: nothing measured");
        return;
    };
    let mut engine = Engine::from_file(pcbcraft_io::read_project(&path()).unwrap()).unwrap();
    let sources = SourceCache::default();
    let mut cache = pcbcraft_core::layout::fill::FillCache::default();
    let t = Instant::now();
    let mut model = project::build_with(engine.project(), &sources, None).unwrap();
    let held = |base: f64| live().map_or(f64::NAN, |now| now - base);
    println!("build {:.1} s: +{:.0} MB", t.elapsed().as_secs_f64(), held(base));
    // The width edit as the app follows it: the model updated with the
    // fills' stale copper, the fills refilled, the model updated again.
    bump(&mut engine, "width", 1.0);
    let t = Instant::now();
    let (next, how) = project::update(model, engine.project(), &sources, None).unwrap();
    model = next;
    println!("width +1: update {:.1} s {how:?}: +{:.0} MB", t.elapsed().as_secs_f64(), held(base));
    let fresh = pcbcraft_core::layout::fill::compute_stale(engine.project(), &mut cache).unwrap();
    let revision = engine.revision();
    engine.store_fills(revision, fresh).unwrap();
    let t = Instant::now();
    let (next, how) = project::update(model, engine.project(), &sources, None).unwrap();
    model = next;
    let updated = held(base);
    println!("refilled: update {:.1} s {how:?}: +{updated:.0} MB", t.elapsed().as_secs_f64());
    // A fresh build of the same board, alone.
    drop(model);
    let base = live().unwrap_or(base);
    let t = Instant::now();
    let built = project::build_with(engine.project(), &sources, None).unwrap();
    let fresh = held(base);
    println!("fresh build {:.1} s: +{fresh:.0} MB", t.elapsed().as_secs_f64());
    drop(built);
    // Measured 1.40× before the fix, 0.99–1.07× after (allocator noise).
    assert!(
        updated <= fresh * 1.15,
        "the updated model holds {updated:.0} MB, a fresh build {fresh:.0} MB"
    );
}
