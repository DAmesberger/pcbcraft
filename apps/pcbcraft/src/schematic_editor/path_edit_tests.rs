//! Editing a selected wire path with synthetic input (docs/schematic-wires.md,
//! "Selecting and hovering", "Editing a selected path"): each gesture sends
//! one `wire.*` command, one undo step; a drag previews what it stores; the
//! nets stay unless the gesture joins or splits them.
use super::path_edit::{self, Grab};
use super::*;
use egui::vec2;
use pcbcraft_core::model::{Corner, FreeEnd, LabelKind, PowerStyle};
use pcbcraft_core::schematic::wire_edit::WireSide;
use solidcraft_ui_egui::fonts;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const CANVAS: Rect = Rect::from_min_max(Pos2::ZERO, pos2(800.0, 800.0));

fn frame_after(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    seconds: f64,
) -> (Vec<egui::Shape>, Option<Operation>) {
    let mut operation = None;
    let time = ctx.input(|i| i.time) + seconds;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(CANVAS),
            events,
            time: Some(time),
            ..Default::default()
        },
        |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(CANVAS), |ui| {
                operation = editor.canvas(ui, engine);
            });
        },
    );
    output.textures_delta.clear();
    let mut shapes = Vec::new();
    fn flatten(shape: egui::Shape, out: &mut Vec<egui::Shape>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.into_iter().for_each(|s| flatten(s, out)),
            other => out.push(other),
        }
    }
    for clipped in output.shapes {
        flatten(clipped.shape, &mut shapes);
    }
    (shapes, operation)
}

fn frame(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (Vec<egui::Shape>, Option<Operation>) {
    frame_after(editor, engine, ctx, events, 0.4)
}

fn button(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn screen(editor: &SchematicEditor, p: Point) -> Pos2 {
    symbol_view::View {
        origin: CANVAS.center() + editor.pan,
        scale: editor.scale,
    }
    .screen(p)
}

fn click(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    at: Point,
) -> Option<Operation> {
    let p = screen(editor, at);
    let mut sent = None;
    for events in [
        vec![egui::Event::PointerMoved(p)],
        vec![button(p, true)],
        vec![button(p, false)],
    ] {
        sent = frame(editor, engine, ctx, events).1.or(sent);
    }
    sent
}

fn double_click(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    at: Point,
) -> Option<Operation> {
    let p = screen(editor, at);
    let mut sent = frame(editor, engine, ctx, vec![egui::Event::PointerMoved(p)]).1;
    for events in [
        vec![button(p, true)],
        vec![button(p, false)],
        vec![button(p, true)],
        vec![button(p, false)],
    ] {
        sent = frame_after(editor, engine, ctx, events, 0.03).1.or(sent);
    }
    sent
}

/// A drag between sheet points: the path it previews on arrival, and what it
/// sends on release.
fn drag(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    from: Point,
    to: Point,
) -> (Option<Vec<Point>>, Option<Operation>) {
    let (a, b) = (screen(editor, from), screen(editor, to));
    for events in [
        vec![egui::Event::PointerMoved(a)],
        vec![button(a, true)],
        vec![egui::Event::PointerMoved(a + (b - a) * 0.5)],
        vec![egui::Event::PointerMoved(b)],
    ] {
        frame(editor, engine, ctx, events);
    }
    let preview = editor
        .path_drag
        .as_ref()
        .map(|d| d.now.iter().map(|c| c.at).collect());
    let sent = frame(editor, engine, ctx, vec![button(b, false)]).1;
    (preview, sent)
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn ep(instance: Id, terminal: u32) -> Endpoint {
    Endpoint {
        instance,
        terminal: pcbcraft_core::TerminalId(terminal),
    }
}

fn pin(engine: &Engine, e: Endpoint) -> TestResult<Point> {
    Ok(schematic::pin_position(engine.project(), e).ok_or("pin")?)
}

fn place(engine: &mut Engine, name: &str, at: Point) -> TestResult<Id> {
    let first = engine.project().instances[0].clone();
    engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet: None,
            component: first.component,
            designator: Some(name.into()),
            properties: vec![],
            representation: first.representation,
            unit: pcbcraft_core::UnitId(1),
            position: at,
            physical: None,
        })),
    )?;
    Ok(engine
        .project()
        .instance_by_designator(name)
        .map(|i| i.id)
        .ok_or("placed")?)
}

fn draw(
    engine: &mut Engine,
    from: WireTarget,
    corners: &[Point],
    to: WireTarget,
) -> TestResult<Id> {
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::DrawWire {
            sheet: None,
            from,
            corners: corners.iter().map(|&at| Corner::sharp(at)).collect(),
            to,
        }),
    )?;
    Ok(engine.project().wires.last().map(|w| w.id).ok_or("wire")?)
}

fn free(p: Point) -> TestResult<WireTarget> {
    Ok(WireTarget::Free(FreeEnd::new(p).ok_or("free")?))
}

/// The demo, and far below it R90 and R91 joined by a dogleg with two corners,
/// and R92 unwired above it. The view centres the dogleg at 12 px/mm.
struct Fixture {
    engine: Engine,
    r90: Id,
    r91: Id,
    r92: Id,
    wire: Id,
    /// The dogleg's vertices: R90.2, two corners, R91.1.
    v: [Point; 4],
    editor: SchematicEditor,
    ctx: egui::Context,
}

fn fixture() -> TestResult<Fixture> {
    let mut engine = Engine::demo()?;
    let r90 = place(&mut engine, "R90", [0.0, -400.0])?;
    let r91 = place(&mut engine, "R91", [50.0, -380.0])?;
    let r92 = place(&mut engine, "R92", [50.0, -420.0])?;
    let (a, b) = (pin(&engine, ep(r90, 2))?, pin(&engine, ep(r91, 1))?);
    let x = 25.0;
    let wire = draw(
        &mut engine,
        ep(r90, 2).into(),
        &[[x, a[1]], [x, b[1]]],
        ep(r91, 1).into(),
    )?;
    let v = [a, [x, a[1]], [x, b[1]], b];
    let scale = 12.0;
    let editor = SchematicEditor {
        scale,
        pan: vec2(-(x as f32) * scale, -390.0 * scale),
        selected_wire: Some(wire),
        ..Default::default()
    };
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    Ok(Fixture {
        engine,
        r90,
        r91,
        r92,
        wire,
        v,
        editor,
        ctx,
    })
}

impl Fixture {
    fn path(&self) -> TestResult<Vec<Point>> {
        let project = self.engine.project();
        let wire = project.wire(self.wire).ok_or("wire")?;
        Ok(schematic::wire_path(project, wire).ok_or("path")?)
    }

    /// Apply `operation`, whose command is `id`, as one undo step.
    fn accept(&mut self, operation: Option<Operation>, id: &str) -> TestResult {
        let operation = operation.ok_or(format!("{id}: nothing was sent"))?;
        let Operation::Schematic(command) = &operation else {
            return Err(format!("{id}: {operation:?}").into());
        };
        let sent = serde_json::to_value(command)?;
        assert_eq!(sent["command"], id);
        let revision = self.engine.revision();
        self.engine.apply(revision, operation)?;
        assert_eq!(self.engine.revision(), revision + 1, "{id}");
        Ok(())
    }
}

/// The square handles of a frame, by centre.
fn squares(shapes: &[egui::Shape]) -> Vec<Pos2> {
    let side = path_edit::HANDLE_HALF_PX * 2.0;
    shapes
        .iter()
        .filter_map(|s| match s {
            egui::Shape::Rect(r) if (r.rect.width() - side).abs() < 1e-3 => Some(r.rect.center()),
            _ => None,
        })
        .collect()
}

/// A click selects the whole path, hovering highlights it and the segment
/// under the pointer, and the selected path shows a square at each corner, a
/// dot at each segment's middle, and no square on an attached end.
#[test]
fn a_click_selects_the_path_and_shows_its_handles() -> TestResult {
    let mut f = fixture()?;
    f.editor.selected_wire = None;
    let middle = [f.v[1][0], (f.v[1][1] + f.v[2][1]) * 0.5];
    let (shapes, _) = frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    assert!(squares(&shapes).is_empty(), "nothing selected, no handles");
    let p = screen(&f.editor, middle);
    let (shapes, _) = frame(
        &mut f.editor,
        &f.engine,
        &f.ctx,
        vec![egui::Event::PointerMoved(p)],
    );
    assert_eq!(f.editor.hovered_segment, Some((f.wire, 1)));
    assert!(
        shapes.iter().any(|s| matches!(
            s,
            egui::Shape::LineSegment { stroke, .. } if (stroke.width - 3.0).abs() < 1e-3
        )),
        "the hovered segment is drawn stronger"
    );
    assert!(click(&mut f.editor, &f.engine, &f.ctx, middle).is_none());
    assert_eq!(f.editor.selected_wire, Some(f.wire));
    let (shapes, _) = frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    let found = squares(&shapes);
    assert_eq!(found.len(), 2, "{found:?}");
    for corner in [f.v[1], f.v[2]] {
        let at = screen(&f.editor, corner);
        assert!(found.iter().any(|q| q.distance(at) < 0.5), "{corner:?}");
    }
    let dots = shapes
        .iter()
        .filter(|s| {
            matches!(s, egui::Shape::Circle(c)
            if (c.radius - path_edit::MIDPOINT_RADIUS_PX).abs() < 1e-3)
        })
        .count();
    assert_eq!(dots, 3, "one midpoint dot per segment");
    Ok(())
}

/// A group selection shows no wire handles, and a drag from a wire in it
/// still moves the group.
#[test]
fn a_group_selection_shows_no_wire_handles() -> TestResult {
    let mut f = fixture()?;
    f.editor.selected_wire = None;
    let project = f.engine.project();
    f.editor
        .toggle_object(ClipObject::Wire { id: f.wire }, project);
    f.editor
        .toggle_object(ClipObject::Instance { id: f.r92 }, project);
    assert_eq!(f.editor.group.len(), 2);
    let (shapes, _) = frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    assert!(squares(&shapes).is_empty());
    let corner = f.v[1];
    let (preview, sent) = drag(
        &mut f.editor,
        &f.engine,
        &f.ctx,
        corner,
        [corner[0] + 5.0, corner[1]],
    );
    assert!(preview.is_none(), "no path drag");
    assert!(
        matches!(
            sent,
            Some(Operation::Schematic(SchematicCommand::Arrange { .. }))
        ),
        "{sent:?}"
    );
    Ok(())
}

/// (case, press, release, path afterwards)
type DragCase = (&'static str, Point, Point, Vec<Point>);

/// A corner or segment drag: the preview is what is stored, one command, one
/// undo, and the nets stay.
#[test]
fn corner_and_segment_drags_store_their_preview_in_one_undo() -> TestResult {
    let start = fixture()?;
    let [a, c1, c2, b] = start.v;
    drop(start);
    let cases: Vec<DragCase> = vec![
        (
            "corner: both neighbours stay orthogonal; the pin end steps",
            c1,
            [c1[0] + 5.0, c1[1] + 5.0],
            vec![
                a,
                [a[0], a[1] + 5.0],
                [c1[0] + 5.0, c1[1] + 5.0],
                [c2[0] + 5.0, c2[1]],
                b,
            ],
        ),
        (
            "corner: along one segment adds no step",
            c2,
            [c2[0] - 5.0, c2[1]],
            vec![a, [c1[0] - 5.0, c1[1]], [c2[0] - 5.0, c2[1]], b],
        ),
        (
            "the middle segment, from anywhere on it: neighbours stretch",
            [c1[0], c1[1] + 5.0],
            [c1[0] + 7.5, c1[1] + 5.0],
            vec![a, [c1[0] + 7.5, c1[1]], [c2[0] + 7.5, c2[1]], b],
        ),
        (
            "the segment on a pin: a short perpendicular segment appears",
            [(a[0] + c1[0]) * 0.5, a[1]],
            [(a[0] + c1[0]) * 0.5, a[1] - 5.0],
            vec![a, [a[0], a[1] - 5.0], [c1[0], c1[1] - 5.0], c2, b],
        ),
    ];
    for (case, from, to, expected) in cases {
        let mut f = fixture()?;
        let wires = f.engine.project().wires.clone();
        let nets = f.engine.project().nets.clone();
        frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
        let (preview, sent) = drag(&mut f.editor, &f.engine, &f.ctx, from, to);
        assert_eq!(preview.as_ref(), Some(&expected), "{case}: preview");
        let id = match &sent {
            Some(Operation::Schematic(SchematicCommand::MoveWireCorner { .. })) => {
                "wire.move_corner"
            }
            _ => "wire.move_segment",
        };
        f.accept(sent, id)?;
        assert_eq!(f.path()?, expected, "{case}: the result is the preview");
        assert_eq!(f.engine.project().nets, nets, "{case}: nets");
        f.engine.undo()?;
        assert_eq!(f.engine.project().wires, wires, "{case}: one undo");
    }
    Ok(())
}

/// Esc during a drag restores the path; nothing is sent.
#[test]
fn esc_during_a_drag_restores_the_path() -> TestResult {
    let mut f = fixture()?;
    let (a, b) = (
        screen(&f.editor, f.v[1]),
        screen(&f.editor, [f.v[1][0] + 5.0, f.v[1][1]]),
    );
    for events in [
        vec![egui::Event::PointerMoved(a)],
        vec![button(a, true)],
        vec![egui::Event::PointerMoved(b)],
    ] {
        frame(&mut f.editor, &f.engine, &f.ctx, events);
    }
    assert!(f.editor.path_drag.is_some());
    frame(
        &mut f.editor,
        &f.engine,
        &f.ctx,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert!(f.editor.path_drag.is_none());
    let (_, sent) = frame(&mut f.editor, &f.engine, &f.ctx, vec![button(b, false)]);
    assert!(sent.is_none(), "{sent:?}");
    Ok(())
}

/// Double-clicking a segment adds a corner without changing the shape, and
/// picks it; a drag makes a step. Double-clicking a corner removes it. Delete
/// on a picked corner removes it too; the merged segment may slant.
#[test]
fn double_clicks_and_delete_add_and_remove_corners() -> TestResult {
    let mut f = fixture()?;
    let [a, c1, c2, b] = f.v;
    let nets = f.engine.project().nets.clone();
    let on = [(a[0] + c1[0]) * 0.5, a[1]];
    let at = snap(on, 2.5);
    let sent = double_click(&mut f.editor, &f.engine, &f.ctx, on);
    f.accept(sent, "wire.insert_corner")?;
    assert_eq!(f.path()?, vec![a, at, c1, c2, b], "the same shape");
    assert_eq!(f.editor.selected_corner, Some((f.wire, 0)));
    assert_eq!(f.engine.project().nets, nets);

    // The new corner breaks the line into a step.
    let to = [at[0], at[1] + 5.0];
    let (preview, sent) = drag(&mut f.editor, &f.engine, &f.ctx, at, to);
    f.accept(sent, "wire.move_corner")?;
    let stepped = f.path()?;
    assert_eq!(Some(stepped.clone()), preview);
    assert!(
        stepped.contains(&to) && stepped.contains(&at),
        "{stepped:?}"
    );
    assert!(
        stepped
            .windows(2)
            .all(|s| s[0][0] == s[1][0] || s[0][1] == s[1][1]),
        "{stepped:?}"
    );
    f.engine.undo()?;
    f.engine.undo()?;
    assert_eq!(f.path()?, vec![a, c1, c2, b]);

    // A double-click on a corner removes it.
    frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    let sent = double_click(&mut f.editor, &f.engine, &f.ctx, c1);
    f.accept(sent, "wire.remove_corner")?;
    assert_eq!(f.path()?, vec![a, c2, b]);
    assert_eq!(f.engine.project().nets, nets);
    f.engine.undo()?;

    // A click picks a corner, and Delete removes it.
    frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    assert!(click(&mut f.editor, &f.engine, &f.ctx, c2).is_none());
    assert_eq!(f.editor.selected_corner, Some((f.wire, 1)));
    let (_, sent) = frame(
        &mut f.editor,
        &f.engine,
        &f.ctx,
        vec![key(egui::Key::Delete, egui::Modifiers::NONE)],
    );
    f.accept(sent, "wire.remove_corner")?;
    assert_eq!(f.path()?, vec![a, c1, b]);
    assert!(!f.editor.update.is_open());
    Ok(())
}

/// Delete or Backspace with no corner picked goes through the disconnect
/// review, as the card's footer does.
#[test]
fn delete_on_a_selected_wire_opens_the_disconnect_review() -> TestResult {
    for pressed in [egui::Key::Delete, egui::Key::Backspace] {
        let mut f = fixture()?;
        let wires = f.engine.project().wires.clone();
        frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
        assert!(f.editor.can_delete(f.engine.project()), "the menu's Delete");
        let (_, sent) = frame(
            &mut f.editor,
            &f.engine,
            &f.ctx,
            vec![key(pressed, egui::Modifiers::NONE)],
        );
        assert!(sent.is_none(), "{pressed:?}: nothing until accepted");
        assert!(f.editor.update.is_open(), "{pressed:?}: the review");
        assert_eq!(f.engine.project().wires, wires);
    }
    Ok(())
}

/// ⇧Delete on the hovered segment splits the path, with free ends at the gap;
/// one undo; with the pointer off the path it does nothing.
#[test]
fn shift_delete_splits_at_the_hovered_segment() -> TestResult {
    let mut f = fixture()?;
    let [a, c1, c2, b] = f.v;
    let wires = f.engine.project().wires.clone();
    let shift = egui::Modifiers::SHIFT;
    let (_, sent) = frame(
        &mut f.editor,
        &f.engine,
        &f.ctx,
        vec![
            egui::Event::PointerMoved(pos2(5.0, 5.0)),
            key(egui::Key::Delete, shift),
        ],
    );
    assert!(sent.is_none(), "nothing hovered");
    let p = screen(&f.editor, [c1[0], (c1[1] + c2[1]) * 0.5]);
    frame(
        &mut f.editor,
        &f.engine,
        &f.ctx,
        vec![egui::Event::PointerMoved(p)],
    );
    let (_, sent) = frame(
        &mut f.editor,
        &f.engine,
        &f.ctx,
        vec![key(egui::Key::Delete, shift)],
    );
    assert!(!f.editor.update.is_open());
    f.accept(sent, "wire.remove_segment")?;
    let project = f.engine.project();
    let paths: Vec<Vec<Point>> = project
        .wires
        .iter()
        .filter(|w| w.id == f.wire || !wires.iter().any(|old| old.id == w.id))
        .filter_map(|w| schematic::wire_path(project, w))
        .collect();
    assert_eq!(paths, vec![vec![a, c1], vec![c2, b]]);
    let net = |e| project.net_of(e).map(|n| n.id);
    assert_ne!(net(ep(f.r90, 2)), net(ep(f.r91, 1)), "the split parts them");
    f.engine.undo()?;
    assert_eq!(f.engine.project().wires, wires);
    Ok(())
}

/// A corner dropped within reach of a pin lands on it, and the path connects
/// there, as a point on a path does (a T): the same undo step.
#[test]
fn a_corner_dropped_on_a_pin_lands_on_it_and_joins() -> TestResult {
    let mut f = fixture()?;
    let target = pin(&f.engine, ep(f.r92, 1))?;
    let wires = f.engine.project().wires.clone();
    let nets = f.engine.project().nets.clone();
    frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    // A little off the pin, inside the snap radius.
    let near = [target[0] + 0.3, target[1] + 0.3];
    let (preview, sent) = drag(&mut f.editor, &f.engine, &f.ctx, f.v[1], near);
    assert!(
        preview.is_some_and(|p| p.contains(&target)),
        "it lands exactly on the pin"
    );
    f.accept(sent, "wire.move_corner")?;
    let project = f.engine.project();
    assert_eq!(project.wires_at(ep(f.r92, 1)).count(), 2, "a T at the pin");
    let net = |e| project.net_of(e).map(|n| n.id);
    assert!(net(ep(f.r90, 2)).is_some());
    assert_eq!(net(ep(f.r92, 1)), net(ep(f.r90, 2)));
    assert_eq!(net(ep(f.r91, 1)), net(ep(f.r90, 2)));
    f.engine.undo()?;
    assert_eq!(f.engine.project().wires, wires);
    assert_eq!(f.engine.project().nets, nets);
    Ok(())
}

/// A wire from R90.2 through one corner to a free end, selected, with R91 and
/// R92 unwired; and the free end's position.
fn with_free_end() -> TestResult<(Fixture, Point)> {
    let mut f = fixture()?;
    f.engine.apply(
        f.engine.revision(),
        Operation::Schematic(SchematicCommand::Disconnect { wire: f.wire }),
    )?;
    let a = f.v[0];
    let end = [25.0, a[1] + 10.0];
    f.wire = draw(
        &mut f.engine,
        ep(f.r90, 2).into(),
        &[[25.0, a[1]]],
        free(end)?,
    )?;
    f.editor.selected_wire = Some(f.wire);
    frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    Ok((f, end))
}

/// A free end shows a square; a drag moves it, the last segment keeping its
/// direction; the preview is the result.
#[test]
fn a_free_end_drag_moves_it() -> TestResult {
    let (mut f, end) = with_free_end()?;
    let (shapes, _) = frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    assert_eq!(squares(&shapes).len(), 2, "its corner and its free end");
    let wires = f.engine.project().wires.clone();
    let a = f.v[0];
    let to = [30.0, end[1] + 2.5];
    let (preview, sent) = drag(&mut f.editor, &f.engine, &f.ctx, end, to);
    let expected = vec![a, [30.0, a[1]], to];
    assert_eq!(preview.as_ref(), Some(&expected));
    f.accept(sent, "wire.move_end")?;
    assert_eq!(f.path()?, expected);
    f.engine.undo()?;
    assert_eq!(f.engine.project().wires, wires);
    Ok(())
}

/// Dropped on a pin it attaches; on another free end the two paths join; on a
/// wire's middle it makes a T. Each is one undo step.
#[test]
fn a_free_end_dropped_on_a_target_attaches_or_joins() -> TestResult {
    let (mut f, end) = with_free_end()?;
    let wires = f.engine.project().wires.clone();
    let target = pin(&f.engine, ep(f.r91, 1))?;
    let (_, sent) = drag(&mut f.editor, &f.engine, &f.ctx, end, target);
    f.accept(sent, "wire.move_end")?;
    let project = f.engine.project();
    assert_eq!(
        project.wire(f.wire).map(|w| w.to),
        Some(WireEnd::Terminal(ep(f.r91, 1)))
    );
    let net = |e| project.net_of(e).map(|n| n.id);
    assert!(net(ep(f.r91, 1)).is_some());
    assert_eq!(net(ep(f.r90, 2)), net(ep(f.r91, 1)));
    f.engine.undo()?;
    assert_eq!(f.engine.project().wires, wires);

    // Another free end: one path, no free end left.
    let other_end = [40.0, f.v[0][1] + 10.0];
    draw(
        &mut f.engine,
        ep(f.r91, 1).into(),
        &[[f.v[3][0], other_end[1]]],
        free(other_end)?,
    )?;
    frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    let (_, sent) = drag(&mut f.editor, &f.engine, &f.ctx, end, other_end);
    f.accept(sent, "wire.move_end")?;
    let project = f.engine.project();
    let on_r90: Vec<_> = project.wires_at(ep(f.r90, 2)).collect();
    assert_eq!(on_r90.len(), 1);
    assert_eq!(
        on_r90[0].to,
        WireEnd::Terminal(ep(f.r91, 1)),
        "one path, no free end left"
    );
    f.engine.undo()?;

    // A wire's middle: a T with a junction.
    let (r90, r92) = (f.r90, f.r92);
    let x = pin(&f.engine, ep(r92, 1))?[0];
    let target_wire = draw(
        &mut f.engine,
        ep(r92, 1).into(),
        &[],
        free([x, end[1] - 5.0])?,
    )?;
    frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    let at = schematic::nearest_on_wire(f.engine.project(), target_wire, [x, end[1] - 17.5])
        .ok_or("on wire")?;
    let junctions = f.engine.project().junctions.len();
    let (_, sent) = drag(&mut f.editor, &f.engine, &f.ctx, end, at);
    f.accept(sent, "wire.move_end")?;
    let project = f.engine.project();
    assert_eq!(project.junctions.len(), junctions + 1);
    assert_eq!(
        project.net_of(ep(r90, 2)).map(|n| n.id),
        project.net_of(ep(r92, 1)).map(|n| n.id)
    );
    Ok(())
}

/// A free end dropped on a pin of another named net opens the review, as the
/// Wire tool does; nothing changes until it is accepted.
#[test]
fn a_free_end_merging_two_named_nets_opens_the_review() -> TestResult {
    let (mut f, end) = with_free_end()?;
    let port = |name: &str, on: Endpoint| SchematicCommand::PlaceLabel {
        sheet: None,
        name: name.into(),
        kind: LabelKind::Power {
            style: PowerStyle::Ground,
        },
        position: [0.0, 0.0],
        rotation: Default::default(),
        attach: Some(WireTarget::Terminal(on)),
    };
    let (r90, r91) = (f.r90, f.r91);
    for command in [port("VBUS", ep(r90, 2)), port("GND", ep(r91, 1))] {
        f.engine
            .apply(f.engine.revision(), Operation::Schematic(command))?;
    }
    frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    let target = pin(&f.engine, ep(r91, 1))?;
    let revision = f.engine.revision();
    let (_, sent) = drag(&mut f.editor, &f.engine, &f.ctx, end, target);
    assert!(sent.is_none(), "{sent:?}");
    assert!(f.editor.update.is_open());
    assert_eq!(f.engine.revision(), revision);
    Ok(())
}

/// W, then a click on a free end: the path is extended, not a new one.
#[test]
fn w_then_a_click_on_a_free_end_extends_the_path() -> TestResult {
    let (mut f, end) = with_free_end()?;
    let a = f.v[0];
    f.editor.on_tool("wire", &f.engine);
    let (shapes, _) = frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    assert!(squares(&shapes).is_empty(), "no handles with the Wire tool");
    assert!(click(&mut f.editor, &f.engine, &f.ctx, end).is_none());
    assert_eq!(f.editor.wire.as_ref().map(|d| d.start), Some(end));
    let target = pin(&f.engine, ep(f.r91, 1))?;
    let Some(sent) = click(&mut f.editor, &f.engine, &f.ctx, target) else {
        return Err("no wire was drawn".into());
    };
    let wires = f.engine.project().wires.clone();
    f.engine.apply(f.engine.revision(), sent)?;
    let project = f.engine.project();
    let on_r90: Vec<_> = project.wires_at(ep(f.r90, 2)).collect();
    let [only] = &on_r90[..] else {
        return Err(format!("{on_r90:?}").into());
    };
    assert_eq!(only.to, WireEnd::Terminal(ep(f.r91, 1)), "one path");
    let path = schematic::wire_path(project, only).ok_or("path")?;
    assert!(path.contains(&[25.0, a[1]]), "{path:?}");
    assert_eq!(project.wires.len(), wires.len(), "the path grew");
    f.engine.undo()?;
    assert_eq!(f.engine.project().wires, wires, "one undo");
    Ok(())
}

/// A symbol still takes a drag on top of the selected path's segment, and a
/// part elsewhere moves as usual while a wire is selected.
#[test]
fn a_symbol_goes_first_over_a_segment() -> TestResult {
    let mut f = fixture()?;
    frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    let occurrence = f.engine.project().occurrences_of(f.r92).next().ok_or("o")?;
    let at = occurrence.position;
    let (preview, sent) = drag(&mut f.editor, &f.engine, &f.ctx, at, [at[0], at[1] - 10.0]);
    assert!(preview.is_none());
    assert!(
        matches!(
            sent,
            Some(Operation::Schematic(SchematicCommand::MoveUnit { .. }))
        ),
        "{sent:?}"
    );
    Ok(())
}

/// The hint for a selected path fits an 800-point canvas.
#[test]
fn the_path_hint_is_not_cut_short() -> TestResult {
    let mut f = fixture()?;
    let hint = canvas_hint(&f.editor, f.engine.project()).unwrap_or_default();
    assert!(hint.contains("corner"), "{hint}");
    let (shapes, _) = frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    let found: Vec<bool> = shapes
        .iter()
        .filter_map(|s| match s {
            egui::Shape::Text(t) if t.galley.job.text == hint => Some(t.galley.elided),
            _ => None,
        })
        .collect();
    assert_eq!(found, vec![false], "{hint}");
    Ok(())
}

/// Handles stay apart at normal zoom on the shortest on-grid segments: no two
/// overlap at the Fit-page and working scales, because a segment too short for
/// a dot shows none, and a square keeps its grab over the dot beside it.
#[test]
fn handles_are_not_crowded_on_short_segments() {
    let grid_mm = 2.5;
    // A staircase of one-grid steps with a free end at each side.
    let path: Vec<Point> = (0..8)
        .map(|i| {
            let k = f64::from(i / 2) * grid_mm;
            if i % 2 == 0 { [k, k] } else { [k + grid_mm, k] }
        })
        .collect();
    for scale in [6.0_f32, 8.0, 12.0, 20.0, 40.0] {
        let points: Vec<Pos2> = path
            .iter()
            .map(|p| pos2(p[0] as f32 * scale, p[1] as f32 * scale))
            .collect();
        let points = path_edit::ScreenPath::sharp(points);
        let handles = path_edit::handles(&points, [true, true]);
        let reach = |grab: &Grab| match grab {
            Grab::Segment(_) => path_edit::MIDPOINT_RADIUS_PX,
            _ => path_edit::HANDLE_HALF_PX * std::f32::consts::SQRT_2,
        };
        for (i, (a, p)) in handles.iter().enumerate() {
            for (b, q) in handles.iter().skip(i + 1) {
                assert!(
                    p.distance(*q) >= reach(a) + reach(b),
                    "{scale} px/mm: {a:?} at {p:?} overlaps {b:?} at {q:?}"
                );
            }
        }
        // Every square is reached before a dot or segment beside it.
        for (grab, at) in handles
            .iter()
            .filter(|(g, _)| !matches!(g, Grab::Segment(_)))
        {
            assert_eq!(
                path_edit::grab_at(&points, [true, true], *at),
                Some(*grab),
                "{scale} px/mm"
            );
        }
    }
}

/// The end kinds show as squares only for free ends.
#[test]
fn handles_list_corners_free_ends_and_long_segments() {
    let points =
        path_edit::ScreenPath::sharp(vec![pos2(0.0, 0.0), pos2(100.0, 0.0), pos2(100.0, 10.0)]);
    let list = path_edit::handles(&points, [false, true]);
    let grabs: Vec<Grab> = list.iter().map(|(g, _)| *g).collect();
    assert_eq!(
        grabs,
        vec![Grab::Corner(0), Grab::End(WireSide::To), Grab::Segment(0)],
        "no square on the attached end, no dot on the short segment"
    );
}

/// Round corner 0 of the fixture's dogleg by `radius` through the engine.
fn round(f: &mut Fixture, radius: f64) -> TestResult {
    let operation = Operation::Schematic(SchematicCommand::SetWireRadius {
        wire: f.wire,
        index: 0,
        radius,
    });
    f.engine.apply(f.engine.revision(), operation)?;
    Ok(())
}

/// Where the middle of a `radius` arc lies at the fixture's corner 0, which
/// turns from +x (R90.2) to +y (towards R91): on the line halving it, inward.
fn arc_middle(f: &Fixture, radius: f64) -> Point {
    let reach = radius * (std::f64::consts::SQRT_2 - 1.0) / std::f64::consts::SQRT_2;
    [f.v[1][0] - reach, f.v[1][1] + reach]
}

/// A rounded corner is drawn and picked on its arc, not at its cut-off
/// vertex; its radius dot shows in the arc's middle, and dragging the dot
/// sends one `wire.set_radius`, previewed as it lands.
#[test]
fn a_rounded_corner_is_picked_on_its_arc_and_its_dot_sets_the_radius() -> TestResult {
    let mut f = fixture()?;
    round(&mut f, 5.0)?;
    f.editor.selected_wire = None;
    click(&mut f.editor, &f.engine, &f.ctx, f.v[1]);
    assert_eq!(
        f.editor.selected_wire, None,
        "the vertex is off the drawn wire"
    );
    let (big, small) = (arc_middle(&f, 5.0), arc_middle(&f, 2.5));
    click(&mut f.editor, &f.engine, &f.ctx, big);
    assert_eq!(f.editor.selected_wire, Some(f.wire), "the arc is the wire");
    let (shapes, _) = frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    let dot = screen(&f.editor, big);
    assert!(
        shapes.iter().any(|s| matches!(s, egui::Shape::Circle(c)
            if (c.radius - path_edit::ARC_DOT_RADIUS_PX).abs() < 1e-3
                && c.center.distance(dot) < 0.5)),
        "the radius dot sits in the arc's middle"
    );
    let path = f.path()?;
    let (preview, sent) = drag(&mut f.editor, &f.engine, &f.ctx, big, small);
    assert_eq!(preview, Some(path.clone()), "the vertices stay");
    match &sent {
        Some(Operation::Schematic(SchematicCommand::SetWireRadius { radius, index, .. })) => {
            assert_eq!((*index, *radius), (0, 2.5));
        }
        other => return Err(format!("{other:?}").into()),
    }
    f.accept(sent, "wire.set_radius")?;
    let radius = |f: &Fixture| f.engine.project().wire(f.wire).map(|w| w.corners[0].radius);
    assert_eq!(radius(&f), Some(2.5));
    assert_eq!(f.path()?, path);
    f.engine.undo()?;
    assert_eq!(radius(&f), Some(5.0), "one undo step");
    Ok(())
}

/// A on a picked corner rounds it by the Wire tool's radius, and again makes
/// it sharp; each is one undo step.
#[test]
fn a_rounds_the_picked_corner_and_makes_it_sharp() -> TestResult {
    let mut f = fixture()?;
    frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    let a = key(egui::Key::A, egui::Modifiers::NONE);
    let (_, sent) = frame(&mut f.editor, &f.engine, &f.ctx, vec![a.clone()]);
    assert!(sent.is_none(), "no corner picked");
    click(&mut f.editor, &f.engine, &f.ctx, f.v[2]);
    let (_, sent) = frame(&mut f.editor, &f.engine, &f.ctx, vec![a.clone()]);
    f.accept(sent, "wire.set_radius")?;
    let radii = |f: &Fixture| -> Vec<f64> {
        f.engine
            .project()
            .wire(f.wire)
            .map(|w| w.corners.iter().map(|c| c.radius).collect())
            .unwrap_or_default()
    };
    assert_eq!(radii(&f), vec![0.0, 2.5]);
    let (_, sent) = frame(&mut f.editor, &f.engine, &f.ctx, vec![a]);
    f.accept(sent, "wire.set_radius")?;
    assert_eq!(radii(&f), vec![0.0, 0.0]);
    Ok(())
}

/// The Wire tool's A is sticky and rounds every new corner by the card's
/// radius; the drawn wire is still one `wire.draw`.
#[test]
fn the_wire_tool_rounds_new_corners_with_a() -> TestResult {
    let mut f = fixture()?;
    f.editor.selected_wire = None;
    f.editor.tool = Tool::Wire;
    frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    let a = key(egui::Key::A, egui::Modifiers::NONE);
    frame(&mut f.editor, &f.engine, &f.ctx, vec![a]);
    assert!(f.editor.wire_arc);
    click(&mut f.editor, &f.engine, &f.ctx, [0.0, -415.0]);
    click(&mut f.editor, &f.engine, &f.ctx, [15.0, -420.0]);
    let (_, sent) = frame(
        &mut f.editor,
        &f.engine,
        &f.ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    // The drawing applies as it is shown (an update without a review).
    let command = match &sent {
        Some(Operation::Update(update)) => match &update.change {
            Change::Schematic { command } => Some(command),
            _ => None,
        },
        _ => None,
    };
    match command {
        Some(SchematicCommand::DrawWire { corners, .. }) => {
            assert_eq!(
                corners,
                &vec![Corner {
                    at: [15.0, -415.0],
                    radius: 2.5
                }]
            );
        }
        other => return Err(format!("{other:?}").into()),
    }
    f.engine
        .apply(f.engine.revision(), sent.ok_or("nothing sent")?)?;
    assert!(f.editor.wire_arc, "A stays on for the next wire");
    Ok(())
}

/// The toasts posted since the last call, as text.
fn toasts(ctx: &egui::Context) -> Vec<String> {
    crate::kit::toast::take_posted(ctx)
        .into_iter()
        .map(|p| p.text)
        .collect()
}

/// A on a corner too tight for the tool's radius rounds it by the most that
/// fits, and says so; the command itself is still exact. A corner in line
/// stays refused.
#[test]
fn a_rounds_a_tight_corner_by_the_most_that_fits() -> TestResult {
    let mut f = fixture()?;
    // Corner 1 turns 90° between the dogleg's vertical segment and the
    // horizontal one into R91; its neighbour is sharp, so the shorter of the
    // two segments is the largest radius that fits.
    let vertical = (f.v[2][1] - f.v[1][1]).abs();
    let horizontal = (f.v[3][0] - f.v[2][0]).abs();
    let most = vertical.min(horizontal);
    f.editor.wire_radius = most + 10.0;
    frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    click(&mut f.editor, &f.engine, &f.ctx, f.v[2]);
    toasts(&f.ctx);
    let a = key(egui::Key::A, egui::Modifiers::NONE);
    let (_, sent) = frame(&mut f.editor, &f.engine, &f.ctx, vec![a]);
    f.accept(sent, "wire.set_radius")?;
    let radius = f
        .engine
        .project()
        .wire(f.wire)
        .map(|w| w.corners[1].radius)
        .ok_or("wire")?;
    assert!((radius - most).abs() < 1e-3, "{radius} vs {most}");
    let text = format!("{radius:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    assert_eq!(
        toasts(&f.ctx),
        vec![format!("Rounded with {text} mm, the most that fits")]
    );
    // Programmatic calls stay exact: the command refuses the tool's radius.
    f.engine.undo()?;
    let refused = f.engine.apply(
        f.engine.revision(),
        Operation::Schematic(SchematicCommand::SetWireRadius {
            wire: f.wire,
            index: 1,
            radius: most + 10.0,
        }),
    );
    assert!(refused.is_err(), "wire.set_radius refuses, not clamps");
    Ok(())
}

/// A new corner too tight for the card's radius is rounded by the most that
/// fits, and the toast says so.
#[test]
fn the_wire_tool_rounds_a_tight_corner_by_the_most_that_fits() -> TestResult {
    let mut f = fixture()?;
    f.editor.selected_wire = None;
    f.editor.tool = Tool::Wire;
    f.editor.wire_arc = true;
    f.editor.wire_radius = 10.0;
    frame(&mut f.editor, &f.engine, &f.ctx, vec![]);
    click(&mut f.editor, &f.engine, &f.ctx, [0.0, -415.0]);
    click(&mut f.editor, &f.engine, &f.ctx, [15.0, -420.0]);
    toasts(&f.ctx);
    let (_, sent) = frame(
        &mut f.editor,
        &f.engine,
        &f.ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    // The elbow at (15, −415) has a 15 mm and a 5 mm segment: 5 mm fits.
    let command = match &sent {
        Some(Operation::Update(update)) => match &update.change {
            Change::Schematic { command } => Some(command),
            _ => None,
        },
        _ => None,
    };
    match command {
        Some(SchematicCommand::DrawWire { corners, .. }) => {
            assert_eq!(
                corners,
                &vec![Corner {
                    at: [15.0, -415.0],
                    radius: 5.0
                }]
            );
        }
        other => return Err(format!("{other:?}").into()),
    }
    assert_eq!(
        toasts(&f.ctx),
        vec!["Rounded with 5 mm, the most that fits".to_owned()]
    );
    Ok(())
}
