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

/// The ribbon's Resolve opens the card in Properties; it shows the
/// groups, Choose… opens the palette in assign mode for the first group,
/// and Close returns to the sheet.
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
    let Ok(QueryResult::ResolveGroups(report)) = engine.query(Query::ResolveGroups {}) else {
        return Err("library.resolve_groups".into());
    };
    let first = report.groups.first().ok_or("the demo has a group")?;
    assert!(shown.iter().any(|(t, _)| t == "Choose…"), "{shown:?}");
    click(&mut editor, &engine, &ctx, "Choose…")?;
    assert_eq!(
        editor.palette.mode(),
        &PaletteMode::Assign {
            instances: first.instances.clone()
        }
    );
    assert_eq!(editor.palette.query(), first.query);
    editor.palette.close();
    click(&mut editor, &engine, &ctx, "Close")?;
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
    assert_eq!(editor.header(&engine).title, "Place component");
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

/// A group whose Part disagrees says so: the card names the current Part
/// and its finding, and the BOM line leads with its state.
#[test]
fn the_resolve_card_names_the_current_part_and_why_it_is_unresolved() -> TestResult {
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
    let mut editor = SchematicEditor::default();
    editor.on_tool("resolve", &engine);
    let (shown, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let texts: Vec<&str> = shown.iter().map(|(t, _)| t.as_str()).collect();
    assert!(texts.contains(&"Current part"), "{texts:?}");
    assert!(
        texts.iter().any(|t| t.contains("is not met by")),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.starts_with("R1: unresolved")),
        "{texts:?}"
    );
    assert!(texts.contains(&"New Part…"), "{texts:?}");
    // New Part… asks the app to open the component's Parts editor with
    // the group's values.
    click(&mut editor, &engine, &ctx, "New Part…")?;
    let (component, values) = editor.take_new_part().ok_or("no request")?;
    let instance = engine.project().instance(r1).ok_or("R1")?;
    assert_eq!(component, instance.component.id);
    assert!(values.iter().any(|(f, _)| f.id == field.id));
    Ok(())
}

/// R1's pinned Part does not meet a changed value: `project.issues`
/// reports it, and locating that issue selects R1 (from another selection
/// and with the palette open) and frames it. Locate never touches the
/// document, and refs the sheet does not show locate nothing.
#[test]
fn an_issue_locates_its_instance_and_frames_it() -> TestResult {
    use crate::issues::Locate;
    use pcbcraft_core::diagnostics::ObjectRef;
    use pcbcraft_core::issues::{IssueArea, IssueScope};
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
    let QueryResult::Issues(report) = engine.query(Query::Issues {
        scope: IssueScope::Area {
            area: IssueArea::Schematic,
        },
    })?
    else {
        return Err("not an issue report".into());
    };
    let row = report
        .rows
        .iter()
        .find(|r| {
            r.finding.code.starts_with("part.")
                && r.objects().any(|o| {
                    matches!(o, ObjectRef::Instance { id } | ObjectRef::Property { instance: id, .. } if *id == r1)
                })
        })
        .ok_or("no purchasing issue on R1")?;
    let before = engine.project().clone();
    let mut editor = SchematicEditor::default();
    editor.select(Some(r2), None, engine.project());
    editor.palette.open();
    let refs: Vec<ObjectRef> = row.objects().copied().collect();
    assert!(editor.locate(&engine, &refs));
    assert_eq!(editor.selected, Some(r1));
    assert!(!editor.palette.is_open());
    assert_eq!(editor.tool, Tool::Select);
    assert!(editor.frame_request.is_some(), "R1 is framed");
    assert_eq!(engine.project(), &before);
    assert!(!editor.locate(&engine, &[ObjectRef::Track { id: 1 }]));
    assert!(!editor.locate(&engine, &[]));
    assert_eq!(editor.selected, Some(r1));
    Ok(())
}

/// Locate prefers an instance (here through a terminal) to a net, and a
/// net alone selects the net through its first wire.
#[test]
fn locate_selects_instances_through_terminals_then_nets() -> TestResult {
    use crate::issues::Locate;
    use pcbcraft_core::diagnostics::ObjectRef;
    let engine = Engine::demo()?;
    let net = engine
        .project()
        .nets
        .iter()
        .find(|n| n.terminals.len() >= 2)
        .ok_or("no wired net")?;
    let endpoint = *net.terminals.last().ok_or("no terminal")?;
    let mut editor = SchematicEditor::default();
    assert!(editor.locate(
        &engine,
        &[
            ObjectRef::Net { id: net.id },
            ObjectRef::Terminal { endpoint }
        ],
    ));
    assert_eq!(editor.selected, Some(endpoint.instance));
    let mut editor = SchematicEditor::default();
    assert!(editor.locate(&engine, &[ObjectRef::Net { id: net.id }]));
    assert_eq!(editor.selected, None);
    assert_eq!(editor.selected_net(&engine), Some(net.id));
    assert_eq!(editor.header(&engine).kind, "Net");
    Ok(())
}

/// A property commit that leaves R1's purchasing unresolved says so once,
/// as a To do toast with Undo.
#[test]
fn a_commit_that_unresolves_purchasing_posts_an_undo_toast() -> TestResult {
    use crate::kit::severity::Level;
    use crate::kit::toast::{ToastAction, take_posted};
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let (mut engine, _, r1, _) = engine_with_part()?;
    let mut editor = SchematicEditor::default();
    editor.select_instance(Some(r1), engine.project());
    let operation =
        edit_field(&mut editor, &engine, &ctx, "10 kΩ", "22k")?.ok_or("the edit sent nothing")?;
    assert!(
        take_posted(&ctx).is_empty(),
        "nothing is said before it applies"
    );
    engine.apply(engine.revision(), operation)?;
    frame(&mut editor, &engine, &ctx, vec![]);
    let posted = take_posted(&ctx);
    let [toast] = &posted[..] else {
        return Err(format!("expected one toast, got {posted:?}").into());
    };
    assert_eq!(toast.level, Level::ToDo);
    assert_eq!(toast.text, "Purchasing unresolved for R1");
    assert_eq!(toast.actions, [ToastAction::Undo]);
    frame(&mut editor, &engine, &ctx, vec![]);
    assert!(take_posted(&ctx).is_empty(), "said once");
    Ok(())
}

/// Focus the field showing `shown`, replace its text with `text` and press
/// Enter; returns the operation of that frame.
fn edit_field(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    shown: &str,
    text: &str,
) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
    click(editor, engine, ctx, shown)?;
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let select_all = egui::Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    };
    frame(
        editor,
        engine,
        ctx,
        vec![select_all, egui::Event::Text(text.into())],
    );
    Ok(frame(editor, engine, ctx, vec![key(egui::Key::Enter)]).1)
}
