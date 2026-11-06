//! Schematic canvas tests of connection marks: solid junction dots where
//! three or more conductors meet (scaled with zoom), no open ring on a wired
//! pin end, the to-do ring on an unconnected one; and the Wire tool
//! (docs/schematic-wires.md, "Drawing"): corners, ends, double-click,
//! Enter, Esc, Backspace and F, one `wire.draw` per wire.
use super::*;
use egui::vec2;
use pcbcraft_core::schematic::WirePoint;
use solidcraft_ui_egui::fonts;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CANVAS: Rect = Rect::from_min_max(Pos2::ZERO, pos2(800.0, 800.0));

fn canvas_frame(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (Vec<egui::Shape>, Option<Operation>) {
    canvas_frame_after(editor, engine, ctx, events, 0.4)
}

/// [`canvas_frame`] `seconds` after the last frame: 0.4 s apart, a press
/// and release still click, and two clicks are never a double-click.
fn canvas_frame_after(
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

/// Filled circles of `radius` and stroked rings of `radius`, by centre.
fn circles(shapes: &[egui::Shape], radius: f32, filled: bool) -> Vec<Pos2> {
    shapes
        .iter()
        .filter_map(|s| match s {
            egui::Shape::Circle(c)
                if (c.radius - radius).abs() < 1e-3
                    && (c.fill != egui::Color32::TRANSPARENT) == filled =>
            {
                Some(c.center)
            }
            _ => None,
        })
        .collect()
}

fn near(points: &[Pos2], p: Pos2) -> bool {
    points.iter().any(|q| q.distance(p) < 0.5)
}

/// The demo plus R90 and R91 wired end to end far below it, and R92, R93
/// unwired above that wire.
fn engine() -> Result<(Engine, [Id; 4], Id), Box<dyn std::error::Error>> {
    let mut engine = Engine::demo()?;
    let first = engine.project().instances[0].clone();
    let mut ids = [0; 4];
    for (i, (name, at)) in [
        ("R90", [0.0, -400.0]),
        ("R91", [40.0, -400.0]),
        ("R92", [20.0, -380.0]),
        ("R93", [60.0, -380.0]),
    ]
    .into_iter()
    .enumerate()
    {
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
        ids[i] = engine
            .project()
            .instance_by_designator(name)
            .map(|i| i.id)
            .ok_or("placed")?;
    }
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::Connect {
            from: ep(ids[0], 2).into(),
            to: ep(ids[1], 1).into(),
        }),
    )?;
    let wire = engine
        .project()
        .wires_at(ep(ids[0], 2))
        .next()
        .map(|w| w.id)
        .ok_or("wire")?;
    Ok((engine, ids, wire))
}

fn ep(instance: Id, terminal: u32) -> Endpoint {
    Endpoint {
        instance,
        terminal: pcbcraft_core::TerminalId(terminal),
    }
}

/// A point on `wire` halfway along its route.
fn middle(engine: &Engine, wire: Id) -> Result<Point, Box<dyn std::error::Error>> {
    let project = engine.project();
    let (a, b) = schematic::wire_ends(project, project.wire(wire).ok_or("wire")?).ok_or("ends")?;
    let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    Ok(schematic::nearest_on_wire(project, wire, mid).ok_or("on wire")?)
}

/// An editor whose view centres `p` at `scale` pixels per millimetre.
fn centred(p: Point, scale: f32) -> SchematicEditor {
    SchematicEditor {
        scale,
        pan: vec2(-(p[0] as f32) * scale, p[1] as f32 * scale),
        ..Default::default()
    }
}

fn screen(editor: &SchematicEditor, p: Point) -> Pos2 {
    symbol_view::View {
        origin: CANVAS.center() + editor.pan,
        scale: editor.scale,
    }
    .screen(p)
}

#[test]
fn junction_dots_mark_three_conductors_and_scale_with_zoom() -> TestResult {
    let (mut engine, [r90, r91, r92, r93], wire) = engine()?;
    let at = middle(&engine, wire)?;
    // R92.1 lands on the wire's middle: a T with a junction.
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::Connect {
            from: ep(r92, 1).into(),
            to: WireTarget::OnWire(WirePoint { wire, at }),
        }),
    )?;
    // R93.1 ends on R91.1, which already has a wire: end on end at a pin.
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::Connect {
            from: ep(r93, 1).into(),
            to: ep(r91, 1).into(),
        }),
    )?;
    let project = engine.project();
    let junction = project.junctions.first().ok_or("junction")?.position;
    let pin = |e| schematic::pin_position(project, e).ok_or("pin");
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    for scale in [10.0, 20.0] {
        let mut editor = centred(junction, scale);
        let (shapes, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
        let dots = circles(&shapes, dot_radius(scale), true);
        assert_eq!(dots.len(), 2, "scale {scale}: {dots:?}");
        assert!(near(&dots, screen(&editor, junction)));
        assert!(near(&dots, screen(&editor, pin(ep(r91, 1))?)));
        // Wired pin ends carry no open ring; unconnected ones keep the
        // Issues to-do ring.
        // The ring sits just outside the pin end, touching it, so the pin
        // line stops at its edge: its centre is one radius plus half its
        // stroke from the end.
        let (radius, width) = pin_ring(scale);
        let rings = circles(&shapes, radius, false);
        let ringed = |e| -> Result<bool, Box<dyn std::error::Error>> {
            let end = screen(&editor, pin(e)?);
            Ok(rings
                .iter()
                .any(|c| (c.distance(end) - (radius + 0.5 * width)).abs() < 0.05))
        };
        for wired in [ep(r90, 2), ep(r91, 1), ep(r92, 1), ep(r93, 1)] {
            assert!(!ringed(wired)?, "{wired:?}");
        }
        for open in [ep(r90, 1), ep(r92, 2)] {
            assert!(ringed(open)?, "{open:?}");
        }
        // Proportioned to the pin text: diameter 0.8 of its height.
        assert!((2.0 * radius - 0.8 * scale).abs() < 1e-3, "scale {scale}");
    }
    assert!((dot_radius(20.0) - 2.0 * dot_radius(10.0)).abs() < 1e-6);
    Ok(())
}

#[test]
fn a_plain_wire_end_on_a_pin_has_no_dot_and_no_ring() -> TestResult {
    let (engine, [r90, ..], _) = engine()?;
    let end = schematic::pin_position(engine.project(), ep(r90, 2)).ok_or("pin")?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = centred(end, 12.0);
    let (shapes, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
    assert!(circles(&shapes, dot_radius(12.0), true).is_empty());
    let (radius, width) = pin_ring(12.0);
    let rings = circles(&shapes, radius, false);
    assert!(
        !rings
            .iter()
            .any(|c| c.distance(screen(&editor, end)) < radius + width)
    );
    Ok(())
}

#[test]
fn the_wire_tool_lands_a_wire_end_on_a_segment() -> TestResult {
    let (engine, [_, _, r92, _], wire) = engine()?;
    let at = middle(&engine, wire)?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = centred(at, 12.0);
    editor.on_tool("wire", &engine);
    let start = screen(
        &editor,
        schematic::pin_position(engine.project(), ep(r92, 1)).ok_or("pin")?,
    );
    // Slightly off the wire: the click still lands on it, snapped to the grid.
    let end = screen(&editor, at) + vec2(0.4, 2.0);
    click(&mut editor, &engine, &ctx, start);
    let operation = click(&mut editor, &engine, &ctx, end);
    let operation = operation.ok_or("nothing sent")?;
    let (from, _, to) = draw_of(Some(operation.clone()))?;
    assert_eq!(from, WireTarget::Terminal(ep(r92, 1)));
    let WireTarget::OnWire(point) = to else {
        return Err(format!("not on the wire: {to:?}").into());
    };
    assert_eq!(point.wire, wire);
    let on = schematic::nearest_on_wire(engine.project(), wire, point.at).ok_or("on")?;
    assert!((on[0] - point.at[0]).hypot(on[1] - point.at[1]) < 1e-9);
    // The wire applies as one undo step that joins the nets.
    let mut engine = engine;
    let before = engine.project().clone();
    engine.apply(engine.revision(), operation)?;
    let net = |e| engine.project().net_of(e).map(|n| n.id);
    assert_eq!(
        net(ep(r92, 1)),
        schematic::wire_net(engine.project(), engine.project().wire(wire).ok_or("wire")?)
            .map(|n| n.id)
    );
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    Ok(())
}

/// Click the Wire tool at `p` (move, press, release); the operation the
/// click sent, if any.
fn click(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    p: Pos2,
) -> Option<Operation> {
    let button = |pressed| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut sent = None;
    for events in [
        vec![egui::Event::PointerMoved(p)],
        vec![button(true)],
        vec![button(false)],
    ] {
        sent = canvas_frame(editor, engine, ctx, events).1.or(sent);
    }
    sent
}

/// The wire the Wire tool applied directly: one `ApplyUpdate` of one
/// `wire.draw`, with no review opened; its ends and corners.
fn draw_of(
    operation: Option<Operation>,
) -> Result<(WireTarget, Vec<Point>, WireTarget), Box<dyn std::error::Error>> {
    let Some(Operation::Update(update)) = operation else {
        return Err(format!("no direct join: {operation:?}").into());
    };
    let Change::Schematic {
        command:
            SchematicCommand::DrawWire {
                from, corners, to, ..
            },
    } = update.change
    else {
        return Err(format!("not a wire.draw: {:?}", update.change).into());
    };
    assert!(!update.remove_conflicting_tracks);
    Ok((from, corners.into_iter().map(|c| c.at).collect(), to))
}

#[test]
fn the_wire_tool_snaps_to_pin_ends_and_pin_lines_with_a_marker() -> TestResult {
    let (engine, [_, _, r92, r93], _) = engine()?;
    let project = engine.project();
    let start = schematic::pin_position(project, ep(r92, 2)).ok_or("pin")?;
    let occurrence = project
        .occurrences_of(r93)
        .next()
        .map(|o| o.id)
        .ok_or("occurrence")?;
    let placed = schematic::placed_unit(project, occurrence)?;
    let pin = placed
        .pins
        .iter()
        .find(|p| p.terminal == pcbcraft_core::TerminalId(1))
        .ok_or("R93.1")?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = centred(start, 12.0);
    editor.on_tool("wire", &engine);
    // Ten pixels off R92.2's end, beyond the old 9 px pick radius: the
    // marker sits exactly on the pin end, and the click lands there.
    let near = screen(&editor, start) + vec2(8.0, -6.0);
    canvas_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(near)],
    );
    let (shapes, _) = canvas_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(near)],
    );
    let markers = circles(&shapes, SNAP_MARKER_PX, false);
    assert!(
        markers
            .iter()
            .any(|c| c.distance(screen(&editor, start)) < 1e-3),
        "{markers:?}"
    );
    assert!(click(&mut editor, &engine, &ctx, near).is_none());
    assert_eq!(
        editor.wire.as_ref().map(|d| d.from),
        Some(WireTarget::Terminal(ep(r92, 2)))
    );
    // Halfway along R93.1's drawn line, well away from its end: the wire
    // still lands on the connection end.
    let along = [
        0.5 * (pin.end[0] + pin.line_end[0]),
        0.5 * (pin.end[1] + pin.line_end[1]),
    ];
    let p = screen(&editor, along) + vec2(0.0, 2.0);
    assert!(
        p.distance(screen(&editor, pin.end)) > SNAP_PX,
        "the pin is long enough"
    );
    let (from, _, to) = draw_of(click(&mut editor, &engine, &ctx, p))?;
    assert_eq!(from, WireTarget::Terminal(ep(r92, 2)));
    assert_eq!(to, WireTarget::Terminal(ep(r93, 1)));
    Ok(())
}

/// e2e UX finding 9: an ordinary wire that joins two nets applies at once,
/// as one undo step, keeps the Wire tool, opens no review and says what
/// it changed in a toast once the engine took it.
#[test]
fn an_ordinary_join_applies_directly_with_a_status_toast() -> TestResult {
    let (mut engine, [_, _, r92, r93], _) = engine()?;
    let project = engine.project();
    let start = schematic::pin_position(project, ep(r92, 2)).ok_or("pin")?;
    let end = schematic::pin_position(project, ep(r93, 1)).ok_or("pin")?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = centred(start, 12.0);
    editor.on_tool("wire", &engine);
    let a = screen(&editor, start);
    let b = screen(&editor, end);
    assert!(click(&mut editor, &engine, &ctx, a).is_none());
    let operation = click(&mut editor, &engine, &ctx, b).ok_or("nothing sent")?;
    assert!(!editor.update.is_open());
    assert_eq!(editor.tool, Tool::Wire);
    let before = engine.project().clone();
    engine.apply(engine.revision(), operation)?;
    assert_eq!(
        engine.project().net_of(ep(r92, 2)).map(|n| n.id),
        engine.project().net_of(ep(r93, 1)).map(|n| n.id)
    );
    crate::kit::toast::take_posted(&ctx);
    for _ in 0..2 {
        canvas_frame(&mut editor, &engine, &ctx, vec![]);
    }
    let posted = crate::kit::toast::take_posted(&ctx);
    assert!(
        posted.iter().any(|p| p.text.starts_with("Drew the wire")
            && p.actions == vec![crate::kit::toast::ToastAction::Undo]),
        "{:?}",
        posted.iter().map(|p| &p.text).collect::<Vec<_>>()
    );
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    Ok(())
}

/// A wire that would merge two nets a person named (VBUS and GND) still
/// opens the review, and applies nothing until Apply.
#[test]
fn merging_two_named_nets_opens_the_review() -> TestResult {
    let (mut engine, [_, _, r92, r93], _) = engine()?;
    for (instance, name) in [(r92, "VBUS"), (r93, "GND")] {
        let at = schematic::pin_position(engine.project(), ep(instance, 2)).ok_or("pin")?;
        engine.apply(
            engine.revision(),
            Operation::Schematic(SchematicCommand::PlaceLabel {
                sheet: None,
                name: name.into(),
                kind: pcbcraft_core::model::LabelKind::Local,
                position: at,
                rotation: Default::default(),
                attach: Some(WireTarget::Terminal(ep(instance, 2))),
            }),
        )?;
    }
    let project = engine.project();
    let start = schematic::pin_position(project, ep(r92, 2)).ok_or("pin")?;
    let end = schematic::pin_position(project, ep(r93, 2)).ok_or("pin")?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mid = [(start[0] + end[0]) / 2.0, (start[1] + end[1]) / 2.0];
    let mut editor = centred(mid, 8.0);
    editor.on_tool("wire", &engine);
    // R92.2 carries the VBUS label, R93.2 the GND label.
    let a = screen(&editor, start);
    let b = screen(&editor, end);
    let revision = engine.revision();
    assert!(click(&mut editor, &engine, &ctx, a).is_none());
    assert!(editor.wire.is_some(), "not started");
    assert!(click(&mut editor, &engine, &ctx, b).is_none());
    assert!(editor.update.is_open());
    assert!(matches!(
        editor.update.change(),
        Some(Change::Schematic {
            command: SchematicCommand::DrawWire { .. }
        })
    ));
    assert_eq!(engine.revision(), revision);
    Ok(())
}

/// e2e UX finding 10: J arms the Junction tool (its own card, not the
/// Wire card); a click on a wire places a junction point drawn as a dot;
/// a click on no wire says so.
#[test]
fn j_places_a_visible_junction_dot_and_a_miss_says_so() -> TestResult {
    let registry = crate::commands::Registry::standard();
    let command = registry
        .resolve(
            crate::commands::Chord::key(egui::Key::J),
            &crate::commands::active_contexts(crate::commands::Context::Schematic),
        )
        .ok_or("J is unbound")?;
    assert_eq!(command.id, "schematic.junction");
    let (mut engine, _, wire) = engine()?;
    let at = middle(&engine, wire)?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = centred(at, 12.0);
    editor.on_tool("junction", &engine);
    assert_eq!(editor.tool, Tool::Junction);
    assert_eq!(editor.header(&engine).title, "Junction point");
    let on_wire = screen(&editor, at) + vec2(0.4, 2.0);
    let operation = click(&mut editor, &engine, &ctx, on_wire).ok_or("nothing placed")?;
    let Operation::Schematic(SchematicCommand::PlaceJunction { position, .. }) = &operation else {
        return Err(format!("not a junction: {operation:?}").into());
    };
    let placed = *position;
    engine.apply(engine.revision(), operation)?;
    let (shapes, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
    let dots = circles(&shapes, dot_radius(12.0), true);
    assert!(near(&dots, screen(&editor, placed)), "{dots:?}");
    // Well off every wire: nothing is sent, and a toast says why.
    crate::kit::toast::take_posted(&ctx);
    let off = screen(&editor, [at[0], at[1] + 12.5]);
    assert!(click(&mut editor, &engine, &ctx, off).is_none());
    let posted = crate::kit::toast::take_posted(&ctx);
    assert!(
        posted.iter().any(|p| p.text == JUNCTION_MISS),
        "{:?}",
        posted.iter().map(|p| &p.text).collect::<Vec<_>>()
    );
    assert!(pcbcraft_core::issues::style::check(JUNCTION_MISS).is_empty());
    Ok(())
}

/// Press `key` on the canvas; the operation it sent, if any.
fn key(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    key: egui::Key,
) -> Option<Operation> {
    let event = egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    canvas_frame(editor, engine, ctx, vec![event]).1
}

/// Click at the sheet point `p`.
fn tap(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    p: Point,
) -> Option<Operation> {
    let at = screen(editor, p);
    click(editor, engine, ctx, at)
}

fn pin(engine: &Engine, e: Endpoint) -> Result<Point, Box<dyn std::error::Error>> {
    Ok(schematic::pin_position(engine.project(), e).ok_or("pin")?)
}

/// The stored path of the one wire on `e`.
fn path_at(engine: &Engine, e: Endpoint) -> Result<Vec<Point>, Box<dyn std::error::Error>> {
    let project = engine.project();
    let wire = project.wires_at(e).next().ok_or("no wire")?;
    Ok(schematic::wire_path(project, wire).ok_or("path")?)
}

fn free(p: Point) -> Result<WireTarget, Box<dyn std::error::Error>> {
    Ok(WireTarget::Free(
        pcbcraft_core::model::FreeEnd::new(p).ok_or("off the sheet")?,
    ))
}

/// The Wire tool armed over R92.2, the context fonts loaded.
fn wiring(engine: &Engine, at: Point) -> (SchematicEditor, egui::Context) {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = centred(at, 8.0);
    editor.on_tool("wire", engine);
    (editor, ctx)
}

#[test]
fn the_wire_tool_draws_corners_and_ends_on_a_pin_in_one_undo_step() -> TestResult {
    let (mut engine, [_, _, r92, r93], _) = engine()?;
    let (a, b) = (pin(&engine, ep(r92, 2))?, pin(&engine, ep(r93, 1))?);
    let up = [a[0], a[1] + 10.0];
    let over = [b[0], a[1] + 10.0];
    let (mut editor, ctx) = wiring(&engine, a);
    let at = |p| screen(&editor, p);
    let (sa, sup, sover, sb) = (at(a), at(up), at(over), at(b));
    for p in [sa, sup, sover] {
        assert!(click(&mut editor, &engine, &ctx, p).is_none(), "{p:?}");
    }
    assert_eq!(editor.wire.as_ref().map(|d| d.legs.len()), Some(2));
    // The preview runs through the corners to the pointer.
    let operation = click(&mut editor, &engine, &ctx, sb);
    let (from, _, to) = draw_of(operation.clone())?;
    assert_eq!(
        (from, to),
        (ep(r92, 2).into(), WireTarget::Terminal(ep(r93, 1)))
    );
    assert!(editor.wire.is_none());
    assert_eq!(editor.tool, Tool::Wire);
    let before = engine.project().clone();
    engine.apply(engine.revision(), operation.ok_or("nothing sent")?)?;
    assert_eq!(path_at(&engine, ep(r92, 2))?, vec![a, up, over, b]);
    // The whole wire is one undo step.
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    Ok(())
}

#[test]
fn a_double_click_ends_a_wire_with_a_free_end_that_survives_save_and_tidy() -> TestResult {
    let (mut engine, [_, _, r92, _], _) = engine()?;
    let a = pin(&engine, ep(r92, 2))?;
    let up = [a[0], a[1] + 10.0];
    let over = [a[0] + 10.0, a[1] + 10.0];
    let (mut editor, ctx) = wiring(&engine, a);
    for p in [a, up] {
        assert!(tap(&mut editor, &engine, &ctx, p).is_none());
    }
    let s = screen(&editor, over);
    assert!(click(&mut editor, &engine, &ctx, s).is_none());
    // The second click follows the first at once: a double-click.
    let button = |pressed| egui::Event::PointerButton {
        pos: s,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut operation = None;
    for event in [button(true), button(false)] {
        operation = canvas_frame_after(&mut editor, &engine, &ctx, vec![event], 0.05)
            .1
            .or(operation);
    }
    let (_, _, to) = draw_of(operation.clone())?;
    assert_eq!(to, free(over)?);
    engine.apply(engine.revision(), operation.ok_or("nothing sent")?)?;
    assert_eq!(path_at(&engine, ep(r92, 2))?, vec![a, up, over]);
    let bytes = pcbcraft_core::format::encode(&engine.to_file())?;
    let opened = Engine::from_file(pcbcraft_core::format::decode(&bytes)?)?;
    assert_eq!(opened.project(), engine.project());
    let mut tidied = engine.project().clone();
    schematic::tidy(&mut tidied)?;
    assert_eq!(&tidied, engine.project());
    // ERC reports the free end on this wire.
    let wire = engine.project().wires_at(ep(r92, 2)).next().ok_or("wire")?.id;
    assert!(
        pcbcraft_core::erc::checks(engine.project())
            .iter()
            .any(|f| f.code == "erc.wire_end_free"
                && f.objects == vec![ObjectRef::Wire { id: wire }])
    );
    Ok(())
}

#[test]
fn esc_ends_a_wire_with_corners_and_cancels_one_without() -> TestResult {
    let (engine, [_, _, r92, _], _) = engine()?;
    let a = pin(&engine, ep(r92, 2))?;
    let up = [a[0], a[1] + 10.0];
    let (mut editor, ctx) = wiring(&engine, a);
    // No corner yet: Esc drops the wire and keeps the tool; again: Select.
    assert!(tap(&mut editor, &engine, &ctx, a).is_none());
    assert!(key(&mut editor, &engine, &ctx, egui::Key::Escape).is_none());
    assert!(editor.wire.is_none());
    assert_eq!(editor.tool, Tool::Wire);
    assert!(key(&mut editor, &engine, &ctx, egui::Key::Escape).is_none());
    assert_eq!(editor.tool, Tool::Select);
    // With a corner fixed, Esc ends the wire there with a free end.
    editor.on_tool("wire", &engine);
    for p in [a, up] {
        assert!(tap(&mut editor, &engine, &ctx, p).is_none());
    }
    let (from, _, to) = draw_of(key(&mut editor, &engine, &ctx, egui::Key::Escape))?;
    assert_eq!((from, to), (ep(r92, 2).into(), free(up)?));
    assert_eq!(editor.tool, Tool::Wire);
    Ok(())
}

#[test]
fn backspace_takes_back_a_corner_and_enter_ends_at_the_last_one() -> TestResult {
    let (mut engine, [_, _, r92, _], _) = engine()?;
    let a = pin(&engine, ep(r92, 2))?;
    let up = [a[0], a[1] + 10.0];
    let over = [a[0] + 10.0, a[1] + 10.0];
    let (mut editor, ctx) = wiring(&engine, a);
    for p in [a, up, over] {
        assert!(tap(&mut editor, &engine, &ctx, p).is_none());
    }
    assert!(key(&mut editor, &engine, &ctx, egui::Key::Backspace).is_none());
    assert_eq!(editor.wire.as_ref().map(|d| d.legs.len()), Some(1));
    let operation = key(&mut editor, &engine, &ctx, egui::Key::Enter);
    let (_, _, to) = draw_of(operation.clone())?;
    assert_eq!(to, free(up)?);
    engine.apply(engine.revision(), operation.ok_or("nothing sent")?)?;
    assert_eq!(path_at(&engine, ep(r92, 2))?, vec![a, up]);
    // Backspace with no corner left drops the wire.
    assert!(tap(&mut editor, &engine, &ctx, over).is_none());
    assert!(key(&mut editor, &engine, &ctx, egui::Key::Backspace).is_none());
    assert!(editor.wire.is_none());
    Ok(())
}

#[test]
fn f_flips_the_elbow_to_vertical_first() -> TestResult {
    let (mut engine, [_, _, r92, _], _) = engine()?;
    let a = pin(&engine, ep(r92, 2))?;
    let d = [a[0] + 10.0, a[1] + 10.0];
    let (mut editor, ctx) = wiring(&engine, a);
    assert!(tap(&mut editor, &engine, &ctx, a).is_none());
    assert!(key(&mut editor, &engine, &ctx, egui::Key::F).is_none());
    assert!(editor.wire_flipped);
    assert!(tap(&mut editor, &engine, &ctx, d).is_none());
    let operation = key(&mut editor, &engine, &ctx, egui::Key::Enter);
    engine.apply(engine.revision(), operation.ok_or("nothing sent")?)?;
    assert_eq!(path_at(&engine, ep(r92, 2))?, vec![a, [a[0], d[1]], d]);
    Ok(())
}

#[test]
fn a_wire_drawn_across_another_does_not_join_it() -> TestResult {
    let (mut engine, [r90, _, r92, _], wire) = engine()?;
    let c = pin(&engine, ep(r92, 1))?;
    let y = middle(&engine, wire)?[1];
    let below = [c[0], y - 10.0];
    let (mut editor, ctx) = wiring(&engine, c);
    for p in [c, below] {
        assert!(tap(&mut editor, &engine, &ctx, p).is_none());
    }
    let operation = key(&mut editor, &engine, &ctx, egui::Key::Enter);
    engine.apply(engine.revision(), operation.ok_or("nothing sent")?)?;
    assert_eq!(path_at(&engine, ep(r92, 1))?, vec![c, below]);
    let project = engine.project();
    assert!(project.junctions.is_empty());
    assert_ne!(
        project.net_of(ep(r92, 1)).map(|n| n.id),
        project.net_of(ep(r90, 2)).map(|n| n.id)
    );
    Ok(())
}

/// The canvas hint is painted whole at the tests' canvas width, idle and
/// while drawing.
#[test]
fn the_wire_hints_are_not_cut_short() -> TestResult {
    let (engine, [_, _, r92, _], _) = engine()?;
    let a = pin(&engine, ep(r92, 2))?;
    let (mut editor, ctx) = wiring(&engine, a);
    let hint_whole = |shapes: &[egui::Shape], editor: &SchematicEditor| {
        let hint = canvas_hint(editor, engine.project()).unwrap_or_default();
        let found: Vec<bool> = shapes
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(t) if t.galley.job.text == hint => Some(t.galley.elided),
                _ => None,
            })
            .collect();
        (hint.clone(), found == vec![false])
    };
    let (shapes, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
    let (hint, whole) = hint_whole(&shapes, &editor);
    assert!(whole, "{hint}");
    assert!(tap(&mut editor, &engine, &ctx, a).is_none());
    let (shapes, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
    let (hint, whole) = hint_whole(&shapes, &editor);
    assert!(hint.contains("corner"), "{hint}");
    assert!(whole, "{hint}");
    Ok(())
}
