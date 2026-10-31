//! Schematic tests of manufacturer parts: the Part row, assign mode and
//! the Resolve parts card, driven through the engine.
use super::*;
use crate::resolve_parts::demo_part;
use pcbcraft_core::format::{EditorFile, FILE_VERSION};
use pcbcraft_core::{Error, PartRef};
use solidcraft_ui_egui::fonts;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn frame(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (Vec<(String, Pos2)>, Option<Operation>) {
    let mut command = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700.0, 1800.0))),
            events,
            ..Default::default()
        },
        |ui| command = kit::form(ui, |ui| editor.inspector(ui, engine)),
    );
    output.textures_delta.clear();
    fn texts(shape: &egui::Shape, out: &mut Vec<(String, Pos2)>) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.job.text.clone(),
                text.pos + text.galley.size() * 0.5,
            )),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for shape in &output.shapes {
        texts(&shape.shape, &mut out);
    }
    (out, command)
}

/// Click the text `label`; returns the operation of the release frame.
fn click(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    label: &str,
) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
    let (shown, _) = frame(editor, engine, ctx, vec![]);
    let point = shown
        .iter()
        .find(|(text, _)| text == label)
        .map(|(_, p)| *p)
        .ok_or_else(|| format!("missing {label}: {shown:?}"))?;
    let press = |pressed| egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(
        editor,
        engine,
        ctx,
        vec![egui::Event::PointerMoved(point), press(true)],
    );
    Ok(frame(editor, engine, ctx, vec![press(false)]).1)
}

/// The demo with an `ExampleCo` Part published for R1's component and
/// pinned on R1 in the saved file, as a reopened project holds it.
fn engine_with_part() -> Result<(Engine, PartRef, Id, Id), Box<dyn std::error::Error>> {
    let mut engine = Engine::demo()?;
    let part = demo_part(&mut engine, "EX-R0603-10K0-F")?;
    let mut project = (*engine.document().project).clone();
    let r1 = project
        .instances
        .iter()
        .position(|i| i.designator == "R1")
        .ok_or("no R1")?;
    project.instances[r1].part = Some(part);
    let r1 = project.instances[r1].id;
    let r2 = project.instance_by_designator("R2").ok_or("no R2")?.id;
    let engine = Engine::from_file(EditorFile {
        version: FILE_VERSION,
        project,
        drafts: (*engine.document().drafts).clone(),
    })?;
    Ok((engine, part, r1, r2))
}

/// The Part row names the pinned Part; Choose… opens the palette in assign
/// mode for the instance with its query; Clear sends one
/// `instance.assign_part` without a Part, which undo reverts.
#[test]
fn the_part_row_chooses_in_assign_mode_and_clears_through_the_engine() -> TestResult {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let (mut engine, part, r1, _) = engine_with_part()?;
    let mut editor = SchematicEditor::default();
    editor.select_instance(Some(r1), engine.project());
    let (shown, _) = frame(&mut editor, &engine, &ctx, vec![]);
    assert!(
        shown
            .iter()
            .any(|(t, _)| t == "ExampleCo EX-R0603-10K0-F · r1"),
        "{shown:?}"
    );
    click(&mut editor, &engine, &ctx, "Choose…")?;
    assert!(editor.palette.is_open());
    assert_eq!(
        editor.palette.mode(),
        &PaletteMode::Assign {
            instances: vec![r1]
        }
    );
    assert_eq!(
        editor.palette.query(),
        crate::resolve_parts::query_for(&engine, r1)
    );
    assert!(editor.palette.query().starts_with("kind:part"));
    assert_eq!(editor.header(&engine).title, "Choose part");
    // Esc closes the palette without a choice.
    editor.palette.close();
    let operation = click(&mut editor, &engine, &ctx, "Clear")?.ok_or("Clear sent nothing")?;
    assert!(matches!(
        &operation,
        Operation::Instance(InstanceCommand::AssignPart { instances, part: None })
            if instances == &[r1]
    ));
    engine.apply(engine.revision(), operation)?;
    assert_eq!(engine.project().instance(r1).and_then(|i| i.part), None);
    engine.undo()?;
    assert_eq!(
        engine.project().instance(r1).and_then(|i| i.part),
        Some(part)
    );
    Ok(())
}

/// One group assignment is one command: every instance gets the Part, or
/// (while the engine does not compare Parts yet) none does.
#[test]
fn a_group_assignment_is_one_atomic_command() -> TestResult {
    let (mut engine, part, r1, r2) = engine_with_part()?;
    engine.apply(
        engine.revision(),
        crate::resolve_parts::assign(vec![r1], None),
    )?;
    let before = engine.document().project.clone();
    let revision = engine.revision();
    let result = engine.apply(
        engine.revision(),
        crate::resolve_parts::assign(vec![r1, r2], Some(part)),
    );
    let pinned = |engine: &Engine, id| engine.project().instance(id).and_then(|i| i.part);
    match result {
        // The contract stub (S2 task 1 not merged): nothing changes.
        Err(Error::Unsupported(_)) => {
            assert_eq!(*engine.document().project, *before);
            assert_eq!(engine.revision(), revision);
        }
        Ok(_) => {
            assert_eq!(pinned(&engine, r1), Some(part));
            assert_eq!(pinned(&engine, r2), Some(part));
            engine.undo()?;
            assert_eq!(pinned(&engine, r1), None);
            assert_eq!(pinned(&engine, r2), None);
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// The ribbon's Resolve opens the card in Properties; it shows the
/// groups or what is unavailable, and Close returns to the sheet.
#[test]
fn resolve_opens_the_card_and_close_returns_to_the_sheet() -> TestResult {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let engine = Engine::demo()?;
    let mut editor = SchematicEditor::default();
    let ribbon = editor.ribbon(&engine);
    assert!(
        ribbon
            .groups
            .iter()
            .flat_map(|g| &g.tools)
            .any(|t| t.id == "resolve")
    );
    editor.on_tool("resolve", &engine);
    assert_eq!(editor.header(&engine).title, "Resolve parts");
    let (shown, _) = frame(&mut editor, &engine, &ctx, vec![]);
    match engine.query(Query::ResolveGroups {}) {
        Err(Error::Unsupported(_)) => {
            assert!(shown.iter().any(|(t, _)| t.contains("not available")));
        }
        Ok(QueryResult::ResolveGroups(report)) => {
            let first = report.groups.first().ok_or("the demo has a group")?;
            click(&mut editor, &engine, &ctx, "Choose…")?;
            assert_eq!(
                editor.palette.mode(),
                &PaletteMode::Assign {
                    instances: first.instances.clone()
                }
            );
            assert_eq!(editor.palette.query(), first.query);
            editor.palette.close();
        }
        other => return Err(format!("{other:?}").into()),
    }
    click(&mut editor, &engine, &ctx, "Close")?;
    assert!(!editor.resolve.is_open());
    assert_eq!(editor.header(&engine).title, "Sheet");
    Ok(())
}
