//! Board shape mode and edge-relative placement: the gesture builders on
//! SolidCraft's real sketch engine, the canvas and card in headless egui
//! frames, and the Placement card's Board edge section on the engine.
use super::*;
use crate::pcb_editor::edge_card;
use crate::pcb_editor::{PcbEditor, board_view};
use crate::refusal;
use egui::{Pos2, Shape};
use pcbcraft_core::layout::outline::{BoardCommand, BoardEvaluation, edge_pose};
use pcbcraft_core::library::footprint_source::SolveStatus;
use pcbcraft_core::library::footprint_source::{EvaluatorRef, FootprintEvaluator, SketchOutcome};
use pcbcraft_core::model::board::{
    ConstraintTarget, EdgeAnchor, NamedEdge, SolvedBoard, SolvedEdge,
};
use pcbcraft_core::{BoardShape, EdgeConstraint, Engine, Operation, Project};
use pcbcraft_solidcraft::footprint::SketchEvaluator;
use pcbcraft_solidcraft::footprint::view::LiveSketch;
use solidcraft_ui_egui::fonts;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A stand-in for the integration crate's board evaluator: SolidCraft's
/// sketch edits (as the footprint evaluator applies them), no solve.
struct Host;

impl BoardShapeEvaluator for Host {
    fn evaluator(&self) -> EvaluatorRef {
        pcbcraft_solidcraft::footprint::evaluator_ref()
    }
    fn rectangle(&self, size: Point) -> Result<BoardSketch> {
        board_of(size, Vec::new())
    }
    fn apply_sketch(&self, sketch: &SketchBlob, edit: &SketchEdit) -> Result<SketchOutcome> {
        SketchEvaluator.apply_sketch(sketch, edit)
    }
    fn evaluate(&self, _: &BoardSketch, _: &[EdgeConstraint]) -> Result<BoardEvaluation> {
        Err(Error::Unsupported("test host".into()))
    }
}

static HOST: Host = Host;

fn blank() -> Result<SketchBlob> {
    SketchEvaluator.new_sketch()
}

/// Apply a commit's sketch edits to `sketch`, as the engine would.
fn applied(sketch: &SketchBlob, edits: &[BoardEdit]) -> Result<SketchBlob> {
    let mut blob = sketch.clone();
    for edit in edits {
        if let BoardEdit::Sketch { edit } = edit {
            blob = SketchEvaluator.apply_sketch(&blob, edit)?.sketch;
        }
    }
    Ok(blob)
}

fn parameters_of(edits: &[BoardEdit]) -> Vec<FootprintParam> {
    edits
        .iter()
        .filter_map(|e| match e {
            BoardEdit::SetParameter { param } => Some(param.clone()),
            _ => None,
        })
        .collect()
}

fn status(sketch: &SketchBlob, parameters: Vec<FootprintParam>) -> Result<SolveStatus> {
    let live = LiveSketch::load(&source_with(sketch.clone(), parameters), &[])?;
    Ok(live.report().status)
}

/// A board sketch of a `size` rectangle drawn by the Rectangle tool's
/// builder, its sides named as a rectangle's.
fn board_of(size: Point, parameters: Vec<FootprintParam>) -> Result<BoardSketch> {
    let mut stage = Stage::new(&HOST, &blank()?);
    let curves = rectangle(&mut stage, [0.0, 0.0], size, 0.0, None, false)?;
    let edges = curves
        .iter()
        .zip(pcbcraft_core::model::board::RECT_EDGES)
        .map(|(c, name)| NamedEdge {
            name: EdgeName(name.into()),
            curve: c.clone(),
        })
        .collect();
    Ok(BoardSketch {
        evaluator: HOST.evaluator(),
        sketch: stage.staged().clone(),
        parameters,
        outline: curves,
        cutouts: Vec::new(),
        holes: Vec::new(),
        edges,
        next_feature: 1,
        solve: Default::default(),
        solved: SolvedBoard::rectangle(size),
    })
}

fn scene_view(board: &BoardSketch) -> Result<SketchView> {
    view::sketch_view(&source_of(board))
}

#[test]
fn a_rounded_rectangle_binds_its_corners_to_one_parameter_and_solves() -> Fallible {
    let mut stage = Stage::new(&HOST, &blank()?);
    let name = fresh_name(&[], "corner_r");
    let curves = rectangle(
        &mut stage,
        [0.0, 0.0],
        [50.0, 30.0],
        3.0,
        Some((name.clone(), true)),
        false,
    )?;
    // Four sides and four corner arcs, in loop order.
    assert_eq!(curves.len(), 8);
    let params = parameters_of(&stage.edits);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, name);
    assert_eq!(params[0].expr.0, "3 mm");
    let bound = stage
        .edits
        .iter()
        .filter(|e| {
            matches!(e, BoardEdit::Sketch { edit: SketchEdit::AddDimension {
                kind: DimKind::Radius, value: DimValue::Param(p), ..
            } } if *p == name)
        })
        .count();
    assert_eq!(bound, 4, "every corner reads the one radius parameter");
    let sketch = stage.staged().clone();
    let solved = status(&sketch, params.clone())?;
    assert!(
        matches!(solved, SolveStatus::Solved | SolveStatus::UnderConstrained),
        "{solved:?}"
    );
    // A second rounded rectangle gets its own parameter.
    assert_eq!(fresh_name(&params, "corner_r").0, "corner_r2");
    // Too large a radius is refused before anything is drawn.
    let mut stage = Stage::new(&HOST, &blank()?);
    assert!(rectangle(&mut stage, [0.0, 0.0], [10.0, 4.0], 2.5, None, false).is_err());
    assert!(stage.edits.is_empty());
    Ok(())
}

#[test]
fn a_fillet_rounds_a_drawn_corner_and_refuses_a_shared_one() -> Fallible {
    let board = board_of([40.0, 20.0], Vec::new())?;
    let view = scene_view(&board)?;
    let mut stage = Stage::new(&HOST, &board.sketch);
    fillet(&mut stage, &view, [0.1, 0.0], 0.5, 4.0)?;
    let after = view_of(stage.staged())?;
    let arcs: Vec<_> = after
        .curves
        .iter()
        .filter_map(|c| match c.geometry {
            ViewGeometry::Arc { center, start, end } => Some((center, start, end)),
            _ => None,
        })
        .collect();
    assert_eq!(arcs.len(), 1);
    let (center, start, end) = arcs[0];
    assert!(dist(center, [4.0, 4.0]) < 1e-6, "{center:?}");
    // The corner's ends moved to the tangent points.
    let ends = [start, end];
    assert!(ends.iter().any(|p| dist(*p, [4.0, 0.0]) < 1e-6));
    assert!(ends.iter().any(|p| dist(*p, [0.0, 4.0]) < 1e-6));
    assert!(matches!(
        status(stage.staged(), Vec::new())?,
        SolveStatus::Solved | SolveStatus::UnderConstrained
    ));
    // Too large for the 20 mm side.
    let mut stage = Stage::new(&HOST, &board.sketch);
    let error = fillet(&mut stage, &view, [0.0, 0.0], 0.5, 25.0)
        .err()
        .ok_or("too large a fillet was accepted")?;
    assert!(error.to_string().contains("too large"), "{error}");
    // A corner drawn as one shared point cannot be opened.
    let shared = SketchEvaluator
        .apply_sketch(
            &blank()?,
            &SketchEdit::AddRect {
                a: [0.0, 0.0],
                b: [10.0, 10.0],
                construction: false,
            },
        )?
        .sketch;
    let view = view_of(&shared)?;
    let mut stage = Stage::new(&HOST, &shared);
    let error = fillet(&mut stage, &view, [10.0, 10.0], 0.5, 1.0)
        .err()
        .ok_or("a shared corner was filleted")?;
    assert!(error.to_string().contains("share"), "{error}");
    Ok(())
}

#[test]
fn cutouts_slots_and_holes_name_the_curves_and_points_they_draw() -> Fallible {
    let board = board_of([60.0, 40.0], Vec::new())?;
    let view = scene_view(&board)?;
    let scene = Scene {
        board: &board,
        view: &view,
        evaluator: &HOST,
        tolerance: 0.3,
    };
    let options = ShapeOptions::default();
    for (tool, clicks) in [
        (ShapeTool::Cutout, [[10.0, 10.0], [20.0, 18.0]]),
        (ShapeTool::Slot, [[30.0, 20.0], [45.0, 20.0]]),
    ] {
        let mut gesture = Gesture::default();
        assert!(matches!(
            press(tool, &options, &mut gesture, clicks[0], &scene)?,
            Step::More
        ));
        let Step::Commit(commit) = press(tool, &options, &mut gesture, clicks[1], &scene)? else {
            return Err(format!("{tool:?} did not commit").into());
        };
        let Some(BoardEdit::AddCutout { kind, curves }) = commit.edits.last() else {
            return Err(format!("{tool:?} added no cutout").into());
        };
        assert_eq!(
            *kind,
            if tool == ShapeTool::Slot {
                CutoutKind::Slot { plated: false }
            } else {
                CutoutKind::Cutout
            }
        );
        assert_eq!(curves.len(), 4);
        let drawn = view_of(&applied(&board.sketch, &commit.edits)?)?;
        assert!(curves.iter().all(|c| drawn.curve(c).is_some()));
        assert!(matches!(
            status(&applied(&board.sketch, &commit.edits)?, Vec::new())?,
            SolveStatus::Solved | SolveStatus::UnderConstrained
        ));
    }
    let mut gesture = Gesture::default();
    let Step::Commit(commit) = press(ShapeTool::Hole, &options, &mut gesture, [5.0, 5.0], &scene)?
    else {
        return Err("the hole did not commit".into());
    };
    let Some(BoardEdit::AddMountingHole { center, drill, .. }) = commit.edits.last() else {
        return Err("no mounting hole".into());
    };
    assert_eq!(drill.0, "3.2 mm");
    let drawn = view_of(&applied(&board.sketch, &commit.edits)?)?;
    assert_eq!(drawn.point(center).map(|p| p.at), Some([5.0, 5.0]));
    Ok(())
}

#[test]
fn lines_chain_and_tie_to_the_points_they_snap_to() -> Fallible {
    let board = board_of([60.0, 40.0], Vec::new())?;
    let view = scene_view(&board)?;
    let scene = Scene {
        board: &board,
        view: &view,
        evaluator: &HOST,
        tolerance: 0.5,
    };
    let mut gesture = Gesture::default();
    let options = ShapeOptions::default();
    press(
        ShapeTool::Line,
        &options,
        &mut gesture,
        [60.2, 39.8],
        &scene,
    )?;
    let Step::Commit(commit) = press(
        ShapeTool::Line,
        &options,
        &mut gesture,
        [70.0, 30.0],
        &scene,
    )?
    else {
        return Err("the line did not commit".into());
    };
    // Snapped to the board's corner and tied there.
    assert!(commit.edits.iter().any(|e| matches!(
        e,
        BoardEdit::Sketch { edit: SketchEdit::AddLine { a, .. } } if *a == [60.0, 40.0]
    )));
    assert!(commit.edits.iter().any(|e| matches!(
        e,
        BoardEdit::Sketch {
            edit: SketchEdit::AddConstraint {
                kind: GeomConstraint::Coincident,
                ..
            }
        }
    )));
    // The chain continues from the line's end.
    assert_eq!(gesture.pending.len(), 1);
    assert_eq!(gesture.pending[0].at, [70.0, 30.0]);
    Ok(())
}

#[test]
fn a_dimension_is_placed_then_bound_to_a_board_parameter() -> Fallible {
    let board = board_of([60.0, 40.0], Vec::new())?;
    let view = scene_view(&board)?;
    let scene = Scene {
        board: &board,
        view: &view,
        evaluator: &HOST,
        tolerance: 0.5,
    };
    let mut gesture = Gesture::default();
    let options = ShapeOptions::default();
    // The bottom side, then a click below it places its length.
    press(
        ShapeTool::Dimension,
        &options,
        &mut gesture,
        [30.0, 0.0],
        &scene,
    )?;
    let Step::Commit(commit) = press(
        ShapeTool::Dimension,
        &options,
        &mut gesture,
        [30.0, -5.0],
        &scene,
    )?
    else {
        return Err("the dimension was not placed".into());
    };
    let id = commit
        .select
        .clone()
        .ok_or("the new dimension is not selected")?;
    let dimensioned = BoardSketch {
        sketch: applied(&board.sketch, &commit.edits)?,
        ..board.clone()
    };
    let view = scene_view(&dimensioned)?;
    let shown = view
        .constraints
        .iter()
        .find(|k| k.id == id)
        .ok_or("the dimension is missing")?;
    assert_eq!(shown.value.map(|v| (v * 1e6).round() / 1e6), Some(60.0));
    let edits = dimension_value_edits(&dimensioned, &view, &id, "width = 80 mm")?;
    assert!(matches!(
        edits.first(),
        Some(BoardEdit::SetParameter { param }) if param.name.0 == "width" && param.expr.0 == "80 mm"
    ));
    assert!(matches!(
        edits.get(1),
        Some(BoardEdit::Sketch { edit: SketchEdit::SetDimension { value: DimValue::Param(p), .. } })
            if p.0 == "width"
    ));
    // A bare name must exist.
    assert!(dimension_value_edits(&dimensioned, &view, &id, "nowhere").is_err());
    Ok(())
}

#[test]
fn deleting_a_named_curve_unnames_it_first() -> Fallible {
    let board = board_of([60.0, 40.0], Vec::new())?;
    let bottom = board.edges[0].curve.clone();
    let edits = delete_edits(&board, &[SketchRef::Curve(bottom)]);
    assert!(matches!(
        edits.first(),
        Some(BoardEdit::UnnameEdge { name }) if name.0 == "bottom"
    ));
    assert!(matches!(
        edits.last(),
        Some(BoardEdit::Sketch {
            edit: SketchEdit::Delete { .. }
        })
    ));
    assert!(delete_edits(&board, &[]).is_empty());
    Ok(())
}

fn sketch_engine(parameters: Vec<FootprintParam>) -> Fallible<Engine> {
    let mut file = Engine::demo()?.to_file();
    let size = file.project.board_size;
    file.project.board_shape = BoardShape::Sketch(Box::new(board_of(size, parameters)?));
    Ok(Engine::from_file(file)?)
}

#[test]
fn edge_values_evaluate_board_parameters() -> Fallible {
    let width = FootprintParam {
        name: ParamName("width".into()),
        expr: Expr("80 mm".into()),
        unit: ParamUnit::Mm,
        comment: String::new(),
    };
    let engine = sketch_engine(vec![width])?;
    let project = engine.project();
    let v = edge_card::evaluate(project, &Expr("width / 2 - 3 mm".into()))
        .ok_or("the expression did not evaluate")?;
    assert!((v - 37.0).abs() < 1e-9, "{v}");
    assert_eq!(
        edge_card::evaluate(project, &Expr("2.5 mm".into())),
        Some(2.5)
    );
    assert!(edge_card::expression_error(project, "width / 2").is_none());
    assert!(edge_card::expression_error(project, "depth").is_some());
    // A rectangular board has no parameters.
    let demo = Engine::demo()?;
    assert!(edge_card::expression_error(demo.project(), "width").is_some());
    assert!(edge_card::expression_error(demo.project(), "4 mm").is_none());
    Ok(())
}

#[test]
fn a_constraint_from_the_current_position_keeps_the_part_in_place() -> Fallible {
    let board = SolvedBoard::rectangle([50.0, 30.0]);
    let at = [10.0, 4.0];
    let nearest = edge_card::nearest_edge(&board, at).ok_or("no edge")?;
    assert_eq!(nearest.name.0, "bottom");
    for edge in &board.edges {
        let spec = edge_card::spec_at(7, edge, at).ok_or("no spec")?;
        let along = kit_number(&spec.along.0)?;
        let offset = kit_number(&spec.offset.0)?;
        let pose = edge_pose(edge, EdgeAnchor::Start, along, offset)?;
        assert!(dist(pose.position, at) < 1e-9, "{} {:?}", edge.name.0, pose);
        assert_eq!(spec.target, ConstraintTarget::Placement(7));
    }
    // Inside the board, the offset is positive (the inward normal).
    let top: &SolvedEdge = board.edge(&EdgeName("top".into())).ok_or("top")?;
    let (along, offset) = edge_card::along_offset(top, at).ok_or("along")?;
    assert!((along - 40.0).abs() < 1e-9 && (offset - 26.0).abs() < 1e-9);
    Ok(())
}

fn kit_number(text: &str) -> Fallible<f64> {
    crate::kit::parse_number(text, "mm").ok_or_else(|| format!("{text} is not a number").into())
}

#[test]
fn ear_clipping_fills_a_concave_board() {
    // An L: 6 corners, 4 triangles, area 3 (of unit squares).
    let ring = [
        Pos2::new(0.0, 0.0),
        Pos2::new(2.0, 0.0),
        Pos2::new(2.0, 1.0),
        Pos2::new(1.0, 1.0),
        Pos2::new(1.0, 2.0),
        Pos2::new(0.0, 2.0),
    ];
    let triangles = crate::pcb_editor::board_shape_ui::triangulate(&ring);
    assert_eq!(triangles.len(), 4);
    let area: f32 = triangles
        .iter()
        .map(|[a, b, c]| {
            let (a, b, c) = (ring[*a as usize], ring[*b as usize], ring[*c as usize]);
            ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() / 2.0
        })
        .sum();
    assert!((area - 3.0).abs() < 1e-5, "{area}");
}

// --- Headless frames ------------------------------------------------------

const SCREEN: egui::Vec2 = egui::vec2(900.0, 700.0);

fn canvas(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<Operation>, egui::Rect) {
    let mut out = None;
    let mut rect = egui::Rect::NOTHING;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, SCREEN)),
            events,
            ..Default::default()
        },
        |ui| {
            rect = ui.available_rect_before_wrap();
            out = editor.canvas(ui, engine);
        },
    );
    output.textures_delta.clear();
    (output, out, rect)
}

fn slot(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<Operation>) {
    let mut out = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(700.0, 900.0),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            out = editor.subject_slot(ui, engine).output;
            let _ = refusal::take_mark(ui.ctx());
        },
    );
    output.textures_delta.clear();
    (output, out)
}

fn texts(shape: &Shape, out: &mut Vec<(String, Pos2)>) {
    match shape {
        Shape::Text(text) => out.push((
            text.galley.job.text.clone(),
            text.pos + text.galley.size() * 0.5,
        )),
        Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
        _ => {}
    }
}

fn painted(output: &egui::FullOutput) -> Vec<(String, Pos2)> {
    let mut out = Vec::new();
    for shape in &output.shapes {
        texts(&shape.shape, &mut out);
    }
    out
}

fn press_at(pos: Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

fn release_at(pos: Pos2) -> Vec<egui::Event> {
    vec![egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]
}

fn key(key: egui::Key) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]
}

/// Click a card button by its text.
fn click_text(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    label: &str,
) -> Fallible<Option<Operation>> {
    let (output, _) = slot(editor, engine, ctx, vec![]);
    let at = painted(&output)
        .into_iter()
        .find(|(t, _)| t == label)
        .map(|(_, p)| p)
        .ok_or_else(|| format!("no {label} on the card"))?;
    slot(editor, engine, ctx, press_at(at));
    Ok(slot(editor, engine, ctx, release_at(at)).1)
}

#[test]
fn the_shape_mode_draws_the_sketch_and_a_rectangle_commits_one_board_edit() -> Fallible {
    let engine = sketch_engine(Vec::new())?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    editor.shape.evaluator = Some(&HOST);
    assert!(
        editor.on_tool("board-shape", &engine).is_none(),
        "already a sketch"
    );
    assert!(editor.shape.active);
    let ribbon = editor.ribbon(&engine);
    let ids: Vec<&str> = ribbon
        .groups
        .iter()
        .flat_map(|g| g.tools.iter().map(|t| t.id))
        .collect();
    for tool in ShapeTool::ALL {
        assert!(ids.contains(&tool.id()), "{}", tool.id());
    }
    assert_eq!(editor.subject(), crate::kit::card::Subject::Tool);
    // The sketch is drawn with its named edges.
    let (output, _, rect) = canvas(&mut editor, &engine, &ctx, vec![]);
    let shown = painted(&output);
    for name in ["bottom", "right", "top", "left"] {
        assert!(shown.iter().any(|(t, _)| t == name), "{name} is not drawn");
    }
    // R arms Rectangle; two clicks draw a cutout-free rectangle.
    let view = board_view(
        rect,
        engine.project().board_size,
        1.0,
        egui::Vec2::ZERO,
        false,
    );
    let hover = view.screen([5.0, 5.0]);
    canvas(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(hover)],
    );
    canvas(&mut editor, &engine, &ctx, key(egui::Key::R));
    assert_eq!(editor.shape.tool, ShapeTool::Rect);
    let mut operation = None;
    for at in [[5.0, 5.0], [12.0, 9.0]] {
        let pos = view.screen(at);
        canvas(&mut editor, &engine, &ctx, press_at(pos));
        operation = canvas(&mut editor, &engine, &ctx, release_at(pos))
            .1
            .or(operation);
    }
    let Some(Operation::Board(BoardCommand::Edit { edits })) = operation else {
        return Err(format!("no board.edit: {operation:?}").into());
    };
    let lines = edits
        .iter()
        .filter(|e| {
            matches!(
                e,
                BoardEdit::Sketch {
                    edit: SketchEdit::AddLine { .. }
                }
            )
        })
        .count();
    assert_eq!(lines, 4);
    // The card names the tool and shows the board parameters.
    let (output, _) = slot(&mut editor, &engine, &ctx, vec![]);
    let card = painted(&output);
    assert!(card.iter().any(|(t, _)| t == "Rectangle"));
    assert!(card.iter().any(|(t, _)| t.starts_with("BOARD PARAMETERS")));
    // Esc on the canvas steps back: the tool, then the mode.
    canvas(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(hover)],
    );
    canvas(&mut editor, &engine, &ctx, key(egui::Key::Escape));
    assert_eq!(editor.shape.tool, ShapeTool::Select);
    canvas(&mut editor, &engine, &ctx, key(egui::Key::Escape));
    assert!(!editor.shape.active);
    Ok(())
}

#[test]
fn the_board_card_offers_edit_shape_and_start_sketch_changes_nothing_when_refused() -> Fallible {
    let mut engine = Engine::demo()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    let (output, _) = slot(&mut editor, &engine, &ctx, vec![]);
    assert!(painted(&output).iter().any(|(t, _)| t == "Rectangle"));
    let operation = click_text(&mut editor, &engine, &ctx, "Edit shape…")?;
    let Some(operation @ Operation::Board(BoardCommand::StartSketch)) = operation else {
        return Err(format!("no board.start_sketch: {operation:?}").into());
    };
    let before = engine.project().clone();
    let revision = engine.revision();
    // Without a board evaluator the engine refuses, and nothing changes.
    assert!(engine.apply(revision, operation).is_err());
    assert_eq!(engine.project(), &before);
    assert_eq!(engine.revision(), revision);
    editor.sync(&engine);
    assert!(!editor.shape.active);
    Ok(())
}

#[test]
fn the_board_card_edits_a_board_parameter() -> Fallible {
    let width = FootprintParam {
        name: ParamName("width".into()),
        expr: Expr("80 mm".into()),
        unit: ParamUnit::Mm,
        comment: String::new(),
    };
    let engine = sketch_engine(vec![width])?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    let (output, _) = slot(&mut editor, &engine, &ctx, vec![]);
    let shown = painted(&output);
    assert!(shown.iter().any(|(t, _)| t == "width"));
    assert!(shown.iter().any(|(t, _)| t == "= 80 mm"));
    let board =
        crate::pcb_editor::board_shape_ui::sketch_of(engine.project()).ok_or("no sketch")?;
    let edit = parameter_edit(board, &ParamName("width".into()), "90 mm", ParamUnit::Mm)?;
    assert!(matches!(edit, BoardEdit::SetParameter { param } if param.expr.0 == "90 mm"));
    assert!(
        parameter_edit(
            board,
            &ParamName("width".into()),
            "depth * 2",
            ParamUnit::Mm
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn the_placement_card_constrains_a_part_to_its_nearest_edge_in_place() -> Fallible {
    let mut engine = Engine::demo()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    let placement = engine
        .project()
        .placements
        .first()
        .cloned()
        .ok_or("the demo has no placement")?;
    editor.set_selection(Some(placement.instance), None, None);
    let operation = click_text(&mut editor, &engine, &ctx, "Constrain to edge")?;
    let Some(operation @ Operation::Board(BoardCommand::Constrain { .. })) = operation else {
        return Err(format!("no board.constrain: {operation:?}").into());
    };
    let revision = engine.revision();
    engine.apply(revision, operation)?;
    let project: &Project = engine.project();
    let c = edge_card::constraint_of(project, placement.instance).ok_or("no constraint")?;
    let board = project.board();
    let edge = board.edge(&c.edge).ok_or("no such edge")?;
    let nearest = edge_card::nearest_edge(&board, placement.position).ok_or("no edge")?;
    assert_eq!(edge.name, nearest.name);
    let pose = edge_pose(
        edge,
        c.anchor,
        kit_number(&c.along.0)?,
        kit_number(&c.offset.0)?,
    )?;
    assert!(dist(pose.position, placement.position) < 1e-6);
    // The canvas shows the two dimensions of the selected part.
    let (output, _, _) = canvas(&mut editor, &engine, &ctx, vec![]);
    let shown = painted(&output);
    assert!(shown.iter().any(|(t, _)| *t == c.along.0));
    assert!(shown.iter().any(|(t, _)| *t == c.offset.0));
    // "Free part" removes the constraint as one step; undo restores it.
    let operation = click_text(&mut editor, &engine, &ctx, "Free part")?;
    let Some(operation @ Operation::Board(BoardCommand::DeleteConstraint { .. })) = operation
    else {
        return Err(format!("no board.constraint_delete: {operation:?}").into());
    };
    engine.apply(engine.revision(), operation)?;
    assert!(edge_card::constraint_of(engine.project(), placement.instance).is_none());
    engine.undo()?;
    assert!(edge_card::constraint_of(engine.project(), placement.instance).is_some());
    engine.undo()?;
    assert!(engine.project().edge_constraints.is_empty());
    Ok(())
}

/// The app's host evaluator is SolidCraft's board sketch engine: on a real
/// engine, Shape's start and a Cutout gesture each solve as one board step,
/// the cutout lands in the solved board and one undo removes it.
#[test]
fn the_host_evaluator_solves_a_cutout_gesture_on_the_engine() -> Fallible {
    let evaluator = host_evaluator().ok_or("no host board evaluator")?;
    let hosted = || pcbcraft_core::Evaluators {
        footprint: None,
        board: Some(evaluator),
    };
    let mut engine = Engine::showcase()?;
    engine.apply_hosted(
        engine.revision(),
        Operation::Board(BoardCommand::StartSketch),
        hosted(),
    )?;
    let BoardShape::Sketch(board) = engine.project().board_shape.clone() else {
        return Err("the board did not become a sketch".into());
    };
    let view = scene_view(&board)?;
    let scene = Scene {
        board: &board,
        view: &view,
        evaluator,
        tolerance: 0.3,
    };
    let options = ShapeOptions::default();
    let mut gesture = Gesture::default();
    press(
        ShapeTool::Cutout,
        &options,
        &mut gesture,
        [10.0, 10.0],
        &scene,
    )?;
    let Step::Commit(commit) = press(
        ShapeTool::Cutout,
        &options,
        &mut gesture,
        [14.0, 13.0],
        &scene,
    )?
    else {
        return Err("the cutout did not commit".into());
    };
    let before = engine.revision();
    engine.apply_hosted(before, operation(commit.edits), hosted())?;
    assert_eq!(engine.revision(), before + 1, "one step");
    assert_eq!(engine.project().board().cutouts.len(), 1);
    engine.undo()?;
    assert!(engine.project().board().cutouts.is_empty());
    Ok(())
}
