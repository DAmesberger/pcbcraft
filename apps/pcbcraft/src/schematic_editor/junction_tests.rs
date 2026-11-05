//! Schematic canvas tests of connection marks: solid junction dots where
//! three or more conductors meet (scaled with zoom), no open ring on a wired
//! pin end, the to-do ring on an unconnected one, and the Wire tool landing a
//! wire end on another wire's segment.
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
    let mut operation = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(CANVAS),
            events,
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
    for p in [start, end] {
        let button = |pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [
            vec![egui::Event::PointerMoved(p)],
            vec![button(true)],
            vec![button(false)],
        ] {
            canvas_frame(&mut editor, &engine, &ctx, events);
        }
    }
    let Some(Change::Schematic {
        command: SchematicCommand::Connect { from, to },
    }) = editor.update.change().cloned()
    else {
        return Err(format!("no connect under review: {:?}", editor.update.change()).into());
    };
    assert_eq!(from, WireTarget::Terminal(ep(r92, 1)));
    let WireTarget::OnWire(point) = to else {
        return Err(format!("not on the wire: {to:?}").into());
    };
    assert_eq!(point.wire, wire);
    let on = schematic::nearest_on_wire(engine.project(), wire, point.at).ok_or("on")?;
    assert!((on[0] - point.at[0]).hypot(on[1] - point.at[1]) < 1e-9);
    // The review's change applies as one undo step that joins the nets.
    let mut engine = engine;
    let before = engine.project().clone();
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::Connect { from, to }),
    )?;
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

/// Click the Wire tool at `p` (move, press, release).
fn click(editor: &mut SchematicEditor, engine: &Engine, ctx: &egui::Context, p: Pos2) {
    let button = |pressed| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    for events in [
        vec![egui::Event::PointerMoved(p)],
        vec![button(true)],
        vec![button(false)],
    ] {
        canvas_frame(editor, engine, ctx, events);
    }
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
    click(&mut editor, &engine, &ctx, near);
    assert_eq!(editor.wire_from, Some(WireTarget::Terminal(ep(r92, 2))));
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
    click(&mut editor, &engine, &ctx, p);
    let Some(Change::Schematic {
        command: SchematicCommand::Connect { from, to },
    }) = editor.update.change().cloned()
    else {
        return Err(format!("no connect under review: {:?}", editor.update.change()).into());
    };
    assert_eq!(from, WireTarget::Terminal(ep(r92, 2)));
    assert_eq!(to, WireTarget::Terminal(ep(r93, 1)));
    Ok(())
}
