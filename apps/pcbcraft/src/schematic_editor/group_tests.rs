//! Multi-select on the sheet as a person does it: rectangle, ⇧-click, ⌘A,
//! then group drag, rotate, delete through the review, copy and paste and the
//! right-click menu, each one undo step.
use super::super::Subject;
use super::*;
use crate::context_menu::Run;
use crate::schematic_editor::menu::{Act, Target};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use pcbcraft_core::library::Rotation;
use pcbcraft_core::{Endpoint, Outcome, PowerStyle, TerminalId};
use solidcraft_ui_egui::fonts;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// One canvas frame in an 800 × 800 canvas: its centre is the sheet's origin,
/// at 12 px/mm.
fn frame(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<Event>,
    modifiers: Modifiers,
) -> Option<Operation> {
    let mut sent = None;
    let mut all = vec![Event::ModifiersChanged(modifiers)];
    all.extend(events);
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 800.0))),
            events: all,
            ..Default::default()
        },
        |ui| sent = editor.canvas(ui, engine),
    );
    output.textures_delta.clear();
    sent
}

fn button(at: Pos2, which: PointerButton, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton {
        pos: at,
        button: which,
        pressed,
        modifiers,
    }
}

fn click(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    at: Pos2,
    which: PointerButton,
    modifiers: Modifiers,
) {
    for events in [
        vec![Event::PointerMoved(at)],
        vec![button(at, which, true, modifiers)],
        vec![button(at, which, false, modifiers)],
    ] {
        frame(editor, engine, ctx, events, modifiers);
    }
}

fn drag(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    from: Pos2,
    to: Pos2,
    modifiers: Modifiers,
) -> Vec<Operation> {
    let mut sent = Vec::new();
    for events in [
        vec![Event::PointerMoved(from)],
        vec![button(from, PointerButton::Primary, true, modifiers)],
        vec![Event::PointerMoved(from + (to - from) * 0.5)],
        vec![Event::PointerMoved(to)],
        vec![button(to, PointerButton::Primary, false, modifiers)],
    ] {
        sent.extend(frame(editor, engine, ctx, events, modifiers));
    }
    sent
}

fn press(key: egui::Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// The screen point of a sheet point.
fn screen(p: Point) -> Pos2 {
    pos2(400.0 + p[0] as f32 * 12.0, 400.0 - p[1] as f32 * 12.0)
}

struct Sheet {
    editor: SchematicEditor,
    engine: Engine,
    ctx: egui::Context,
    r1: Id,
    r2: Id,
    /// The demo's wire R1.2 to R2.1, straight along y = 0.
    wire: Id,
}

fn sheet() -> Result<Sheet, Box<dyn std::error::Error>> {
    let engine = Engine::demo()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = SchematicEditor::default();
    frame(&mut editor, &engine, &ctx, vec![], Modifiers::NONE);
    let project = engine.project();
    let (r1, r2) = (project.instances[0].id, project.instances[1].id);
    let wire = project.wires[0].id;
    Ok(Sheet {
        editor,
        engine,
        ctx,
        r1,
        r2,
        wire,
    })
}

/// A third resistor at (12, 15), wired from R2's pin 2 with a corner, so a
/// group of R1 and R2 has one wire inside it and one leaving it.
fn with_r3(s: &mut Sheet) -> Result<(Id, Id), Box<dyn std::error::Error>> {
    let like = s.engine.project().instances[1].clone();
    let Outcome::InstancePlaced { instance: r3, .. } = s.engine.apply(
        s.engine.revision(),
        Operation::Instance(pcbcraft_core::InstanceCommand::Place(
            pcbcraft_core::instances::NewInstance {
                sheet: None,
                component: like.component,
                designator: None,
                properties: Vec::new(),
                representation: like.representation,
                unit: pcbcraft_core::UnitId(1),
                position: [30.0, 15.0],
                physical: like.physical,
            },
        )),
    )?
    else {
        return Err("no instance".into());
    };
    let terminal = |instance, t| Endpoint {
        instance,
        terminal: TerminalId(t),
    };
    s.engine.apply(
        s.engine.revision(),
        op(SchematicCommand::Connect {
            from: terminal(s.r2, 2).into(),
            to: terminal(r3, 1).into(),
        }),
    )?;
    let outer = s
        .engine
        .project()
        .wires
        .iter()
        .map(|w| w.id)
        .find(|id| *id != s.wire)
        .ok_or("no wire")?;
    Ok((r3, outer))
}

fn op(command: SchematicCommand) -> Operation {
    Operation::Schematic(command)
}

fn apply(s: &mut Sheet, operation: Operation) -> Result<Outcome, Box<dyn std::error::Error>> {
    let outcome = s.engine.apply(s.engine.revision(), operation)?;
    if let Outcome::Pasted { report } = &outcome {
        s.editor.select_pasted(s.engine.project(), report);
    }
    frame(&mut s.editor, &s.engine, &s.ctx, vec![], Modifiers::NONE);
    Ok(outcome)
}

fn group_of(objects: &[ClipObject]) -> Group {
    let mut group = Group::default();
    for object in objects {
        group.insert(*object);
    }
    group
}

/// Left to right takes what is wholly inside; right to left what it touches;
/// ⇧ adds; a rectangle over nothing clears. One object is the single
/// selection.
#[test]
fn a_rectangle_picks_by_solidcrafts_rule() -> TestResult {
    let Sheet {
        mut editor,
        engine,
        ctx,
        r1,
        r2,
        wire,
    } = sheet()?;
    let none = Modifiers::NONE;
    let (r1_at, r2_at) = (screen([-12.0, 0.0]), screen([12.0, 0.0]));
    let around = vec2(112.0, 100.0);
    // Left to right around R1: R1 only, not the wire that leaves it.
    drag(
        &mut editor,
        &engine,
        &ctx,
        r1_at - around,
        r1_at + around,
        none,
    );
    assert_eq!(editor.selected, Some(r1));
    assert!(editor.group.is_empty(), "one object: the single selection");
    // Left to right around everything: both symbols and the wire.
    drag(
        &mut editor,
        &engine,
        &ctx,
        r1_at - around,
        r2_at + around,
        none,
    );
    assert_eq!(
        editor.group,
        group_of(&[
            ClipObject::Instance { id: r1 },
            ClipObject::Instance { id: r2 },
            ClipObject::Wire { id: wire },
        ])
    );
    assert_eq!(editor.subject(engine.project()), Subject::Group);
    assert_eq!(editor.group.summary(engine.project()), "2 symbols, 1 wire");
    // Right to left across the wire and over R2: both touched. It starts on
    // empty canvas; a start on a selected object would move the group.
    drag(
        &mut editor,
        &engine,
        &ctx,
        r2_at + vec2(0.0, 80.0),
        screen([0.0, 0.0]) + vec2(0.0, -30.0),
        none,
    );
    assert_eq!(
        editor.group,
        group_of(&[
            ClipObject::Instance { id: r2 },
            ClipObject::Wire { id: wire },
        ])
    );
    // The same rectangle left to right holds nothing wholly: it clears.
    drag(
        &mut editor,
        &engine,
        &ctx,
        screen([0.0, 0.0]) + vec2(0.0, -30.0),
        r2_at + vec2(0.0, 80.0),
        none,
    );
    assert_eq!(editor.selected, None);
    assert_eq!(editor.selected_wire, None);
    assert!(editor.group.is_empty());
    // ⇧ adds a second rectangle to the first.
    drag(
        &mut editor,
        &engine,
        &ctx,
        r1_at - around,
        r1_at + around,
        none,
    );
    drag(
        &mut editor,
        &engine,
        &ctx,
        r2_at - around,
        r2_at + around,
        Modifiers::SHIFT,
    );
    assert_eq!(
        editor.group,
        group_of(&[
            ClipObject::Instance { id: r1 },
            ClipObject::Instance { id: r2 },
        ])
    );
    Ok(())
}

/// ⇧-click adds and removes; a plain click on empty canvas clears; a plain
/// click on a group's object selects that object alone.
#[test]
fn shift_click_toggles_and_a_plain_click_clears() -> TestResult {
    let Sheet {
        mut editor,
        engine,
        ctx,
        r1,
        r2,
        wire,
    } = sheet()?;
    let left = PointerButton::Primary;
    let (r1_at, r2_at) = (screen([-12.0, 0.0]), screen([12.0, 0.0]));
    let shift = Modifiers::SHIFT;
    click(&mut editor, &engine, &ctx, r1_at, left, shift);
    assert_eq!(editor.selected, Some(r1), "the first ⇧-click selects it");
    click(&mut editor, &engine, &ctx, r2_at, left, shift);
    click(&mut editor, &engine, &ctx, screen([0.0, 0.0]), left, shift);
    assert_eq!(
        editor.group,
        group_of(&[
            ClipObject::Instance { id: r1 },
            ClipObject::Instance { id: r2 },
            ClipObject::Wire { id: wire },
        ])
    );
    // Out again, one at a time, down to the single selection.
    click(&mut editor, &engine, &ctx, r2_at, left, shift);
    assert_eq!(editor.group.len(), 2);
    click(&mut editor, &engine, &ctx, screen([0.0, 0.0]), left, shift);
    assert!(editor.group.is_empty());
    assert_eq!(editor.selected, Some(r1));
    // ⇧ on nothing leaves it; a plain click on nothing clears.
    click(&mut editor, &engine, &ctx, pos2(30.0, 30.0), left, shift);
    assert_eq!(editor.selected, Some(r1));
    click(
        &mut editor,
        &engine,
        &ctx,
        pos2(30.0, 30.0),
        left,
        Modifiers::NONE,
    );
    assert_eq!(editor.selected, None);
    // A plain click on a group's object selects that object alone.
    click(&mut editor, &engine, &ctx, r1_at, left, shift);
    click(&mut editor, &engine, &ctx, r2_at, left, shift);
    click(&mut editor, &engine, &ctx, r1_at, left, Modifiers::NONE);
    assert!(editor.group.is_empty());
    assert_eq!(editor.selected, Some(r1));
    Ok(())
}

/// ⌘A takes every kind of object on this sheet and arms Select, and the card
/// shows the group, not one object's fields.
#[test]
fn select_all_takes_everything_on_the_sheet() -> TestResult {
    let mut s = sheet()?;
    let label = apply(
        &mut s,
        op(SchematicCommand::PlaceLabel {
            sheet: None,
            name: "SIG".into(),
            kind: pcbcraft_core::LabelKind::Local,
            position: [0.0, 20.0],
            rotation: Rotation::R0,
            attach: None,
        }),
    )?;
    let Outcome::LabelPlaced { label } = label else {
        return Err("no label".into());
    };
    let port = apply(
        &mut s,
        op(SchematicCommand::PlaceLabel {
            sheet: None,
            name: "GND".into(),
            kind: pcbcraft_core::LabelKind::Power {
                style: PowerStyle::Ground,
            },
            position: [10.0, 20.0],
            rotation: Rotation::R0,
            attach: None,
        }),
    )?;
    let Outcome::LabelPlaced { label: port } = port else {
        return Err("no port".into());
    };
    let drawing = apply(
        &mut s,
        op(SchematicCommand::AddDrawing {
            sheet: None,
            item: SheetItem::Line {
                from: [0.0, 30.0],
                to: [10.0, 30.0],
            },
        }),
    )?;
    let Outcome::DrawingAdded { drawing } = drawing else {
        return Err("no drawing".into());
    };
    s.editor.tool = Tool::Wire;
    s.editor.select_all(&s.engine);
    assert_eq!(s.editor.tool, Tool::Select);
    assert_eq!(
        s.editor.group,
        group_of(&[
            ClipObject::Instance { id: s.r1 },
            ClipObject::Instance { id: s.r2 },
            ClipObject::Label { id: label },
            ClipObject::Label { id: port },
            ClipObject::Drawing { id: drawing },
            ClipObject::Wire { id: s.wire },
        ])
    );
    assert_eq!(
        s.editor.group.summary(s.engine.project()),
        "2 symbols, 1 label, 1 power port, 1 drawing, 1 wire"
    );
    assert!(s.editor.can_delete(s.engine.project()));
    let header = s.editor.header(&s.engine);
    assert_eq!(header.title, "6 objects");
    Ok(())
}

/// A drag from a selected object sends one `schematic.arrange`: the wire
/// between the two symbols moves rigidly, the one leaving rubber-bands, the
/// preview shows just what lands, and one undo takes it all back.
#[test]
fn a_group_drag_is_one_undo_step_with_rigid_and_rubber_banded_wires() -> TestResult {
    let mut s = sheet()?;
    let (r3, outer) = with_r3(&mut s)?;
    frame(&mut s.editor, &s.engine, &s.ctx, vec![], Modifiers::NONE);
    s.editor.set_selection(
        group_of(&[
            ClipObject::Instance { id: s.r1 },
            ClipObject::Instance { id: s.r2 },
        ]),
        s.engine.project(),
    );
    let before = s.engine.project().clone();
    let revision = s.engine.revision();
    let path = |p: &Project, id: Id| p.wire(id).and_then(|w| schematic::wire_path(p, w));
    let inner_before = path(&before, s.wire).ok_or("no path")?;
    let from = screen([-12.0, 0.0]);
    // 5 mm up, 2.5 mm right at 12 px/mm.
    let to = from + vec2(30.0, -60.0);
    // The preview, half way through the gesture.
    for events in [
        vec![Event::PointerMoved(from)],
        vec![button(from, PointerButton::Primary, true, Modifiers::NONE)],
        vec![Event::PointerMoved(to)],
        vec![Event::PointerMoved(to)],
    ] {
        frame(&mut s.editor, &s.engine, &s.ctx, events, Modifiers::NONE);
    }
    let dragged = s.editor.group_drag.clone().ok_or("no group drag")?;
    assert_eq!(dragged.by(), [2.5, 5.0]);
    assert_eq!(s.engine.revision(), revision, "a preview changes nothing");
    let sent = frame(
        &mut s.editor,
        &s.engine,
        &s.ctx,
        vec![button(to, PointerButton::Primary, false, Modifiers::NONE)],
        Modifiers::NONE,
    )
    .ok_or("no move")?;
    assert!(matches!(
        &sent,
        Operation::Schematic(SchematicCommand::Arrange { objects, .. }) if objects.len() == 2
    ));
    s.engine.apply(revision, sent)?;
    assert_eq!(s.engine.revision(), revision + 1);
    let project = s.engine.project();
    let moved = |id: Id| project.occurrences_of(id).next().map(|o| o.position);
    assert_eq!(moved(s.r1), Some([-9.5, 5.0]));
    assert_eq!(moved(s.r2), Some([14.5, 5.0]));
    assert_eq!(moved(r3), Some([30.0, 15.0]), "R3 stays");
    let inner = path(project, s.wire).ok_or("no path")?;
    let shifted: Vec<Point> = inner_before
        .iter()
        .map(|p| [p[0] + 2.5, p[1] + 5.0])
        .collect();
    assert_eq!(inner, shifted, "rigid");
    let band = path(project, outer).ok_or("no path")?;
    assert!(
        band.windows(2)
            .all(|s| s[0][0] == s[1][0] || s[0][1] == s[1][1]),
        "{band:?}"
    );
    assert_eq!(
        dragged.wire(s.wire),
        Some(&inner),
        "the preview is what landed"
    );
    assert_eq!(dragged.wire(outer), Some(&band));
    frame(&mut s.editor, &s.engine, &s.ctx, vec![], Modifiers::NONE);
    assert_eq!(s.editor.group.len(), 2, "still selected");
    s.engine.undo()?;
    assert_eq!(s.engine.project().occurrences, before.occurrences);
    assert_eq!(s.engine.project().wires, before.wires);
    Ok(())
}

/// A drag from empty canvas with a group selected replaces it by the
/// rectangle's pick and moves nothing.
#[test]
fn a_drag_from_empty_canvas_does_not_move_the_group() -> TestResult {
    let mut s = sheet()?;
    s.editor.select_all(&s.engine);
    let sent = drag(
        &mut s.editor,
        &s.engine,
        &s.ctx,
        pos2(30.0, 30.0),
        pos2(60.0, 60.0),
        Modifiers::NONE,
    );
    assert!(sent.is_empty(), "{sent:?}");
    assert!(s.editor.group.is_empty());
    assert!(s.editor.selected.is_none());
    Ok(())
}

/// ⌘R and ⇧X turn and mirror the group about its centre as one undo step;
/// the Arrange buttons and the card's footer send the same.
#[test]
fn rotate_and_mirror_act_on_the_group_about_its_centre() -> TestResult {
    let mut s = sheet()?;
    s.editor.select_all(&s.engine);
    let project = s.engine.project().clone();
    let about = pcbcraft_core::schematic::arrange::centre(
        &project,
        s.editor.active_sheet(),
        &s.editor.group.objects(),
    )
    .ok_or("no centre")?;
    assert_eq!(about, [0.0, 0.0]);
    let keyed = frame(
        &mut s.editor,
        &s.engine,
        &s.ctx,
        vec![press(egui::Key::R, Modifiers::COMMAND)],
        Modifiers::COMMAND,
    )
    .ok_or("no turn")?;
    let tool = s.editor.on_tool("rotate", &s.engine).ok_or("no turn")?;
    assert_eq!(format!("{keyed:?}"), format!("{tool:?}"));
    let revision = s.engine.revision();
    s.engine.apply(revision, keyed)?;
    assert_eq!(s.engine.revision(), revision + 1);
    let turned = s.engine.project();
    assert_eq!(
        turned
            .occurrences_of(s.r1)
            .next()
            .map(|o| (o.position, o.rotation)),
        Some(([0.0, -12.0], Rotation::R90)),
    );
    assert_eq!(
        turned
            .occurrences_of(s.r2)
            .next()
            .map(|o| (o.position, o.rotation)),
        Some(([0.0, 12.0], Rotation::R90)),
    );
    let wire = turned.wire(s.wire).ok_or("no wire")?;
    let path = schematic::wire_path(turned, wire).ok_or("no path")?;
    assert!(path.iter().all(|p| p[0].abs() < 1e-9), "turned: {path:?}");
    s.engine.undo()?;
    assert_eq!(s.engine.project().occurrences, project.occurrences);
    let mirrored = s.editor.on_tool("mirror", &s.engine).ok_or("no mirror")?;
    s.engine.apply(s.engine.revision(), mirrored)?;
    let flipped = s.engine.project();
    assert_eq!(
        flipped.occurrences_of(s.r1).next().map(|o| o.position),
        Some([12.0, 0.0]),
    );
    Ok(())
}

/// Delete and Backspace open the review with everything listed and change
/// nothing; applying it is one undo step.
#[test]
fn delete_opens_the_review_with_the_whole_group() -> TestResult {
    for key in [egui::Key::Delete, egui::Key::Backspace] {
        let mut s = sheet()?;
        let Outcome::LabelPlaced { label } = apply(
            &mut s,
            op(SchematicCommand::PlaceLabel {
                sheet: None,
                name: "SIG".into(),
                kind: pcbcraft_core::LabelKind::Local,
                position: [0.0, 20.0],
                rotation: Rotation::R0,
                attach: None,
            }),
        )?
        else {
            return Err("no label".into());
        };
        s.editor.set_selection(
            group_of(&[
                ClipObject::Instance { id: s.r1 },
                ClipObject::Wire { id: s.wire },
                ClipObject::Label { id: label },
            ]),
            s.engine.project(),
        );
        let revision = s.engine.revision();
        let sent = frame(
            &mut s.editor,
            &s.engine,
            &s.ctx,
            vec![press(key, Modifiers::NONE)],
            Modifiers::NONE,
        );
        assert!(sent.is_none(), "the review comes first: {sent:?}");
        assert!(s.editor.update.is_open(), "{key:?}");
        assert_eq!(s.editor.subject(s.engine.project()), Subject::Review);
        assert_eq!(s.engine.revision(), revision);
        let change = Change::RemoveObjects {
            objects: s.editor.group.objects(),
        };
        let plan = s.engine.preview_update(change.clone());
        let lines: Vec<&str> = plan.items.iter().map(|i| i.line.as_str()).collect();
        assert!(lines.contains(&"Removes R1"), "{lines:?}");
        assert!(lines.contains(&"Removes label SIG"), "{lines:?}");
        assert!(plan.removed_wires.contains(&s.wire), "{lines:?}");
        s.engine.apply_update(revision, change, true)?;
        let project = s.engine.project();
        assert!(project.instance(s.r1).is_none());
        assert!(project.label(label).is_none());
        assert!(project.wire(s.wire).is_none());
        assert_eq!(s.engine.revision(), revision + 1);
        s.engine.undo()?;
        assert!(s.engine.project().instance(s.r1).is_some(), "one undo step");
        assert!(s.engine.project().label(label).is_some());
    }
    Ok(())
}

/// Copying a group takes a wire with both ends inside as a wire between the
/// pasted pins, and a named wire with one end inside gets a free end. What
/// was pasted ends up selected, in one undo step. Cut opens the review.
#[test]
fn group_copy_and_paste_keeps_wires_and_selects_the_result() -> TestResult {
    let mut s = sheet()?;
    let (_, outer) = with_r3(&mut s)?;
    frame(&mut s.editor, &s.engine, &s.ctx, vec![], Modifiers::NONE);
    s.editor.set_selection(
        group_of(&[
            ClipObject::Instance { id: s.r1 },
            ClipObject::Instance { id: s.r2 },
            ClipObject::Wire { id: s.wire },
            ClipObject::Wire { id: outer },
        ]),
        s.engine.project(),
    );
    s.editor.can_copy(s.engine.project())?;
    s.editor.copy_selection(&s.ctx, &s.engine)?;
    let before = s.engine.project().clone();
    let revision = s.engine.revision();
    let operation = s
        .editor
        .paste_at(&s.engine, Some([-12.0, -30.0]))
        .ok_or("no paste")?;
    apply(&mut s, operation)?;
    assert_eq!(s.engine.revision(), revision + 1);
    let project = s.engine.project();
    assert_eq!(project.instances.len(), before.instances.len() + 2);
    assert_eq!(project.wires.len(), before.wires.len() + 2);
    let new_instances: Vec<Id> = project
        .instances
        .iter()
        .map(|i| i.id)
        .filter(|id| before.instance(*id).is_none())
        .collect();
    let new_wires: Vec<_> = project
        .wires
        .iter()
        .filter(|w| before.wire(w.id).is_none())
        .collect();
    let both_ends = new_wires
        .iter()
        .find(|w| w.from.free_point().is_none() && w.to.free_point().is_none())
        .ok_or("a wire with both ends on the pasted pins")?;
    assert!([both_ends.from, both_ends.to].iter().all(|e| {
        e.terminal()
            .is_some_and(|t| new_instances.contains(&t.instance))
    }));
    let one_end = new_wires
        .iter()
        .find(|w| w.from.free_point().is_some() != w.to.free_point().is_some())
        .ok_or("a wire with one free end")?;
    assert!(
        [one_end.from, one_end.to]
            .iter()
            .filter_map(|e| e.terminal())
            .all(|t| new_instances.contains(&t.instance))
    );
    // Left selected: both symbols and both wires.
    assert_eq!(
        s.editor.group.instances,
        new_instances.iter().copied().collect()
    );
    assert_eq!(s.editor.group.wires.len(), 2);
    s.engine.undo()?;
    assert_eq!(s.engine.project().instances, before.instances);
    assert_eq!(s.engine.project().wires, before.wires);
    frame(&mut s.editor, &s.engine, &s.ctx, vec![], Modifiers::NONE);
    assert!(s.editor.group.is_empty(), "the pasted objects are gone");
    // Cut copies and opens the review.
    s.editor.set_selection(
        group_of(&[
            ClipObject::Instance { id: s.r1 },
            ClipObject::Wire { id: s.wire },
        ]),
        s.engine.project(),
    );
    let sent = s.editor.cut_selection(&s.ctx, &s.engine)?;
    assert!(sent.is_none());
    assert!(s.editor.update.is_open());
    assert!(s.editor.clipboard.is_some(), "copied before the review");
    Ok(())
}

/// A right-click on a selected object keeps the selection and offers what
/// applies to all; on another object it selects that object alone.
#[test]
fn a_right_click_on_the_group_keeps_it_and_offers_group_actions() -> TestResult {
    let mut s = sheet()?;
    let (r3, _) = with_r3(&mut s)?;
    frame(&mut s.editor, &s.engine, &s.ctx, vec![], Modifiers::NONE);
    let group = group_of(&[
        ClipObject::Instance { id: s.r1 },
        ClipObject::Instance { id: s.r2 },
        ClipObject::Wire { id: s.wire },
    ]);
    s.editor.set_selection(group.clone(), s.engine.project());
    let right = PointerButton::Secondary;
    click(
        &mut s.editor,
        &s.engine,
        &s.ctx,
        screen([-12.0, 0.0]),
        right,
        Modifiers::NONE,
    );
    assert_eq!(s.editor.menu.target(), Some(&Target::Group));
    assert_eq!(s.editor.group, group, "the selection stays");
    let rows = s.editor.menu_rows(&s.ctx, &s.engine, Target::Group);
    let labels: Vec<&str> = rows.iter().flatten().map(|e| e.label.as_str()).collect();
    assert_eq!(labels, ["Rotate", "Mirror", "Cut", "Copy", "Delete…"]);
    assert!(rows.iter().flatten().all(|e| e.enabled));
    assert!(
        rows.iter()
            .flatten()
            .any(|e| matches!(e.run, Run::Act(Act::Arrange(Arrange::Rotate))))
    );
    // The wire of the group too.
    s.editor.menu.close();
    click(
        &mut s.editor,
        &s.engine,
        &s.ctx,
        screen([0.0, 0.0]),
        right,
        Modifiers::NONE,
    );
    assert_eq!(s.editor.menu.target(), Some(&Target::Group));
    assert_eq!(s.editor.group, group);
    // Its rows act on all of it.
    let turned = s
        .editor
        .menu_act(&s.engine, Act::Arrange(Arrange::Rotate))
        .ok_or("no turn")?;
    assert!(matches!(
        turned,
        Operation::Schematic(SchematicCommand::Arrange { ref objects, .. }) if objects.len() == 3
    ));
    assert!(s.editor.menu_act(&s.engine, Act::RemoveGroup).is_none());
    assert!(s.editor.update.is_open());
    s.editor.update = Default::default();
    // Another object takes the selection over.
    s.editor.menu.close();
    let at = s
        .engine
        .project()
        .occurrences_of(r3)
        .next()
        .map(|o| o.position)
        .ok_or("R3")?;
    click(
        &mut s.editor,
        &s.engine,
        &s.ctx,
        screen(at),
        right,
        Modifiers::NONE,
    );
    assert!(s.editor.group.is_empty());
    assert_eq!(s.editor.selected, Some(r3));
    Ok(())
}

/// A wire tidy removes after an edit leaves the group; one object left is the
/// single selection.
#[test]
fn an_object_that_goes_away_leaves_the_group() -> TestResult {
    let mut s = sheet()?;
    s.editor.set_selection(
        group_of(&[
            ClipObject::Instance { id: s.r1 },
            ClipObject::Wire { id: s.wire },
        ]),
        s.engine.project(),
    );
    let wire = s.wire;
    apply(&mut s, op(SchematicCommand::Disconnect { wire }))?;
    assert!(s.editor.group.is_empty());
    assert_eq!(s.editor.selected, Some(s.r1));
    assert!(matches!(
        s.editor.clip_object(s.engine.project()),
        Ok(ClipObject::Instance { .. })
    ));
    Ok(())
}
