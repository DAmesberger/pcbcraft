//! Lean parts over the ESP32-S31-WROOM-3 example: check, use, set values,
//! refusals that leave the document unchanged, and a board using the part
//! file built, saved, reopened and built again.
use super::super::Session;
use serde_json::{Value, json};
use sparksmith_core::Engine;
use sparksmith_core::library::{Definition, PackageModel};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const EXAMPLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/esp32-s31-wroom-3/esp32-s31-wroom-3.sparksmith"
);

const BUILTIN: &str = "esp32-s31-wroom-3";

/// The values the reference replay gives (`wroom3.replay.jsonl`, from the
/// module drawing).
fn wroom3_values() -> Value {
    json!({"width": {"mm": 22.0}, "length": {"mm": 30.0}, "board_thickness": {"mm": 1.0},
        "can_width": {"mm": 21.0}, "can_length": {"mm": 23.0}, "can_height": {"mm": 2.465}})
}

fn open() -> TestResult<(Session, u64)> {
    let mut session = Session::new(Engine::default());
    let opened = session.dispatch(json!({"command": "project.open", "params": {"expected_revision": session.engine().revision(), "path": EXAMPLE}}));
    assert_eq!(opened["ok"], true, "{opened}");
    let package = session
        .engine()
        .drafts()
        .items
        .iter()
        .find(|d| matches!(d.definition, Definition::Package(_)))
        .map(|d| d.id.0)
        .ok_or("no package draft")?;
    Ok((session, package))
}

fn model(session: &Session, package: u64) -> Option<PackageModel> {
    session
        .engine()
        .drafts()
        .items
        .iter()
        .find(|d| d.id.0 == package)
        .and_then(|d| match &d.definition {
            Definition::Package(p) => Some(p.model.clone()),
            _ => None,
        })
}

fn call(session: &mut Session, command: &str, params: Value) -> Value {
    session.dispatch(json!({"command": command, "params": params}))
}

#[test]
fn the_builtin_wroom3_part_file_checks_clean() -> TestResult {
    let (mut session, _) = open()?;
    let reply = call(
        &mut session,
        "library.check_part_file",
        json!({"builtin": "esp32-s31-wroom-3", "values": {"can_height": {"mm": 2.0}}}),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let report = &reply["result"]["report"];
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(report["bodies"].as_array().map(Vec::len), Some(110));
    assert!(
        reply["result"]["materials"]
            .as_array()
            .is_some_and(|m| m.iter().any(|n| n == "nickel-silver"))
    );
    let bad = call(
        &mut session,
        "library.check_part_file",
        json!({"builtin": "esp32-s31-wroom-3", "values": {"can_hight": {"mm": 2.0}}}),
    );
    assert_eq!(bad["ok"], false, "{bad}");
    let neither = call(&mut session, "library.check_part_file", json!({}));
    assert_eq!(neither["ok"], false);
    let not_a_part = call(
        &mut session,
        "library.check_part_file",
        json!({"path": EXAMPLE}),
    );
    assert_eq!(not_a_part["ok"], false, "{not_a_part}");
    Ok(())
}

#[test]
fn a_package_uses_the_part_file_in_one_step_and_refusals_change_nothing() -> TestResult {
    let (mut session, package) = open()?;
    let start = session.engine().revision();
    let before = session.engine().to_file();
    // A refused value: nothing changes.
    let refused = call(
        &mut session,
        "library.use_part_file",
        json!({"package": package, "builtin": "esp32-s31-wroom-3", "expected_revision": start,
            "values": {"can_height": {"count": 2}}}),
    );
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(session.engine().revision(), start);
    assert_eq!(session.engine().to_file(), before);
    // A file of its own (other bytes, so not yet a model here) without a
    // licence is refused too.
    let dir = tempfile::tempdir()?;
    let own = dir.path().join("own.solidcraft");
    let mut bytes = super::builtin_parts()?.require(BUILTIN)?.bytes.to_vec();
    bytes.push(b'\n');
    std::fs::write(&own, bytes)?;
    let unlicensed = call(
        &mut session,
        "library.use_part_file",
        json!({"package": package, "path": own.to_string_lossy(), "expected_revision": start}),
    );
    assert_eq!(unlicensed["ok"], false, "{unlicensed}");
    assert_eq!(session.engine().to_file(), before);
    // Used: one step.
    let used = call(
        &mut session,
        "library.use_part_file",
        json!({"package": package, "builtin": "esp32-s31-wroom-3", "expected_revision": start,
            "values": {"can_height": {"mm": 2.465}}}),
    );
    assert_eq!(used["ok"], true, "{used}");
    assert_eq!(session.engine().revision(), start + 1);
    let Some(PackageModel::PartFile(part)) = model(&session, package) else {
        return Err("the package uses no part file".into());
    };
    assert_eq!(part.derived.bodies, 110);
    let surfaces = call(
        &mut session,
        "library.marking_surfaces",
        json!({"package": package}),
    );
    let text = surfaces.to_string();
    assert!(
        text.contains("can_top") && text.contains("pcb_top"),
        "{surfaces}"
    );
    // New values.
    let set = call(
        &mut session,
        "library.set_part_values",
        json!({"package": package, "expected_revision": start + 1,
            "values": {"can_height": {"mm": 2.0}}}),
    );
    assert_eq!(set["ok"], true, "{set}");
    let Some(PackageModel::PartFile(lower)) = model(&session, package) else {
        return Err("the package uses no part file".into());
    };
    assert!((part.derived.bounds()[1][2] - lower.derived.bounds()[1][2] - 0.465).abs() < 1e-6);
    let now = session.engine().revision();
    let undo = call(
        &mut session,
        "project.undo",
        json!({"expected_revision": now}),
    );
    assert_eq!(undo["ok"], true, "{undo}");
    assert_eq!(model(&session, package), Some(PackageModel::PartFile(part)));
    Ok(())
}

/// The example's drafts published and U1 placed on the board, through the
/// engine's typed operations (as `scene::fixtures::wroom_board`).
fn placed(session: &mut Session) -> TestResult {
    let drafts: Vec<u64> = session
        .engine()
        .drafts()
        .items
        .iter()
        .map(|d| d.id.0)
        .collect();
    let apply = |session: &mut Session, operation: Value| -> TestResult<Value> {
        let revision = session.engine().revision();
        let reply = call(
            session,
            "project.apply",
            json!({"expected_revision": revision, "operation": operation}),
        );
        if reply["ok"] != true {
            return Err(reply.to_string().into());
        }
        Ok(reply["result"]["outcome"].clone())
    };
    let published = apply(
        session,
        json!({"editor": "Library", "operation": {"command": "library.publish",
            "params": {"drafts": drafts}}}),
    )?;
    let revision = |kind: &str| -> TestResult<Value> {
        published["revisions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| r["kind"] == kind)
            .cloned()
            .ok_or_else(|| format!("no {kind} revision in {published}").into())
    };
    let component = revision("component")?;
    let symbol = revision("symbol")?;
    let footprint = revision("footprint")?;
    let binding = revision("binding")?;
    let placed = apply(
        session,
        json!({"editor": "Instance", "operation": {"command": "component.place", "params": {
            "component": {"id": component["id"], "revision": component["revision"]},
            "designator": "U1", "properties": [], "representation": symbol["id"], "unit": 1,
            "position": [60.0, 0.0],
            "physical": {"binding": binding["id"], "footprint": footprint["id"]}}}}),
    )?;
    let size = session.engine().project().board_size;
    apply(
        session,
        json!({"editor": "Layout", "operation": {"command": "layout.place", "params": {
            "instance": placed["instance"], "position": [size[0] / 2.0, size[1] / 2.0],
            "rotation_deg": 0.0, "layer": "Top"}}}),
    )?;
    Ok(())
}

/// The part file's bodies in a built board, by name.
fn part_bodies(model: &sparksmith_solidcraft::project::NativeModel) -> (usize, Vec<String>) {
    let names: Vec<String> = model
        .models
        .iter()
        .filter(|d| d.component.starts_with("pkgp"))
        .flat_map(|d| d.bodies.iter().map(|b| b.name.clone()))
        .collect();
    (names.len(), names)
}

/// A board whose module is the part file builds, saves, reopens and builds
/// the same; its native export reopens in Solidcraft with the part's
/// parameters live.
#[test]
fn a_board_using_the_part_file_saves_reopens_and_builds_the_same() -> TestResult {
    let (mut session, package) = open()?;
    if !matches!(model(&session, package), Some(PackageModel::PartFile(_))) {
        let revision = session.engine().revision();
        let used = call(
            &mut session,
            "library.use_part_file",
            json!({"package": package, "builtin": BUILTIN, "expected_revision": revision,
                "values": wroom3_values()}),
        );
        assert_eq!(used["ok"], true, "{used}");
    }
    placed(&mut session)?;
    let built = sparksmith_solidcraft::project::build(session.engine().project())?;
    let (count, names) = part_bodies(&built);
    assert_eq!(count, 110, "{names:?}");
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("board.sparksmith");
    sparksmith_io::persistence::write_project(&path, &session.engine().to_file())?;
    let reopened = sparksmith_io::read_project(&path)?;
    let engine = Engine::from_file(reopened)?;
    let again = sparksmith_solidcraft::project::build(engine.project())?;
    assert_eq!(part_bodies(&again), (count, names));
    // The native board: parametric, opens in plain Solidcraft.
    let native = dir.path().join("board.solidcraft");
    let exported = call(
        &mut session,
        "project.export_native",
        json!({"path": native.to_string_lossy()}),
    );
    assert_eq!(exported["ok"], true, "{exported}");
    let opened = sparksmith_solidcraft::native::Native::open(&native.to_string_lossy())?;
    assert_eq!(opened.evaluation_error(), None);
    assert!(
        opened
            .session()
            .doc
            .params
            .iter()
            .any(|p| p.name.starts_with("pkgp") && p.name.ends_with("_can_height")),
        "the part's parameters are live in the board"
    );
    Ok(())
}

/// Switches the example's package to the built-in part file at the
/// reference replay's values (`wroom3.replay.jsonl`, step 06b), as the
/// replay does: `SPARKSMITH_WRITE_EXAMPLE=1 cargo test -p sparksmith-automation
/// write_the_example_part_file -- --ignored`.
#[test]
#[ignore = "writes examples/esp32-s31-wroom-3"]
fn write_the_example_part_file() -> TestResult {
    if std::env::var_os("SPARKSMITH_WRITE_EXAMPLE").is_none() {
        return Err("set SPARKSMITH_WRITE_EXAMPLE".into());
    }
    let (mut session, package) = open()?;
    let revision = session.engine().revision();
    let used = call(
        &mut session,
        "library.use_part_file",
        json!({"package": package, "builtin": BUILTIN, "expected_revision": revision,
            "values": wroom3_values()}),
    );
    assert_eq!(used["ok"], true, "{used}");
    sparksmith_io::persistence::write_project(
        std::path::Path::new(EXAMPLE),
        &session.engine().to_file(),
    )?;
    Ok(())
}
