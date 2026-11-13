//! The board interface in the desktop (slice B6), frame by frame: the Board
//! card's Interface section in every state (not published, published,
//! stale, broken), marking and unmarking datums for export from their rows
//! and from the Placement card, the automatic rules, both docked previews
//! in every state (waiting, shown, gated by the dry run, stale, failed) and
//! the publish itself off the UI thread, and the datums drawn through the
//! one painter Layout and 3D share.
use super::interface_card::{self, State};
use super::interface_host::{Done, Running};
use super::*;
use egui::{Pos2, Shape};
use pcbcraft_core::Engine;
use pcbcraft_core::ids::ContentHash;
use pcbcraft_core::interface::assembly_link::{
    AssemblyCommand, AssemblyPreview, AssemblyRead, Motion, ObjectChange, ObjectKind,
};
use pcbcraft_core::interface::{
    AssemblyEffect, ChangeKind, DatumChange, DryRun, EffectKind, ExportTarget, InterfaceCommand,
    PublishPreview, publish,
};
use pcbcraft_core::library::datum::{
    DatumFrame, DatumName, DatumProfile, PackageDatum, PackageDatumGeometry, PackageDatumOrigin,
};
use pcbcraft_core::library::package_model::{PackageEdit, edit_package};
use pcbcraft_core::model::assembly_link::Consumer;
use pcbcraft_core::model::interface::{AutoRule, DatumId, Slot, Tier};
use pcbcraft_core::units::Nm;
use pcbcraft_ui::fonts;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const SOURCE: pcbcraft_core::ids::InterfaceSourceId = pcbcraft_core::ids::InterfaceSourceId(0x5555);

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    ctx
}

/// One frame of the subject slot, tall enough for the whole Board card.
fn frame(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<Operation>) {
    let mut command = None;
    // A second a frame, so two presses never read as a double-click.
    let time = ctx.input(|i| i.time) + 1.0;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(420.0, 4000.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| {
            command = editor.subject_slot(ui, engine).output;
        },
    );
    output.textures_delta.clear();
    (output, command)
}

fn walk_texts(shape: &Shape, out: &mut Vec<(String, Pos2)>) {
    match shape {
        Shape::Vec(shapes) => shapes.iter().for_each(|s| walk_texts(s, out)),
        Shape::Text(text) => out.push((
            text.galley.job.text.clone(),
            text.pos + text.galley.size() * 0.5,
        )),
        _ => {}
    }
}

fn texts(output: &egui::FullOutput) -> Vec<String> {
    let mut out = Vec::new();
    for shape in &output.shapes {
        walk_texts(&shape.shape, &mut out);
    }
    out.into_iter().map(|(t, _)| t).collect()
}

fn position(output: &egui::FullOutput, label: &str) -> Option<Pos2> {
    let mut out = Vec::new();
    for shape in &output.shapes {
        walk_texts(&shape.shape, &mut out);
    }
    out.into_iter()
        .rev()
        .find(|(t, _)| t == label)
        .map(|(_, p)| p)
}

fn press(point: Pos2) -> Vec<egui::Event> {
    let button = |pressed| egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![
        egui::Event::PointerMoved(point),
        button(true),
        button(false),
    ]
}

/// Draw, then click the painted `label`; the clicked frame's operation.
fn click(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    label: &str,
) -> TestResult<Option<Operation>> {
    let (output, _) = frame(editor, engine, ctx, vec![]);
    let point =
        position(&output, label).ok_or_else(|| format!("no {label} in {:?}", texts(&output)))?;
    Ok(frame(editor, engine, ctx, press(point)).1)
}

/// Rest the pointer on the painted `label` (hover actions show then).
fn hover(editor: &mut PcbEditor, engine: &Engine, ctx: &egui::Context, label: &str) -> TestResult {
    let (output, _) = frame(editor, engine, ctx, vec![]);
    let point =
        position(&output, label).ok_or_else(|| format!("no {label} in {:?}", texts(&output)))?;
    frame(editor, engine, ctx, vec![egui::Event::PointerMoved(point)]);
    Ok(())
}

fn shows(editor: &mut PcbEditor, engine: &Engine, ctx: &egui::Context) -> Vec<String> {
    frame(editor, engine, ctx, vec![]);
    texts(&frame(editor, engine, ctx, vec![]).0)
}

/// Operations compare by what they send.
fn same(a: &Operation, b: &Operation) -> TestResult {
    let (a, b) = (serde_json::to_value(a)?, serde_json::to_value(b)?);
    if a == b {
        Ok(())
    } else {
        Err(format!("{a} is not {b}").into())
    }
}

fn apply(engine: &mut Engine, operation: Operation) -> TestResult {
    let revision = engine.revision();
    engine.apply(revision, operation)?;
    Ok(())
}

fn mm(v: f64) -> TestResult<Nm> {
    Ok(Nm::from_mm_f64_rounded(v)?)
}

/// The demo board with an opening on its first package marked for the
/// linked design, and an axis that is not.
fn demo() -> TestResult<Engine> {
    let mut file = Engine::demo()?.to_file();
    let entry = file
        .project
        .library
        .packages
        .first_mut()
        .ok_or("a package")?;
    let opening = PackageDatum {
        name: DatumName("opening".into()),
        geometry: PackageDatumGeometry::Volume {
            frame: DatumFrame {
                origin_nm: [Nm::ZERO, mm(-0.5)?, mm(0.25)?],
                x: [1.0, 0.0, 0.0],
                y: [0.0, 0.0, 1.0],
            },
            profile: DatumProfile::Rect {
                size_nm: [mm(1.0)?, mm(0.5)?],
                corner_radius_nm: Nm::ZERO,
            },
            height_nm: mm(2.0)?,
        },
        clearance_nm: None,
        external: true,
        origin: PackageDatumOrigin::Authored,
    };
    let axis = PackageDatum {
        name: DatumName("light".into()),
        geometry: PackageDatumGeometry::Axis {
            point_nm: [Nm::ZERO, Nm::ZERO, mm(0.5)?],
            direction: [0.0, 0.0, 1.0],
            diameter_nm: None,
        },
        clearance_nm: None,
        external: false,
        origin: PackageDatumOrigin::Authored,
    };
    entry.definition = edit_package(
        &entry.definition,
        &[PackageEdit::AddDatum(opening), PackageEdit::AddDatum(axis)],
    )?;
    file.project.interface = Default::default();
    Ok(Engine::from_file(file)?)
}

/// The first placement's instance and designator.
fn first_part(engine: &Engine) -> TestResult<(Id, String)> {
    let p = engine.project().placements.first().ok_or("a placement")?;
    let designator = engine
        .project()
        .instance(p.instance)
        .ok_or("its instance")?
        .designator
        .clone();
    Ok((p.instance, designator))
}

/// Publish as the host records it, without the files.
fn record_publish(engine: &mut Engine) -> TestResult<pcbcraft_core::interface::Sidecar> {
    let sidecar = publish::candidate(engine.project(), Tier::Interface, SOURCE)?;
    let record = publish::record(engine.project(), &sidecar)?;
    apply(
        engine,
        Operation::Interface(InterfaceCommand::RecordPublish {
            source_id: SOURCE,
            record,
        }),
    )?;
    Ok(sidecar)
}

fn move_part(engine: &mut Engine, instance: Id, by: [f64; 2]) -> TestResult {
    let p = engine
        .project()
        .placement_of(instance)
        .ok_or("placement")?
        .clone();
    apply(
        engine,
        Operation::Layout(pcbcraft_core::layout::LayoutCommand::Move {
            instance,
            position: [p.position[0] + by[0], p.position[1] + by[1]],
            rotation_deg: p.rotation_deg,
            layer: p.layer,
        }),
    )
}

/// The Interface section's state chip, as the card paints it.
fn status_of(editor: &mut PcbEditor, engine: &Engine, ctx: &egui::Context) -> TestResult<String> {
    let shown = shows(editor, engine, ctx);
    let state = interface_card::shown(engine)?;
    let words = State::of(&state.list, &state.assembly_link).words();
    if !shown.contains(&words) {
        return Err(format!("{words} not painted in {shown:?}").into());
    }
    Ok(words)
}

#[test]
fn the_interface_section_lists_datums_by_group_with_kind_and_slot() -> TestResult {
    let engine = demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (_, part) = first_part(&engine)?;
    let shown = shows(&mut editor, &engine, &ctx);
    for text in [
        "INTERFACE",
        "Not published",
        "PARTS",
        "Publish…",
        "Publish first to link a design",
        "NOT EXPORTED",
        &format!("{part} light"),
        "AUTOMATIC",
    ] {
        assert!(shown.iter().any(|t| t == text), "{text} in {shown:?}");
    }
    assert!(
        shown.iter().any(|t| t == &format!("{part} opening")),
        "the package's opening is exported for its part: {shown:?}"
    );
    // The board's own datums start folded; their group opens.
    assert!(!shown.iter().any(|t| t == "Board frame"), "{shown:?}");
    assert!(click(&mut editor, &engine, &ctx, "BOARD")?.is_none());
    let shown = shows(&mut editor, &engine, &ctx);
    for text in ["Board frame", "frame · slot 0 · automatic"] {
        assert!(shown.iter().any(|t| t == text), "{text} in {shown:?}");
    }
    Ok(())
}

#[test]
fn the_status_reads_published_then_stale_then_broken() -> TestResult {
    let mut engine = demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    assert_eq!(status_of(&mut editor, &engine, &ctx)?, "Not published");

    let sidecar = record_publish(&mut engine)?;
    assert_eq!(
        status_of(&mut editor, &engine, &ctx)?,
        "Published · revision 1"
    );

    let (instance, part) = first_part(&engine)?;
    move_part(&mut engine, instance, [2.0, 0.0])?;
    let stale = status_of(&mut editor, &engine, &ctx)?;
    assert!(
        stale.ends_with("changed since revision 1"),
        "a moved part's datums changed: {stale}"
    );
    assert!(
        shows(&mut editor, &engine, &ctx)
            .iter()
            .any(|t| t == "Changed"),
        "a changed datum's row is badged"
    );

    // A linked design feature uses the part's opening; deleting the part
    // breaks it.
    let token = format!("i{instance}.opening");
    let datum = sidecar
        .datums
        .iter()
        .find(|d| d.token == token)
        .ok_or("the opening was published")?;
    apply(
        &mut engine,
        Operation::Assembly(AssemblyCommand::RecordLink {
            read: AssemblyRead {
                assembly_hint: "assembly.solidcraft".into(),
                design_hint: "board.solidcraft".into(),
                component: "Board".into(),
                file_sha256: ContentHash([7; 32]),
                board_revision: Some(1),
                references: Vec::new(),
                bindings: Vec::new(),
                consumers: vec![Consumer {
                    feature: "USB opening".into(),
                    datum: datum.id,
                    slot: datum.slot,
                    via: Default::default(),
                    token,
                    name: format!("{part} opening"),
                }],
                board_sha256: None,
                unexported: Vec::new(),
            },
        }),
    )?;
    let shown = shows(&mut editor, &engine, &ctx);
    assert!(
        shown.iter().any(|t| t == "assembly.solidcraft · linked"),
        "the link and its state: {shown:?}"
    );
    let revision = engine.revision();
    engine.apply_update(
        revision,
        pcbcraft_core::updates::Change::Instance {
            edit: pcbcraft_core::updates::InstanceChange::Remove { instance },
        },
        false,
    )?;
    assert_eq!(
        status_of(&mut editor, &engine, &ctx)?,
        "Broken reference · fabrication output is blocked"
    );
    Ok(())
}

#[test]
fn a_row_marks_a_datum_for_export_and_unmarks_it() -> TestResult {
    let mut engine = demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (instance, part) = first_part(&engine)?;
    let light = format!("{part} light");
    hover(&mut editor, &engine, &ctx, &light)?;
    let operation = click(&mut editor, &engine, &ctx, "Export")?.ok_or("Export sends")?;
    same(
        &operation,
        &Operation::Interface(InterfaceCommand::Export {
            target: ExportTarget::PackageDatum {
                instance,
                name: DatumName("light".into()),
            },
            name: None,
        }),
    )?;
    engine.apply(engine.revision(), operation)?;
    let shown = shows(&mut editor, &engine, &ctx);
    // The axis is exported now, in its own slot, among the parts.
    let at = shown
        .iter()
        .position(|t| *t == light)
        .ok_or("the axis row")?;
    assert!(
        shown
            .get(at + 1)
            .is_some_and(|t| t.starts_with("axis · slot ")),
        "{shown:?}"
    );
    let parts = shown.iter().position(|t| t == "PARTS");
    let rest = shown.iter().position(|t| t == "NOT EXPORTED");
    assert!(parts < Some(at) && rest.is_none_or(|r| at < r), "{shown:?}");
    let row = interface_card::shown(&engine)?
        .list
        .datums
        .into_iter()
        .find(|r| r.name == light)
        .ok_or("the exported axis")?;
    hover(&mut editor, &engine, &ctx, &light)?;
    let operation = click(&mut editor, &engine, &ctx, "Unexport")?.ok_or("Unexport sends")?;
    same(
        &operation,
        &Operation::Interface(InterfaceCommand::Unexport { datum: row.id }),
    )?;
    Ok(())
}

#[test]
fn the_placement_card_marks_a_parts_frame_for_export() -> TestResult {
    let engine = demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (instance, _) = first_part(&engine)?;
    editor.instance = Some(instance);
    assert!(
        click(&mut editor, &engine, &ctx, "INTERFACE")?.is_none(),
        "opening the section changes nothing"
    );
    let shown = shows(&mut editor, &engine, &ctx);
    assert!(
        shown.iter().any(|t| t == "Exported automatically"),
        "the package's external opening: {shown:?}"
    );
    let operation = click(&mut editor, &engine, &ctx, "Frame")?.ok_or("the check sends")?;
    same(
        &operation,
        &Operation::Interface(InterfaceCommand::Export {
            target: ExportTarget::PartFrame { instance },
            name: None,
        }),
    )?;
    Ok(())
}

#[test]
fn an_automatic_rule_is_switched_from_its_check() -> TestResult {
    let engine = demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    assert!(click(&mut editor, &engine, &ctx, "AUTOMATIC")?.is_none());
    let operation =
        click(&mut editor, &engine, &ctx, "Mounting holes")?.ok_or("the rule's check sends")?;
    same(
        &operation,
        &Operation::Interface(InterfaceCommand::Auto {
            rule: AutoRule::Holes,
            on: false,
        }),
    )?;
    Ok(())
}

/// A preview of one moved opening from revision 1, its linked design's cutout
/// following it and a lid rib that would fail.
fn preview(engine: &Engine, name: &str) -> PublishPreview {
    PublishPreview {
        document_revision: engine.revision(),
        from: Some(1),
        to: 2,
        hash: ContentHash([1; 32]),
        tier: Tier::Interface,
        changes: vec![DatumChange {
            datum: DatumId(1),
            slot: Slot(7),
            name: name.into(),
            change: ChangeKind::Moved {
                offset: pcbcraft_core::interface::Vec3([2.0, 0.0, 0.0]),
                turn_deg: 0.0,
            },
            summary: format!("{name}, 2 mm along X"),
        }],
        dry_run: Some(DryRun {
            linked_design: "/tmp/linked.solidcraft".into(),
            effects: vec![
                AssemblyEffect {
                    feature: "USB opening".into(),
                    effect: EffectKind::Moves,
                    datum: Some(DatumId(1)),
                    message: format!(
                        "The linked design's ‘USB opening’ follows {name}: it moves 2 mm along X"
                    ),
                },
                AssemblyEffect {
                    feature: "Lid rib".into(),
                    effect: EffectKind::Fails,
                    datum: None,
                    message: "The linked design's ‘Lid rib’ would fail".into(),
                },
            ],
        }),
    }
}

/// The open review, waiting on a request that has not answered.
fn waiting(editor: &mut PcbEditor) -> std::sync::mpsc::Sender<Done> {
    let (sender, running) = Running::pending();
    editor.interface_hold(running);
    sender
}

#[test]
fn the_publish_preview_waits_then_shows_changes_and_the_dry_run() -> TestResult {
    let mut engine = demo()?;
    record_publish(&mut engine)?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    editor.open_publish(&ctx, &engine);
    let _hold = waiting(&mut editor);
    assert_eq!(editor.subject(), Subject::Review);
    let shown = shows(&mut editor, &engine, &ctx);
    for text in [
        "Publish interface",
        "Comparing with the last publish…",
        "Contents",
    ] {
        assert!(shown.iter().any(|t| t == text), "{text} in {shown:?}");
    }
    assert!(
        click(&mut editor, &engine, &ctx, "Publish")?.is_none() && !editor.interface_publishing(),
        "Publish waits for the preview"
    );

    editor.interface_answer(
        &ctx,
        Done::PublishPreview(Ok(preview(&engine, "J1 opening"))),
    );
    let shown = shows(&mut editor, &engine, &ctx);
    for text in [
        "Revision 1 → 2 · 1 datum changed",
        "The linked design's ‘Lid rib’ would fail",
        "Publish anyway: ‘Lid rib’ will fail in SolidCraft",
        "DATUMS",
        "J1 opening, 2 mm along X",
        "The linked design's ‘USB opening’ follows J1 opening: it moves 2 mm along X",
    ] {
        assert!(shown.iter().any(|t| t == text), "{text} in {shown:?}");
    }
    click(&mut editor, &engine, &ctx, "Publish")?;
    assert!(
        !editor.interface_publishing(),
        "a feature that would fail gates Publish until confirmed"
    );
    // Confirmed, Publish asks for a file: the project was never saved.
    click(
        &mut editor,
        &engine,
        &ctx,
        "Publish anyway: ‘Lid rib’ will fail in SolidCraft",
    )?;
    let shown = shows(&mut editor, &engine, &ctx);
    assert!(
        shown
            .iter()
            .any(|t| t == "Save the project, or choose a file, to publish"),
        "{shown:?}"
    );
    Ok(())
}

#[test]
fn a_stale_or_failed_publish_preview_offers_to_compute_it_again() -> TestResult {
    let mut engine = demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    editor.open_publish(&ctx, &engine);
    let _hold = waiting(&mut editor);
    editor.interface_answer(
        &ctx,
        Done::PublishPreview(Ok(preview(&engine, "J1 opening"))),
    );
    let (instance, _) = first_part(&engine)?;
    move_part(&mut engine, instance, [1.0, 0.0])?;
    let shown = shows(&mut editor, &engine, &ctx);
    for text in ["The project changed", "Recompute"] {
        assert!(shown.iter().any(|t| t == text), "{text} in {shown:?}");
    }

    // Computed again (as Recompute or another tier does), and refused.
    editor.interface_set_tier(&ctx, &engine, Tier::Interface);
    let _hold = waiting(&mut editor);
    editor.interface_answer(
        &ctx,
        Done::PublishPreview(Err("The published interface could not be read".into())),
    );
    let shown = shows(&mut editor, &engine, &ctx);
    for text in [
        "The preview could not be made",
        "Try again",
        "The published interface could not be read",
    ] {
        assert!(shown.iter().any(|t| t == text), "{text} in {shown:?}");
    }
    Ok(())
}

#[test]
fn the_linked_design_preview_lists_what_moves_or_why_it_is_refused() -> TestResult {
    let engine = demo()?;
    let ctx = context();
    // No linked design is there: the real request says so, in the user's words.
    let mut editor = PcbEditor {
        project_path: Some(std::env::temp_dir().join("sparksmith-b6-absent/board.pcbcraft")),
        ..PcbEditor::default()
    };
    let mut linked = engine.fork();
    apply(
        &mut linked,
        Operation::Assembly(AssemblyCommand::RecordLink {
            read: AssemblyRead {
                assembly_hint: "assembly.solidcraft".into(),
                design_hint: "board.solidcraft".into(),
                component: "Board".into(),
                file_sha256: ContentHash([7; 32]),
                board_revision: None,
                references: Vec::new(),
                bindings: Vec::new(),
                consumers: Vec::new(),
                board_sha256: None,
                unexported: Vec::new(),
            },
        }),
    )?;
    let engine = linked;
    editor.open_assembly(&ctx, &engine);
    let started = std::time::Instant::now();
    while editor.interface_waiting() {
        frame(&mut editor, &engine, &ctx, vec![]);
        if started.elapsed() > std::time::Duration::from_secs(60) {
            return Err("the linked design read did not answer".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let shown = shows(&mut editor, &engine, &ctx);
    assert!(
        shown
            .iter()
            .any(|t| t.starts_with("The linked design file was not found")),
        "{shown:?}"
    );

    let _hold = waiting(&mut editor);
    editor.interface_answer(
        &ctx,
        Done::AssemblyPreview(Ok(AssemblyPreview {
            document_revision: engine.revision(),
            expect_hash: ContentHash([8; 32]),
            changed: true,
            references: Vec::new(),
            objects: vec![
                ObjectChange {
                    kind: ObjectKind::Edge,
                    name: "top".into(),
                    motion: Motion::Moved {
                        offset: [0.0, 1.0],
                        turn_deg: 0.0,
                    },
                    summary: "Board edge top, 1 mm along Y".into(),
                    objects: Vec::new(),
                },
                ObjectChange {
                    kind: ObjectKind::Part,
                    name: "SW2".into(),
                    motion: Motion::Moved {
                        offset: [0.0, 1.0],
                        turn_deg: 0.0,
                    },
                    summary: "SW2, 1 mm along Y".into(),
                    objects: Vec::new(),
                },
            ],
            consumers: Vec::new(),
            refused: None,
            findings: Vec::new(),
        })),
    );
    let shown = shows(&mut editor, &engine, &ctx);
    for text in [
        "Update from linked design",
        "The linked design changed · 2 board objects move",
        "Board edge top, 1 mm along Y",
        "SW2, 1 mm along Y",
        "Update",
    ] {
        assert!(shown.iter().any(|t| t == text), "{text} in {shown:?}");
    }

    let _hold = waiting(&mut editor);
    editor.interface_answer(
        &ctx,
        Done::AssemblyPreview(Ok(AssemblyPreview {
            document_revision: engine.revision(),
            expect_hash: ContentHash([8; 32]),
            changed: true,
            references: Vec::new(),
            objects: Vec::new(),
            consumers: Vec::new(),
            refused: Some("The board would not solve with the wall moved".into()),
            findings: Vec::new(),
        })),
    );
    let shown = shows(&mut editor, &engine, &ctx);
    for text in [
        "The update would be refused",
        "The board would not solve with the wall moved",
        "Fix the errors above to apply",
    ] {
        assert!(shown.iter().any(|t| t == text), "{text} in {shown:?}");
    }
    Ok(())
}

/// Publishing for real: the interface tier of the demo is built and
/// written off the UI thread, and its record applied on the window's
/// engine makes the card read "Published".
#[test]
fn publish_writes_the_design_off_the_ui_thread_and_records_it() -> TestResult {
    let mut engine = demo()?;
    let dir = tempfile::tempdir()?;
    let ctx = context();
    let mut editor = PcbEditor {
        project_path: Some(dir.path().join("demo.pcbcraft")),
        ..PcbEditor::default()
    };
    let wait = |editor: &mut PcbEditor, engine: &Engine| -> TestResult<Option<Operation>> {
        let started = std::time::Instant::now();
        loop {
            let (_, operation) = frame(editor, engine, &ctx, vec![]);
            if operation.is_some() || !editor.interface_waiting() {
                return Ok(operation);
            }
            if started.elapsed() > std::time::Duration::from_secs(120) {
                return Err("the request did not answer".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    editor.open_publish(&ctx, &engine);
    wait(&mut editor, &engine)?;
    let shown = shows(&mut editor, &engine, &ctx);
    assert!(
        shown.iter().any(|t| t.starts_with("First publish · ")),
        "{shown:?}"
    );
    // The interface tier, the slots only.
    let options = shows(&mut editor, &engine, &ctx);
    assert!(
        options
            .iter()
            .any(|t| t == "Interface and mechanical bodies")
    );
    editor.interface_set_tier(&ctx, &engine, Tier::Interface);
    wait(&mut editor, &engine)?;
    click(&mut editor, &engine, &ctx, "Publish")?;
    assert!(
        editor.interface_publishing(),
        "{:?}",
        shows(&mut editor, &engine, &ctx)
    );
    let operation = wait(&mut editor, &engine)?.ok_or("the record comes back")?;
    assert!(matches!(
        operation,
        Operation::Interface(InterfaceCommand::RecordPublish { .. })
    ));
    assert!(dir.path().join("demo-board.solidcraft").is_file());
    assert!(dir.path().join("demo-board.interface.json").is_file());
    assert_eq!(
        engine.project().interface.published,
        None,
        "not before it is applied"
    );
    engine.apply(engine.revision(), operation)?;
    assert_eq!(editor.subject(), Subject::Properties, "the review closed");
    assert_eq!(
        status_of(&mut editor, &engine, &ctx)?,
        "Published · revision 1"
    );
    Ok(())
}

/// A view mapping for the painter: Layout's top view, or an oblique 3D one
/// where height shows.
struct View {
    oblique: bool,
}

impl super::dimensions::Surface for View {
    fn to_screen(&self, p: [f64; 3]) -> Option<Pos2> {
        let z = if self.oblique { p[2] } else { 0.0 };
        Some(Pos2::new(
            (200.0 + p[0] * 10.0 + z * 6.0) as f32,
            (600.0 - p[1] * 10.0 - z * 8.0) as f32,
        ))
    }
}

/// What the painter drew: the texts and the colours of its strokes.
fn painted(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    view: &View,
) -> (Vec<String>, Vec<egui::Color32>, Vec<Pos2>) {
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let painter = ui.painter().clone();
        editor.paint_datums(ui, &painter, view, engine);
    });
    output.textures_delta.clear();
    let mut words = Vec::new();
    let mut colours = Vec::new();
    let mut points = Vec::new();
    fn walk(s: &Shape, w: &mut Vec<String>, c: &mut Vec<egui::Color32>, p: &mut Vec<Pos2>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, w, c, p)),
            Shape::Text(t) => w.push(t.galley.job.text.clone()),
            Shape::LineSegment { points, stroke } => {
                c.push(stroke.color);
                p.extend(points);
            }
            Shape::Path(path) => {
                if let egui::epaint::ColorMode::Solid(colour) = path.stroke.color {
                    c.push(colour);
                }
                p.extend(&path.points);
            }
            Shape::Circle(circle) => c.push(circle.stroke.color),
            _ => {}
        }
    }
    for s in &output.shapes {
        walk(&s.shape, &mut words, &mut colours, &mut points);
    }
    (words, colours, points)
}

#[test]
fn datums_are_drawn_in_layout_and_3d_by_one_painter() -> TestResult {
    let engine = demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (instance, part) = first_part(&engine)?;
    let opening = format!("{part} opening");
    let t = pcbcraft_ui::theme::Tokens::get();

    // With the Board card shown, the interface is on the canvas: the
    // opening carries its chip; the board's own frame and outline do not
    // trace the board again.
    let (words, colours, flat) = painted(&mut editor, &engine, &ctx, &View { oblique: false });
    assert!(words.contains(&opening), "{words:?}");
    assert!(!words.iter().any(|w| w == "Board outline"), "{words:?}");
    assert!(colours.iter().all(|c| *c != t.accent), "nothing is hot");
    // In 3D the same datums are drawn, their heights apart.
    let (words_3d, _, oblique) = painted(&mut editor, &engine, &ctx, &View { oblique: true });
    assert_eq!(words, words_3d, "the same datums in both views");
    assert_eq!(flat.len(), oblique.len());
    assert_ne!(flat, oblique, "3D shows height");

    // The selected datum is hot; the board outline shows while it is.
    let shown = interface_card::shown(&engine)?;
    let outline = shown
        .list
        .datums
        .iter()
        .find(|r| r.role == "board.outline")
        .ok_or("the outline")?;
    editor.interface.datum = Some(outline.id);
    let (words, colours, _) = painted(&mut editor, &engine, &ctx, &View { oblique: false });
    assert!(words.iter().any(|w| w == "Board outline"), "{words:?}");
    assert!(colours.contains(&t.accent), "the selected datum is lit");

    // A part selected elsewhere shows its own datums only.
    editor.interface.datum = None;
    let other = engine
        .project()
        .placements
        .iter()
        .map(|p| p.instance)
        .find(|id| *id != instance)
        .ok_or("another part")?;
    editor.instance = Some(other);
    let (words, _, _) = painted(&mut editor, &engine, &ctx, &View { oblique: false });
    assert!(
        !words.contains(&opening),
        "another part's datums: {words:?}"
    );
    editor.instance = Some(instance);
    let (words, _, _) = painted(&mut editor, &engine, &ctx, &View { oblique: false });
    assert!(words.contains(&opening), "the part's own datums: {words:?}");
    Ok(())
}

/// Selecting a datum's row lights it on the canvas; selecting it changes
/// nothing in the document.
#[test]
fn selecting_a_row_selects_its_datum() -> TestResult {
    let engine = demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    let (_, part) = first_part(&engine)?;
    let opening = format!("{part} opening");
    let revision = engine.revision();
    assert!(click(&mut editor, &engine, &ctx, &opening)?.is_none());
    let shown = interface_card::shown(&engine)?;
    let id = shown
        .list
        .datums
        .iter()
        .find(|r| r.name == opening)
        .map(|r| r.id);
    assert_eq!(editor.interface.datum, id);
    assert_eq!(engine.revision(), revision);
    Ok(())
}
