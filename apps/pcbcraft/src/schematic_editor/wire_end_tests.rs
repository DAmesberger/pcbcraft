//! Where a wire may end, from the e2e batch check: on a port already wired,
//! on a pin marked no-connect, and on a pin that cannot be wired; a cancelled
//! review goes back to the drawing; and deleting a label on routed copper
//! opens the removal review.
use super::*;
use crate::kit::toast;
use egui::vec2;
use pcbcraft_core::geometry::Shape;
use pcbcraft_core::model::{LabelKind, PowerStyle};
use pcbcraft_core::schematic::scene;
use solidcraft_ui_egui::fonts;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const CANVAS: Rect = Rect::from_min_max(Pos2::ZERO, pos2(800.0, 800.0));

fn frame(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (Vec<egui::Shape>, Option<Operation>) {
    let mut operation = None;
    let time = ctx.input(|i| i.time) + 0.4;
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
        sent = frame(editor, engine, ctx, events).1.or(sent);
    }
    sent
}

/// [`click`] at the sheet point `at`.
fn click_at(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    at: Point,
) -> Option<Operation> {
    let p = screen(editor, at);
    click(editor, engine, ctx, p)
}

fn ep(instance: Id, terminal: u32) -> Endpoint {
    Endpoint {
        instance,
        terminal: pcbcraft_core::TerminalId(terminal),
    }
}

fn wiring(engine: &Engine, at: Point) -> (SchematicEditor, egui::Context) {
    let scale = 12.0;
    let mut editor = SchematicEditor {
        scale,
        pan: vec2(-(at[0] as f32) * scale, at[1] as f32 * scale),
        ..Default::default()
    };
    editor.on_tool("wire", engine);
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    (editor, ctx)
}

fn screen(editor: &SchematicEditor, p: Point) -> Pos2 {
    symbol_view::View {
        origin: CANVAS.center() + editor.pan,
        scale: editor.scale,
    }
    .screen(p)
}

/// The demo plus two resistors, far from it: R90 with a ground port on pin 2,
/// and R91.
fn ported() -> TestResult<(Engine, Id, Id, Id)> {
    let mut engine = Engine::demo()?;
    let first = engine.project().instances[0].clone();
    let mut ids = [0; 2];
    for (i, (name, at)) in [("R90", [0.0, -400.0]), ("R91", [0.0, -430.0])]
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
                position: pt(at),
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
        Operation::Schematic(SchematicCommand::PlaceLabel {
            sheet: None,
            name: "GND".into(),
            kind: LabelKind::Power {
                style: PowerStyle::Ground,
            },
            position: pt([0.0, -400.0]),
            rotation: Default::default(),
            attach: Some(WireTarget::Terminal(ep(ids[0], 2))),
        }),
    )?;
    let port = engine.project().labels.last().map(|l| l.id).ok_or("port")?;
    Ok((engine, ids[0], ids[1], port))
}

/// The end of the label's drawing farthest from its connection point: where
/// a person aims for the port, outside the pin end's snap radius.
fn far_end_of(engine: &Engine, label: Id) -> TestResult<Point> {
    let project = engine.project();
    let at = project.label(label).ok_or("label")?;
    let anchor = schematic::labels::anchor(project, at);
    let drawings = scene::sheet_labels(project, at.sheet);
    let (_, drawing) = drawings
        .iter()
        .find(|(id, _)| *id == label)
        .ok_or("no drawing")?;
    drawing
        .strokes
        .iter()
        .filter_map(|s| match s.shape {
            Shape::Line { a, b } => Some([a, b]),
            _ => None,
        })
        .flatten()
        .max_by(|p, q| {
            let d = |r: &Point| (r[0] - anchor[0]).hypot(r[1] - anchor[1]);
            d(p).total_cmp(&d(q))
        })
        .ok_or_else(|| "no line".into())
}

/// F2: a port already wired to its pin stands off that pin on its leader. Its
/// drawing is a place to end a wire: the ring shows on hover at the pin end
/// the wire lands on, a click ends the wire there, and the port stays on its
/// pin.
#[test]
fn a_wire_ends_on_a_port_that_is_already_wired() -> TestResult {
    let (mut engine, r90, r91, port) = ported()?;
    let aim = far_end_of(&engine, port)?;
    let pin_end = schematic::pin_position(engine.project(), ep(r90, 2)).ok_or("pin")?;
    let start = schematic::pin_position(engine.project(), ep(r91, 2)).ok_or("pin")?;
    let (mut editor, ctx) = wiring(&engine, aim);
    let (aim_px, end_px) = (screen(&editor, aim), screen(&editor, pin_end));
    assert!(
        aim_px.distance(end_px) > SNAP_PX,
        "the port is out of the pin end's snap radius, or this tests nothing"
    );
    assert!(click_at(&mut editor, &engine, &ctx, start).is_none());
    assert!(editor.wire.is_some(), "not started");
    let (shapes, _) = frame(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(aim_px)],
    );
    assert!(
        shapes.iter().any(|s| matches!(
            s,
            egui::Shape::Circle(c)
                if (c.radius - SNAP_MARKER_PX).abs() < 1e-3 && c.center.distance(end_px) < 0.5
        )),
        "no snap ring where the wire will land"
    );
    let operation = click(&mut editor, &engine, &ctx, aim_px).ok_or("the click added a corner")?;
    assert!(editor.wire.is_none(), "the wire carries on");
    engine.apply(engine.revision(), operation)?;
    let project = engine.project();
    assert!(
        project
            .wires
            .iter()
            .all(|w| !matches!(w.from, WireEnd::Free(_)) && !matches!(w.to, WireEnd::Free(_))),
        "a free end was left"
    );
    let net = project.net_of(ep(r91, 2)).ok_or("R91 pin 2 has no net")?;
    assert!(net.terminals.contains(&ep(r90, 2)), "not joined to R90");
    let label = project.label(port).ok_or("port gone")?;
    assert_eq!(
        schematic::labels::attached_pin(project, label),
        Some(ep(r90, 2)),
        "the port left its pin"
    );
    Ok(())
}

/// F3: wiring a pin marked no-connect applies with no review, and Issues
/// reports it as the error `instance.no_connect_connected`.
#[test]
fn a_wire_ends_on_a_pin_marked_no_connect_and_issues_says_so() -> TestResult {
    let (mut engine, r90, r91, _) = ported()?;
    engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::SetNoConnect {
            instance: r90,
            terminals: vec![pcbcraft_core::TerminalId(1)],
            no_connect: true,
        }),
    )?;
    let project = engine.project();
    let start = schematic::pin_position(project, ep(r91, 1)).ok_or("pin")?;
    let end = schematic::pin_position(project, ep(r90, 1)).ok_or("pin")?;
    let (mut editor, ctx) = wiring(&engine, end);
    assert!(click_at(&mut editor, &engine, &ctx, start).is_none());
    let operation = click_at(&mut editor, &engine, &ctx, end).ok_or("nothing was sent")?;
    assert!(!editor.update.is_open(), "a review opened");
    assert!(matches!(operation, Operation::Update(_)));
    engine.apply(engine.revision(), operation)?;
    let report = match engine.query(pcbcraft_core::Query::InspectInstance { instance: r90 })? {
        pcbcraft_core::QueryResult::Instance(report) => report,
        other => return Err(format!("unexpected {other:?}").into()),
    };
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == "instance.no_connect_connected"),
        "{:?}",
        report.findings
    );
    Ok(())
}

/// F3: a pin the part leaves unconnected (device NC) cannot be wired, so
/// there is nothing to decide in a review: a toast says why, nothing changes
/// and the drawing goes on.
#[test]
fn a_device_nc_pin_refuses_the_wire_and_keeps_the_drawing() -> TestResult {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft"
    );
    let mut engine = Engine::demo()?;
    engine.replace_document(pcbcraft_io::read_project(std::path::Path::new(path))?)?;
    let project = engine.project();
    let u2 = project.instance_by_designator("U2").ok_or("U2")?;
    let component = project
        .library
        .component(u2.component)
        .ok_or("U2's component")?;
    let nc = component
        .definition
        .terminals
        .iter()
        .find(|t| !t.is_electrical())
        .ok_or("U2 has no device-NC pin")?;
    let nc = Endpoint {
        instance: u2.id,
        terminal: nc.id,
    };
    let end = schematic::pin_position(project, nc).ok_or("pin")?;
    let revision = engine.revision();
    let (mut editor, ctx) = wiring(&engine, end);
    let start = [end[0] + 10.0, end[1] - 20.0];
    assert!(click_at(&mut editor, &engine, &ctx, start).is_none());
    let draft = editor.wire.clone().ok_or("not started")?;
    assert!(click_at(&mut editor, &engine, &ctx, end).is_none());
    assert!(!editor.update.is_open(), "a dead-end review opened");
    assert_eq!(editor.wire, Some(draft), "the drawing was lost");
    assert_eq!(engine.revision(), revision);
    let posted = toast::take_posted(&ctx);
    let said = posted.first().map(|p| p.text.as_str()).ok_or("no toast")?;
    assert!(said.contains("cannot be wired"), "{said}");
    // Plain words that name the pin, not "device-NC or reserved terminal".
    assert!(said.starts_with("U2 pin ‘"), "{said}");
    assert!(said.contains("must stay unconnected"), "{said}");
    assert_eq!(pcbcraft_core::issues::style::check(said), vec![], "{said}");
    Ok(())
}

/// F3: cancelling a real review (a wire merging two named nets) goes back to
/// drawing that wire; applying it does not.
#[test]
fn cancelling_a_wire_review_returns_to_the_drawing() -> TestResult {
    let (mut engine, _, r91, _) = ported()?;
    // R90's pin 2 carries GND; this names R91's net VBUS.
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::PlaceLabel {
            sheet: None,
            name: "VBUS".into(),
            kind: LabelKind::Local,
            position: pt([0.0, -430.0]),
            rotation: Default::default(),
            attach: Some(WireTarget::Terminal(ep(r91, 2))),
        }),
    )?;
    let project = engine.project();
    let r90 = project.instance_by_designator("R90").ok_or("R90")?.id;
    let start = schematic::pin_position(project, ep(r91, 2)).ok_or("pin")?;
    let end = schematic::pin_position(project, ep(r90, 2)).ok_or("pin")?;
    let corner = [start[0] + 15.0, start[1] + 5.0];
    let (mut editor, ctx) = wiring(&engine, end);
    assert!(click_at(&mut editor, &engine, &ctx, start).is_none());
    assert!(click_at(&mut editor, &engine, &ctx, corner).is_none());
    let draft = editor.wire.clone().ok_or("not started")?;
    assert_eq!(draft.legs.len(), 1);
    assert!(click_at(&mut editor, &engine, &ctx, end).is_none());
    assert!(editor.update.is_open(), "no review");
    assert!(editor.wire.is_none());
    editor.update.cancel();
    frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(editor.tool, Tool::Wire);
    assert_eq!(editor.wire, Some(draft), "cancel threw the wire away");
    Ok(())
}

/// F4: deleting a port on a routed net opens the removal review and keeps the
/// selection; the review applies, taking the copper it lists. A label on no
/// copper still goes at once.
#[test]
fn deleting_a_label_on_routed_copper_opens_the_review() -> TestResult {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft"
    );
    let mut engine = Engine::demo()?;
    engine.replace_document(pcbcraft_io::read_project(std::path::Path::new(path))?)?;
    let label = engine
        .project()
        .labels
        .iter()
        .map(|l| l.id)
        .find(|id| {
            let plan = engine.preview_update(Change::Schematic {
                command: SchematicCommand::RemoveLabel { label: *id },
            });
            plan.blockers.is_empty() && !plan.conflicting_tracks.is_empty()
        })
        .ok_or("no label on routed copper")?;
    let direct = engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::RemoveLabel { label }),
    );
    let said = direct.err().ok_or("the engine took a routed removal")?;
    let said = refusal::shown_error(&said);
    assert!(said.contains("routed copper"), "{said}");
    assert_eq!(pcbcraft_core::issues::style::check(&said), vec![], "{said}");

    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    for key in [egui::Key::Delete, egui::Key::Backspace] {
        let mut editor = SchematicEditor::default();
        editor.refresh_connectivity(&engine);
        editor.sheet = engine.project().label(label).ok_or("label")?.sheet;
        editor.select_item(Some(Item::Label(label)), engine.project());
        frame(&mut editor, &engine, &ctx, vec![]);
        let (_, sent) = frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(sent.is_none(), "{key:?} sent {sent:?}");
        assert!(editor.update.is_open(), "{key:?} opened no review");
        assert_eq!(editor.item, Some(Item::Label(label)), "selection lost");
    }
    let plan = engine.preview_update(Change::Schematic {
        command: SchematicCommand::RemoveLabel { label },
    });
    assert_eq!(plan.blockers, Vec::<String>::new());
    engine.apply_update(plan.revision, plan.change, true)?;
    assert!(engine.project().label(label).is_none());
    Ok(())
}

/// Run 3, finding 22: a wire can't start on a pin the part leaves
/// unconnected; the first click says so and no wire starts.
#[test]
fn a_wire_does_not_start_on_a_device_nc_pin() -> TestResult {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft"
    );
    let mut engine = Engine::demo()?;
    engine.replace_document(pcbcraft_io::read_project(std::path::Path::new(path))?)?;
    let project = engine.project();
    let u2 = project.instance_by_designator("U2").ok_or("U2")?;
    let component = project
        .library
        .component(u2.component)
        .ok_or("U2's component")?;
    let nc = component
        .definition
        .terminals
        .iter()
        .find(|t| !t.is_electrical())
        .ok_or("U2 has no device-NC pin")?;
    let nc = Endpoint {
        instance: u2.id,
        terminal: nc.id,
    };
    let end = schematic::pin_position(project, nc).ok_or("pin")?;
    let (mut editor, ctx) = wiring(&engine, end);
    assert!(click_at(&mut editor, &engine, &ctx, end).is_none());
    assert_eq!(editor.wire, None, "no wire starts");
    let posted = toast::take_posted(&ctx);
    let said = posted.first().map(|p| p.text.as_str()).ok_or("no toast")?;
    assert!(said.contains("cannot be wired"), "{said}");
    // Plain words that name the pin, not "device-NC or reserved terminal".
    assert!(said.starts_with("U2 pin ‘"), "{said}");
    assert!(said.contains("must stay unconnected"), "{said}");
    assert_eq!(pcbcraft_core::issues::style::check(said), vec![], "{said}");
    Ok(())
}
