//! The sheet's right-click menus: what each object kind offers, that a
//! right-click selects first, and that a row runs the right command.
use super::super::Tool;
use super::*;
use crate::context_menu::Run;
use egui::{Event, PointerButton, Pos2, Rect, pos2, vec2};
use pcbcraft_core::library::Rotation;
use pcbcraft_core::{Endpoint, LabelKind, Outcome, TerminalId};
use pcbcraft_ui::fonts;

/// Operations compare by their Debug text (`Operation` has no `PartialEq`).
macro_rules! assert_op {
    ($left:expr, $right:expr $(,)?) => {
        assert_eq!(format!("{:?}", $left), format!("{:?}", $right))
    };
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// One canvas frame in an 800 × 800 canvas, whose centre is the sheet's
/// origin: returns the output and the operation sent.
fn frame(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<Event>,
) -> (egui::FullOutput, Option<Operation>) {
    let mut sent = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 800.0))),
            events,
            ..Default::default()
        },
        |ui| sent = editor.canvas(ui, engine),
    );
    output.textures_delta.clear();
    (output, sent)
}

fn button(at: Pos2, which: PointerButton, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: which,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

/// Press and release `which` at `at`; returns the operations sent.
fn click(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    at: Pos2,
    which: PointerButton,
) -> Vec<Operation> {
    let mut sent = Vec::new();
    for events in [
        vec![Event::PointerMoved(at)],
        vec![button(at, which, true)],
        vec![button(at, which, false)],
    ] {
        sent.extend(frame(editor, engine, ctx, events).1);
    }
    sent
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    ctx
}

/// Every row's icon exists (an unknown name draws the placeholder).
fn check_icons(rows: &Rows<Act>) {
    for entry in rows.iter().flatten() {
        assert!(
            !pcbcraft_ui::icon_svg::resolve(entry.icon).placeholder,
            "{} has no icon {}",
            entry.label,
            entry.icon
        );
    }
}

/// The rows as (label, enabled), with `—` for a separator.
fn listed(rows: &Rows<Act>) -> Vec<(String, bool)> {
    check_icons(rows);
    rows.iter()
        .map(|row| match row {
            Some(entry) => (entry.label.clone(), entry.enabled),
            None => ("—".to_owned(), false),
        })
        .collect()
}

fn labels(rows: &Rows<Act>) -> Vec<String> {
    listed(rows).into_iter().map(|(label, _)| label).collect()
}

fn enabled(rows: &Rows<Act>, label: &str) -> Option<bool> {
    listed(rows)
        .into_iter()
        .find(|(l, _)| l == label)
        .map(|(_, on)| on)
}

/// The screen position of the text `label` in `output`.
fn text_at(output: &egui::FullOutput, label: &str) -> Result<Pos2, String> {
    crate::kit::text::tests::painted(output)
        .into_iter()
        .find(|(t, ..)| t == label)
        .map(|(_, rect, ..)| rect.center())
        .ok_or_else(|| format!("{label} is not painted"))
}

/// A symbol: Rotate, Mirror, Edit component, No connect on all open pins
/// and Delete…, with the keys of the registry's commands.
#[test]
fn a_symbol_offers_arrange_edit_no_connect_and_delete() -> TestResult {
    let engine = Engine::demo()?;
    let ctx = context();
    let mut editor = SchematicEditor::default();
    frame(&mut editor, &engine, &ctx, vec![]);
    let r1 = engine.project().instances[0].id;
    let occurrence = engine.project().occurrences_of(r1).next().ok_or("R1")?.id;
    let target = Target::Symbol {
        instance: r1,
        occurrence,
    };
    // N8: Edit › Delete has something to delete only with a selection.
    assert!(!editor.can_delete(engine.project()));
    editor.menu_select(engine.project(), target);
    assert_eq!(editor.selected_instance(), Some(r1));
    assert!(editor.can_delete(engine.project()));
    let rows = editor.menu_rows(&ctx, &engine, target);
    assert_eq!(
        labels(&rows),
        [
            "Rotate",
            "Mirror",
            "—",
            "Edit component",
            "No connect on all open pins",
            "—",
            "Delete…"
        ]
    );
    let key = |label: &str| {
        rows.iter()
            .flatten()
            .find(|e| e.label == label)
            .and_then(|e| e.key.clone())
    };
    assert_eq!(key("Rotate"), crate::keys::shown(&ctx, "schematic.rotate"));
    assert_eq!(key("Mirror"), crate::keys::shown(&ctx, "schematic.mirror"));
    assert_eq!(
        key("Delete…"),
        Some(crate::commands::Chord::key(egui::Key::Delete).shown())
    );
    assert_eq!(enabled(&rows, "Rotate"), Some(true));
    Ok(())
}

/// A pin to mark offers Mark no-connect; a marked pin Clear no-connect; a
/// wired pin keeps the row, disabled (Solidcraft dims what cannot run).
#[test]
fn a_pin_offers_its_no_connect_mark_and_start_wire() -> TestResult {
    let mut engine = Engine::demo()?;
    let ctx = context();
    let mut editor = SchematicEditor::default();
    let wired: Endpoint = engine
        .project()
        .wires
        .iter()
        .find_map(|w| w.from.terminal())
        .ok_or("a wired pin")?;
    let free = Endpoint {
        instance: wired.instance,
        terminal: TerminalId(if wired.terminal == TerminalId(1) {
            2
        } else {
            1
        }),
    };
    frame(&mut editor, &engine, &ctx, vec![]);
    let rows = editor.menu_rows(&ctx, &engine, Target::Pin(wired));
    assert_eq!(labels(&rows), ["Mark no-connect", "Start wire"]);
    assert_eq!(
        enabled(&rows, "Mark no-connect"),
        Some(false),
        "it is wired"
    );
    assert_eq!(enabled(&rows, "Start wire"), Some(true));

    let rows = editor.menu_rows(&ctx, &engine, Target::Pin(free));
    assert_eq!(enabled(&rows, "Mark no-connect"), Some(true));
    let mark = editor
        .menu_act(&engine, Act::NoConnectPin(free))
        .ok_or("the mark")?;
    assert!(matches!(
        &mark,
        Operation::Instance(pcbcraft_core::InstanceCommand::SetNoConnect {
            no_connect: true,
            ..
        })
    ));
    engine.apply(engine.revision(), mark)?;
    frame(&mut editor, &engine, &ctx, vec![]);
    let rows = editor.menu_rows(&ctx, &engine, Target::Pin(free));
    assert_eq!(enabled(&rows, "Clear no-connect"), Some(true));

    // Start wire arms the Wire tool with the wire drawn from that pin.
    assert!(editor.menu_act(&engine, Act::StartWire(free)).is_none());
    assert_eq!(editor.tool, Tool::Wire);
    assert_eq!(
        editor.wire.as_ref().map(|d| d.from),
        Some(WireTarget::Terminal(free))
    );
    Ok(())
}

/// A wire: Add junction here places one at the point under the pointer,
/// Delete… opens the review its card's Disconnect opens.
#[test]
fn a_wire_offers_a_junction_and_its_reviewed_removal() -> TestResult {
    let engine = Engine::demo()?;
    let ctx = context();
    let mut editor = SchematicEditor::default();
    frame(&mut editor, &engine, &ctx, vec![]);
    let wire = engine.project().wires[0].id;
    let at = [0.0, 0.0];
    let target = Target::Wire { wire, at };
    editor.menu_select(engine.project(), target);
    assert_eq!(editor.selected_wire, Some(wire));
    let rows = editor.menu_rows(&ctx, &engine, target);
    assert_eq!(labels(&rows), ["Add junction here", "—", "Delete…"]);
    assert_op!(
        editor.menu_act(&engine, Act::Junction(at)),
        Some(Operation::Schematic(SchematicCommand::PlaceJunction {
            sheet: Some(editor.sheet),
            position: crate::schematic_editor::pt(at),
        }))
    );
    assert!(editor.menu_act(&engine, Act::Disconnect(wire)).is_none());
    assert!(editor.update.is_open());
    Ok(())
}

/// A label: Edit gives its card's Name field the keyboard, Rotate and
/// Delete send what its keys send.
#[test]
fn a_label_offers_edit_rotate_and_delete() -> TestResult {
    let mut engine = Engine::demo()?;
    let ctx = context();
    let mut editor = SchematicEditor::default();
    let Outcome::LabelPlaced { label } = engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::PlaceLabel {
            sheet: None,
            name: "SIG".into(),
            kind: LabelKind::Local,
            position: crate::schematic_editor::pt([0.0, 30.0]),
            rotation: Rotation::R0,
            attach: None,
        }),
    )?
    else {
        return Err("a label".into());
    };
    frame(&mut editor, &engine, &ctx, vec![]);
    let item = Item::Label(label);
    let target = Target::Item(item);
    editor.menu_select(engine.project(), target);
    let rows = editor.menu_rows(&ctx, &engine, target);
    assert_eq!(labels(&rows), ["Edit", "Rotate", "—", "Delete"]);
    assert_eq!(enabled(&rows, "Rotate"), Some(true));
    assert_op!(
        editor.menu_act(&engine, Act::RotateItem),
        editor.rotated_item(engine.project())
    );
    assert!(editor.menu_act(&engine, Act::EditItem(item)).is_none());
    assert_eq!(
        editor.focus_field.as_deref(),
        Some(format!("label/{label}/name").as_str())
    );
    // The card's Name field takes it, once.
    let mut typing = false;
    let mut out = ctx.run_ui(Default::default(), |ui| {
        crate::kit::form(ui, |ui| {
            let _ = editor.inspector(ui, &engine);
        });
        typing = ui.ctx().memory(|m| m.focused().is_some());
    });
    out.textures_delta.clear();
    assert!(typing, "the Name field has the keyboard");
    assert_eq!(editor.focus_field, None);
    assert_op!(
        editor.menu_act(&engine, Act::RemoveItem(item)),
        Some(sheet_ui::removed(item))
    );
    Ok(())
}

/// A right-click selects what is under it and opens its menu; a click on
/// Rotate sends the operation ⌘R sends; an open menu leaves the canvas
/// alone, and a click on the sheet's Fit view asks the app to run it.
#[test]
fn a_right_click_selects_and_a_row_runs_its_command() -> TestResult {
    let engine = Engine::demo()?;
    let ctx = context();
    let mut editor = SchematicEditor::default();
    frame(&mut editor, &engine, &ctx, vec![]);
    let r1 = engine.project().instances[0].id;
    let occurrence = engine.project().occurrences_of(r1).next().ok_or("R1")?.id;
    // R1's body, 12 mm left of the sheet's origin at 12 points a mm.
    let body = pos2(400.0 - 12.0 * 12.0, 400.0);
    assert!(click(&mut editor, &engine, &ctx, body, PointerButton::Secondary).is_empty());
    assert_eq!(editor.selected_instance(), Some(r1), "selected first");
    assert_eq!(
        editor.menu.target(),
        Some(&Target::Symbol {
            instance: r1,
            occurrence
        })
    );
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let rotate = text_at(&output, "Rotate")?;
    let expected = editor.arranged(engine.project(), Arrange::Rotate);
    assert!(expected.is_some());
    let sent = click(&mut editor, &engine, &ctx, rotate, PointerButton::Primary);
    assert_op!(sent.first().cloned(), expected);
    assert!(!editor.menu.is_open(), "a chosen row closes it");
    assert_eq!(
        editor.selected_instance(),
        Some(r1),
        "the row did not reselect"
    );

    // A right-click on a part already selected keeps its selection.
    let before = editor.selected_occurrence;
    click(&mut editor, &engine, &ctx, body, PointerButton::Secondary);
    assert_eq!(editor.selected_occurrence, before);
    let (_, none) = frame(
        &mut editor,
        &engine,
        &ctx,
        vec![
            Event::PointerMoved(pos2(790.0, 790.0)),
            button(pos2(790.0, 790.0), PointerButton::Primary, true),
        ],
    );
    assert!(none.is_none());
    assert!(!editor.menu.is_open(), "a press outside closes it");
    frame(
        &mut editor,
        &engine,
        &ctx,
        vec![button(pos2(790.0, 790.0), PointerButton::Primary, false)],
    );

    // The empty sheet: the tools and Fit view, as registry commands, with
    // no selection of its own.
    let empty = pos2(700.0, 100.0);
    editor.select_instance(Some(r1), engine.project());
    click(&mut editor, &engine, &ctx, empty, PointerButton::Secondary);
    let target = editor.menu.target().copied().ok_or("no menu")?;
    assert!(matches!(target, Target::Sheet { .. }));
    assert_eq!(editor.selected_instance(), Some(r1), "selection kept");
    let rows = editor.menu_rows(&ctx, &engine, target);
    assert_eq!(
        labels(&rows),
        ["Place part", "Wire", "Label", "—", "Paste", "—", "Fit view"]
    );
    // Everything is a registry command but Paste, which pastes at the
    // click and has nothing to paste yet.
    for entry in rows.iter().flatten() {
        assert_eq!(entry.enabled, entry.label != "Paste", "{}", entry.label);
        assert_eq!(
            matches!(entry.run, Run::Command(_)),
            entry.label != "Paste",
            "{}",
            entry.label
        );
    }
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let fit = text_at(&output, "Fit view")?;
    click(&mut editor, &engine, &ctx, fit, PointerButton::Primary);
    assert_eq!(
        editor.take_menu_request(),
        Some(Request::Command("view.fit"))
    );
    assert_eq!(editor.take_menu_request(), None, "once");
    Ok(())
}

/// Delete… on a symbol opens the review its card's Remove… opens, and Edit
/// component asks the app to open it; No connect marks every open pin.
#[test]
fn a_symbols_rows_send_what_its_card_sends() -> TestResult {
    let engine = Engine::demo()?;
    let ctx = context();
    let mut editor = SchematicEditor::default();
    frame(&mut editor, &engine, &ctx, vec![]);
    let r1 = engine.project().instances[0].clone();
    let occurrence = engine
        .project()
        .occurrences_of(r1.id)
        .next()
        .ok_or("R1")?
        .id;
    assert!(
        editor
            .menu_act(&engine, Act::EditComponent(r1.component.id))
            .is_none()
    );
    assert_eq!(
        editor.take_menu_request(),
        Some(Request::EditComponent(r1.component.id))
    );
    let open = editor.menu_act(&engine, Act::NoConnectAll(r1.id, occurrence));
    assert!(matches!(
        open,
        Some(Operation::Instance(
            pcbcraft_core::InstanceCommand::SetNoConnect {
                no_connect: true,
                ..
            }
        ))
    ));
    assert!(editor.menu_act(&engine, Act::Remove(r1.id)).is_none());
    assert!(editor.update.is_open());
    // While a review is open, a right-click opens nothing.
    let body = pos2(400.0 - 12.0 * 12.0, 400.0);
    click(&mut editor, &engine, &ctx, body, PointerButton::Secondary);
    assert!(!editor.menu.is_open());
    Ok(())
}

/// Run 3, finding 3: ⌘R and the ribbon's Rotate turn a port on a pin a
/// quarter from the way it is drawn: its first press always shows.
#[test]
fn rotate_turns_a_port_on_a_pin_from_its_drawn_rotation() -> TestResult {
    let mut engine = Engine::demo()?;
    let ctx = context();
    let mut editor = SchematicEditor::default();
    let r1 = engine.project().instances[0].id;
    let Outcome::LabelPlaced { label } = engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::PlaceLabel {
            sheet: None,
            name: "GND".into(),
            kind: LabelKind::Power {
                style: pcbcraft_core::model::PowerStyle::Ground,
            },
            position: crate::schematic_editor::pt([0.0, 0.0]),
            rotation: Rotation::R0,
            attach: Some(WireTarget::Terminal(Endpoint {
                instance: r1,
                terminal: TerminalId(1),
            })),
        }),
    )?
    else {
        return Err("a label".into());
    };
    frame(&mut editor, &engine, &ctx, vec![]);
    editor.menu_select(engine.project(), Target::Item(Item::Label(label)));
    let tools = editor.ribbon(&engine);
    let rotate = tools
        .groups
        .iter()
        .flat_map(|g| &g.tools)
        .find(|t| t.id == "rotate")
        .ok_or("Rotate")?;
    assert!(rotate.enabled, "the ribbon's Rotate takes a port");
    let drawn = pcbcraft_core::schematic::labels::current_rotation(engine.project(), label)?;
    let Some(Operation::Schematic(SchematicCommand::EditLabel { rotation, .. })) =
        editor.on_tool("rotate", &engine)
    else {
        return Err("Rotate sends the turn".into());
    };
    assert_eq!(rotation, Some(drawn.turned(1)));
    let (_, sent) = frame(
        &mut editor,
        &engine,
        &ctx,
        vec![Event::Key {
            key: egui::Key::R,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }],
    );
    let Some(Operation::Schematic(SchematicCommand::EditLabel { rotation, .. })) = sent else {
        return Err("⌘R sends the turn".into());
    };
    assert_eq!(rotation, Some(drawn.turned(1)));
    Ok(())
}

/// Run 3, finding 7: Delete and Backspace on a wire open the review the
/// menu's Delete… opens; nothing changes until it is applied.
#[test]
fn delete_on_a_wire_opens_the_disconnect_review() -> TestResult {
    let engine = Engine::demo()?;
    let ctx = context();
    let wire = engine.project().wires[0].id;
    let before = engine.revision();
    for key in [egui::Key::Delete, egui::Key::Backspace] {
        let mut editor = SchematicEditor::default();
        frame(&mut editor, &engine, &ctx, vec![]);
        editor.menu_select(
            engine.project(),
            Target::Wire {
                wire,
                at: [0.0, 0.0],
            },
        );
        assert!(editor.can_delete(engine.project()));
        let (_, sent) = frame(
            &mut editor,
            &engine,
            &ctx,
            vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(sent.is_none(), "{key:?} sends nothing yet");
        assert!(editor.update.is_open(), "{key:?} opens the review");
        assert_eq!(engine.revision(), before);
        let same = editor.menu_act(&engine, Act::Disconnect(wire));
        assert!(same.is_none());
        assert!(editor.update.is_open());
    }
    Ok(())
}
