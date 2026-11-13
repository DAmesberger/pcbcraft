//! Slice B3's gate (acceptance 1 to 5 and 8 of the design note) through
//! the JSON transport, over the ESP32-S31 dev board example, with a
//! headless SolidCraft assembly (`sparksmith_solidcraft::linked_design`) that
//! links the published interface as SolidCraft's Insert Design does.
//!
//! The fixture is the example, whose J1 package carries the opening its
//! USB4105 part file declares (the part-file reader stored it when the
//! replay used the part file); every change after that is a typed command.
use super::Session;
use serde_json::{Value, json};
use sparksmith_core::layout::fill::FillCommand;
use sparksmith_core::library::datum::PackageDatumOrigin;
use sparksmith_core::model::fill::{FillRegion, FillSpec};
use sparksmith_core::{CopperLayer, Engine, InstanceCommand, Operation};
use sparksmith_solidcraft::linked_design::{Face, LinkedDesign, Operation as Extrude};
use std::path::{Path, PathBuf};

pub(super) type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const EXAMPLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/esp32-s31-devboard/esp32-s31-devboard.sparksmith"
);
const USB4105: &[u8] =
    include_bytes!("../../../../examples/esp32-s31-devboard/parts/usb4105.solidcraft");

pub(super) fn ok(session: &mut Session, request: Value) -> TestResult<Value> {
    let reply = session.dispatch(request);
    if reply["ok"] != true {
        return Err(format!("refused: {reply}").into());
    }
    Ok(reply["result"].clone())
}

pub(super) fn apply(session: &mut Session, operation: Operation) -> TestResult<Value> {
    let revision = session.engine().revision();
    ok(
        session,
        json!({"command": "project.apply", "params": {
            "expected_revision": revision, "operation": operation}}),
    )
}

pub(super) fn instance(session: &Session, designator: &str) -> TestResult<u64> {
    Ok(session
        .engine()
        .project()
        .instances
        .iter()
        .find(|i| i.designator == designator)
        .ok_or(designator.to_owned())?
        .id)
}

/// The example, saved in `dir` and opened. J1's package carries the
/// opening its USB4105 part file declares, copied by the part-file reader
/// when the example's replay used the part file.
pub(super) fn open(dir: &Path) -> TestResult<Session> {
    let file = sparksmith_io::read_project(Path::new(EXAMPLE))?;
    let part = sparksmith_solidcraft::models::part_file::evaluate(USB4105, &Default::default())?;
    let project = &file.project;
    let j1 = project
        .instances
        .iter()
        .find(|i| i.designator == "J1")
        .ok_or("J1")?;
    let package = sparksmith_core::instances::resolve(&project.library, j1)?
        .physical
        .ok_or("J1 is placed")?
        .package;
    let entry = project
        .library
        .packages
        .iter()
        .find(|p| p.id == package.id && p.revision == package.revision)
        .ok_or("J1's package")?;
    assert_eq!(part.datums.len(), 1, "the part file declares its opening");
    let stored: Vec<_> = entry
        .definition
        .datums
        .iter()
        .filter(|d| d.origin == PackageDatumOrigin::PartFile)
        .map(|d| (&d.name, &d.geometry, d.clearance_nm, d.external))
        .collect();
    let declared: Vec<_> = part
        .datums
        .iter()
        .map(|d| (&d.name, &d.geometry, d.clearance_nm, d.external))
        .collect();
    assert_eq!(
        stored, declared,
        "J1's package carries the part file's opening"
    );
    let path = dir.join("board.sparksmith");
    sparksmith_io::write_project(&path, &file)?;
    let mut session = Session::new(Engine::default());
    ok(
        &mut session,
        json!({"command": "project.open", "params": {"expected_revision": 0,
            "path": path.display().to_string()}}),
    )?;
    Ok(session)
}

pub(super) fn publish(session: &mut Session, path: &Path, tier: &str) -> TestResult<Value> {
    let revision = session.engine().revision();
    ok(
        session,
        json!({"command": "interface.publish", "params": {
            "path": path.display().to_string(), "tier": tier, "expected_revision": revision}}),
    )
}

pub(super) fn preview(session: &mut Session, path: &Path) -> TestResult<Value> {
    ok(
        session,
        json!({"command": "interface.publish_preview", "params": {
            "path": path.display().to_string()}}),
    )
}

/// The rows of a preview that are not unchanged.
pub(super) fn changes(preview: &Value) -> Vec<Value> {
    preview["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["change"] != "unchanged")
        .cloned()
        .collect()
}

/// Move J1 by `dx` mm along X as the example holds it: by its dimension to
/// the board edge (`width / 2`), edited with `layout.dimension_edit`. The
/// parts dimensioned from J1 come along; their datums are not exported
/// unless a test exports them.
pub(super) fn move_j1(session: &mut Session, dx: f64) -> TestResult {
    use sparksmith_core::library::footprint_source::{DimValue, Expr};
    use sparksmith_core::model::board::ConstraintTarget;
    use sparksmith_core::model::dimension::{BoardRef, FeatureRef};
    let j1 = instance(session, "J1")?;
    let x = |session: &Session| -> TestResult<f64> {
        Ok(session
            .engine()
            .project()
            .placement_of(j1)
            .ok_or("J1 placed")?
            .position[0])
    };
    let held = |d: &&sparksmith_core::model::dimension::PartDimension| {
        d.refs.len() == 2
            && matches!(d.refs.first(), Some(BoardRef::Part(FeatureRef::Origin { target: ConstraintTarget::Placement(p) })) if *p == j1)
            && matches!(d.refs.get(1), Some(BoardRef::Board(_)))
            && matches!(d.value, Some(DimValue::Expr(_)))
    };
    let before = x(session)?;
    for sign in [1.0, -1.0] {
        let mut dimension = session
            .engine()
            .project()
            .part_dimensions
            .iter()
            .find(held)
            .ok_or("J1's dimension to the board edge")?
            .clone();
        let Some(DimValue::Expr(Expr(text))) = &dimension.value else {
            return Err("an expression".into());
        };
        let step = sign * dx;
        dimension.value = Some(DimValue::Expr(Expr(format!("{text} + {step}"))));
        apply(
            session,
            Operation::Dimension(
                sparksmith_core::layout::dimensions::DimensionCommand::Edit { dimension },
            ),
        )?;
        let moved = x(session)? - before;
        if (moved - dx).abs() <= 1e-9 {
            return Ok(());
        }
        // Measured from the other edge: undo and go the other way.
        let revision = session.engine().revision();
        ok(
            session,
            json!({"command": "project.undo", "params": {"expected_revision": revision}}),
        )?;
    }
    Err("J1 did not move by its dimension".into())
}

/// Delete a part from the design (`project.apply_update`), its tracks
/// with it.
pub(super) fn remove(session: &mut Session, id: u64) -> TestResult {
    let revision = session.engine().revision();
    let change = sparksmith_core::updates::Change::Instance {
        edit: sparksmith_core::updates::InstanceChange::Remove { instance: id },
    };
    ok(
        session,
        json!({"command": "project.apply_update", "params": {
            "expected_revision": revision, "change": change,
            "remove_conflicting_tracks": true}}),
    )?;
    Ok(())
}

pub(super) fn sidecar(design: &Path) -> TestResult<sparksmith_core::interface::Sidecar> {
    Ok(sparksmith_io::interface::read_sidecar(design)?.ok_or("a sidecar")?)
}

/// The test linked design (acceptance 3): the board linked and grounded; a
/// 2 mm wall across J1's opening, 1 mm in front of the board edge, with a
/// cutout projected from the opening's outer face; a boss under the board
/// projected from mounting hole 1's wall. Returns it with the wall's own
/// box faces.
pub(super) fn linked_design(
    design: &Path,
    opening: &str,
) -> TestResult<(LinkedDesign, Vec<String>)> {
    let mut assembly_link = LinkedDesign::new("Assembly")?;
    let bodies = assembly_link.link(&design.display().to_string(), "Board")?;
    assert!(bodies.iter().any(|b| b == opening), "{bodies:?}");
    let front = assembly_link
        .faces(opening)?
        .into_iter()
        .find(|f| f.normal.is_some_and(|n| n[1] < -0.99))
        .ok_or("the opening's outer face")?;
    assembly_link.solid_box("Wall", [-5.0, -3.0, -2.0], [50.0, 2.0, 12.0])?;
    let box_faces = assembly_link
        .faces("Wall")?
        .into_iter()
        .map(|f| f.name)
        .collect();
    assembly_link.projected(
        "USB opening",
        [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        opening,
        front.point,
        2.0,
        Extrude::Cut,
    )?;
    let wall = assembly_link
        .faces("board")?
        .into_iter()
        .find(|f| f.normal.is_none())
        .ok_or("a hole wall")?;
    assembly_link.projected(
        "Boss",
        [[0.0, 0.0, -6.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        "board",
        wall.point,
        4.0,
        Extrude::New,
    )?;
    let errors = assembly_link.feature_errors();
    assert!(errors.is_empty(), "{errors:?}");
    Ok((assembly_link, box_faces))
}

/// Every face of `body`, by name, with its centroid and area.
pub(super) fn faces(assembly_link: &mut LinkedDesign, body: &str) -> TestResult<Vec<Face>> {
    let mut faces = assembly_link.faces(body)?;
    faces.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(faces)
}

fn same_faces(a: &[Face], b: &[Face], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}: face count");
    for (p, q) in a.iter().zip(b) {
        assert_eq!(p.name, q.name, "{what}");
        let moved = (0..3)
            .map(|k| (p.centroid[k] - q.centroid[k]).abs())
            .fold(0.0, f64::max);
        assert!(moved <= 1e-9, "{what}: {} moved by {moved}", p.name);
        assert!(
            (p.area - q.area).abs() <= 1e-9,
            "{what}: {} changed area",
            p.name
        );
    }
}

/// Acceptance 1 and 2, through typed commands only: J1 moved 2 mm along X
/// by its dimension (`layout.dimension_edit`); `interface.publish_preview` lists exactly one
/// changed datum, "J1 opening, 2 mm along X"; `interface.publish` records
/// revision 2.
#[test]
fn b3_the_preview_lists_the_one_moved_datum() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let mut session = open(dir.path())?;
    let first = publish(&mut session, &design, "interface")?;
    assert_eq!(first["published"]["revision"], 1);
    move_j1(&mut session, 2.0)?;
    let preview = preview(&mut session, &design)?;
    assert_eq!(
        (preview["from"].clone(), preview["to"].clone()),
        (json!(1), json!(2))
    );
    let changed = changes(&preview);
    let [only] = changed.as_slice() else {
        return Err(format!("{} changed: {changed:?}", changed.len()).into());
    };
    assert_eq!(only["name"], "J1 opening");
    assert_eq!(only["change"], "moved");
    assert_eq!(only["summary"], "J1 opening, 2 mm along X");
    let offset = only["offset"].as_array().ok_or("offset")?;
    let x = offset.first().and_then(Value::as_f64).ok_or("x")?;
    assert!((x - 2.0).abs() <= 1e-9, "{offset:?}");
    let second = publish(&mut session, &design, "interface")?;
    assert_eq!(second["published"]["revision"], 2);
    // A publish, like a save, is no undo step: undo takes back the move
    // before it and keeps the record, and the preview compares with the
    // file on disk, which holds the move.
    let revision = session.engine().revision();
    ok(
        &mut session,
        json!({"command": "project.undo", "params": {"expected_revision": revision}}),
    )?;
    assert_eq!(
        session
            .engine()
            .project()
            .interface
            .published
            .as_ref()
            .map(|r| r.revision),
        Some(2)
    );
    let back = self::preview(&mut session, &design)?;
    assert_eq!(back["from"], 2, "the revision on disk");
    let changed = changes(&back);
    let [only] = changed.as_slice() else {
        return Err(format!("{} changed: {changed:?}", changed.len()).into());
    };
    assert_eq!(only["summary"], "J1 opening, -2 mm along X");
    Ok(())
}

/// Acceptance 3: the test linked design re-evaluates after
/// `component.update_links` with the cutout moved 2 mm ± 1 µm along X, no
/// other face of the wall moved, and the boss on the mounting hole where
/// it was.
#[test]
fn b3_the_linked_design_cutout_follows_by_two_millimetres() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let mut session = open(dir.path())?;
    let opening = format!("i{}.opening", instance(&session, "J1")?);
    publish(&mut session, &design, "interface")?;
    let (mut assembly_link, box_faces) = linked_design(&design, &opening)?;
    let wall = faces(&mut assembly_link, "Wall")?;
    let boss = faces(&mut assembly_link, "Boss")?;

    move_j1(&mut session, 2.0)?;
    publish(&mut session, &design, "interface")?;
    let update = assembly_link.update_links()?;
    assert!(
        update.failed.is_empty() && !update.updated.is_empty(),
        "{update:?}"
    );
    assert!(
        assembly_link.feature_errors().is_empty(),
        "{:?}",
        assembly_link.feature_errors()
    );
    let after = faces(&mut assembly_link, "Wall")?;
    assert_eq!(after.len(), wall.len());
    let mut followed = 0;
    for a in &wall {
        let b = after
            .iter()
            .find(|b| b.name == a.name)
            .ok_or("a wall face")?;
        let d: Vec<f64> = (0..3).map(|k| b.centroid[k] - a.centroid[k]).collect();
        if box_faces.contains(&a.name) {
            // The box's own faces stay on their planes with their areas
            // (a pierced face's centroid moves with its hole).
            let n = a.normal.ok_or("a plane")?;
            let along = n[0] * d[0] + n[1] * d[1] + n[2] * d[2];
            assert!(along.abs() <= 1e-9, "{} left its plane", a.name);
            assert!((a.area - b.area).abs() <= 1e-9, "{} changed area", a.name);
        } else {
            assert!(
                (d[0] - 2.0).abs() <= 1e-3 && d[1].abs() <= 1e-9 && d[2].abs() <= 1e-9,
                "cutout face {} moved by {d:?}",
                a.name
            );
            followed += 1;
        }
    }
    assert!(followed >= 4, "the cutout's faces: {followed}");
    same_faces(&boss, &faces(&mut assembly_link, "Boss")?, "the boss");
    Ok(())
}

/// Acceptance 4: closing and reopening the project and publishing again,
/// and publishing it in another folder (as another machine would), give
/// the sidecar byte for byte, with the same face names.
#[test]
fn b3_the_sidecar_is_byte_identical_after_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let mut session = open(dir.path())?;
    publish(&mut session, &design, "interface")?;
    let saved = dir.path().join("saved.sparksmith");
    ok(
        &mut session,
        json!({"command": "project.save", "params": {"path": saved.display().to_string()}}),
    )?;
    let first = std::fs::read(sparksmith_io::interface::sidecar_path(&design))?;
    drop(session);

    let mut reopened = Session::new(Engine::default());
    ok(
        &mut reopened,
        json!({"command": "project.open", "params": {"expected_revision": 0,
            "path": saved.display().to_string()}}),
    )?;
    publish(&mut reopened, &design, "interface")?;
    let again = std::fs::read(sparksmith_io::interface::sidecar_path(&design))?;
    assert_eq!(first, again, "republished after reopening");
    let elsewhere = tempfile::tempdir()?;
    let other: PathBuf = elsewhere.path().join("board.solidcraft");
    publish(&mut reopened, &other, "interface")?;
    let there = std::fs::read(sparksmith_io::interface::sidecar_path(&other))?;
    assert_eq!(first, there, "published in another folder");
    let read = sidecar(&design)?;
    assert!(read.datums.iter().any(|d| !d.faces.is_empty()));
    Ok(())
}

/// Acceptance 5: R4's and R5's envelopes exported (R4's slot before
/// R5's), and a body over R5 projected from its envelope's top. Deleting
/// R4, adding a board fill and renaming J1, each published, change nothing
/// in the linked design: every face of the wall, the boss and the body over
/// R5 stays where it was, and Update Links reports no failure.
#[test]
fn b3_unrelated_edits_change_nothing_in_the_linked_design() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let mut session = open(dir.path())?;
    let opening = format!("i{}.opening", instance(&session, "J1")?);
    for part in ["R4", "R5"] {
        let id = instance(&session, part)?;
        apply(
            &mut session,
            Operation::Interface(sparksmith_core::interface::InterfaceCommand::Export {
                target: sparksmith_core::interface::ExportTarget::PartEnvelope { instance: id },
                name: None,
            }),
        )?;
    }
    let r5 = format!("i{}.envelope", instance(&session, "R5")?);
    publish(&mut session, &design, "interface")?;
    let (mut assembly_link, _) = linked_design(&design, &opening)?;
    let top = assembly_link
        .faces(&r5)?
        .into_iter()
        .find(|f| f.normal.is_some_and(|n| n[2] > 0.99))
        .ok_or("R5's envelope top")?;
    assembly_link.projected(
        "Over R5",
        [[0.0, 0.0, 10.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        &r5,
        top.point,
        1.0,
        Extrude::New,
    )?;
    let watched = ["Wall", "Boss", "Over R5"];
    let mut before = Vec::new();
    for body in watched {
        before.push(faces(&mut assembly_link, body)?);
    }

    // R4 deleted (its tracks with it).
    let r4 = instance(&session, "R4")?;
    remove(&mut session, r4)?;
    // A fill over the whole board, on the bottom.
    apply(
        &mut session,
        Operation::Fill(FillCommand::Add {
            fill: Box::new(FillSpec {
                name: "Pour".into(),
                net: None,
                layer: CopperLayer::Bottom,
                region: FillRegion::Board,
                outline: sparksmith_core::library::Outline {
                    start: [0.0, 0.0],
                    segments: vec![],
                },
                priority: 0,
                settings: Default::default(),
            }),
        }),
    )?;
    // J1 renamed.
    let j1 = instance(&session, "J1")?;
    apply(
        &mut session,
        Operation::Instance(InstanceCommand::SetDesignator {
            instance: j1,
            designator: "J7".into(),
        }),
    )?;
    let preview = preview(&mut session, &design)?;
    let changed = changes(&preview);
    for row in &changed {
        assert!(
            row["change"] != "moved" && row["change"] != "reshaped",
            "nothing moves: {row}"
        );
    }
    assert!(
        changed
            .iter()
            .any(|r| r["name"] == "R4 envelope" && r["to"] == "lost"),
        "R4's envelope is reported lost under its published name: {changed:?}"
    );
    publish(&mut session, &design, "interface")?;
    let update = assembly_link.update_links()?;
    assert!(update.failed.is_empty(), "{update:?}");
    assert!(
        assembly_link.feature_errors().is_empty(),
        "{:?}",
        assembly_link.feature_errors()
    );
    for (body, faces_before) in watched.iter().zip(&before) {
        same_faces(faces_before, &faces(&mut assembly_link, body)?, body);
    }
    // The sidecar names J1's opening by its new designator, under the same
    // token and faces.
    let read = sidecar(&design)?;
    let j1_opening = read
        .datums
        .iter()
        .find(|d| d.token == opening)
        .ok_or("J1's opening")?;
    assert_eq!(j1_opening.name, "J7 opening");
    Ok(())
}

/// Deleting J1 is reported before anything is written: the preview names
/// "J1 opening" as removed. Published anyway, the linked design's cutout no
/// longer evaluates, and Solidcraft names the feature (slice B4 turns that
/// into the user's words and blocks fabrication).
#[test]
fn b3_deleting_j1_is_reported() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let mut session = open(dir.path())?;
    let opening = format!("i{}.opening", instance(&session, "J1")?);
    publish(&mut session, &design, "interface")?;
    let (mut assembly_link, _) = linked_design(&design, &opening)?;
    let j1 = instance(&session, "J1")?;
    remove(&mut session, j1)?;
    let before = std::fs::read(&design)?;
    let preview = preview(&mut session, &design)?;
    assert_eq!(std::fs::read(&design)?, before, "a preview writes nothing");
    let changed = changes(&preview);
    let removed: Vec<&Value> = changed
        .iter()
        .filter(|r| r["change"] == "retired")
        .collect();
    let [row] = removed.as_slice() else {
        return Err(format!("{changed:?}").into());
    };
    assert_eq!(row["name"], "J1 opening");
    assert_eq!(
        row["summary"],
        "J1 opening, removed: a linked design feature that uses it will fail"
    );
    publish(&mut session, &design, "interface")?;
    assembly_link.update_links()?;
    let errors = assembly_link.feature_errors();
    assert!(
        errors.iter().any(|e| e.feature == "USB opening"),
        "the cutout's sketch fails: {errors:?}"
    );
    Ok(())
}

/// A publish at a stale revision writes nothing; a publish over another
/// board's interface is refused, writing nothing.
#[test]
fn b3_publish_at_a_stale_revision_writes_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let mut session = open(dir.path())?;
    let revision = session.engine().revision();
    let reply = session.dispatch(json!({"command": "interface.publish", "params": {
        "path": design.display().to_string(), "tier": "interface",
        "expected_revision": revision + 1}}));
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(!design.exists());
    assert!(!sparksmith_io::interface::sidecar_path(&design).exists());
    assert_eq!(
        std::fs::read_dir(dir.path())?.count(),
        1,
        "only the project"
    );

    publish(&mut session, &design, "interface")?;
    let other = open(dir.path())?;
    let mut other = other;
    let written = std::fs::read(&design)?;
    let revision = other.engine().revision();
    let reply = other.dispatch(json!({"command": "interface.publish", "params": {
        "path": design.display().to_string(), "tier": "interface",
        "expected_revision": revision}}));
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(std::fs::read(&design)?, written);
    Ok(())
}

/// The mechanical tier: the interface tier's slots, the same faces, then
/// the board's laminate, pads and parts.
#[test]
fn b3_the_mechanical_tier_keeps_the_slots() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut session = open(dir.path())?;
    let slots = dir.path().join("slots.solidcraft");
    let board = dir.path().join("board.solidcraft");
    let interface = publish(&mut session, &slots, "interface")?;
    let mechanical = publish(&mut session, &board, "mechanical")?;
    // The board after the slots is checked against the board plan (A5);
    // the slots alone build no board.
    assert!(interface["published"].get("check").is_none(), "{interface}");
    let check = &mechanical["published"]["check"];
    assert_eq!(check["differs"], false, "{check}");
    assert!(check["planned"].as_u64() > Some(0), "{check}");
    let faces = |path: &Path| -> TestResult<Vec<_>> {
        Ok(sidecar(path)?
            .datums
            .into_iter()
            .map(|d| (d.token, d.faces))
            .collect())
    };
    assert_eq!(faces(&slots)?, faces(&board)?);
    assert!(std::fs::metadata(&board)?.len() > std::fs::metadata(&slots)?.len());
    Ok(())
}

/// Acceptance 8 (a target until measured in release): publishing the
/// interface tier of the ESP32 board takes under 100 ms. Run with
/// `--ignored --nocapture` to measure.
#[test]
#[ignore = "timing target; run with --ignored --nocapture"]
fn b3_publishing_the_interface_tier_is_under_100_ms() -> TestResult {
    let dir = tempfile::tempdir()?;
    let design = dir.path().join("board.solidcraft");
    let mut session = open(dir.path())?;
    publish(&mut session, &design, "interface")?;
    let mut times = Vec::new();
    for k in 0..10 {
        move_j1(&mut session, if k % 2 == 0 { 0.5 } else { -0.5 })?;
        let start = std::time::Instant::now();
        publish(&mut session, &design, "interface")?;
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let median = times.get(times.len() / 2).copied().unwrap_or(f64::NAN);
    eprintln!("interface.publish, interface tier, ESP32: median {median:.1} ms, all {times:.1?}");
    assert!(median < 100.0, "{median} ms");
    Ok(())
}

/// Slice P1: the project keeps where it published, relative to its folder
/// when the design lies inside it, else as given, so the next publish
/// defaults there after a restart; saved and opened again, it holds.
#[test]
fn the_project_keeps_where_it_published() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut session = open(dir.path())?;
    let hint = |session: &Session| {
        session
            .engine()
            .project()
            .interface
            .published
            .as_ref()
            .and_then(|r| r.design_hint.clone())
    };
    std::fs::create_dir_all(dir.path().join("shared"))?;
    publish(
        &mut session,
        &dir.path().join("shared").join("board.solidcraft"),
        "interface",
    )?;
    assert_eq!(
        hint(&session).map(std::path::PathBuf::from),
        Some(Path::new("shared").join("board.solidcraft"))
    );
    let elsewhere = tempfile::tempdir()?;
    let outside = elsewhere.path().join("board.solidcraft");
    publish(&mut session, &outside, "interface")?;
    let kept = hint(&session).ok_or("a hint")?;
    assert!(Path::new(&kept).is_absolute(), "{kept}");
    assert_eq!(Path::new(&kept).file_name(), outside.file_name(), "{kept}");
    let saved = dir.path().join("board.sparksmith");
    ok(
        &mut session,
        json!({"command": "project.save", "params": {"path": saved.display().to_string()}}),
    )?;
    let mut reopened = Session::new(Engine::default());
    ok(
        &mut reopened,
        json!({"command": "project.open", "params": {"expected_revision": 0,
            "path": saved.display().to_string()}}),
    )?;
    assert_eq!(hint(&reopened), Some(kept));
    Ok(())
}
