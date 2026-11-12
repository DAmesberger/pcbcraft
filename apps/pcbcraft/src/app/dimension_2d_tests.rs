//! Part dimensions in Layout through the whole app's frame, headless, on
//! the ESP32-S31 dev board (docs/board-sketch-references.md §20): the same
//! dimensioning system as PCB 3D. J1's dimensions are drawn between the
//! front face's trace and the bottom edge and edited at the label; a new
//! dimension picked on R5's body trace is the reference a 3D pick makes;
//! J1's stacked back faces open the chooser; and a dimension made in
//! either view shows in the other, alike.
use super::*;
use crate::commands::Registry;
use pcbcraft_core::layout::dimensions::DimensionCommand;
use pcbcraft_core::layout::dimensions::model::{self, BodyFace};
use pcbcraft_core::layout::dimensions::view;
use pcbcraft_core::library::footprint_source::{DimValue, SketchRef};
use pcbcraft_core::model::dimension::{BoardRef, FeatureGeometry, FeatureRef};
use pcbcraft_core::{Id, Project};
use pcbcraft_solidcraft::board::SketchBoardEvaluator;
use pcbcraft_ui::fonts;

type Fallible<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// These tests run one at a time: each drives a whole app.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Each laid-out row of the texts in `shape`.
fn lines(shape: &egui::Shape, out: &mut Vec<String>) {
    match shape {
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| lines(s, out)),
        egui::Shape::Text(t) => out.extend(t.galley.rows.iter().map(|r| r.text())),
        _ => {}
    }
}

struct Harness {
    app: App,
    ctx: egui::Context,
    frame: eframe::Frame,
    _dir: tempfile::TempDir,
    /// The lines of text the last frame painted, one per laid-out row.
    lines: Vec<String>,
}

impl Harness {
    /// The app on `engine`'s PCB, in Layout.
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
        app.pcb.set_3d(false);
        crate::keys::publish(&ctx, &app.registry);
        let mut h = Self {
            app,
            ctx,
            frame: eframe::Frame::_new_kittest(),
            _dir: dir,
            lines: Vec::new(),
        };
        h.frame(vec![]);
        h.frame(vec![]);
        Ok(h)
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
        let mut out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, frame));
        // New glyphs (the lists' mono sizes) grow the font atlas in later
        // frames too; a headless frame has no renderer to hand them to.
        out.textures_delta.clear();
        self.lines.clear();
        for shape in &out.shapes {
            lines(&shape.shape, &mut self.lines);
        }
    }

    /// Frames until the board's 3D model is shown.
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

    /// Frames until the parts' body traces are listed.
    fn until_bodies(&mut self) -> Fallible {
        for _ in 0..24_000 {
            self.frame(vec![]);
            if self.app.pcb.body_traces_pending() == 0 {
                self.frame(vec![]);
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Err("the body traces were never listed".into())
    }

    /// Where a board point is on the Layout canvas.
    fn screen(&self, p: [f64; 2]) -> Fallible<egui::Pos2> {
        self.app
            .pcb
            .layout_to_screen(p)
            .ok_or_else(|| "Layout was not drawn".into())
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

    fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        self.frame(vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }]);
        self.frame(vec![]);
    }

    /// The dimension labelled `label`, as drawn last frame.
    fn drawn(&self, label: &str) -> Fallible<(Id, egui::Rect, [[f64; 3]; 2])> {
        let drawn = self.app.pcb.dimensions_drawn();
        drawn
            .iter()
            .find(|d| d.1 == label)
            .map(|d| (d.0, d.2, d.3))
            .ok_or_else(|| format!("no dimension reads {label:?}: {drawn:?}").into())
    }

    /// Arm the Dimension tool with its key, Shift+D, as in 3D, and wait for
    /// the body traces.
    fn arm(&mut self) -> Fallible {
        let canvas = self.screen([0.0, 0.0])?;
        self.frame(vec![egui::Event::PointerMoved(canvas)]);
        self.key(egui::Key::D, egui::Modifiers::SHIFT);
        let (n, _, _) = self.app.pcb.dimension_draft();
        assert_eq!(n, 0);
        self.until_bodies()
    }
}

fn esp32() -> Fallible<Engine> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    Ok(Engine::from_file(pcbcraft_io::read_project(&path)?)?)
}

fn part(project: &Project, designator: &str) -> Fallible<Id> {
    project
        .instances
        .iter()
        .find(|i| i.designator == designator)
        .map(|i| i.id)
        .ok_or_else(|| format!("no {designator}").into())
}

/// The board's lowest y and x (mm).
fn bottom_left(project: &Project) -> (f64, f64) {
    let board = project.board();
    let outline: Vec<[f64; 2]> = std::iter::once(board.outer.start)
        .chain(board.outer.segments.iter().map(|s| s.to()))
        .collect();
    (
        outline.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min),
        outline.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min),
    )
}

/// The distance from `p` to the line through `a` and `b` (board mm).
fn off_line(p: [f64; 3], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / dx.hypot(dy)
}

/// J1's face dimension and its face's trace in the part frame.
fn j1_face(project: &Project, j1: Id) -> Fallible<(Id, [f64; 2], [f64; 2])> {
    let id = project
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
    Ok((id, a, b))
}

/// The middle of a line trace, moved into the board by `placement`.
fn board_mid(placement: &pcbcraft_core::Placement, g: &FeatureGeometry) -> Fallible<[f64; 2]> {
    let FeatureGeometry::Line { a, b } = view::transformed(*g, &view::part_transform(placement))
    else {
        return Err(format!("{g:?} is not a line").into());
    };
    Ok([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0])
}

/// The founder's ask, in Layout: J1's dimensions drawn between what they
/// measure, and the value edited at the label. With J1 selected by a click,
/// its face dimension runs from the shell's front-face trace to the bottom
/// edge. A double-click on "0.00 mm" opens it there, the value selected;
/// "1 mm" and Enter move J1 1 mm off the edge in one undo step.
#[test]
fn j1s_dimensions_are_drawn_and_edited_in_place_in_layout() -> Fallible {
    let _serial = serial();
    let engine = esp32()?;
    let project = engine.project();
    let j1 = part(project, "J1")?;
    let (face_dim, a, b) = j1_face(project, j1)?;
    let placement = project.placement_of(j1).ok_or("J1 placed")?.clone();
    let (bottom, _) = bottom_left(project);
    let face_at = |at: &pcbcraft_core::Placement| {
        let to_board = view::part_transform(at);
        (to_board.apply(a), to_board.apply(b))
    };
    let (fa, fb) = face_at(&placement);
    let mut h = Harness::new(engine)?;
    assert!(h.app.pcb.dimensions_drawn().is_empty(), "nothing selected");
    let revision = h.app.engine.revision();

    // A click on J1 selects it: both its dimensions are drawn.
    let at = h.screen(placement.position)?;
    h.click(at);
    assert_eq!(h.app.pcb.selected_instance(), Some(j1));
    let drawn = h.app.pcb.dimensions_drawn();
    assert_eq!(drawn.len(), 2, "{drawn:?}");
    let (id, label_rect, ends) = h.drawn("0.00 mm")?;
    assert_eq!(id, face_dim);
    let on_face = ends.iter().any(|e| off_line(*e, fa, fb) < 1e-6);
    let on_edge = ends.iter().any(|e| (e[1] - bottom).abs() < 1e-6);
    assert!(on_face && on_edge, "{ends:?}");
    assert_eq!(h.app.engine.revision(), revision);

    // A double-click on "0.00 mm" opens it at the label, all selected.
    h.double_click(label_rect.center());
    assert_eq!(h.app.pcb.selected_dimension(), Some(face_dim));
    assert_eq!(
        h.app.pcb.dimension_in_place(),
        Some((face_dim, "0 mm".to_owned(), None))
    );
    assert!(h.ctx.text_edit_focused(), "the value takes the keyboard");
    h.frame(vec![egui::Event::Text("1 mm".into())]);
    h.key(egui::Key::Enter, egui::Modifiers::NONE);
    h.frame(vec![]);
    let moved = h.app.engine.project().placement_of(j1).ok_or("J1")?.clone();
    assert!(
        (moved.position[1] - placement.position[1] - 1.0).abs() < 1e-6,
        "{} -> {}",
        placement.position[1],
        moved.position[1]
    );
    assert_eq!(h.app.pcb.dimension_in_place(), None, "committed and closed");
    let (fa, fb) = face_at(&moved);
    let (_, _, ends) = h.drawn("1.00 mm")?;
    assert!(ends.iter().any(|e| off_line(*e, fa, fb) < 1e-6), "{ends:?}");
    assert!(
        ends.iter().any(|e| (e[1] - bottom).abs() < 1e-6),
        "{ends:?}"
    );

    // One undo step puts J1 back.
    let ctx = h.ctx.clone();
    h.app.run_command(&ctx, "edit.undo");
    h.frame(vec![]);
    assert_eq!(h.app.engine.project().placement_of(j1), Some(&placement));
    h.drawn("0.00 mm")?;
    Ok(())
}

/// R5's body traces are listed by the host; its end face is the body face
/// whose trace lies farthest along the part's x.
fn r5_end_face(project: &Project, r5: Id) -> Fallible<BodyFace> {
    let listed = model::body_faces(project, &SketchBoardEvaluator, r5)?;
    let mid_x = |f: &BodyFace| match f.trace {
        FeatureGeometry::Line { a, b } => Some(((a[0] + b[0]) / 2.0).abs()),
        _ => None,
    };
    listed
        .faces
        .iter()
        .filter(|f| mid_x(f).is_some())
        .max_by(|p, q| mid_x(p).unwrap_or(0.0).total_cmp(&mid_x(q).unwrap_or(0.0)))
        .cloned()
        .ok_or_else(|| "R5 has no face square to the board".into())
}

/// With the Dimension tool in Layout (Shift+D, as in 3D), R5's body edge
/// (its end face's trace) and the board's bottom edge make a dimension.
/// The face reference is the one a 3D pick of that face makes, at the same
/// point; a 3D pick elsewhere on the face names the same face.
#[test]
fn a_layout_pick_on_r5s_body_trace_is_the_reference_a_3d_pick_makes() -> Fallible {
    let _serial = serial();
    let engine = esp32()?;
    let project = engine.project();
    let r5 = part(project, "R5")?;
    let placement = project.placement_of(r5).ok_or("R5 placed")?.clone();
    let end = r5_end_face(project, r5)?;
    let mid = board_mid(&placement, &end.trace)?;
    let (bottom, left) = bottom_left(project);
    let mut h = Harness::new(engine)?;
    h.arm()?;
    let revision = h.app.engine.revision();

    // The end face's trace: named on hover as 3D names it, then picked.
    let on_trace = h.screen(mid)?;
    h.click(on_trace);
    if let Some(rows) = h.app.pcb.dimension_chooser() {
        // Faces in one plane (a termination's end on the body's): the
        // chooser offers each; the first is taken.
        let (_, row) = rows.first().ok_or("an empty chooser")?;
        h.click(row.center());
    }
    let picks = h.app.pcb.dimension_picks();
    let [(BoardRef::Part(chosen), words)] = picks.as_slice() else {
        return Err(format!("{picks:?} {:?}", h.app.pcb.dimension_draft()).into());
    };
    assert!(words.starts_with("R5 · "), "{words}");
    assert!(words.contains("face"), "{words}");
    let FeatureRef::Face {
        placement: on,
        face,
    } = chosen
    else {
        return Err(format!("{chosen:?}").into());
    };
    assert_eq!(*on, r5);
    // The bottom edge, away from the parts.
    h.click(h.screen([left + 1.5, bottom])?);
    let (n, value, error) = h.app.pcb.dimension_draft();
    assert_eq!((n, error), (2, None));
    let value = value.ok_or("the value opened")?;
    assert_eq!(h.app.engine.revision(), revision, "nothing committed yet");
    h.frame(vec![]);
    assert!(h.ctx.text_edit_focused(), "the value takes the keyboard");
    h.key(egui::Key::Enter, egui::Modifiers::NONE);
    h.frame(vec![]);

    let project = h.app.engine.project();
    let added = project
        .part_dimensions
        .iter()
        .find(|d| d.refs.first() == Some(&BoardRef::Part(chosen.clone())))
        .ok_or("the new dimension")?;
    assert!(
        matches!(
            added.refs.get(1),
            Some(BoardRef::Board(SketchRef::Curve(_)))
        ),
        "{:?}",
        added.refs
    );
    assert_eq!(h.app.pcb.selected_dimension(), Some(added.id));
    // The value measured is the one committed: R5 stays.
    let measured = value.trim_end_matches(" mm").parse::<f64>()?;
    assert_eq!(added.value, Some(DimValue::Fixed(measured)));
    assert_eq!(project.placement_of(r5), Some(&placement));

    // The 3D pick of that face, at the same point, makes the same
    // reference.
    let listed = model::body_faces(project, &SketchBoardEvaluator, r5)?;
    let picked_face = listed
        .faces
        .iter()
        .find(|f| f.face.body == face.body && f.face.face == face.face)
        .ok_or("the chosen face is listed")?;
    let at = model::board_point(project, &placement, picked_face.at);
    let (in_3d, _) = model::pick(project, &SketchBoardEvaluator, r5, at, 0.0)?;
    assert_eq!(&in_3d, chosen);
    // Anywhere else on the face, a 3D pick names the same face.
    let elsewhere = match picked_face.trace {
        FeatureGeometry::Line { a, b } => [
            a[0] * 0.3 + b[0] * 0.7,
            a[1] * 0.3 + b[1] * 0.7,
            (picked_face.z[0] + picked_face.z[1]) / 2.0,
        ],
        _ => return Err("a line trace".into()),
    };
    let at = model::board_point(project, &placement, elsewhere);
    let (other, _) = model::pick(project, &SketchBoardEvaluator, r5, at, 0.0)?;
    let FeatureRef::Face { face: other, .. } = other else {
        return Err(format!("{other:?}").into());
    };
    assert_eq!(
        (other.body, other.face),
        (face.body.clone(), face.face.clone())
    );
    Ok(())
}

/// J1's shell back face and its insulator's back face lie in one plane,
/// so their traces are one line: a click there offers both by name and
/// body. The insulator's row picks the insulator's face; Esc on an open
/// chooser closes it and keeps the tool.
#[test]
fn stacked_traces_on_j1_open_the_chooser() -> Fallible {
    let _serial = serial();
    let engine = esp32()?;
    let project = engine.project();
    let j1 = part(project, "J1")?;
    let placement = project.placement_of(j1).ok_or("J1 placed")?.clone();
    let listed = model::body_faces(project, &SketchBoardEvaluator, j1)?;
    // The back faces: lines across the part where the shell's back is
    // (the tails reach further back, beside it).
    let back_y = |f: &BodyFace| match f.trace {
        FeatureGeometry::Line { a, b } if (a[1] - b[1]).abs() < 1e-9 => Some(a[1]),
        _ => None,
    };
    let top = listed
        .faces
        .iter()
        .filter(|f| f.face.body.0 == "Shell")
        .filter_map(back_y)
        .fold(f64::NEG_INFINITY, f64::max);
    let backs: Vec<&BodyFace> = listed
        .faces
        .iter()
        .filter(|f| back_y(f).is_some_and(|y| (y - top).abs() < 1e-6))
        .collect();
    let bodies: Vec<&str> = backs.iter().map(|f| f.face.body.0.as_str()).collect();
    assert!(
        bodies.contains(&"Shell") && bodies.contains(&"Insulator"),
        "{bodies:?} at y {top}: {:?}",
        listed
            .faces
            .iter()
            .map(|f| (&f.face.body.0, f.trace, f.z))
            .collect::<Vec<_>>()
    );
    // Off the tails, on both the shell's and the insulator's spans.
    let at = view::part_transform(&placement).apply([3.95, top]);
    let mut h = Harness::new(engine)?;
    h.arm()?;
    h.click(h.screen(at)?);
    let rows = h.app.pcb.dimension_chooser().ok_or("no chooser")?;
    let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert!(
        names.contains(&"J1 · back face (Shell)") && names.contains(&"J1 · back face (Insulator)"),
        "{names:?}"
    );
    assert_eq!(h.app.pcb.dimension_draft().0, 0, "nothing picked yet");
    // Each row reads its face on one line and how high it reaches whole
    // on the next, not wrapped mid-phrase in the menu's width.
    let heights: Vec<&String> = h.lines.iter().filter(|l| l.contains("mm high")).collect();
    assert!(
        heights.len() >= 2
            && heights
                .iter()
                .all(|l| l.ends_with(" mm high") && !l.contains("J1") && l.contains('–')),
        "{heights:?}"
    );
    // Esc closes the chooser and keeps the tool.
    h.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(h.app.pcb.dimension_chooser().is_none());
    assert!(h.app.pcb.dimension_tool_on());
    // Again, and the insulator's row picks its face.
    h.click(h.screen(at)?);
    let rows = h.app.pcb.dimension_chooser().ok_or("no chooser")?;
    let (_, row) = rows
        .iter()
        .find(|r| r.0 == "J1 · back face (Insulator)")
        .ok_or("the insulator's row")?;
    h.click(row.center());
    assert!(h.app.pcb.dimension_chooser().is_none());
    let picks = h.app.pcb.dimension_picks();
    let [(BoardRef::Part(FeatureRef::Face { placement, face }), words)] = picks.as_slice() else {
        return Err(format!("{picks:?} {:?}", h.app.pcb.dimension_draft()).into());
    };
    assert_eq!((*placement, face.body.0.as_str()), (j1, "Insulator"));
    assert_eq!(words, "J1 · back face (Insulator)");
    Ok(())
}

/// One system, two views: a dimension made with the 3D tool shows in
/// Layout, and one made in Layout shows in 3D, each with the same label,
/// the same ends and the same selection.
#[test]
fn a_dimension_made_in_either_view_shows_in_the_other() -> Fallible {
    let _serial = serial();
    let mut engine = esp32()?;
    let project = engine.project();
    let j1 = part(project, "J1")?;
    let r5 = part(project, "R5")?;
    let (face_dim, a, b) = j1_face(project, j1)?;
    let placement = project.placement_of(j1).ok_or("J1 placed")?.clone();
    let r5_placement = project.placement_of(r5).ok_or("R5 placed")?.clone();
    let end = r5_end_face(project, r5)?;
    let r5_mid = board_mid(&r5_placement, &end.trace)?;
    let (bottom, left) = bottom_left(project);
    // J1's face dimension is made again with the 3D tool.
    engine.apply_hosted(
        engine.revision(),
        Operation::Dimension(DimensionCommand::Delete { id: face_dim }),
        pcbcraft_automation::session::hosted(),
    )?;
    let to_board = view::part_transform(&placement);
    let mid = to_board.apply([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
    let seat = engine
        .project()
        .stackup
        .copper_span(pcbcraft_core::CopperLayer::Top)
        .map_or(0.0, |s| s.1);
    let laminate = engine.project().stackup.dielectric_mm();
    let mut h = Harness::new(engine)?;
    h.app.pcb.set_3d(true);
    h.until_shown()?;
    let items = h.app.scene.selected_items(&crate::pcb_editor::Group {
        parts: [j1].into(),
        ..Default::default()
    });
    h.app
        .scene
        .viewport
        .view_from(pcbcraft_solidcraft::geom::Vec3::new(0.15, -1.0, 0.35));
    assert!(h.app.scene.viewport.fit_items(&items));
    h.frame(vec![]);
    h.frame(vec![]);
    let screen3d = |h: &Harness, p: [f64; 3]| h.app.scene.viewport.to_screen(p).ok_or("off screen");
    let engine = &h.app.engine;
    let _ = h.app.pcb.on_tool("dimension", engine);
    h.frame(vec![]);
    h.click(screen3d(&h, [mid[0] + 1.0, mid[1], seat + 0.3])?);
    h.click(screen3d(&h, [mid[0] + 1.0, mid[1], laminate / 2.0])?);
    assert_eq!(h.app.pcb.dimension_draft().0, 2);
    h.frame(vec![]);
    h.key(egui::Key::Enter, egui::Modifiers::NONE);
    h.frame(vec![]);
    let made_3d = h.app.pcb.selected_dimension().ok_or("the 3D dimension")?;
    h.frame(vec![]);
    let in_3d = h
        .app
        .pcb
        .dimensions_drawn()
        .into_iter()
        .find(|d| d.0 == made_3d)
        .ok_or("drawn in 3D")?;
    // Layout: the same dimension, label and ends, still selected.
    h.app.pcb.set_3d(false);
    h.frame(vec![]);
    h.frame(vec![]);
    assert_eq!(h.app.pcb.selected_dimension(), Some(made_3d));
    let in_2d = h
        .app
        .pcb
        .dimensions_drawn()
        .into_iter()
        .find(|d| d.0 == made_3d)
        .ok_or("drawn in Layout")?;
    assert_eq!((&in_2d.1, in_2d.3), (&in_3d.1, in_3d.3));
    assert_eq!(in_2d.1, "0.00 mm");

    // Made in Layout (the tool stays on across the switch): R5's end face
    // to the bottom edge, then seen in 3D.
    assert!(h.app.pcb.dimension_tool_on());
    assert_eq!(h.app.pcb.dimension_draft().0, 0);
    h.until_bodies()?;
    h.click(h.screen(r5_mid)?);
    if let Some(rows) = h.app.pcb.dimension_chooser() {
        let (_, row) = rows.first().ok_or("an empty chooser")?;
        h.click(row.center());
    }
    h.click(h.screen([left + 1.5, bottom])?);
    assert_eq!(
        h.app.pcb.dimension_draft().0,
        2,
        "{:?}",
        h.app.pcb.dimension_draft()
    );
    h.frame(vec![]);
    h.key(egui::Key::Enter, egui::Modifiers::NONE);
    h.frame(vec![]);
    let made_2d = h
        .app
        .pcb
        .selected_dimension()
        .ok_or("the Layout dimension")?;
    assert_ne!(made_2d, made_3d);
    h.frame(vec![]);
    let in_2d = h
        .app
        .pcb
        .dimensions_drawn()
        .into_iter()
        .find(|d| d.0 == made_2d)
        .ok_or("drawn in Layout")?;
    h.app.pcb.set_3d(true);
    h.until_shown()?;
    h.frame(vec![]);
    assert_eq!(h.app.pcb.selected_dimension(), Some(made_2d));
    let in_3d = h
        .app
        .pcb
        .dimensions_drawn()
        .into_iter()
        .find(|d| d.0 == made_2d)
        .ok_or("drawn in 3D")?;
    assert_eq!((&in_3d.1, in_3d.3), (&in_2d.1, in_2d.3));
    Ok(())
}
