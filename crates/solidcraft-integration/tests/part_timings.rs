//! Lean parts (docs/part-templates.md §11): cold and cached cost of the
//! built-in ESP32-S31-WROOM-3 part file against the module builder it
//! replaces. Measured in a release build:
//!
//! `cargo test --release -p sparksmith-solidcraft --test part_timings -- --ignored --nocapture`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sparksmith_core::ids::PackageTerminalId;
use sparksmith_core::library::package_model::PartValue;
use sparksmith_core::library::{
    GeneratedMaterials, PackageDefinition, PackageForm, PackageModel, PackageTerminal,
};
use sparksmith_solidcraft::models::{self, part_file};
use sparksmith_solidcraft::native::Native;
use std::time::{Duration, Instant};

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn wroom3() -> PackageDefinition {
    // The fixture is the module datasheet's millimetres: the wire form.
    let form: PackageForm = serde_json::from_str::<
        sparksmith_core::library::package_form::PackageFormParams,
    >(include_str!("template_spike_wroom3.json"))
    .unwrap()
    .into();
    PackageDefinition {
        next_local: 100,
        terminals: (1..=99)
            .map(|i| PackageTerminal {
                id: PackageTerminalId(i),
                designation: i.to_string(),
            })
            .collect(),
        model: PackageModel::Generated {
            family_rev: 1,
            materials: GeneratedMaterials::default(),
        },
        form,
        ..PackageDefinition::default()
    }
}

#[test]
#[ignore = "timings"]
fn part_file_timings() {
    // The repository's shipped material library (the program loads it
    // through sparksmith-io at start-up).
    let materials =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/materials");
    let library = sparksmith_core::materials::MaterialLibrary::load(|name| {
        std::fs::read_to_string(materials.join(name))
            .map_err(|e| sparksmith_core::Error::Invalid(e.to_string()))
    })
    .map_err(|e| e.to_string());
    let standards = sparksmith_core::library::standards::Context::fixture().unwrap();
    let context = std::sync::Arc::new(
        sparksmith_core::library::standards::Context::new((**standards.standards()).clone())
            .with_materials(library),
    );
    let _context = context.enter();
    let bytes = &std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../library/parts/esp32-s31-wroom-3.solidcraft"
    ))
    .unwrap();
    for round in 1..=3 {
        // Today's path: the module builder.
        let t = Instant::now();
        let mut native = Native::new("generated").unwrap();
        models::build_definition(&mut native, "pkgT", &wroom3()).unwrap();
        let generated = t.elapsed();
        // A parameter set this process has not evaluated: cold.
        let mut values = part_file::PartValues::new();
        values.insert(
            "can_round".into(),
            PartValue::Nm(250_000 + i64::from(round) * 10_000),
        );
        let t = Instant::now();
        let part = part_file::evaluate(bytes, &values).unwrap();
        let cold = t.elapsed();
        assert!(!part.blocked(), "{:?}", part.findings);
        // The same set again: the cache.
        let t = Instant::now();
        let again = part_file::evaluate(bytes, &values).unwrap();
        let cached = t.elapsed();
        assert!(std::sync::Arc::ptr_eq(&part, &again));
        // Into a board design: one definition, one evaluation.
        let mut board = Native::new("board").unwrap();
        let t = Instant::now();
        part_file::insert(&mut board, &part.stem(), &part, &Default::default()).unwrap();
        let insert = t.elapsed();
        assert!(board.evaluation_error().is_none());
        eprintln!(
            "round {round}: module builder {:.1} ms | part file cold (read + apply values + evaluate + inspect) {:.1} ms \
             (Solidcraft evaluation {:.1} ms) | cached {:.3} ms | insert into a board {:.1} ms | {} bodies",
            ms(generated),
            ms(cold),
            part.evaluate_ms,
            ms(cached),
            ms(insert),
            part.bodies.len()
        );
    }
    let (hits, evaluations) = part_file::cache_counts();
    eprintln!("cache: {hits} hits, {evaluations} evaluations");
}
