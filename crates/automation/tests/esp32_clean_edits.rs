//! The ESP32-S31 dev board stays clean through the parametric edits that
//! move J1, the USB-C receptacle: its 3D dimension from the bottom edge
//! 0 → 1 mm and back, the board's width 40 → 50 mm and back, and its height
//! 64 → 70 mm. J1's ESD array and CC pull-downs are held at fixed offsets
//! from it, so the cluster moves as one, and the routes leaving it keep
//! their directions (`layout::dimensions::carry`): the USB pair stays
//! coupled and its skew stays within its class limit.
//!
//! After each edit the stale fills are refilled as the desktop's fill
//! worker does (`compute_stale`, stored without an undo step); then the
//! production check (`fabrication.preflight`) has no unwaived error, no
//! warning and only the board's two waivers (the J1 locating-peg holes by
//! the GCT USB4105 recommended layout), and one undo restores the board
//! exactly.
use pcbcraft_automation::session::hosted;
use pcbcraft_core::format::EditorFile;
use pcbcraft_core::layout::dimensions::DimensionCommand;
use pcbcraft_core::layout::fill::{FillCache, compute_stale};
use pcbcraft_core::layout::length;
use pcbcraft_core::layout::outline::{BoardCommand, BoardEdit};
use pcbcraft_core::library::footprint_source::{
    DimValue, Expr, FootprintParam, ParamName, ParamUnit,
};
use pcbcraft_core::model::dimension::{BoardRef, FeatureRef, PartDimension};
use pcbcraft_core::{Engine, Operation, Project, Query, QueryResult};
use std::path::Path;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn example() -> Fallible<Engine> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    Ok(Engine::from_file(pcbcraft_io::read_project(&path)?)?)
}

/// No unwaived error, no warning, and the two J1 locating-peg waivers.
const CLEAN: (usize, usize, usize) = (0, 0, 2);

/// The production check's unwaived errors, warnings and waived findings,
/// listed.
fn issues(engine: &Engine) -> Fallible<((usize, usize, usize), Vec<String>)> {
    let QueryResult::Preflight(report) = engine.query(Query::Preflight { profile: None })? else {
        return Err("not a preflight report".into());
    };
    let items = report
        .items
        .iter()
        .map(|i| {
            let waived = if i.waiver.is_some() { " (waived)" } else { "" };
            format!("{:?} {}: {}{waived}", i.severity, i.code, i.title)
        })
        .collect();
    Ok(((report.blocking, report.warnings, report.waived), items))
}

/// The USB pair's skew and its class limit.
fn skew(project: &Project) -> Fallible<(f64, f64)> {
    let report = length::report(project);
    let usb = report.pairs.first().ok_or("no pair row")?;
    Ok((usb.skew, usb.max_skew.ok_or("no skew limit")?))
}

/// Apply `operation`, refill the stale fills without an undo step, and
/// require a clean board. Returns the file before the edit.
fn clean_edit(engine: &mut Engine, what: &str, operation: Operation) -> Fallible<EditorFile> {
    let before = engine.to_file();
    engine.apply_hosted(engine.revision(), operation, hosted())?;
    let fresh = compute_stale(engine.project(), &mut FillCache::default())?;
    engine.store_fills(engine.revision(), fresh)?;
    let (counts, items) = issues(engine)?;
    let (skew, limit) = skew(engine.project())?;
    println!("{what}: {counts:?} (blocking, warnings, waived), skew {skew:.4} mm");
    assert_eq!(counts, CLEAN, "{what}: {items:#?}");
    assert!(skew <= limit, "{what}: skew {skew} > {limit}");
    Ok(before)
}

/// Undo one step: the board is exactly as it was.
fn undo_to(engine: &mut Engine, what: &str, before: &EditorFile) -> Fallible {
    engine.undo()?;
    assert!(&engine.to_file() == before, "{what}: one undo restores it");
    Ok(())
}

fn set_board(name: &str, mm: f64) -> Operation {
    Operation::Board(BoardCommand::Edit {
        edits: vec![BoardEdit::SetParameter {
            param: FootprintParam {
                name: ParamName(name.into()),
                expr: Expr(format!("{mm} mm")),
                unit: ParamUnit::Mm,
                comment: String::new(),
            },
        }],
    })
}

/// The dimension holding J1's shell face to the bottom edge.
fn j1_face(engine: &Engine) -> Fallible<PartDimension> {
    let j1 = engine
        .project()
        .instance_by_designator("J1")
        .ok_or("J1")?
        .id;
    Ok(engine
        .project()
        .part_dimensions
        .iter()
        .find(|d| {
            d.refs.iter().any(|r| {
                matches!(r, BoardRef::Part(FeatureRef::Face { placement, .. }) if *placement == j1)
            })
        })
        .cloned()
        .ok_or("no dimension on J1's face")?)
}

fn set_j1(held: &PartDimension, mm: f64) -> Operation {
    let mut dimension = held.clone();
    dimension.value = Some(DimValue::Fixed(mm));
    Operation::Dimension(DimensionCommand::Edit { dimension })
}

#[test]
fn the_board_starts_clean() -> Fallible {
    let engine = example()?;
    let (counts, items) = issues(&engine)?;
    assert_eq!(counts, CLEAN, "{items:#?}");
    Ok(())
}

#[test]
fn j1_moved_up_by_its_dimension_and_back_stays_clean() -> Fallible {
    let mut engine = example()?;
    let held = j1_face(&engine)?;
    assert_eq!(held.value, Some(DimValue::Fixed(0.0)));
    let start = clean_edit(&mut engine, "J1 1 mm from the edge", set_j1(&held, 1.0))?;
    let moved = clean_edit(&mut engine, "J1 back on the edge", set_j1(&held, 0.0))?;
    undo_to(&mut engine, "J1 back on the edge", &moved)?;
    undo_to(&mut engine, "J1 1 mm from the edge", &start)?;
    Ok(())
}

#[test]
fn the_board_widened_and_back_stays_clean() -> Fallible {
    let mut engine = example()?;
    assert_eq!(engine.project().board_size, [40.0, 64.0]);
    let start = clean_edit(
        &mut engine,
        "the board 50 mm wide",
        set_board("width", 50.0),
    )?;
    let wide = clean_edit(
        &mut engine,
        "the board 40 mm wide again",
        set_board("width", 40.0),
    )?;
    undo_to(&mut engine, "the board 40 mm wide again", &wide)?;
    undo_to(&mut engine, "the board 50 mm wide", &start)?;
    Ok(())
}

#[test]
fn the_board_made_taller_stays_clean() -> Fallible {
    let mut engine = example()?;
    let start = clean_edit(
        &mut engine,
        "the board 70 mm tall",
        set_board("height", 70.0),
    )?;
    undo_to(&mut engine, "the board 70 mm tall", &start)?;
    Ok(())
}
