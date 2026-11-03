//! Interactive routing driven through the Layout canvas: continuous
//! multi-step routes, the 45° and 90° elbows, Backspace unwinding, layer
//! changes through a via of a chosen size, and width changes mid-route.
//! Every operation the editor sends is applied through the engine, as the
//! app does.
use super::*;
use pcbcraft_core::layout::{RouteEditSet, posture::Posture};
use pcbcraft_core::rules::{NetClassSpec, RulesCommand};

pub(super) type TestResult = Result<(), Box<dyn std::error::Error>>;

const SCREEN: [f32; 2] = [900.0, 700.0];

/// One canvas with a clock that moves a second a frame, so separate clicks
/// never make a double-click.
pub(super) struct Canvas {
    ctx: egui::Context,
    time: f64,
}

impl Canvas {
    pub(super) fn new() -> Self {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        Self { ctx, time: 0.0 }
    }

    pub(super) fn frame(
        &mut self,
        editor: &mut PcbEditor,
        engine: &Engine,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> Option<Operation> {
        self.time += 1.0;
        let mut command = None;
        let mut events = events;
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(SCREEN[0], SCREEN[1]),
                )),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| command = editor.canvas(ui, engine),
        );
        output.textures_delta.clear();
        command
    }

    pub(super) fn click(
        &mut self,
        editor: &mut PcbEditor,
        engine: &Engine,
        at: Point,
    ) -> Option<Operation> {
        let p = screen(editor, engine, at);
        let button = |pressed| egui::Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.frame(
            editor,
            engine,
            vec![egui::Event::PointerMoved(p)],
            egui::Modifiers::NONE,
        );
        self.frame(
            editor,
            engine,
            vec![button(true), button(false)],
            egui::Modifiers::NONE,
        )
    }

    pub(super) fn key(
        &mut self,
        editor: &mut PcbEditor,
        engine: &Engine,
        at: Point,
        key: egui::Key,
        modifiers: egui::Modifiers,
    ) -> Option<Operation> {
        let p = screen(editor, engine, at);
        self.frame(
            editor,
            engine,
            vec![
                egui::Event::PointerMoved(p),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            modifiers,
        )
    }

    /// Apply what the editor sent, as `App::apply` does, then let the
    /// editor see the result.
    pub(super) fn apply(
        &mut self,
        editor: &mut PcbEditor,
        engine: &mut Engine,
        operation: Option<Operation>,
    ) -> TestResult {
        let operation = operation.ok_or("the editor sent nothing")?;
        engine.apply(engine.revision(), operation)?;
        self.frame(editor, engine, vec![], egui::Modifiers::NONE);
        Ok(())
    }
}

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

/// The demo's net of R1 pad 2 and R2 pad 1, with R1 pad 2 centred at
/// [8.8, 8.0] and R2 pad 1 at [15.2, 8.0].
fn shared_net(engine: &Engine) -> Result<Id, Box<dyn std::error::Error>> {
    let r1 = engine
        .project()
        .instance_by_designator("R1")
        .map(|i| i.id)
        .ok_or("R1")?;
    Ok(engine
        .project()
        .net_of(pcbcraft_core::Endpoint {
            instance: r1,
            terminal: pcbcraft_core::TerminalId(2),
        })
        .map(|n| n.id)
        .ok_or("demo net")?)
}

const R1_2: Point = [8.8, 8.0];
const R2_1: Point = [15.2, 8.0];

pub(super) fn edits(
    operation: &Option<Operation>,
) -> Result<&RouteEditSet, Box<dyn std::error::Error>> {
    match operation {
        Some(Operation::Layout(LayoutCommand::ApplyRoute { edits })) => Ok(edits),
        other => Err(format!("expected layout.route_apply, got {other:?}").into()),
    }
}

pub(super) fn close(a: Point, b: Point) -> bool {
    pcbcraft_core::geometry::distance(a, b) < 1e-6
}

/// Start a route on R1 pad 2 with the Route tool.
fn start(canvas: &mut Canvas, editor: &mut PcbEditor, engine: &Engine) -> TestResult {
    editor.on_tool("route", engine);
    canvas.frame(editor, engine, vec![], egui::Modifiers::NONE);
    assert!(canvas.click(editor, engine, R1_2).is_none());
    if editor.route_start.is_none_or(|head| !close(head, R1_2)) {
        return Err(format!(
            "the route started at {:?}, not R1 pad 2",
            editor.route_start
        )
        .into());
    }
    Ok(())
}

/// A route keeps going: each click is one step and one undo step, the
/// next step starts where the last ended, and a click on a pad of the net
/// finishes it, leaving Route armed. The steps join R1 to R2.
#[test]
fn a_route_continues_across_clicks_until_a_pad_of_the_net() -> TestResult {
    let mut engine = Engine::demo()?;
    let net = shared_net(&engine)?;
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    start(&mut canvas, &mut editor, &engine)?;
    assert_eq!(editor.net, Some(net));
    let before = engine.project().tracks.len();
    let first = canvas.click(&mut editor, &engine, [11.0, 11.0]);
    assert!(close(edits(&first)?.add[0].start, R1_2));
    canvas.apply(&mut editor, &mut engine, first)?;
    assert_eq!(editor.route_start, Some([11.0, 11.0]), "the head moved on");
    let second = canvas.click(&mut editor, &engine, [13.0, 11.0]);
    let legs = &edits(&second)?.add;
    assert_eq!(legs.len(), 1);
    assert_eq!((legs[0].start, legs[0].end), ([11.0, 11.0], [13.0, 11.0]));
    canvas.apply(&mut editor, &mut engine, second)?;
    assert_eq!(editor.route.steps.len(), 2);
    // R2 pad 1 is the net's: the step there finishes the route.
    let last = canvas.click(&mut editor, &engine, R2_1);
    let end = edits(&last)?.add.last().map(|r| r.end).ok_or("no legs")?;
    assert!(close(end, R2_1));
    let last_step = edits(&last)?.add.len();
    canvas.apply(&mut editor, &mut engine, last)?;
    assert_eq!(editor.route_start, None, "finished");
    assert!(editor.route.steps.is_empty());
    assert!(editor.tool == Tool::Route, "Route stays armed");
    let project = engine.project();
    assert!(project.tracks.len() >= before + 3);
    assert!(
        layout::airwires(project).is_empty(),
        "the route joins R1 to R2"
    );
    // One undo takes back the last step only.
    let tracks = project.tracks.len();
    engine.undo()?;
    assert_eq!(engine.project().tracks.len(), tracks - last_step);
    Ok(())
}

/// 45° is an orthogonal leg then an exact diagonal to the target; F flips
/// the corner; / steps to 90° (horizontal then vertical) and to free (one
/// straight segment). The preview paints the same legs the click sends.
#[test]
fn the_45_degree_elbow_flips_and_the_mode_cycles() -> TestResult {
    let engine = Engine::demo()?;
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    start(&mut canvas, &mut editor, &engine)?;
    assert_eq!(editor.route.posture, Posture::Diagonal);
    let target = [12.0, 9.0];
    let legs = |op: &Option<Operation>| -> Result<Vec<(Point, Point)>, Box<dyn std::error::Error>> {
        Ok(edits(op)?.add.iter().map(|r| (r.start, r.end)).collect())
    };
    let step = canvas.click(&mut editor, &engine, target);
    let diagonal = legs(&step)?;
    assert_eq!(diagonal.len(), 2);
    // dx 3.2, dy 1 (on the grid): 2.2 mm straight, then 1 mm by 1 mm at 45°.
    assert!(close(diagonal[0].1, [11.0, 8.0]), "{diagonal:?}");
    let (a, b) = diagonal[1];
    assert!(((b[0] - a[0]).abs() - (b[1] - a[1]).abs()).abs() < 1e-9);
    assert!(close(b, target));
    // Refused or not taken: the route stays at its head.
    canvas.frame(&mut editor, &engine, vec![], egui::Modifiers::NONE);
    assert!(close(editor.route_start.ok_or("route ended")?, R1_2));
    canvas.key(
        &mut editor,
        &engine,
        target,
        egui::Key::F,
        egui::Modifiers::NONE,
    );
    let flipped = legs(&canvas.click(&mut editor, &engine, target))?;
    assert!(
        close(flipped[0].1, [9.8, 9.0]),
        "diagonal first: {flipped:?}"
    );
    canvas.key(
        &mut editor,
        &engine,
        target,
        egui::Key::F,
        egui::Modifiers::NONE,
    );
    canvas.key(
        &mut editor,
        &engine,
        target,
        egui::Key::Slash,
        egui::Modifiers::NONE,
    );
    assert_eq!(editor.route.posture, Posture::Orthogonal);
    let orthogonal = legs(&canvas.click(&mut editor, &engine, target))?;
    assert!(close(orthogonal[0].1, [12.0, 8.0]), "{orthogonal:?}");
    canvas.key(
        &mut editor,
        &engine,
        target,
        egui::Key::Slash,
        egui::Modifiers::NONE,
    );
    assert_eq!(editor.route.posture, Posture::Free);
    assert_eq!(legs(&canvas.click(&mut editor, &engine, target))?.len(), 1);
    // / wraps back to 45°.
    canvas.key(
        &mut editor,
        &engine,
        target,
        egui::Key::Slash,
        egui::Modifiers::NONE,
    );
    assert_eq!(editor.route.posture, Posture::Diagonal);
    Ok(())
}

/// Backspace removes the last step, exactly its tracks, with one
/// `layout.route_apply`, and moves the head back; again unwinds the step
/// before, and once at the start it ends the route.
#[test]
fn backspace_unwinds_the_route_step_by_step() -> TestResult {
    let mut engine = Engine::demo()?;
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    let original = engine.project().tracks.clone();
    start(&mut canvas, &mut editor, &engine)?;
    let first = canvas.click(&mut editor, &engine, [11.0, 11.0]);
    canvas.apply(&mut editor, &mut engine, first)?;
    let after_first = engine.project().tracks.clone();
    let second = canvas.click(&mut editor, &engine, [13.0, 12.0]);
    canvas.apply(&mut editor, &mut engine, second)?;
    let second_ids = editor.route.steps[1].tracks.clone();
    assert_eq!(second_ids.len(), 2, "an elbow");
    let back = |canvas: &mut Canvas, editor: &mut PcbEditor, engine: &Engine| {
        canvas.key(
            editor,
            engine,
            [13.0, 12.0],
            egui::Key::Backspace,
            egui::Modifiers::NONE,
        )
    };
    let unwind = back(&mut canvas, &mut editor, &engine);
    let removed = edits(&unwind)?;
    assert_eq!(removed.remove, second_ids);
    assert!(removed.add.is_empty() && removed.remove_vias.is_empty());
    canvas.apply(&mut editor, &mut engine, unwind)?;
    assert_eq!(engine.project().tracks, after_first);
    assert_eq!(editor.route_start, Some([11.0, 11.0]));
    let unwind = back(&mut canvas, &mut editor, &engine);
    canvas.apply(&mut editor, &mut engine, unwind)?;
    assert_eq!(engine.project().tracks, original);
    assert!(close(editor.route_start.ok_or("route ended")?, R1_2));
    // At the start Backspace sends nothing and ends the route.
    assert!(back(&mut canvas, &mut editor, &engine).is_none());
    assert_eq!(editor.route_start, None);
    Ok(())
}

/// A layer key during a route puts a via of the chosen size at the
/// pointer: D steps to the next via preset of the rules (a net class's),
/// the number key picks the layer, and the next click routes to the via,
/// places it and routes on. Backspace takes the step back with its via
/// and returns to the first layer.
#[test]
fn a_layer_key_routes_through_a_via_of_the_chosen_size() -> TestResult {
    let mut engine = Engine::demo()?;
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "Power".into(),
                via_drill: Some(0.4),
                via_diameter: Some(0.9),
                ..Default::default()
            },
        }),
    )?;
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    start(&mut canvas, &mut editor, &engine)?;
    assert_eq!(editor.route_via_size(engine.project()), (0.3, 0.6));
    let via_at = [11.0, 11.0];
    canvas.key(
        &mut editor,
        &engine,
        via_at,
        egui::Key::D,
        egui::Modifiers::NONE,
    );
    assert_eq!(editor.route_via_size(engine.project()), (0.4, 0.9));
    canvas.key(
        &mut editor,
        &engine,
        via_at,
        egui::Key::D,
        egui::Modifiers::SHIFT,
    );
    assert_eq!(editor.route_via_size(engine.project()), (0.3, 0.6));
    canvas.key(
        &mut editor,
        &engine,
        via_at,
        egui::Key::D,
        egui::Modifiers::NONE,
    );
    // "2" is the stackup's second layer, Bottom on the demo.
    canvas.key(
        &mut editor,
        &engine,
        via_at,
        egui::Key::Num2,
        egui::Modifiers::NONE,
    );
    assert_eq!(editor.layer, CopperLayer::Bottom);
    assert_eq!(editor.pending_via, Some((via_at, CopperLayer::Top)));
    let step = canvas.click(&mut editor, &engine, [14.0, 11.0]);
    let sent = edits(&step)?;
    assert_eq!(sent.vias.len(), 1);
    let via = sent.vias[0];
    assert_eq!((via.position, via.drill, via.diameter), (via_at, 0.4, 0.9));
    assert!(
        sent.add
            .iter()
            .any(|r| r.layer == CopperLayer::Top && r.end == via_at)
    );
    assert!(
        sent.add
            .iter()
            .all(|r| r.layer == CopperLayer::Top || r.start == via_at)
    );
    canvas.apply(&mut editor, &mut engine, step)?;
    let placed = engine.project().vias.first().cloned().ok_or("no via")?;
    assert_eq!((placed.drill, placed.diameter), (0.4, 0.9));
    assert_eq!(editor.route_start, Some([14.0, 11.0]));
    assert_eq!(editor.route.steps[0].vias, vec![placed.id]);
    // Backspace removes the via with its tracks and goes back to Top.
    let unwind = canvas.key(
        &mut editor,
        &engine,
        via_at,
        egui::Key::Backspace,
        egui::Modifiers::NONE,
    );
    assert_eq!(edits(&unwind)?.remove_vias, vec![placed.id]);
    canvas.apply(&mut editor, &mut engine, unwind)?;
    assert!(engine.project().vias.is_empty());
    assert_eq!(editor.layer, CopperLayer::Top);
    // A via typed below the rules' annular ring is marked, never used.
    editor.route.via_draft = Some((0.4, 0.5));
    assert_eq!(editor.route_via_size(engine.project()), (0.4, 0.9));
    Ok(())
}

/// W during a route steps to the next width preset (a net class's width),
/// Shift+W back; the new width applies from the next step and the
/// accepted ones keep theirs.
#[test]
fn w_changes_the_width_from_the_next_step() -> TestResult {
    let mut engine = Engine::demo()?;
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "Power".into(),
                track_width: Some(0.8),
                ..Default::default()
            },
        }),
    )?;
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    start(&mut canvas, &mut editor, &engine)?;
    let first = canvas.click(&mut editor, &engine, [11.0, 11.0]);
    assert!(edits(&first)?.add.iter().all(|r| r.width == 0.3));
    canvas.apply(&mut editor, &mut engine, first)?;
    let at = [13.0, 11.0];
    canvas.key(
        &mut editor,
        &engine,
        at,
        egui::Key::W,
        egui::Modifiers::NONE,
    );
    assert_eq!(editor.width, 0.8);
    canvas.key(
        &mut editor,
        &engine,
        at,
        egui::Key::W,
        egui::Modifiers::SHIFT,
    );
    assert_eq!(editor.width, 0.3);
    canvas.key(
        &mut editor,
        &engine,
        at,
        egui::Key::W,
        egui::Modifiers::NONE,
    );
    let second = canvas.click(&mut editor, &engine, at);
    assert!(edits(&second)?.add.iter().all(|r| r.width == 0.8));
    canvas.apply(&mut editor, &mut engine, second)?;
    let widths: BTreeSet<u64> = engine
        .project()
        .tracks
        .iter()
        .map(|t| (t.width * 10.0).round() as u64)
        .collect();
    assert_eq!(widths, BTreeSet::from([3, 8]));
    assert_eq!(editor.route_start, Some(at), "the route goes on");
    Ok(())
}

/// Enter, Esc and a right-click finish the route and keep Route armed;
/// Esc with no route returns to Select. The Route card lists the keys.
#[test]
fn finishing_keys_end_the_route_and_the_card_lists_the_keys() -> TestResult {
    let engine = Engine::demo()?;
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    start(&mut canvas, &mut editor, &engine)?;
    canvas.key(
        &mut editor,
        &engine,
        R1_2,
        egui::Key::Enter,
        egui::Modifiers::NONE,
    );
    assert_eq!(editor.route_start, None);
    assert!(editor.tool == Tool::Route);
    start(&mut canvas, &mut editor, &engine)?;
    let p = screen(&editor, &engine, [12.0, 12.0]);
    let button = |pressed| egui::Event::PointerButton {
        pos: p,
        button: PointerButton::Secondary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    canvas.frame(
        &mut editor,
        &engine,
        vec![egui::Event::PointerMoved(p), button(true), button(false)],
        egui::Modifiers::NONE,
    );
    assert_eq!(editor.route_start, None, "right-click finishes");
    start(&mut canvas, &mut editor, &engine)?;
    canvas.key(
        &mut editor,
        &engine,
        R1_2,
        egui::Key::Escape,
        egui::Modifiers::NONE,
    );
    assert_eq!(editor.route_start, None);
    assert!(editor.tool == Tool::Route);
    canvas.key(
        &mut editor,
        &engine,
        R1_2,
        egui::Key::Escape,
        egui::Modifiers::NONE,
    );
    assert!(editor.tool == Tool::Select);
    // The card shows the mode and the keys.
    editor.on_tool("route", &engine);
    let mut shown = Vec::new();
    let mut output = canvas.ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700.0, 1400.0))),
            ..Default::default()
        },
        |ui| {
            editor.subject_slot(ui, &engine);
        },
    );
    output.textures_delta.clear();
    fn collect(shape: &Shape, out: &mut Vec<String>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
            Shape::Text(text) => out.push(text.galley.text().to_owned()),
            _ => {}
        }
    }
    for shape in &output.shapes {
        collect(&shape.shape, &mut shown);
    }
    let previous = crate::commands::Chord::key(egui::Key::V)
        .with_shift()
        .shown();
    for label in [
        "Mode",
        "45°",
        "Via drill",
        "Via pad",
        "KEYS",
        "Backspace",
        "V",
        previous.as_str(),
        "1–9",
    ] {
        assert!(
            shown.iter().any(|t| t == label),
            "{label} missing: {shown:?}"
        );
    }
    Ok(())
}

/// The Route tool reads its keys through the registry: with Next track
/// width rebound from W to Q, W does nothing and Q steps the width, and
/// the card lists Q.
#[test]
fn a_rebound_route_key_drives_the_tool() -> TestResult {
    let mut engine = Engine::demo()?;
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "Power".into(),
                track_width: Some(0.8),
                ..Default::default()
            },
        }),
    )?;
    let mut canvas = Canvas::new();
    let mut registry = crate::commands::Registry::standard();
    registry.rebind(
        "route.width-next",
        Some(crate::commands::Chord::key(egui::Key::Q)),
    )?;
    crate::keys::publish(&canvas.ctx, &std::sync::Arc::new(registry));
    let mut editor = PcbEditor::default();
    start(&mut canvas, &mut editor, &engine)?;
    let first = canvas.click(&mut editor, &engine, [11.0, 11.0]);
    canvas.apply(&mut editor, &mut engine, first)?;
    let at = [13.0, 11.0];
    canvas.key(
        &mut editor,
        &engine,
        at,
        egui::Key::W,
        egui::Modifiers::NONE,
    );
    assert_eq!(editor.width, 0.3, "W is no longer the width key");
    canvas.key(
        &mut editor,
        &engine,
        at,
        egui::Key::Q,
        egui::Modifiers::NONE,
    );
    assert_eq!(editor.width, 0.8);
    let rows = crate::keys::rows(&canvas.ctx, super::route_tool::ROUTE_KEYS);
    assert!(rows.contains(&("Q".to_owned(), "Next track width".to_owned())));
    Ok(())
}

/// Fill copper the fill worker stores after a step (a new revision with
/// no undo step) does not end the route: the next click continues from
/// the head. Boards with pours ended every route after one step.
#[test]
fn a_route_continues_through_fill_copper_stored_after_a_step() -> TestResult {
    use pcbcraft_core::layout::fill::{FillCache, FillCommand, compute_stale};
    use pcbcraft_core::library::{Outline, Segment};
    use pcbcraft_core::model::fill::FillSpec;
    let mut engine = Engine::demo()?;
    let net = shared_net(&engine)?;
    let other = engine
        .project()
        .nets
        .iter()
        .find(|n| n.id != net)
        .map(|n| n.id)
        .ok_or("another net")?;
    let [w, h] = engine.project().board_size;
    let outline = Outline {
        start: [0.0, 0.0],
        segments: vec![
            Segment::Line { to: [w, 0.0] },
            Segment::Line { to: [w, h] },
            Segment::Line { to: [0.0, h] },
            Segment::Line { to: [0.0, 0.0] },
        ],
    };
    engine.apply(
        engine.revision(),
        Operation::Fill(FillCommand::Add {
            fill: Box::new(FillSpec {
                name: "pour".into(),
                net: Some(other),
                layer: CopperLayer::Top,
                outline,
                priority: 0,
                settings: Default::default(),
            }),
        }),
    )?;
    let mut cache = FillCache::default();
    // What the app's fill worker does: store the stale fills' copper
    // without an undo step and tell the editor.
    let mut store = |editor: &mut PcbEditor,
                     engine: &mut Engine|
     -> Result<usize, Box<dyn std::error::Error>> {
        let before = engine.revision();
        let copper = compute_stale(engine.project(), &mut cache)?;
        let stored = engine.store_fills(before, copper)?;
        editor.fills_stored(before, engine.revision());
        Ok(stored)
    };
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    store(&mut editor, &mut engine)?;
    start(&mut canvas, &mut editor, &engine)?;
    let first = canvas.click(&mut editor, &engine, [11.0, 11.0]);
    canvas.apply(&mut editor, &mut engine, first)?;
    assert_eq!(editor.route.steps.len(), 1);
    // The pour is stale after the step; its new copper lands next.
    let before = engine.revision();
    assert!(store(&mut editor, &mut engine)? > 0, "the pour refilled");
    assert!(engine.revision() > before);
    canvas.frame(&mut editor, &engine, vec![], egui::Modifiers::NONE);
    assert_eq!(editor.route_start, Some([11.0, 11.0]), "the route goes on");
    let second = canvas.click(&mut editor, &engine, [13.0, 11.0]);
    let legs = &edits(&second)?.add;
    assert_eq!((legs[0].start, legs[0].end), ([11.0, 11.0], [13.0, 11.0]));
    canvas.apply(&mut editor, &mut engine, second)?;
    assert_eq!(
        editor.route.steps.len(),
        2,
        "both steps belong to the route"
    );
    // A store that lands between a click and the next frame too.
    let third = canvas.click(&mut editor, &engine, [13.0, 13.0]);
    engine.apply(engine.revision(), third.ok_or("no third step")?)?;
    store(&mut editor, &mut engine)?;
    canvas.frame(&mut editor, &engine, vec![], egui::Modifiers::NONE);
    assert_eq!(editor.route.steps.len(), 3);
    assert_eq!(editor.route_start, Some([13.0, 13.0]));
    Ok(())
}
