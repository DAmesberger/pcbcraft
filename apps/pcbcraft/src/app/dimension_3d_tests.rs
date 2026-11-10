//! Slice S5 through the whole app's frame, headless: in PCB 3D the
//! Dimension tool picks the ESP32-S31 dev board's J1 shell front face and
//! the board's bottom edge, measures them, and commits the typed distance
//! as one `layout.dimension_add` (one undo step), which moves J1. Hovering
//! highlights the face under the pointer without asking for a model or
//! tracing anything.
use super::*;
use crate::commands::Registry;
use crate::viewport::surface::Hovered;
use pcbcraft_core::layout::dimensions::DimensionCommand;
use pcbcraft_core::layout::dimensions::model::{ModelPicked, PickRequest, TraceRequest, Traced};
use pcbcraft_core::layout::dimensions::{PartsRequest, PartsSolution};
use pcbcraft_core::layout::outline::{
    BoardEvaluation, BoardShapeEvaluator, DrawnSlot, Fillets, FreeFormCurve,
};
use pcbcraft_core::library::footprint_source::{
    DimValue, EvaluatorRef, SketchBlob, SketchEdit, SketchOutcome, SketchPointId, SketchRef,
};
use pcbcraft_core::model::board::{BoardShape, BoardSketch};
use pcbcraft_core::model::dimension::{BoardRef, FeatureGeometry, FeatureRef};
use pcbcraft_solidcraft::board::SketchBoardEvaluator;
use solidcraft_ui_egui::fonts;
use std::sync::atomic::{AtomicUsize, Ordering};

type Fallible<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The host's board evaluator, counting what reads the part's model.
struct Counting;

static TRACES: AtomicUsize = AtomicUsize::new(0);
static PICKS: AtomicUsize = AtomicUsize::new(0);

impl BoardShapeEvaluator for Counting {
    fn evaluator(&self) -> EvaluatorRef {
        SketchBoardEvaluator.evaluator()
    }
    fn rectangle(&self, size: pcbcraft_core::Point) -> pcbcraft_core::Result<BoardSketch> {
        SketchBoardEvaluator.rectangle(size)
    }
    fn apply_sketch(
        &self,
        sketch: &SketchBlob,
        edit: &SketchEdit,
    ) -> pcbcraft_core::Result<SketchOutcome> {
        SketchBoardEvaluator.apply_sketch(sketch, edit)
    }
    fn evaluate(&self, board: &BoardSketch) -> pcbcraft_core::Result<BoardEvaluation> {
        SketchBoardEvaluator.evaluate(board)
    }
    fn fillet(
        &self,
        board: &BoardSketch,
        corners: &[SketchPointId],
        radius: &DimValue,
    ) -> pcbcraft_core::Result<Fillets> {
        SketchBoardEvaluator.fillet(board, corners, radius)
    }
    fn slot(
        &self,
        board: &BoardSketch,
        a: pcbcraft_core::Point,
        b: pcbcraft_core::Point,
        width: &DimValue,
    ) -> pcbcraft_core::Result<DrawnSlot> {
        SketchBoardEvaluator.slot(board, a, b, width)
    }
    fn add_curve(
        &self,
        board: &BoardSketch,
        curve: &FreeFormCurve,
    ) -> pcbcraft_core::Result<SketchOutcome> {
        SketchBoardEvaluator.add_curve(board, curve)
    }
    fn solve_parts(&self, request: &PartsRequest<'_>) -> pcbcraft_core::Result<PartsSolution> {
        SketchBoardEvaluator.solve_parts(request)
    }
    fn trace_model(&self, request: &TraceRequest<'_>) -> pcbcraft_core::Result<Traced> {
        TRACES.fetch_add(1, Ordering::SeqCst);
        SketchBoardEvaluator.trace_model(request)
    }
    fn pick_model(&self, request: &PickRequest<'_>) -> pcbcraft_core::Result<ModelPicked> {
        PICKS.fetch_add(1, Ordering::SeqCst);
        SketchBoardEvaluator.pick_model(request)
    }
}

static COUNTING: Counting = Counting;

/// The counters are shared: the tests that read them run one at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Harness {
    app: App,
    ctx: egui::Context,
    frame: eframe::Frame,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new(engine: Engine) -> Fallible<Self> {
        let dir = tempfile::tempdir()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let files = FileUi::with_directory(
            &ctx,
            rfd::AsyncFileDialog::new(),
            &engine,
            dir.path().to_path_buf(),
        );
        let mut app = App::with_parts(
            &ctx,
            viewport::Viewport::detached(),
            Parts {
                engine,
                files,
                sources: LibrarySources::new(None),
                registry: Arc::new(Registry::standard()),
                keymap_problems: Vec::new(),
                messages: Messages::default(),
                browser_width: chrome::BROWSER_WIDTH,
                #[cfg(target_os = "macos")]
                native_menu: None,
            },
        );
        app.workspace = Workspace::Pcb;
        app.pcb.set_3d(true);
        app.pcb.set_board_evaluator(&COUNTING);
        crate::keys::publish(&ctx, &app.registry);
        Ok(Self {
            app,
            ctx,
            frame: eframe::Frame::_new_kittest(),
            _dir: dir,
        })
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1380.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        let Self {
            app, ctx, frame, ..
        } = self;
        ctx.run_ui(input, |ui| eframe::App::ui(app, ui, frame))
            .textures_delta
            .clear();
    }

    /// Frames until the board's model is shown.
    fn until_shown(&mut self) -> Fallible {
        for _ in 0..24_000 {
            self.frame(vec![]);
            let key = crate::scene::ModelKey::Project(Arc::clone(self.app.engine.project_arc()));
            if let crate::scene::Status::Failed(e) = self.app.scene.status() {
                return Err(e.clone().into());
            }
            if self.app.scene.display(&key) == crate::scene::Display::Current {
                self.frame(vec![]);
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Err("the board's model was never shown".into())
    }

    fn click(&mut self, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.frame(vec![egui::Event::PointerMoved(at)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
        self.frame(vec![]);
    }
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

fn esp32() -> Fallible<Engine> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    Ok(Engine::from_file(pcbcraft_io::read_project(&path)?)?)
}

#[test]
fn a_3d_pick_dimensions_j1_to_the_bottom_edge_in_one_undo_step() -> Fallible {
    let _serial = serial();
    let mut engine = esp32()?;
    let project = engine.project();
    let j1 = project
        .instances
        .iter()
        .find(|i| i.designator == "J1")
        .map(|i| i.id)
        .ok_or("no J1")?;
    // The example holds J1's shell front face on the bottom edge; that
    // dimension is taken away, to be made again in 3D.
    let held = project
        .part_dimensions
        .iter()
        .find(|d| {
            d.refs
                .iter()
                .any(|r| matches!(r, BoardRef::Part(FeatureRef::Face { .. })))
        })
        .ok_or("J1's face dimension")?;
    let (face, edge) = match (&held.refs[0], &held.refs[1]) {
        (BoardRef::Part(f), BoardRef::Board(SketchRef::Curve(c)))
        | (BoardRef::Board(SketchRef::Curve(c)), BoardRef::Part(f)) => (f.clone(), c.clone()),
        _ => return Err("face and edge".into()),
    };
    let trace = project
        .model_traces
        .iter()
        .find(|t| t.feature == face)
        .and_then(|t| t.geometry)
        .ok_or("the face's trace")?;
    let FeatureGeometry::Line { a, b } = trace else {
        return Err("the face traces a line".into());
    };
    let id = held.id;
    engine.apply_hosted(
        engine.revision(),
        Operation::Dimension(DimensionCommand::Delete { id }),
        pcbcraft_automation::session::hosted(),
    )?;
    let placement = engine
        .project()
        .placement_of(j1)
        .ok_or("J1 placed")?
        .clone();
    let to_board = pcbcraft_core::layout::dimensions::view::part_transform(&placement);
    let mid = to_board.apply([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
    let BoardShape::Sketch(_) = &engine.project().board_shape else {
        return Err("a sketched board".into());
    };
    let seat = engine
        .project()
        .stackup
        .copper_span(pcbcraft_core::CopperLayer::Top)
        .map_or(0.0, |s| s.1);
    let laminate = engine.project().stackup.dielectric_mm();
    let mut h = Harness::new(engine)?;
    h.until_shown()?;
    // Look at J1's front from a little above, framed on J1.
    let items = h.app.scene.selected_items(Some(j1), None, None);
    h.app
        .scene
        .viewport
        .view_from(pcbcraft_solidcraft::geom::Vec3::new(0.15, -1.0, 0.35));
    assert!(h.app.scene.viewport.fit_items(&items));
    h.frame(vec![]);
    h.frame(vec![]);
    let screen = |h: &Harness, p: [f64; 3]| h.app.scene.viewport.to_screen(p).ok_or("off screen");
    // On the shell's rim below the opening, and on the laminate's side
    // under J1, a little to the side of the face's middle.
    let on_face = screen(&h, [mid[0] + 1.0, mid[1], seat + 0.2])?;
    let on_edge = screen(&h, [mid[0] + 1.0, mid[1], laminate / 2.0])?;
    let hit = h
        .app
        .scene
        .viewport
        .surface_hit(on_face)
        .ok_or("nothing under the face point")?;
    let source = hit.item.and_then(|i| h.app.scene.source(i)).ok_or("J1")?;
    assert_eq!(source.instance, Some(j1));

    // Arm the tool, as its ribbon button does.
    let revision = h.app.engine.revision();
    let engine = &h.app.engine;
    let _ = h.app.pcb.on_tool("dimension", engine);
    h.frame(vec![]);
    // Hover: the face lights, and nothing is built or traced.
    let (requests, traces, picks) = (
        h.app.scene.requests(),
        TRACES.load(Ordering::SeqCst),
        PICKS.load(Ordering::SeqCst),
    );
    for dx in [0.0, 2.0, -2.0, 0.0] {
        h.frame(vec![egui::Event::PointerMoved(
            on_face + egui::vec2(dx, 0.0),
        )]);
    }
    assert!(matches!(
        h.app.scene.viewport.hovered(),
        Some(Hovered::Face { .. })
    ));
    h.frame(vec![egui::Event::PointerMoved(on_edge)]);
    assert!(h.app.scene.viewport.hovered().is_some(), "the board edge");
    assert_eq!(h.app.scene.requests(), requests, "hover asked for a model");
    assert_eq!(TRACES.load(Ordering::SeqCst), traces, "hover traced");
    assert_eq!(PICKS.load(Ordering::SeqCst), picks, "hover picked");
    assert_eq!(h.app.engine.revision(), revision);

    // The shell's top is parallel to the board: refused in the tool's
    // words, before anything is committed, and nothing is picked.
    let on_top = screen(&h, [mid[0] + 1.0, mid[1] + 3.0, seat + 3.31])?;
    h.click(on_top);
    let (n, _, error) = h.app.pcb.dimension_draft();
    assert_eq!(n, 0);
    let error = error.ok_or("the top face was not refused")?;
    assert!(error.contains("pick an edge"), "{error}");
    assert_eq!(h.app.engine.revision(), revision);
    let picks = PICKS.load(Ordering::SeqCst);

    // Pick the face, then the edge: the value opens at 0 mm (flush).
    h.click(on_face);
    let (n, _, error) = h.app.pcb.dimension_draft();
    assert_eq!((n, error), (1, None));
    assert_eq!(PICKS.load(Ordering::SeqCst), picks + 1, "one pick named it");
    h.click(on_edge);
    let (n, value, error) = h.app.pcb.dimension_draft();
    assert_eq!((n, error), (2, None));
    assert_eq!(value.as_deref(), Some("0 mm"));
    let words: Vec<String> = h
        .app
        .pcb
        .dimension_picks()
        .into_iter()
        .map(|p| p.1)
        .collect();
    assert_eq!(words, ["J1 · front face", "Board · bottom edge"]);
    assert_eq!(h.app.engine.revision(), revision, "nothing committed yet");
    h.frame(vec![]);
    h.frame(vec![]);
    assert!(h.ctx.text_edit_focused(), "the value takes the keyboard");
    h.frame(vec![
        key(egui::Key::A, egui::Modifiers::COMMAND),
        egui::Event::Text("1 mm".into()),
    ]);
    h.frame(vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
    h.frame(vec![]);
    h.frame(vec![]);

    // One dimension, face to edge, at 1 mm: J1 moved 1 mm off the edge.
    let project = h.app.engine.project();
    let added = project
        .part_dimensions
        .iter()
        .find(|d| d.refs.contains(&BoardRef::Part(face.clone())) || {
            d.refs
                .iter()
                .any(|r| matches!(r, BoardRef::Part(FeatureRef::Face { placement, .. }) if *placement == j1))
        })
        .ok_or("the new dimension")?;
    assert!(
        added
            .refs
            .contains(&BoardRef::Board(SketchRef::Curve(edge.clone()))),
        "{:?}",
        added.refs
    );
    assert_eq!(added.value, Some(DimValue::Fixed(1.0)));
    let moved = project.placement_of(j1).ok_or("J1")?;
    assert!(
        (moved.position[1] - placement.position[1] - 1.0).abs() < 1e-6,
        "{} -> {}",
        placement.position[1],
        moved.position[1]
    );
    assert_eq!(h.app.pcb.selected_dimension(), Some(added.id));
    assert_eq!(h.app.pcb.selected_instance(), Some(j1));
    // The selection is Layout's too.
    let added = added.id;
    h.app.pcb.set_3d(false);
    h.frame(vec![]);
    assert_eq!(h.app.pcb.selected_dimension(), Some(added));
    // One undo step takes it all back.
    let after = h.app.engine.revision();
    assert!(after > revision);
    h.app.engine.undo()?;
    let project = h.app.engine.project();
    assert!(!project.part_dimensions.iter().any(|d| d.refs.iter().any(
        |r| matches!(r, BoardRef::Part(FeatureRef::Face { placement, .. }) if *placement == j1)
    )));
    assert_eq!(
        project.placement_of(j1).map(|p| p.position),
        Some(placement.position)
    );
    Ok(())
}

/// Demo run 2, finding 4: at overview zoom the board's edge is picked from
/// a few points inside or outside it, and J1's top front edge from just
/// above it; what the pointer highlights is what the click picks, named in
/// a designer's words. Hover never builds, traces or picks.
#[test]
fn overview_picks_land_on_edges_and_read_as_words() -> Fallible {
    let _serial = serial();
    let engine = esp32()?;
    let project = engine.project();
    let j1 = project
        .instances
        .iter()
        .find(|i| i.designator == "J1")
        .map(|i| i.id)
        .ok_or("no J1")?;
    let face = project
        .model_traces
        .iter()
        .find(|t| matches!(&t.feature, FeatureRef::Face { placement, .. } if *placement == j1))
        .ok_or("J1's face trace")?;
    let Some(FeatureGeometry::Line { a, b }) = face.geometry else {
        return Err("the face traces a line".into());
    };
    let placement = project.placement_of(j1).ok_or("J1 placed")?.clone();
    let to_board = pcbcraft_core::layout::dimensions::view::part_transform(&placement);
    let mid = to_board.apply([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
    let seat = project
        .stackup
        .copper_span(pcbcraft_core::CopperLayer::Top)
        .map_or(0.0, |s| s.1);
    let board = project.board();
    let outline: Vec<[f64; 2]> = std::iter::once(board.outer.start)
        .chain(board.outer.segments.iter().map(|s| s.to()))
        .collect();
    let bottom = outline.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
    let left = outline.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
    let mut h = Harness::new(engine)?;
    h.until_shown()?;
    h.app
        .scene
        .viewport
        .view_from(pcbcraft_solidcraft::geom::Vec3::new(0.15, -1.0, 0.8));
    h.app.scene.viewport.fit();
    h.frame(vec![]);
    h.frame(vec![]);
    let screen = |h: &Harness, p: [f64; 3]| h.app.scene.viewport.to_screen(p).ok_or("off screen");
    let engine = &h.app.engine;
    let _ = h.app.pcb.on_tool("dimension", engine);
    h.frame(vec![]);
    let (requests, traces, picks) = (
        h.app.scene.requests(),
        TRACES.load(Ordering::SeqCst),
        PICKS.load(Ordering::SeqCst),
    );

    // The bottom edge, 8 mm in from the left, 3 points inside the board
    // and 3 points below it: the edge lights either way.
    let edge = screen(&h, [left + 8.0, bottom, seat])?;
    for dy in [-3.0, 3.0] {
        h.frame(vec![egui::Event::PointerMoved(edge + egui::vec2(0.0, dy))]);
        assert!(
            matches!(h.app.scene.viewport.hovered(), Some(Hovered::Edge { .. })),
            "{dy}: {:?}",
            h.app.scene.viewport.hovered()
        );
    }
    // J1's top front edge, from 3 points above it.
    let top_edge = screen(&h, [mid[0], mid[1], seat + 3.2])? + egui::vec2(0.0, -3.0);
    h.frame(vec![egui::Event::PointerMoved(top_edge)]);
    let hovered = h.app.scene.viewport.hovered();
    assert!(matches!(hovered, Some(Hovered::Edge { .. })), "{hovered:?}");
    assert_eq!(h.app.scene.requests(), requests, "hover asked for a model");
    assert_eq!(TRACES.load(Ordering::SeqCst), traces, "hover traced");
    assert_eq!(PICKS.load(Ordering::SeqCst), picks, "hover picked");

    // The clicks pick what lit: J1's edge, then the board's bottom edge.
    h.click(top_edge);
    h.click(edge + egui::vec2(0.0, -3.0));
    let (n, value, error) = h.app.pcb.dimension_draft();
    assert_eq!((n, error), (2, None));
    assert!(value.is_some());
    let picked = h.app.pcb.dimension_picks();
    let [(first, j1_words), (second, board_words)] = picked.as_slice() else {
        return Err(format!("{picked:?}").into());
    };
    assert!(
        matches!(first, BoardRef::Part(FeatureRef::Edge { placement, .. }) if *placement == j1),
        "{first:?}"
    );
    assert!(
        matches!(second, BoardRef::Board(SketchRef::Curve(_))),
        "{second:?}"
    );
    assert_eq!(j1_words, "J1 · top-front edge");
    assert_eq!(board_words, "Board · bottom edge");
    // The card shows the words, never the stored name.
    for words in [j1_words, board_words] {
        assert!(!words.contains(':') && !words.contains('|'), "{words}");
    }
    Ok(())
}
