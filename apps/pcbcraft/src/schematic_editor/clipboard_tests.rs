//! The sheet's Cut, Copy and Paste as the app runs them: enable state from
//! the selection and buffer, paste at the pointer or moved on from the
//! source, the result selected, one undo step, and the review a cut opens.
use super::super::Tool;
use super::*;
use egui::{Event, Pos2, Rect, pos2, vec2};
use pcbcraft_core::Outcome;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

/// One canvas frame in an 800 × 800 canvas, whose centre is the sheet's
/// origin.
fn frame(editor: &mut SchematicEditor, engine: &Engine, ctx: &egui::Context, events: Vec<Event>) {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 800.0))),
            events,
            ..Default::default()
        },
        |ui| {
            let _ = editor.canvas(ui, engine);
        },
    );
    output.textures_delta.clear();
}

fn editor_on_demo()
-> std::result::Result<(SchematicEditor, Engine, egui::Context), Box<dyn std::error::Error>> {
    let engine = Engine::demo()?;
    let ctx = egui::Context::default();
    solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
    let mut editor = SchematicEditor::default();
    frame(&mut editor, &engine, &ctx, vec![]);
    Ok((editor, engine, ctx))
}

/// Apply as the app does, selecting what a paste created.
fn apply(editor: &mut SchematicEditor, engine: &mut Engine, operation: Operation) -> TestResult {
    let revision = engine.revision();
    if let Outcome::Pasted { report } = engine.apply(revision, operation)? {
        editor.select_pasted(engine.project(), &report);
    }
    Ok(())
}

/// Copy needs a copyable selection, Paste a buffer; a copied symbol pastes
/// as a new instance of its component, selected, in one undo step.
#[test]
fn copy_then_paste_makes_a_selected_new_instance_in_one_undo_step() -> TestResult {
    let (mut editor, mut engine, ctx) = editor_on_demo()?;
    let r1 = engine.project().instances[0].id;
    assert!(editor.can_copy(engine.project()).is_err(), "no selection");
    assert!(editor.can_paste().is_err(), "nothing copied");
    editor.select_instance(Some(r1), engine.project());
    editor.can_copy(engine.project())?;
    editor.copy_selection(&ctx, &engine)?;
    editor.can_paste()?;
    let count = engine.project().instances.len();
    let component = engine.project().instance(r1).map(|i| i.component);
    let operation = editor.paste_at(&engine, None).ok_or("no paste")?;
    apply(&mut editor, &mut engine, operation)?;
    let project = engine.project();
    assert_eq!(project.instances.len(), count + 1);
    let new = project.instances.last().ok_or("no instance")?;
    assert_eq!(Some(new.component), component);
    assert_ne!(Some(new.id), Some(r1));
    assert_eq!(editor.selected_instance(), Some(new.id), "left selected");
    engine.undo()?;
    assert_eq!(engine.project().instances.len(), count, "one undo step");
    Ok(())
}

/// ⌘V pastes at the pointer when it is over the canvas; off the canvas, or
/// again at the same point, it moves on, so nothing lands exactly on what it
/// was copied from. The `schematic.paste` offset says where.
#[test]
fn paste_goes_to_the_pointer_or_moves_on_from_the_last_one() -> TestResult {
    let (mut editor, engine, ctx) = editor_on_demo()?;
    let r1 = engine.project().instances[0].id;
    editor.select_instance(Some(r1), engine.project());
    editor.copy_selection(&ctx, &engine)?;
    let anchor = engine
        .project()
        .occurrences_of(r1)
        .next()
        .ok_or("R1")?
        .position
        .to_mm_f64();
    let grid = engine.drafts().grid.symbol_mm;
    let offset = |operation: Option<Operation>| match operation {
        Some(Operation::Schematic(SchematicCommand::Paste { offset, .. })) => Some(offset),
        _ => None,
    };
    // Off the canvas: down and to the right of the source.
    assert!(editor.pointer.is_none());
    let first = offset(editor.paste_at_pointer(&engine)).ok_or("no paste")?;
    assert_eq!(
        first,
        crate::schematic_editor::pt([2.0 * grid, -2.0 * grid])
    );
    let second = offset(editor.paste_at_pointer(&engine)).ok_or("no paste")?;
    assert_eq!(
        second,
        crate::schematic_editor::pt([4.0 * grid, -4.0 * grid]),
        "moves on"
    );
    // Over the canvas: the pointer's snapped sheet point.
    frame(
        &mut editor,
        &engine,
        &ctx,
        vec![Event::PointerMoved(pos2(500.0, 300.0))],
    );
    let at = editor.pointer.ok_or("the pointer is over the canvas")?;
    let shifted = offset(editor.paste_at_pointer(&engine)).ok_or("no paste")?;
    assert_eq!(
        shifted,
        crate::schematic_editor::pt([at[0] - anchor[0], at[1] - anchor[1]])
    );
    let again = offset(editor.paste_at_pointer(&engine)).ok_or("no paste")?;
    assert_ne!(again, shifted, "the same point again moves on");
    Ok(())
}

/// A paste while a tool is armed ends the tool, so its result is selected.
#[test]
fn paste_returns_to_the_select_tool() -> TestResult {
    let (mut editor, mut engine, ctx) = editor_on_demo()?;
    let r1 = engine.project().instances[0].id;
    editor.select_instance(Some(r1), engine.project());
    editor.copy_selection(&ctx, &engine)?;
    let _ = editor.on_tool("wire", &engine);
    assert_eq!(editor.tool, Tool::Wire);
    let operation = editor.paste_at(&engine, None).ok_or("no paste")?;
    assert_eq!(editor.tool, Tool::Select);
    apply(&mut editor, &mut engine, operation)?;
    assert!(editor.selected_instance().is_some());
    Ok(())
}

/// Cutting a label removes it at once and it pastes back; a placed symbol
/// goes through the review Delete opens, leaving the document as it was, and
/// nothing else cuts or pastes until the review ends.
#[test]
fn cut_removes_as_delete_does() -> TestResult {
    let (mut editor, mut engine, ctx) = editor_on_demo()?;
    let label = match engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::PlaceLabel {
            sheet: None,
            name: "CUTME".into(),
            kind: pcbcraft_core::LabelKind::Local,
            position: crate::schematic_editor::pt([0.0, 40.0]),
            rotation: Default::default(),
            attach: None,
        }),
    )? {
        Outcome::LabelPlaced { label } => label,
        other => return Err(format!("unexpected {other:?}").into()),
    };
    editor.select_item(Some(Item::Label(label)), engine.project());
    let remove = editor.cut_selection(&ctx, &engine)?.ok_or("no removal")?;
    apply(&mut editor, &mut engine, remove)?;
    assert!(engine.project().label(label).is_none());
    let operation = editor.paste_at(&engine, None).ok_or("no paste")?;
    apply(&mut editor, &mut engine, operation)?;
    assert!(engine.project().labels.iter().any(|l| l.name == "CUTME"));

    let r1 = engine.project().instances[0].id;
    let before = engine.project().clone();
    editor.select_instance(Some(r1), engine.project());
    assert!(editor.cut_selection(&ctx, &engine)?.is_none());
    assert!(editor.update.is_open(), "the review");
    assert_eq!(engine.project(), &before);
    assert!(editor.can_cut(engine.project()).is_err());
    assert!(editor.can_paste().is_err());
    Ok(())
}

/// What the sheet cannot copy says so, and a refused paste keeps the
/// document: a sheet symbol has no copy, and a clip from another project
/// names a component this one does not hold.
#[test]
fn what_cannot_be_copied_or_pasted_says_so() -> TestResult {
    let (mut editor, mut engine, ctx) = editor_on_demo()?;
    let symbol = match engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::PlaceSheetSymbol {
            sheet: None,
            child: None,
            name: Some("Child".into()),
            position: crate::schematic_editor::pt([0.0, 60.0]),
            size: crate::schematic_editor::pt([20.0, 10.0]),
        }),
    )? {
        Outcome::SheetSymbolPlaced { symbol, .. } => symbol,
        other => return Err(format!("unexpected {other:?}").into()),
    };
    editor.select_item(Some(Item::SheetSymbol(symbol)), engine.project());
    assert!(editor.can_copy(engine.project()).is_err());
    assert!(editor.can_cut(engine.project()).is_err());
    assert!(editor.cut_selection(&ctx, &engine).is_err());
    editor.select_item(None, engine.project());
    let r1 = engine.project().instances[0].id;
    editor.select_instance(Some(r1), engine.project());
    editor.copy_selection(&ctx, &engine)?;
    let operation = editor.paste_at(&engine, None).ok_or("no paste")?;
    let mut other = Engine::default();
    let revision = other.revision();
    let before = other.project().clone();
    assert!(other.apply(revision, operation).is_err());
    assert_eq!(other.project(), &before);
    assert_eq!(other.revision(), revision);
    Ok(())
}

/// The empty sheet's Paste row is enabled once something is copied, shows
/// the key, and pastes at the point clicked.
#[test]
fn the_sheet_menu_pastes_where_it_was_opened() -> TestResult {
    use super::super::menu::{Act, Target};
    let (mut editor, engine, ctx) = editor_on_demo()?;
    let at = [50.0, 60.0];
    let paste_row = |editor: &mut SchematicEditor| {
        editor
            .menu_rows(&ctx, &engine, Target::Sheet { at })
            .into_iter()
            .flatten()
            .find(|row| row.label == "Paste")
    };
    let row = paste_row(&mut editor).ok_or("no Paste row")?;
    assert!(!row.enabled, "nothing copied");
    assert_eq!(row.key, crate::keys::shown(&ctx, "edit.paste"));
    let r1 = engine.project().instances[0].id;
    editor.select_instance(Some(r1), engine.project());
    editor.copy_selection(&ctx, &engine)?;
    assert!(paste_row(&mut editor).ok_or("no Paste row")?.enabled);
    let anchor = engine
        .project()
        .occurrences_of(r1)
        .next()
        .ok_or("R1")?
        .position
        .to_mm_f64();
    match editor.menu_act(&engine, Act::Paste(at)) {
        Some(Operation::Schematic(SchematicCommand::Paste { offset, .. })) => {
            assert_eq!(
                offset,
                crate::schematic_editor::pt([at[0] - anchor[0], at[1] - anchor[1]])
            );
        }
        other => return Err(format!("unexpected {other:?}").into()),
    }
    Ok(())
}
