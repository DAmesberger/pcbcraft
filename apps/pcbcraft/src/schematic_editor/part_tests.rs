//! Schematic tests of manufacturer parts: the Part row, assign mode and
//! the Resolve parts card, driven through the engine.
use super::*;
use crate::kit;
use crate::resolve_parts::demo_part;
use pcbcraft_core::PartRef;
use pcbcraft_core::fields::{PropertyEdit, PropertyInput};
use pcbcraft_core::format::{EditorFile, FILE_VERSION};
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

/// One group assignment is one command and one undo step: every instance
/// gets the Part.
#[test]
fn a_group_assignment_is_one_atomic_command() -> TestResult {
    let (mut engine, part, r1, r2) = engine_with_part()?;
    engine.apply(
        engine.revision(),
        crate::resolve_parts::assign(vec![r1], None),
    )?;
    engine.apply(
        engine.revision(),
        crate::resolve_parts::assign(vec![r1, r2], Some(part)),
    )?;
    let pinned = |engine: &Engine, id| engine.project().instance(id).and_then(|i| i.part);
    assert_eq!(pinned(&engine, r1), Some(part));
    assert_eq!(pinned(&engine, r2), Some(part));
    engine.undo()?;
    assert_eq!(pinned(&engine, r1), None);
    assert_eq!(pinned(&engine, r2), None);
    Ok(())
}

/// One frame of the Resolve parts window at the audit's narrow size.
fn window_frame(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> Vec<(String, Pos2)> {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(860.0, 1081.0))),
            events,
            ..Default::default()
        },
        |ui| editor.resolve_window(ui.ctx(), engine),
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
    out
}

/// Click the text `label` in the Resolve parts window.
fn window_click(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    label: &str,
) -> TestResult {
    let mut shown = Vec::new();
    for _ in 0..3 {
        shown = window_frame(editor, engine, ctx, vec![]);
    }
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
    window_frame(
        editor,
        engine,
        ctx,
        vec![egui::Event::PointerMoved(point), press(true)],
    );
    window_frame(editor, engine, ctx, vec![press(false)]);
    Ok(())
}

/// The ribbon's Resolve opens the Resolve parts window and leaves the
/// Properties card on the sheet; Choose… opens the docked Choose part
/// dialog for the first group beside the window, and Close closes it.
#[test]
fn resolve_opens_the_window_and_choose_docks_the_dialog() -> TestResult {
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
    assert!(editor.resolve.is_open());
    assert_eq!(editor.header(&engine).title, "Sheet");
    assert!(!editor.dialog_open());
    let Ok(QueryResult::ResolveGroups(report)) = engine.query(Query::ResolveGroups {}) else {
        return Err("library.resolve_groups".into());
    };
    let first = report.groups.first().ok_or("the demo has a group")?;
    window_click(&mut editor, &engine, &ctx, "Choose…")?;
    assert_eq!(
        editor.palette.mode(),
        &PaletteMode::Assign {
            instances: first.instances.clone()
        }
    );
    assert_eq!(editor.palette.query(), first.query);
    assert!(editor.dialog_open());
    assert!(
        editor.resolve.is_open(),
        "the table returns after the dialog"
    );
    // The window steps aside while the dialog is open.
    let hidden = window_frame(&mut editor, &engine, &ctx, vec![]);
    assert!(
        !hidden.iter().any(|(t, _)| t == "Resolve parts"),
        "{hidden:?}"
    );
    editor.palette.close();
    window_click(&mut editor, &engine, &ctx, "Close")?;
    assert!(!editor.resolve.is_open());
    assert_eq!(editor.header(&engine).title, "Sheet");
    Ok(())
}

/// Place takes the card over from Resolve parts: only one ribbon tool is
/// active and the properties show the palette, not the Resolve card.
#[test]
fn place_closes_resolve_parts() -> TestResult {
    let engine = Engine::demo()?;
    let mut editor = SchematicEditor::default();
    editor.on_tool("resolve", &engine);
    assert!(editor.resolve.is_open());
    editor.on_tool("place", &engine);
    assert!(!editor.resolve.is_open());
    assert!(editor.palette.is_open());
    assert_eq!(editor.header(&engine).title, "Place part");
    let ribbon = editor.ribbon(&engine);
    let active: Vec<&str> = ribbon
        .groups
        .iter()
        .flat_map(|g| &g.tools)
        .filter(|t| t.selected)
        .map(|t| t.id)
        .collect();
    assert_eq!(active, ["place"]);
    Ok(())
}

/// A group whose Part disagrees says so: its row names the current Part
/// and, open, core's own title for why it is unresolved. New Part… asks
/// the app for a draft rated at the group's values.
#[test]
fn the_resolve_window_names_the_current_part_and_why_it_is_unresolved() -> TestResult {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let (mut engine, _, r1, _) = engine_with_part()?;
    let field = engine
        .project()
        .instance(r1)
        .and_then(|i| instances::requirements(&engine.project().library, i).ok())
        .and_then(|r| r.first().map(|r| r.field))
        .ok_or("R1 has a value")?;
    engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::SetProperties {
            instance: r1,
            edits: vec![PropertyEdit {
                field: field.id,
                input: PropertyInput::Text("22k".into()),
            }],
        }),
    )?;
    let Ok(QueryResult::Issues(issues)) = engine.query(Query::Issues {
        scope: pcbcraft_core::issues::IssueScope::Selection {
            objects: vec![pcbcraft_core::diagnostics::ObjectRef::Instance { id: r1 }],
        },
    }) else {
        return Err("project.issues".into());
    };
    let title = issues
        .rows
        .iter()
        .find(|r| r.group.code.starts_with("part."))
        .map(|r| r.title.clone())
        .ok_or("R1 has a purchasing issue")?;
    let mut editor = SchematicEditor::default();
    editor.on_tool("resolve", &engine);
    let mut shown = Vec::new();
    for _ in 0..3 {
        shown = window_frame(&mut editor, &engine, &ctx, vec![]);
    }
    let texts: Vec<&str> = shown.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        texts.contains(&"ExampleCo EX-R0603-10K0-F · r1"),
        "{texts:?}"
    );
    // The first row is open: R1 sorts first.
    assert!(texts.contains(&title.as_str()), "{title} in {texts:?}");
    assert!(texts.contains(&"New Part…"), "{texts:?}");
    window_click(&mut editor, &engine, &ctx, "New Part…")?;
    let (component, values) = editor.take_new_part().ok_or("no request")?;
    let instance = engine.project().instance(r1).ok_or("R1")?;
    assert_eq!(component, instance.component.id);
    assert!(values.iter().any(|(f, _)| f.id == field.id));
    assert!(
        !editor.resolve.is_open(),
        "New Part… leaves for the Component"
    );
    Ok(())
}

/// B-b: a Checks row is a button that selects the instance it names. R1's
/// pinned Part does not meet a changed value, an advisory purchasing row.
#[test]
fn a_check_row_click_selects_the_named_instance() -> TestResult {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let (mut engine, _, r1, r2) = engine_with_part()?;
    let field = engine
        .project()
        .instance(r1)
        .and_then(|i| instances::requirements(&engine.project().library, i).ok())
        .and_then(|r| r.first().map(|r| r.field))
        .ok_or("R1 has a value")?;
    engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::SetProperties {
            instance: r1,
            edits: vec![PropertyEdit {
                field: field.id,
                input: PropertyInput::Text("22k".into()),
            }],
        }),
    )?;
    let message = engine
        .project()
        .instance(r1)
        .and_then(|i| {
            instances::part_findings(&engine.project().library, i)
                .into_iter()
                .next()
        })
        .map(|f| f.message)
        .ok_or("R1 has no purchasing finding")?;
    let mut editor = SchematicEditor {
        show_checks: true,
        ..Default::default()
    };
    editor.select(Some(r2), None, engine.project());
    assert!(click(&mut editor, &engine, &ctx, &message)?.is_none());
    assert_eq!(editor.selected, Some(r1));
    Ok(())
}

#[test]
fn a_check_row_locates_instances_through_terminals_then_nets() {
    use pcbcraft_core::diagnostics::ObjectRef;
    let terminal = ObjectRef::Terminal {
        endpoint: Endpoint {
            instance: 7,
            terminal: pcbcraft_core::TerminalId(1),
        },
    };
    assert_eq!(
        locate(&[ObjectRef::Net { id: 3 }, terminal]),
        Some(Locate::Instance(7))
    );
    assert_eq!(locate(&[ObjectRef::Net { id: 3 }]), Some(Locate::Net(3)));
    assert_eq!(locate(&[]), None);
}
