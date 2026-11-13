//! Slice S5 through the whole app's frame, headless: in PCB 3D the
//! Dimension tool picks the ESP32-S31 dev board's J1 shell front face and
//! the board's bottom edge, measures them, and commits the typed distance
//! as one `layout.dimension_add` (one undo step), which moves J1. Hovering
//! highlights the face under the pointer without asking for a model or
//! tracing anything.
use super::*;
use crate::commands::Registry;
use crate::viewport::surface::Hovered;
use sparksmith_core::layout::dimensions::DimensionCommand;
use sparksmith_core::layout::dimensions::model::{ModelPicked, PickRequest, TraceRequest, Traced};
use sparksmith_core::layout::dimensions::{PartsRequest, PartsSolution};
use sparksmith_core::layout::outline::{
    BoardEvaluation, BoardShapeEvaluator, DrawnSlot, Fillets, FreeFormCurve,
};
use sparksmith_core::library::footprint_source::{
    DimValue, EvaluatorRef, SketchBlob, SketchEdit, SketchOutcome, SketchPointId, SketchRef,
};
use sparksmith_core::model::board::{BoardShape, BoardSketch};
use sparksmith_core::model::dimension::{BoardRef, FeatureGeometry, FeatureRef};
use sparksmith_solidcraft::board::SketchBoardEvaluator;
use sparksmith_ui::fonts;
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
    fn rectangle(&self, size: sparksmith_core::Point) -> sparksmith_core::Result<BoardSketch> {
        SketchBoardEvaluator.rectangle(size)
    }
    fn apply_sketch(
        &self,
        sketch: &SketchBlob,
        edit: &SketchEdit,
    ) -> sparksmith_core::Result<SketchOutcome> {
        SketchBoardEvaluator.apply_sketch(sketch, edit)
    }
    fn evaluate(&self, board: &BoardSketch) -> sparksmith_core::Result<BoardEvaluation> {
        SketchBoardEvaluator.evaluate(board)
    }
    fn fillet(
        &self,
        board: &BoardSketch,
        corners: &[SketchPointId],
        radius: &DimValue,
    ) -> sparksmith_core::Result<Fillets> {
        SketchBoardEvaluator.fillet(board, corners, radius)
    }
    fn slot(
        &self,
        board: &BoardSketch,
        a: sparksmith_core::Point,
        b: sparksmith_core::Point,
        width: &DimValue,
    ) -> sparksmith_core::Result<DrawnSlot> {
        SketchBoardEvaluator.slot(board, a, b, width)
    }
    fn add_curve(
        &self,
        board: &BoardSketch,
        curve: &FreeFormCurve,
    ) -> sparksmith_core::Result<SketchOutcome> {
        SketchBoardEvaluator.add_curve(board, curve)
    }
    fn solve_parts(&self, request: &PartsRequest<'_>) -> sparksmith_core::Result<PartsSolution> {
        SketchBoardEvaluator.solve_parts(request)
    }
    fn trace_model(&self, request: &TraceRequest<'_>) -> sparksmith_core::Result<Traced> {
        TRACES.fetch_add(1, Ordering::SeqCst);
        SketchBoardEvaluator.trace_model(request)
    }
    fn pick_model(&self, request: &PickRequest<'_>) -> sparksmith_core::Result<ModelPicked> {
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

/// Whether `text` names a face or edge by its persistent Solidcraft name
/// (`F1:+z`, `F3:-x|F6:blend(…)`): an `F`, digits and a colon.
fn names_a_model_id(text: &str) -> bool {
    let b = text.as_bytes();
    (0..b.len()).any(|i| {
        b[i] == b'F'
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
            && b[i + 1..]
                .iter()
                .position(|c| !c.is_ascii_digit())
                .is_some_and(|n| n > 0 && b.get(i + 1 + n) == Some(&b':'))
    })
}

fn esp32() -> Fallible<Engine> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.sparksmith");
    Ok(Engine::from_file(sparksmith_io::read_project(&path)?)?)
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
        sparksmith_automation::session::hosted(),
    )?;
    let placement = engine
        .project()
        .placement_of(j1)
        .ok_or("J1 placed")?
        .clone();
    let to_board = sparksmith_core::layout::dimensions::view::part_transform(&placement);
    let mid = to_board.apply([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
    let BoardShape::Sketch(_) = &engine.project().board_shape else {
        return Err("a sketched board".into());
    };
    let seat = engine
        .project()
        .stackup
        .copper_span(sparksmith_core::CopperLayer::Top)
        .map_or(0.0, |s| s.1);
    let laminate = engine.project().stackup.dielectric_mm();
    let mut h = Harness::new(engine)?;
    h.until_shown()?;
    // Look at J1's front from a little above, framed on J1.
    let items = h.app.scene.selected_items(&crate::pcb_editor::Group {
        parts: [j1].into(),
        ..Default::default()
    });
    h.app
        .scene
        .viewport
        .view_from(sparksmith_solidcraft::geom::Vec3::new(0.15, -1.0, 0.35));
    assert!(h.app.scene.viewport.fit_items(&items));
    h.frame(vec![]);
    h.frame(vec![]);
    let screen = |h: &Harness, p: [f64; 3]| h.app.scene.viewport.to_screen(p).ok_or("off screen");
    // On the shell's rim below the opening (the stadium's lower wall, from
    // 0.15 to 0.45 mm up: half way, clear of its edges), and on the
    // laminate's side under J1, a little to the side of the face's middle.
    let on_face = screen(&h, [mid[0] + 1.0, mid[1], seat + 0.3])?;
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
    // Named in a designer's words, never by its persistent name ("face
    // ‘F1:+z’").
    assert!(
        error.starts_with("J1 · top face is parallel to the board"),
        "{error}"
    );
    assert!(!names_a_model_id(&error), "{error}");
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
    let to_board = sparksmith_core::layout::dimensions::view::part_transform(&placement);
    let mid = to_board.apply([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
    let seat = project
        .stackup
        .copper_span(sparksmith_core::CopperLayer::Top)
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
        .view_from(sparksmith_solidcraft::geom::Vec3::new(0.15, -1.0, 0.8));
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

/// Demo run 3, finding 4: at overview zoom the board's bottom edge is
/// taken within Solidcraft's edge reach of it (6 points) all along it,
/// inside the board and off it, as a part's edge is. J1 stands flush on
/// that edge: a click on J1's front face picks J1 (its face, or its own
/// edge near the bottom), never the board edge behind it; the board edge
/// is still picked just below J1, off the part. Hover is the pick.
#[test]
fn board_edges_are_reached_like_part_edges_and_j1_wins_over_its_flush_edge() -> Fallible {
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
    let to_board = sparksmith_core::layout::dimensions::view::part_transform(&placement);
    let mid = to_board.apply([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
    let seat = project
        .stackup
        .copper_span(sparksmith_core::CopperLayer::Top)
        .map_or(0.0, |s| s.1);
    let board = project.board();
    let outline: Vec<[f64; 2]> = std::iter::once(board.outer.start)
        .chain(board.outer.segments.iter().map(|s| s.to()))
        .collect();
    let bottom = outline.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
    let left = outline.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
    let right = outline
        .iter()
        .map(|p| p[0])
        .fold(f64::NEG_INFINITY, f64::max);
    let mut h = Harness::new(engine)?;
    h.until_shown()?;
    h.app
        .scene
        .viewport
        .view_from(sparksmith_solidcraft::geom::Vec3::new(0.15, -1.0, 0.8));
    h.app.scene.viewport.fit();
    h.frame(vec![]);
    h.frame(vec![]);
    let screen = |h: &Harness, p: [f64; 3]| h.app.scene.viewport.to_screen(p).ok_or("off screen");
    let engine = &h.app.engine;
    let _ = h.app.pcb.on_tool("dimension", engine);
    h.frame(vec![]);

    // Along the bottom edge, away from J1: 5 points inside or outside the
    // board, the edge lights.
    for x in [left + 3.0, left + 8.0, (left + right) / 2.0, right - 8.0] {
        if (x - mid[0]).abs() < 8.0 {
            continue;
        }
        let at = screen(&h, [x, bottom, seat])?;
        for dy in [-5.0, -3.0, 0.0, 3.0, 5.0] {
            h.frame(vec![egui::Event::PointerMoved(at + egui::vec2(0.0, dy))]);
            let hovered = h.app.scene.viewport.hovered();
            assert!(
                matches!(hovered, Some(Hovered::Edge { .. })),
                "x {x}, {dy} points: {hovered:?}"
            );
        }
    }

    // J1's front, low on it (where the flush board edge is within reach)
    // and half way up (the shell's opening): J1 each time.
    for height in [0.4, 1.6] {
        let at = screen(&h, [mid[0] + 1.0, mid[1], seat + height])?;
        h.click(at);
        let picked = h.app.pcb.dimension_picks();
        let [(first, said)] = picked.as_slice() else {
            return Err(format!("{height}: {picked:?} {:?}", h.app.pcb.dimension_draft()).into());
        };
        assert!(
            matches!(first, BoardRef::Part(f) if f.target() == sparksmith_core::model::board::ConstraintTarget::Placement(j1)),
            "{height}: {first:?} ({said})"
        );
        assert!(said.starts_with("J1 · "), "{said}");
        // Esc drops the pick for the next one.
        h.frame(vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
        h.frame(vec![]);
        assert!(h.app.pcb.dimension_picks().is_empty());
    }
    // Just below J1, off the part: the board's bottom edge.
    let below = screen(&h, [mid[0] + 1.0, mid[1], 0.0])? + egui::vec2(0.0, 3.0);
    h.click(below);
    let picked = h.app.pcb.dimension_picks();
    let [(first, said)] = picked.as_slice() else {
        return Err(format!("below J1: {picked:?}").into());
    };
    assert!(
        matches!(first, BoardRef::Board(SketchRef::Curve(_))),
        "{first:?}"
    );
    assert_eq!(said, "Board · bottom edge");
    Ok(())
}

/// With Select, a click on J1 in PCB 3D selects J1, shared with Layout
/// (the same selection), and nothing is rebuilt for it. The board is
/// opened in place over another one (the showcase), then edited and the
/// edit undone, as in the demo: its model comes up to date (the Updating
/// chip clears) and the click is not dropped.
#[test]
fn a_3d_click_with_select_selects_the_part() -> Fallible {
    let _serial = serial();
    let mut h = Harness::new(Engine::showcase()?)?;
    h.until_shown()?;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.sparksmith");
    h.app
        .replace(sparksmith_io::read_project(&path)?, None, false);
    h.until_shown()?;
    let j1 = h
        .app
        .engine
        .project()
        .instances
        .iter()
        .find(|i| i.designator == "J1")
        .map(|i| i.id)
        .ok_or("no J1")?;
    let placement = h
        .app
        .engine
        .project()
        .placement_of(j1)
        .ok_or("J1 placed")?
        .clone();
    let ctx = h.ctx.clone();
    h.app.apply(
        &ctx,
        Origin::Command,
        Some(Operation::Layout(
            sparksmith_core::layout::LayoutCommand::Move {
                instance: j1,
                position: [placement.position[0] + 1.0, placement.position[1]],
                rotation_deg: placement.rotation_deg,
                layer: placement.layer,
            },
        )),
    );
    h.until_shown()?;
    h.app.run_command(&ctx, "edit.undo");
    h.until_shown()?;
    assert_eq!(h.app.engine.project().placement_of(j1), Some(&placement));
    // The Board card's parameters, as in the demo, in quick succession
    // (each build is replaced by the next): edits, some undone.
    for (name, expr, undo) in [
        ("width", "44 mm", true),
        ("inset", "4 mm", true),
        ("height", "70 mm", false),
        ("height", "64 mm", false),
    ] {
        let BoardShape::Sketch(sketch) = &h.app.engine.project().board_shape else {
            return Err("a sketched board".into());
        };
        let mut param = sketch
            .parameters
            .iter()
            .find(|p| p.name.0 == name)
            .cloned()
            .ok_or(name)?;
        param.expr = sparksmith_core::library::footprint_source::Expr(expr.into());
        let before = h.app.engine.revision();
        h.app.apply(
            &ctx,
            Origin::Command,
            Some(Operation::Board(
                sparksmith_core::layout::outline::BoardCommand::Edit {
                    edits: vec![sparksmith_core::layout::outline::BoardEdit::SetParameter {
                        param,
                    }],
                },
            )),
        );
        assert_ne!(h.app.engine.revision(), before, "{name} = {expr}");
        for _ in 0..30 {
            h.frame(vec![]);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if undo {
            h.app.run_command(&ctx, "edit.undo");
            for _ in 0..30 {
                h.frame(vec![]);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    h.until_shown()?;
    // Fills and the model settle before the click.
    for _ in 0..200 {
        h.frame(vec![]);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.until_shown()?;
    let items = h.app.scene.selected_items(&crate::pcb_editor::Group {
        parts: [j1].into(),
        ..Default::default()
    });
    h.app
        .scene
        .viewport
        .view_from(sparksmith_solidcraft::geom::Vec3::new(0.15, -1.0, 0.35));
    assert!(h.app.scene.viewport.fit_items(&items));
    h.frame(vec![]);
    h.frame(vec![]);
    let rect = h.app.scene.viewport.rect().ok_or("no 3D view")?;
    let at = rect.center();
    let hit = h
        .app
        .scene
        .viewport
        .surface_hit(at)
        .ok_or("nothing there")?;
    let source = hit.item.and_then(|i| h.app.scene.source(i)).ok_or("J1")?;
    assert_eq!(source.instance, Some(j1));
    assert_eq!(h.app.pcb.selected_instance(), None);
    let (requests, revision) = (h.app.scene.requests(), h.app.engine.revision());
    h.click(at);
    assert_eq!(h.app.pcb.selected_instance(), Some(j1));
    assert_eq!(h.app.scene.requests(), requests, "selection built a model");
    assert_eq!(h.app.engine.revision(), revision);
    Ok(())
}

/// A click on U1 in the showcase's PCB 3D, sent as the window sends it,
/// selects U1 and leaves the camera where it was, after text with a space
/// was typed into a field and the Space key's release never arrived (as
/// the VM's synthetic typing delivers it: egui then still holds Space down,
/// which made the 3D view's Space+drag pan swallow every click). Each
/// press shape counts: still, pressed and released in one frame, pressed
/// right after a jump of the pointer, and moved 2 pt before the release.
#[test]
fn a_3d_click_selects_after_a_space_was_typed_into_a_field() -> Fallible {
    use egui::accesskit::Role;
    let _serial = serial();
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    for shape in 0..4 {
        let mut ax = super::ax_harness::Ax::with_engine(Engine::showcase()?, Workspace::Pcb)?;
        ax.button("3D")?;
        let shown = |ax: &super::ax_harness::Ax| {
            let key = crate::scene::ModelKey::Project(Arc::clone(ax.app.engine.project_arc()));
            ax.app.scene.display(&key) == crate::scene::Display::Current
        };
        for _ in 0..24_000 {
            if shown(&ax) {
                break;
            }
            ax.frame(vec![]);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if !shown(&ax) {
            return Err("the board's model was never shown".into());
        }
        ax.settle();
        let rect = ax.app.scene.viewport.rect().ok_or("no 3D view")?;
        let u1 = ax
            .app
            .engine
            .project()
            .instances
            .iter()
            .find(|i| i.designator == "U1")
            .map(|i| i.id)
            .ok_or("no U1")?;
        let at = (0..60 * 60)
            .map(|k| {
                rect.min
                    + egui::vec2(
                        rect.width() * (k % 60) as f32 / 60.0,
                        rect.height() * (k / 60) as f32 / 60.0,
                    )
            })
            .find(|p| {
                ax.app
                    .scene
                    .viewport
                    .surface_hit(*p)
                    .and_then(|hit| hit.item)
                    .and_then(|i| ax.app.scene.source(i))
                    .is_some_and(|s| s.instance == Some(u1))
            })
            .ok_or("U1 is not on screen")?;
        // "1 mm" typed into the first field, its Space never let go.
        let field = ax.find_prefix(Role::TextInput, "")?;
        ax.click_at(field.center());
        assert!(
            ax.ctx.egui_wants_keyboard_input(),
            "the field has the keyboard"
        );
        ax.type_text("1");
        ax.frame(vec![
            egui::Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text(" ".into()),
        ]);
        ax.type_text("mm");
        ax.key(egui::Key::Enter);
        assert!(
            !ax.ctx.egui_wants_keyboard_input(),
            "Enter leaves the field"
        );
        assert!(ax.ctx.input(|i| i.key_down(egui::Key::Space)));
        assert_eq!(ax.app.pcb.selected_instance(), None);
        // The pointer comes to the view; the first click leaves the field.
        ax.hover(at);
        let (camera, requests) = (ax.app.scene.viewport.camera(), ax.app.scene.requests());
        let far = at + egui::vec2(-120.0, -90.0);
        let near = at + egui::vec2(2.0, 1.0);
        match shape {
            0 => {
                ax.frame(vec![button(at, true)]);
                ax.frame(vec![button(at, false)]);
            }
            1 => ax.frame(vec![button(at, true), button(at, false)]),
            2 => {
                ax.hover(far);
                ax.frame(vec![egui::Event::PointerMoved(at), button(at, true)]);
                ax.frame(vec![button(at, false)]);
            }
            _ => {
                ax.frame(vec![button(at, true)]);
                ax.frame(vec![egui::Event::PointerMoved(near)]);
                ax.frame(vec![button(near, false)]);
            }
        }
        ax.settle();
        assert_eq!(
            ax.app.pcb.selected_instance(),
            Some(u1),
            "press shape {shape}"
        );
        assert_eq!(
            ax.app.scene.viewport.camera(),
            camera,
            "press shape {shape}"
        );
        assert_eq!(ax.app.scene.requests(), requests, "selection built a model");
    }
    Ok(())
}

/// The distance from `p` to the line through `a` and `b` (board mm).
fn off_line(p: [f64; 3], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / dx.hypot(dy)
}

impl Harness {
    fn double_click(&mut self, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Half a second after any earlier click, as a person pauses.
        for _ in 0..30 {
            self.frame(vec![]);
        }
        self.frame(vec![egui::Event::PointerMoved(at)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
        self.frame(vec![]);
    }

    /// The 3D dimension labelled `label`, as drawn last frame.
    fn drawn(&self, label: &str) -> Fallible<(sparksmith_core::Id, egui::Rect, [[f64; 3]; 2])> {
        let drawn = self.app.pcb.dimensions_drawn();
        drawn
            .iter()
            .find(|d| d.1 == label)
            .map(|d| (d.0, d.2, d.3))
            .ok_or_else(|| format!("no dimension reads {label:?}: {drawn:?}").into())
    }
}

/// The founder's ask after the demo: J1's dimensions drawn in PCB 3D
/// between what they measure, and the value edited at the label. With J1
/// selected, its face dimension runs from the shell's front face to the
/// board's bottom edge and its origin dimension from J1's origin to the
/// left edge. A double-click on "0.00 mm" opens it in place, with the
/// value selected; "1 mm" and Enter move J1 1 mm off the edge in one undo
/// step, and the dimension shows 1 mm at J1's new place, also while the
/// model updates. A refused value shows the engine's words at the label
/// and changes nothing; Esc closes it. A click selects a dimension and
/// Delete removes it, one undo step.
#[test]
fn j1s_dimensions_are_drawn_and_edited_in_place_in_3d() -> Fallible {
    let _serial = serial();
    let engine = esp32()?;
    let project = engine.project();
    let j1 = project
        .instances
        .iter()
        .find(|i| i.designator == "J1")
        .map(|i| i.id)
        .ok_or("no J1")?;
    let face_dim = project
        .part_dimensions
        .iter()
        .find(|d| {
            d.refs.iter().any(
                |r| matches!(r, BoardRef::Part(FeatureRef::Face { placement, .. }) if *placement == j1),
            )
        })
        .ok_or("J1's face dimension")?
        .id;
    let trace = project
        .model_traces
        .iter()
        .find(|t| matches!(&t.feature, FeatureRef::Face { placement, .. } if *placement == j1))
        .and_then(|t| t.geometry)
        .ok_or("J1's face trace")?;
    let FeatureGeometry::Line { a, b } = trace else {
        return Err("the face traces a line".into());
    };
    let placement = project.placement_of(j1).ok_or("J1 placed")?.clone();
    let seat = project
        .stackup
        .copper_span(sparksmith_core::CopperLayer::Top)
        .map_or(0.0, |s| s.1);
    let board = project.board();
    let outline: Vec<[f64; 2]> = std::iter::once(board.outer.start)
        .chain(board.outer.segments.iter().map(|s| s.to()))
        .collect();
    let bottom = outline.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
    let left = outline.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
    // The face's trace on the board, with J1 where `at` puts it.
    let face_at = |at: &sparksmith_core::Placement| {
        let to_board = sparksmith_core::layout::dimensions::view::part_transform(at);
        (to_board.apply(a), to_board.apply(b))
    };
    let (fa, fb) = face_at(&placement);
    let mid = [(fa[0] + fb[0]) / 2.0, (fa[1] + fb[1]) / 2.0];

    let mut h = Harness::new(engine)?;
    h.until_shown()?;
    let items = h.app.scene.selected_items(&crate::pcb_editor::Group {
        parts: [j1].into(),
        ..Default::default()
    });
    h.app
        .scene
        .viewport
        .view_from(sparksmith_solidcraft::geom::Vec3::new(0.15, -1.0, 0.6));
    assert!(h.app.scene.viewport.fit_items(&items));
    h.frame(vec![]);
    h.frame(vec![]);
    let screen = |h: &Harness, p: [f64; 3]| h.app.scene.viewport.to_screen(p).ok_or("off screen");

    // Select J1 with a click on its body: both its dimensions are drawn.
    assert!(h.app.pcb.dimensions_drawn().is_empty(), "nothing selected");
    let (requests, revision) = (h.app.scene.requests(), h.app.engine.revision());
    h.click(screen(&h, [mid[0] + 1.0, mid[1] + 2.0, seat + 3.31])?);
    assert_eq!(h.app.pcb.selected_instance(), Some(j1));
    let drawn = h.app.pcb.dimensions_drawn();
    assert_eq!(drawn.len(), 2, "{drawn:?}");
    // The face dimension: one end on the face's trace at the seat, the
    // other on the bottom edge.
    let (id, label_rect, ends) = h.drawn("0.00 mm")?;
    assert_eq!(id, face_dim);
    let on_face = ends.iter().any(|e| off_line(*e, fa, fb) < 1e-6);
    let on_edge = ends.iter().any(|e| (e[1] - bottom).abs() < 1e-6);
    assert!(on_face && on_edge, "{ends:?}");
    assert!(ends.iter().all(|e| (e[2] - seat).abs() < 1e-9), "{ends:?}");
    // The origin dimension: from J1's origin to the left edge, driven by
    // an expression.
    let origin = drawn
        .iter()
        .find(|d| d.0 != face_dim)
        .ok_or("the origin dimension")?;
    assert!(origin.1.starts_with("fx: "), "{}", origin.1);
    let at_origin = origin.3.iter().any(|e| {
        (e[0] - placement.position[0]).abs() < 1e-6 && (e[1] - placement.position[1]).abs() < 1e-6
    });
    let on_left = origin.3.iter().any(|e| (e[0] - left).abs() < 1e-6);
    assert!(at_origin && on_left, "{:?}", origin.3);
    assert_eq!(h.app.scene.requests(), requests, "drawing built a model");
    assert_eq!(h.app.engine.revision(), revision);

    // A double-click on "0.00 mm" opens it there, the value selected:
    // typing replaces it.
    h.double_click(label_rect.center());
    assert_eq!(h.app.pcb.selected_dimension(), Some(face_dim));
    assert_eq!(
        h.app.pcb.dimension_in_place(),
        Some((face_dim, "0 mm".to_owned(), None))
    );
    assert!(h.ctx.text_edit_focused(), "the value takes the keyboard");
    assert_eq!(h.app.engine.revision(), revision, "opening changes nothing");
    h.frame(vec![egui::Event::Text("1 mm".into())]);
    assert_eq!(
        h.app.pcb.dimension_in_place().map(|e| e.1),
        Some("1 mm".to_owned())
    );
    h.frame(vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
    h.frame(vec![]);
    let moved = h.app.engine.project().placement_of(j1).ok_or("J1")?.clone();
    assert!(
        (moved.position[1] - placement.position[1] - 1.0).abs() < 1e-6,
        "{} -> {}",
        placement.position[1],
        moved.position[1]
    );
    assert_eq!(h.app.pcb.dimension_in_place(), None, "committed and closed");
    // While the model updates, the dimension already reads 1 mm, from the
    // face's new place to the edge.
    let (fa, fb) = face_at(&moved);
    let check = |h: &Harness| -> Fallible {
        let (_, _, ends) = h.drawn("1.00 mm")?;
        let on_face = ends.iter().any(|e| off_line(*e, fa, fb) < 1e-6);
        let on_edge = ends.iter().any(|e| (e[1] - bottom).abs() < 1e-6);
        assert!(on_face && on_edge, "{ends:?}");
        let span = (ends[0][1] - ends[1][1]).abs();
        assert!((span - 1.0).abs() < 1e-6, "{span}");
        Ok(())
    };
    let now = crate::scene::ModelKey::Project(Arc::clone(h.app.engine.project_arc()));
    assert_eq!(
        h.app.scene.display(&now),
        crate::scene::Display::Stale,
        "the model is updating"
    );
    check(&h)?;
    h.until_shown()?;
    check(&h)?;
    // One undo step puts J1 and the value back.
    let ctx = h.ctx.clone();
    h.app.run_command(&ctx, "edit.undo");
    h.until_shown()?;
    assert_eq!(h.app.engine.project().placement_of(j1), Some(&placement));
    let (_, label_rect, _) = h.drawn("0.00 mm")?;

    // A value past the board's coordinate range is refused by the engine:
    // its words show at the label, the text stays, and nothing changes.
    let revision = h.app.engine.revision();
    h.double_click(label_rect.center());
    h.frame(vec![egui::Event::Text("20000 mm".into())]);
    h.frame(vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
    h.frame(vec![]);
    h.frame(vec![]);
    let (id, text, why) = h
        .app
        .pcb
        .dimension_in_place()
        .ok_or("the refused value closed")?;
    assert_eq!((id, text.as_str()), (face_dim, "20000 mm"));
    let why = why.ok_or("no refusal at the label")?;
    assert!(why.contains("out of range"), "{why}");
    assert!(!why.contains("layout."), "{why}");
    assert_eq!(h.app.engine.revision(), revision);
    assert_eq!(h.app.engine.project().placement_of(j1), Some(&placement));
    // A field-level refusal shows there too, before anything is sent.
    h.frame(vec![
        key(egui::Key::A, egui::Modifiers::COMMAND),
        egui::Event::Text("-2 mm".into()),
    ]);
    h.frame(vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
    h.frame(vec![]);
    let why = h.app.pcb.dimension_in_place().and_then(|e| e.2);
    assert_eq!(why.as_deref(), Some("A distance is at least 0 mm"));
    // Esc closes it, unchanged.
    h.frame(vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
    h.frame(vec![]);
    assert_eq!(h.app.pcb.dimension_in_place(), None);
    assert_eq!(h.app.engine.revision(), revision);
    assert_eq!(h.app.pcb.selected_instance(), Some(j1), "Esc kept J1");

    // A click on the label selects the dimension, and Delete removes it:
    // one undo step brings it back. (Half a second on, so the click is
    // not a double-click with the last one.)
    for _ in 0..30 {
        h.frame(vec![]);
    }
    h.click(label_rect.center());
    assert_eq!(h.app.pcb.selected_dimension(), Some(face_dim));
    assert_eq!(
        h.app.pcb.dimension_in_place(),
        None,
        "one click only selects"
    );
    h.frame(vec![key(egui::Key::Delete, egui::Modifiers::NONE)]);
    h.frame(vec![]);
    let project = h.app.engine.project();
    assert!(!project.part_dimensions.iter().any(|d| d.id == face_dim));
    assert!(project.instance(j1).is_some(), "J1 stays");
    assert_eq!(h.app.pcb.selected_dimension(), None);
    h.app.run_command(&ctx, "edit.undo");
    h.frame(vec![]);
    assert!(
        h.app
            .engine
            .project()
            .part_dimensions
            .iter()
            .any(|d| d.id == face_dim && d.value == Some(DimValue::Fixed(0.0)))
    );
    assert_eq!(h.app.engine.project().placement_of(j1), Some(&placement));
    Ok(())
}
