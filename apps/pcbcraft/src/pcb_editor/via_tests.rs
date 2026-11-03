//! PCB 2D with stackups and vias: inner-layer tracks and vias drawn with
//! their toggles, V routing through a via onto the next layer, the Via
//! tool, picking and locating a via.
use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SCREEN: [f32; 2] = [900.0, 700.0];

fn frame(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<Operation>) {
    let mut command = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(SCREEN[0], SCREEN[1]),
            )),
            events,
            ..Default::default()
        },
        |ui| command = editor.canvas(ui, engine),
    );
    output.textures_delta.clear();
    (output, command)
}

/// Where board point `p` is on the test canvas.
fn screen(editor: &PcbEditor, engine: &Engine, p: Point) -> Pos2 {
    let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(SCREEN[0], SCREEN[1]));
    board_view(
        rect,
        engine.project().board_size,
        editor.zoom,
        editor.pan,
        editor.from_bottom,
    )
    .screen(p)
}

fn click(at: Pos2) -> Vec<egui::Event> {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![egui::Event::PointerMoved(at), button(true), button(false)]
}

fn key(at: Pos2, key: egui::Key) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(at),
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

fn circles(output: &egui::FullOutput, fill: Color32) -> usize {
    fn count(shape: &Shape, fill: Color32) -> usize {
        match shape {
            Shape::Vec(shapes) => shapes.iter().map(|s| count(s, fill)).sum(),
            Shape::Circle(c) if c.fill == fill => 1,
            _ => 0,
        }
    }
    output.shapes.iter().map(|s| count(&s.shape, fill)).sum()
}

fn paths_filled(output: &egui::FullOutput, fill: Color32) -> usize {
    fn count(shape: &Shape, fill: Color32) -> usize {
        match shape {
            Shape::Vec(shapes) => shapes.iter().map(|s| count(s, fill)).sum(),
            Shape::Path(p) if p.fill == fill => 1,
            _ => 0,
        }
    }
    output.shapes.iter().map(|s| count(&s.shape, fill)).sum()
}

fn net_of(engine: &Engine, instance: Id, terminal: u32) -> Result<Id, String> {
    engine
        .project()
        .net_of(pcbcraft_core::Endpoint {
            instance,
            terminal: pcbcraft_core::TerminalId(terminal),
        })
        .map(|n| n.id)
        .ok_or_else(|| "net".to_owned())
}

/// The showcase's four copper layers, its vias and its inner tracks all
/// show in Layout, each under its own toggle; a via shows while any layer
/// it joins does. The Route card offers every stackup layer.
#[test]
fn the_showcase_draws_vias_and_inner_tracks_under_their_toggles() -> TestResult {
    let engine = Engine::showcase()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let project = engine.project();
    let vias = project.vias.len();
    assert!(vias >= 4, "the showcase has via examples");
    assert!(project.vias.iter().any(|v| v.tented) && project.vias.iter().any(|v| !v.tented));
    let drill = BoardInk::get().drill;
    assert_eq!(circles(&output, drill), vias, "every via's drill");
    let layers = [
        CopperLayer::Top,
        CopperLayer::Inner(1),
        CopperLayer::Inner(2),
        CopperLayer::Bottom,
    ];
    assert_eq!(editor.stack, layers);
    assert_eq!(
        copper_options(&editor.stack)
            .into_iter()
            .map(|(_, name)| name)
            .collect::<Vec<_>>(),
        ["Top", "Inner 1", "Inner 2", "Bottom"]
    );
    let colors = crate::palette::board();
    for inner in [CopperLayer::Inner(1), CopperLayer::Inner(2)] {
        assert!(project.tracks.iter().any(|t| t.layer == inner));
        let fill = BoardInk::get().on_board(colors.copper_layer(inner), OTHER_SHADE);
        assert!(paths_filled(&output, fill) > 0, "{inner:?} tracks drawn");
    }
    // Hiding Inner 1 hides its tracks only.
    editor.layers.inner[0] = false;
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let inner1 = BoardInk::get().on_board(colors.copper_layer(CopperLayer::Inner(1)), OTHER_SHADE);
    assert_eq!(paths_filled(&output, inner1), 0);
    assert_eq!(
        circles(&output, drill),
        vias,
        "vias join other shown layers"
    );
    // A via shows while one layer it joins does…
    editor.layers.copper_top = false;
    editor.layers.copper_bottom = false;
    editor.layers.inner[1] = false;
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(circles(&output, drill), 0);
    editor.layers.inner[1] = true;
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(circles(&output, drill), vias);
    // …and the via toggle hides them all.
    editor.layers.vias = false;
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(circles(&output, drill), 0);
    // The 3D view shares the inner copper and via toggles.
    let view = editor.layer_view();
    use crate::layer_view::ViewLayer;
    assert!(!view.visible(ViewLayer::Vias));
    assert!(!view.visible(ViewLayer::Copper(CopperLayer::Inner(1))));
    assert!(view.visible(ViewLayer::Copper(CopperLayer::Inner(2))));
    Ok(())
}

/// V during a route places a via at the pointer and moves the route to
/// the next stackup layer; the next click routes to the via, places it and
/// routes on, as one `layout.route_apply` the engine takes, and the route
/// continues from there on the new layer.
#[test]
fn v_routes_through_a_via_onto_the_next_layer() -> TestResult {
    let mut engine = Engine::demo()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    editor.sync(&engine);
    let r1 = engine
        .project()
        .instances
        .iter()
        .find(|i| i.designator == "R1")
        .map(|i| i.id)
        .ok_or("R1")?;
    let net = net_of(&engine, r1, 2)?;
    editor.on_tool("route", &engine);
    editor.net = Some(net);
    editor.route_start = Some([9.2, 8.0]);
    editor.route.posture = pcbcraft_core::layout::posture::Posture::Free;
    let via_at = [9.0, 11.0];
    let at = screen(&editor, &engine, via_at);
    frame(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(at)],
    );
    frame(&mut editor, &engine, &ctx, key(at, egui::Key::V));
    assert_eq!(editor.pending_via, Some((via_at, CopperLayer::Top)));
    assert_eq!(editor.layer, CopperLayer::Bottom);
    let end = screen(&editor, &engine, [14.0, 11.0]);
    let (_, operation) = frame(&mut editor, &engine, &ctx, click(end));
    let operation = operation.ok_or("no layer change requested")?;
    match &operation {
        Operation::Layout(LayoutCommand::ApplyRoute { edits }) => {
            let legs: Vec<_> = edits
                .add
                .iter()
                .map(|r| (r.layer, r.start, r.end))
                .collect();
            assert_eq!(
                legs,
                [
                    (CopperLayer::Top, [9.2, 8.0], via_at),
                    (CopperLayer::Bottom, via_at, [14.0, 11.0])
                ]
            );
            assert_eq!(edits.vias.len(), 1);
            assert_eq!(edits.vias[0].position, via_at);
        }
        other => return Err(format!("unexpected {other:?}").into()),
    }
    let tracks = engine.project().tracks.len();
    engine.apply(engine.revision(), operation)?;
    let project = engine.project();
    assert_eq!(project.vias.len(), 1);
    assert_eq!(project.tracks.len(), tracks + 2);
    // The step continues the route from its end on the new layer.
    frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(editor.route_start, Some([14.0, 11.0]));
    assert_eq!(editor.layer, CopperLayer::Bottom);
    assert_eq!(editor.pending_via, None);
    // With no route in progress V only cycles the layer.
    frame(&mut editor, &engine, &ctx, key(at, egui::Key::Escape));
    assert_eq!(editor.route_start, None);
    let (_, none) = frame(&mut editor, &engine, &ctx, key(at, egui::Key::V));
    assert!(none.is_none());
    assert_eq!(editor.layer, CopperLayer::Top);
    assert_eq!(editor.pending_via, None);
    Ok(())
}

/// The Via tool places a via on the chosen net where the user clicks;
/// clicking it with Select selects the via, whose card names it, and
/// Locate on a via finding selects and frames it.
#[test]
fn the_via_tool_places_vias_that_pick_and_locate() -> TestResult {
    let mut engine = Engine::demo()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    editor.sync(&engine);
    let r1 = engine
        .project()
        .instances
        .iter()
        .find(|i| i.designator == "R1")
        .map(|i| i.id)
        .ok_or("R1")?;
    let net = net_of(&engine, r1, 2)?;
    editor.on_tool("via", &engine);
    assert_eq!(editor.subject(), Subject::Tool);
    editor.net = Some(net);
    let place = [12.0, 14.0];
    let at = screen(&editor, &engine, place);
    frame(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(at)],
    );
    let (_, operation) = frame(&mut editor, &engine, &ctx, click(at));
    let operation = operation.ok_or("no via placed")?;
    assert!(matches!(
        &operation,
        Operation::Layout(LayoutCommand::AddVia { via })
            if via.net == net && via.position == place && via.tented
    ));
    engine.apply(engine.revision(), operation)?;
    let via = engine.project().vias.first().cloned().ok_or("via")?;
    editor.on_tool("select", &engine);
    frame(&mut editor, &engine, &ctx, click(at));
    assert_eq!(editor.via, Some(via.id));
    assert_eq!(editor.highlighted, Some(net));
    assert_eq!(editor.inspected(engine.project()), Inspected::Via(via.id));
    let header = editor.header(&engine);
    assert_eq!(
        header.title,
        format!("Via · {}", engine.project().net_name(net))
    );
    // A finding naming only the via locates it.
    editor.set_selection(None, None, None);
    assert!(crate::issues::Locate::locate(
        &mut editor,
        &engine,
        &[ObjectRef::Via { id: via.id }]
    ));
    assert_eq!(editor.via, Some(via.id));
    assert_eq!(editor.highlighted, Some(net));
    let bounds = editor.frame_request.ok_or("no framing")?;
    assert!(bounds[0][0] < place[0] && place[0] < bounds[1][0]);
    Ok(())
}
