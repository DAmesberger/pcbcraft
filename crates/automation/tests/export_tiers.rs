//! The ESP32-S31 example exported at the mechanical and full tiers
//! (`sparksmith_solidcraft::export::board`, slice A5): every board body
//! checked against the plan, and the time each stage took. A measurement,
//! not a gate: run with `cargo test -p sparksmith-automation --profile ci
//! --test export_tiers -- --ignored --nocapture`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sparksmith_solidcraft::export::{self, Stage, Tier, Watch};
use std::path::Path;
use std::time::Instant;

const EXAMPLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/esp32-s31-devboard/esp32-s31-devboard.sparksmith"
);

#[test]
#[ignore = "measurement; run with --ignored --nocapture"]
fn esp32_export_tiers_are_checked_against_the_plan() {
    // The 3D build reads the shipped material library.
    sparksmith_automation::load_materials();
    let project = sparksmith_io::read_project(Path::new(EXAMPLE))
        .unwrap()
        .into_document()
        .project
        .as_ref()
        .clone();
    let dir = tempfile::tempdir().unwrap();
    for tier in [Tier::Mechanical, Tier::Full] {
        let path = dir.path().join(format!("{tier:?}.solidcraft"));
        let watch = Watch::new();
        let start = Instant::now();
        let report = export::board(
            &project,
            tier,
            path.to_str().unwrap(),
            &watch,
            export::Unfit::Refuse,
        )
        .unwrap();
        let wall = start.elapsed().as_secs_f64();
        let check = report.check.as_ref().unwrap();
        let progress = watch.progress();
        eprintln!(
            "ESP32 {tier:?}: {wall:.1} s wall; build {:.0} ms, check {:.0} ms, save {:.0} ms; \
             {} commands; {} bodies written; {} planned flat bodies, {} built; {} differences; \
             outlines within {:.6} mm, {:.3e} mm²; {} KB",
            report.ms.build.unwrap_or(f64::NAN),
            report.ms.check.unwrap_or(f64::NAN),
            report.ms.save.unwrap_or(f64::NAN),
            progress.commands,
            report.bodies,
            check.planned,
            check.built,
            report.difference_count(),
            check.max_deviation_mm,
            check.max_area_mm2,
            std::fs::metadata(&path).unwrap().len() / 1024
        );
        assert_eq!(progress.stage, Stage::Done);
        assert!(!report.differs(), "{:#?}", report.lines(&project));
    }
}
