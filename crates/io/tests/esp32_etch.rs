//! Etch hazards on the shipped ESP32-S31 dev board. Its rules leave both
//! checks unset (no citable default exists), so its board and production
//! checks have no etch finding. Set for a report, the example's findings
//! are printed, not asserted: they are the board's, to fix or accept.
use sparksmith_core::layout;
use sparksmith_core::rules::{RuleKey, RuleValue, RulesCommand};
use sparksmith_core::{Engine, Operation, Query, QueryResult};
use std::path::Path;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const CODES: [&str; 2] = ["board.acute_copper", "board.copper_sliver"];

fn engine() -> Fallible<Engine> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.sparksmith");
    Ok(Engine::from_file(sparksmith_io::read_project(&path)?)?)
}

fn preflight_codes(engine: &Engine) -> Fallible<Vec<String>> {
    match engine.query(Query::Preflight { profile: None })? {
        QueryResult::Preflight(report) => Ok(report.items.into_iter().map(|i| i.code).collect()),
        _ => Err("preflight".into()),
    }
}

#[test]
fn the_example_has_no_etch_findings_under_its_own_rules() -> Fallible {
    let engine = engine()?;
    let etch = engine.project().rules.effective().etch;
    assert_eq!((etch.copper_angle_min, etch.sliver_width_min), (0.0, 0.0));
    let checks = layout::checks(engine.project(), engine.check_settings());
    assert!(!checks.iter().any(|f| CODES.contains(&f.code.as_str())));
    let codes = preflight_codes(&engine)?;
    assert!(
        !codes.iter().any(|c| CODES.contains(&c.as_str())),
        "{codes:?}"
    );
    Ok(())
}

/// With a 90° minimum angle and the board's own flow limit as the sliver
/// width (2 × fill clearance + min fill width: the narrowest gap its pours
/// can enter), list what the checks find. The fills stay fresh.
#[test]
fn the_example_under_set_etch_rules() -> Fallible {
    let mut engine = engine()?;
    let rules = engine.project().rules.effective();
    let flow = 2.0 * rules.fill.clearance + rules.fill.width_min;
    for (key, value) in [
        (RuleKey::CopperAngleMin, RuleValue::Angle(90.0)),
        (RuleKey::SliverWidthMin, RuleValue::Length(flow)),
    ] {
        engine.apply(
            engine.revision(),
            Operation::Rules(RulesCommand::SetOverride { key, value }),
        )?;
    }
    let start = std::time::Instant::now();
    let checks = layout::checks(engine.project(), engine.check_settings());
    let elapsed = start.elapsed();
    let found: Vec<_> = checks
        .iter()
        .filter(|f| CODES.contains(&f.code.as_str()))
        .collect();
    println!(
        "ESP32 dev board, min angle 90°, min sliver {flow} mm: {} acute, {} slivers (board checks {elapsed:?})",
        found
            .iter()
            .filter(|f| f.code == "board.acute_copper")
            .count(),
        found
            .iter()
            .filter(|f| f.code == "board.copper_sliver")
            .count()
    );
    for f in &found {
        println!("  {}: {}", f.code, f.message);
    }
    let codes = preflight_codes(&engine)?;
    assert!(!codes.iter().any(|c| c == "fill.stale"), "{codes:?}");
    println!(
        "production check: {} items, {} etch: {codes:?}",
        codes.len(),
        codes.iter().filter(|c| CODES.contains(&c.as_str())).count()
    );
    Ok(())
}
