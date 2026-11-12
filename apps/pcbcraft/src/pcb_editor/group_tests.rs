//! Several board objects selected, in headless egui frames on the engine:
//! ⇧- and ⌘-click, the rectangle and ⌘A under the selection filter, one
//! Delete for the whole selection, a dangling stub located and deleted in
//! one step, and the same group in 2D and 3D.
use super::super::raster_tests::view_of;
use super::*;
use crate::pcb_editor::Inspected;
use egui::{PointerButton, Rect};
use pcbcraft_core::layout::RouteRequest;
use pcbcraft_core::{Endpoint, Engine, Layer, TerminalId};
use pcbcraft_ui::fonts;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn frame(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    modifiers: egui::Modifiers,
    events: Vec<egui::Event>,
) -> Option<Operation> {
    let mut command = None;
    let mut all = vec![egui::Event::ModifiersChanged(modifiers)];
    all.extend(events);
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 700.0))),
            events: all,
            ..Default::default()
        },
        |ui| command = editor.canvas(ui, engine),
    );
    output.textures_delta.clear();
    command
}

fn button(at: Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    }
}

/// A click at screen point `at` with `modifiers` held.
fn click(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    at: Pos2,
    modifiers: egui::Modifiers,
) -> Option<Operation> {
    let events = vec![
        egui::Event::PointerMoved(at),
        button(at, true, modifiers),
        button(at, false, modifiers),
    ];
    let op = frame(editor, engine, ctx, modifiers, events);
    // The next frame forgets the click.
    frame(editor, engine, ctx, modifiers, vec![]).or(op)
}

/// A drag from `from` to `to` with `modifiers` held.
fn drag(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    from: Pos2,
    to: Pos2,
    modifiers: egui::Modifiers,
) -> Option<Operation> {
    let mut last = None;
    for events in [
        vec![
            egui::Event::PointerMoved(from),
            button(from, true, modifiers),
        ],
        vec![egui::Event::PointerMoved(from + (to - from) * 0.25)],
        vec![egui::Event::PointerMoved(to)],
        vec![button(to, false, modifiers)],
    ] {
        last = frame(editor, engine, ctx, modifiers, events).or(last);
    }
    last
}

fn key(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    key: egui::Key,
) -> Option<Operation> {
    let press = egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(editor, engine, ctx, egui::Modifiers::NONE, vec![press])
}

fn by(engine: &Engine, designator: &str) -> Fallible<Id> {
    Ok(engine
        .project()
        .instance_by_designator(designator)
        .ok_or(designator.to_owned())?
        .id)
}

/// The demo with R1 at (10, 10) and R2 at (25, 10), and `legs` routed on
/// the net of R1's pad 2 at 0.3 mm on the top.
fn board(legs: &[(Point, Point)]) -> Fallible<Engine> {
    let mut engine = Engine::demo()?;
    let (r1, r2) = (by(&engine, "R1")?, by(&engine, "R2")?);
    for (instance, position) in [(r1, [10.0, 10.0]), (r2, [25.0, 10.0])] {
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Move {
                instance,
                position,
                rotation_deg: 0.0,
                layer: Layer::Top,
            }),
        )?;
    }
    let net = engine
        .project()
        .net_of(Endpoint {
            instance: r1,
            terminal: TerminalId(2),
        })
        .map(|n| n.id)
        .ok_or("no net")?;
    for &(start, end) in legs {
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Route {
                request: RouteRequest {
                    net,
                    start,
                    end,
                    width: 0.3,
                    layer: Layer::Top.into(),
                },
            }),
        )?;
    }
    Ok(engine)
}

const ROUTED: &[(Point, Point)] = &[([11.2, 10.0], [23.8, 10.0])];

fn setup(engine: &Engine) -> (PcbEditor, egui::Context) {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    frame(&mut editor, engine, &ctx, egui::Modifiers::NONE, vec![]);
    (editor, ctx)
}

/// The screen point of a part's body centre (its datum).
fn part_at(editor: &PcbEditor, engine: &Engine, id: Id) -> Fallible<Pos2> {
    let p = engine.project().placement_of(id).ok_or("not placed")?;
    Ok(view_of(editor, engine).screen(p.position))
}

/// ⇧-click and ⌘-click add a part, and take a selected one out; a plain
/// click replaces the selection. Two parts make the Selection card.
#[test]
fn shift_and_command_click_add_and_toggle() -> Fallible {
    let engine = board(ROUTED)?;
    let (r1, r2) = (by(&engine, "R1")?, by(&engine, "R2")?);
    let track = engine.project().tracks.first().ok_or("no track")?.id;
    let (mut editor, ctx) = setup(&engine);
    let (a, b) = (
        part_at(&editor, &engine, r1)?,
        part_at(&editor, &engine, r2)?,
    );
    click(&mut editor, &engine, &ctx, a, egui::Modifiers::NONE);
    assert_eq!(editor.selected_instance(), Some(r1));
    click(&mut editor, &engine, &ctx, b, egui::Modifiers::SHIFT);
    assert_eq!(editor.selected().parts, [r1, r2].into());
    assert_eq!(editor.inspected(engine.project()), Inspected::Group);
    assert_eq!(editor.header(&engine).title, "2 objects");
    // ⌘ adds a track, then takes R1 out.
    let mid = view_of(&editor, &engine).screen([17.5, 10.0]);
    click(&mut editor, &engine, &ctx, mid, egui::Modifiers::COMMAND);
    assert_eq!(editor.selected().tracks, [track].into());
    assert_eq!(editor.selected().summary(), "2 parts, 1 track");
    click(&mut editor, &engine, &ctx, a, egui::Modifiers::COMMAND);
    assert_eq!(editor.selected().parts, [r2].into());
    // R2 again leaves the track alone: the single selection.
    click(&mut editor, &engine, &ctx, b, egui::Modifiers::SHIFT);
    assert_eq!(editor.selected_track(), Some(track));
    assert_eq!(editor.inspected(engine.project()), Inspected::Track(track));
    // A plain click replaces it.
    click(&mut editor, &engine, &ctx, a, egui::Modifiers::NONE);
    assert_eq!(
        editor.selected(),
        Group {
            parts: [r1].into(),
            ..Group::default()
        }
    );
    Ok(())
}

/// A rectangle from empty canvas takes what lies wholly inside it (left
/// to right) of the kinds the filter allows; right to left it takes what
/// it touches. A filtered-out kind is not clicked either.
#[test]
fn box_select_respects_the_filters() -> Fallible {
    let engine = board(ROUTED)?;
    let (r1, r2) = (by(&engine, "R1")?, by(&engine, "R2")?);
    let track = engine.project().tracks.first().ok_or("no track")?.id;
    let (mut editor, ctx) = setup(&engine);
    let view = view_of(&editor, &engine);
    let (low, high) = (view.screen([2.0, 2.0]), view.screen([33.0, 18.0]));
    let none = egui::Modifiers::NONE;
    // Left to right round both parts and the track between them.
    let (from, to) = (Pos2::new(low.x, high.y), Pos2::new(high.x, low.y));
    drag(&mut editor, &engine, &ctx, from, to, none);
    let all = Group {
        parts: [r1, r2].into(),
        tracks: [track].into(),
        ..Group::default()
    };
    assert_eq!(editor.selected(), all);
    // Without tracks.
    editor.filter_mut().tracks = false;
    drag(&mut editor, &engine, &ctx, from, to, none);
    assert_eq!(editor.selected().parts, [r1, r2].into());
    assert!(editor.selected().tracks.is_empty());
    // Without parts: the one track is the single selection.
    *editor.filter_mut() = SelectFilter {
        parts: false,
        ..SelectFilter::default()
    };
    drag(&mut editor, &engine, &ctx, from, to, none);
    assert_eq!(editor.selected_track(), Some(track));
    // A click on R1 passes through to nothing.
    let a = part_at(&editor, &engine, r1)?;
    click(&mut editor, &engine, &ctx, a, none);
    assert!(editor.selected().is_empty());
    // Right to left over the middle of the track only: it touches the
    // track and neither part.
    *editor.filter_mut() = SelectFilter::default();
    let (right, left) = (view.screen([19.0, 12.0]), view.screen([16.0, 8.0]));
    drag(&mut editor, &engine, &ctx, right, left, none);
    assert_eq!(editor.selected_track(), Some(track));
    // ⇧ adds a rectangle round R2 to it.
    let (p, q) = (view.screen([22.0, 13.0]), view.screen([28.0, 7.0]));
    drag(&mut editor, &engine, &ctx, p, q, egui::Modifiers::SHIFT);
    assert_eq!(editor.selected().parts, [r2].into());
    assert_eq!(editor.selected().tracks, [track].into());
    Ok(())
}

/// ⌘A takes every placed part, shown track, via and fill the filter
/// allows; the app's Select All is available on the PCB.
#[test]
fn select_all_takes_what_the_filters_allow() -> Fallible {
    let engine = board(ROUTED)?;
    let (r1, r2) = (by(&engine, "R1")?, by(&engine, "R2")?);
    let track = engine.project().tracks.first().ok_or("no track")?.id;
    let (mut editor, _) = setup(&engine);
    assert_eq!(editor.can_select_all(), Ok(()));
    editor.select_all(engine.project());
    assert_eq!(
        editor.selected(),
        Group {
            parts: [r1, r2].into(),
            tracks: [track].into(),
            ..Group::default()
        }
    );
    editor.filter_mut().parts = false;
    editor.select_all(engine.project());
    assert_eq!(editor.selected_track(), Some(track));
    editor.filter_mut().tracks = false;
    editor.select_all(engine.project());
    assert!(editor.selected().is_empty());
    Ok(())
}

/// Delete takes the selection's tracks in one `layout.delete_objects`:
/// one undo step brings them all back. Parts are left to the schematic.
#[test]
fn delete_removes_the_selection_in_one_undo_step() -> Fallible {
    let mut engine = board(&[
        ([11.2, 10.0], [17.0, 10.0]),
        ([17.0, 10.0], [17.0, 14.0]),
        ([17.0, 14.0], [23.8, 14.0]),
    ])?;
    let before = engine.project().clone();
    let (mut editor, ctx) = setup(&engine);
    editor.select_all(engine.project());
    assert_eq!(editor.selected().tracks.len(), 3);
    assert!(editor.can_delete(engine.project()));
    let op = key(&mut editor, &engine, &ctx, egui::Key::Delete).ok_or("no delete")?;
    engine.apply(engine.revision(), op)?;
    assert!(engine.project().tracks.is_empty());
    assert_eq!(engine.project().placements, before.placements);
    engine.undo()?;
    assert_eq!(engine.project(), &before, "one undo step");
    // Only parts: nothing to delete here.
    editor.filter_mut().tracks = false;
    editor.select_all(engine.project());
    assert!(!editor.can_delete(engine.project()));
    assert!(key(&mut editor, &engine, &ctx, egui::Key::Delete).is_none());
    Ok(())
}

/// Night finding R6-2: Locate on a "joining nothing" finding selects the
/// stub's every track, so one Delete removes the stub in one undo step and
/// the finding goes.
#[test]
fn a_dangling_stub_is_located_and_deleted_in_one_step() -> Fallible {
    use crate::issues::Locate;
    let mut engine = board(&[([11.2, 10.0], [14.0, 10.0]), ([14.0, 10.0], [14.0, 13.0])])?;
    let dangling = |engine: &Engine| {
        engine
            .copper_graph()
            .findings(engine.project())
            .into_iter()
            .filter(|f| f.code == "board.track_dangling")
            .collect::<Vec<_>>()
    };
    let finding = dangling(&engine)
        .into_iter()
        .next()
        .ok_or("no stub finding")?;
    let (mut editor, ctx) = setup(&engine);
    assert!(editor.locate(&engine, &finding.objects));
    let tracks: BTreeSet<Id> = engine.project().tracks.iter().map(|t| t.id).collect();
    assert_eq!(tracks.len(), 2);
    assert_eq!(editor.selected().tracks, tracks);
    let before = engine.project().clone();
    let op = key(&mut editor, &engine, &ctx, egui::Key::Delete).ok_or("no delete")?;
    engine.apply(engine.revision(), op)?;
    assert!(engine.project().tracks.is_empty());
    assert!(dangling(&engine).is_empty());
    engine.undo()?;
    assert_eq!(engine.project(), &before, "one undo step");
    Ok(())
}

/// A group drag moves every part of the group in one `layout.move_parts`;
/// the track between them comes along whole (the engine's carry).
#[test]
fn dragging_the_group_moves_its_parts_and_their_copper() -> Fallible {
    let mut engine = board(ROUTED)?;
    let (r1, r2) = (by(&engine, "R1")?, by(&engine, "R2")?);
    let (mut editor, ctx) = setup(&engine);
    editor.select_all(engine.project());
    let from = part_at(&editor, &engine, r1)?;
    let view = view_of(&editor, &engine);
    let to = view.screen([12.0, 13.0]);
    let op = drag(&mut editor, &engine, &ctx, from, to, egui::Modifiers::NONE).ok_or("no move")?;
    let Operation::Layout(LayoutCommand::MoveParts { moves, .. }) = &op else {
        return Err(format!("{op:?}").into());
    };
    assert_eq!(moves.len(), 2);
    engine.apply(engine.revision(), op.clone())?;
    let p = |id| engine.project().placement_of(id).map(|p| p.position);
    let (a, b) = (p(r1).ok_or("R1")?, p(r2).ok_or("R2")?);
    assert!((b[0] - a[0] - 15.0).abs() < 1e-9 && (b[1] - a[1]).abs() < 1e-9);
    assert!(
        (a[0] - 12.0).abs() < 1e-9 && (a[1] - 13.0).abs() < 1e-9,
        "{a:?}"
    );
    let t = engine.project().tracks.first().ok_or("no track")?;
    assert!(
        (t.start[1] - 13.0).abs() < 1e-9 && (t.end[1] - 13.0).abs() < 1e-9,
        "{t:?}"
    );
    Ok(())
}

/// A group drag also slides the selected loose tracks and vias (attached
/// to no moved part) with the parts, in the same undo step, while the
/// track between the parts is carried as before.
#[test]
fn a_group_drag_slides_the_selected_loose_copper() -> Fallible {
    use pcbcraft_core::layout::via::ViaRequest;
    let mut engine = board(&[ROUTED[0], ([14.0, 16.0], [20.0, 16.0])])?;
    let r1 = by(&engine, "R1")?;
    let net = engine.project().tracks.first().ok_or("no track")?.net;
    let via = ViaRequest::through(&engine.project().rules.effective(), net, [17.0, 19.0]);
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::AddVia { via }),
    )?;
    let before = engine.project().clone();
    let (mut editor, ctx) = setup(&engine);
    editor.select_all(engine.project());
    assert_eq!(editor.selected().summary(), "2 parts, 2 tracks, 1 via");
    let from = part_at(&editor, &engine, r1)?;
    let to = view_of(&editor, &engine).screen([12.0, 13.0]);
    let op = drag(&mut editor, &engine, &ctx, from, to, egui::Modifiers::NONE).ok_or("no move")?;
    engine.apply(engine.revision(), op)?;
    let near = |a: Point, b: Point| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
    let tracks = &engine.project().tracks;
    assert_eq!(tracks.len(), 2, "{tracks:?}");
    assert!(
        tracks
            .iter()
            .any(|t| near(t.start, [13.2, 13.0]) && near(t.end, [25.8, 13.0])),
        "the routed track carried: {tracks:?}"
    );
    assert!(
        tracks
            .iter()
            .any(|t| near(t.start, [16.0, 19.0]) && near(t.end, [22.0, 19.0])),
        "the loose track slid: {tracks:?}"
    );
    let via = engine.project().vias.first().ok_or("no via")?;
    assert!(near(via.position, [19.0, 22.0]), "the via slid: {via:?}");
    engine.undo()?;
    assert_eq!(engine.project(), &before, "one undo step");
    Ok(())
}

/// The group's Align and Distribute run through the registry's arrange:
/// two parts or more for align, three for distribute, in Layout.
#[test]
fn arrange_needs_enough_selected_parts() -> Fallible {
    use crate::commands::PcbArrange;
    use pcbcraft_core::layout::arrange::{AlignTo, Axis};
    let engine = board(ROUTED)?;
    let (r1, r2) = (by(&engine, "R1")?, by(&engine, "R2")?);
    let (mut editor, _) = setup(&engine);
    let left = PcbArrange::Align(AlignTo::Left);
    assert_eq!(
        editor.arrange(left).err().as_deref(),
        Some("Select two or more parts")
    );
    editor.select_all(engine.project());
    let Ok(Operation::Layout(LayoutCommand::Align { instances, to })) = editor.arrange(left) else {
        return Err("no align".into());
    };
    assert_eq!((instances, to), (vec![r1, r2], AlignTo::Left));
    assert_eq!(
        editor
            .arrange(PcbArrange::Distribute(Axis::Vertical))
            .err()
            .as_deref(),
        Some("Select three or more parts")
    );
    Ok(())
}

/// The same group in both views: ⇧-picks in 3D add as ⇧-clicks in Layout
/// do, the 3D highlight takes every body of the group's objects, and the
/// group survives the switch between the views.
#[test]
fn a_group_is_the_same_in_layout_and_3d() -> Fallible {
    let engine = board(ROUTED)?;
    let (r1, r2) = (by(&engine, "R1")?, by(&engine, "R2")?);
    let track = engine.project().tracks.first().ok_or("no track")?.id;
    let model = pcbcraft_solidcraft::project::build(engine.project())?;
    let (mut layout, ctx) = setup(&engine);
    for id in [r1, r2] {
        let at = part_at(&layout, &engine, id)?;
        click(&mut layout, &engine, &ctx, at, egui::Modifiers::SHIFT);
    }
    let mid = view_of(&layout, &engine).screen([17.5, 10.0]);
    click(&mut layout, &engine, &ctx, mid, egui::Modifiers::SHIFT);
    let mut picked = PcbEditor::default();
    picked.set_3d(true);
    let body = |pred: &dyn Fn(&pcbcraft_solidcraft::project::Source) -> bool| {
        model.sources.iter().find(|s| pred(s)).ok_or("no body")
    };
    for id in [r1, r2] {
        picked.toggle_picked(
            Some(body(&|s| s.instance == Some(id) && s.pad.is_none())?),
            engine.project(),
        );
    }
    picked.toggle_picked(Some(body(&|s| s.track == Some(track))?), engine.project());
    assert_eq!(picked.selected(), layout.selected());
    assert_eq!(picked.selected().len(), 3);
    // The 3D highlight takes every body of the three objects, no other.
    let group = picked.selected();
    for s in &model.sources {
        let ours = s.instance.is_some_and(|i| i == r1 || i == r2) || s.track == Some(track);
        assert_eq!(crate::scene::picks(s, &group), ours, "{}", s.label);
    }
    picked.set_3d(false);
    layout.set_3d(true);
    assert_eq!(picked.selected(), layout.selected());
    // Selection is display state: no rebuild of the board.
    let cached = layout.cached_revision;
    layout.toggle_hit(super::Hit::Part(r1), engine.project());
    assert_eq!(layout.cached_revision, cached);
    Ok(())
}
