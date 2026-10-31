//! Footprint dimensions, constraints and parameters: what pick sequences
//! make, measured values and labels (pure), and the canvas and panel
//! gestures driven frame by frame, each committed once as one
//! `library.edit_footprint` through the engine with the real evaluator.
use super::*;
use crate::part_editor::{PartEditor, footprint_canvas};
use egui::{Pos2, Rect};
use pcbcraft_core::library::Definition;
use pcbcraft_core::library::footprint_source::FootprintSource;
use pcbcraft_core::{Engine, Operation};
use pcbcraft_solidcraft::footprint::SketchEvaluator;
use pcbcraft_solidcraft::footprint::view::{ViewCurve, ViewPoint};
use solidcraft_ui_egui::fonts;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn p(id: &str) -> SketchRef {
    SketchRef::Point(SketchPointId(id.into()))
}

fn c(id: &str) -> SketchRef {
    SketchRef::Curve(SketchCurveId(id.into()))
}

/// A hand-made view: points a(0,0) b(2,0) d(2,1), a horizontal line ab, a
/// line ad, a circle of radius 0.5 about (5,0) and an arc of radius 1.
fn fixture() -> SketchView {
    let point = |id: &str, at: Point| ViewPoint {
        id: SketchPointId(id.into()),
        at,
        fixed: false,
    };
    let curve = |id: &str, geometry: ViewGeometry| ViewCurve {
        id: SketchCurveId(id.into()),
        geometry,
        construction: false,
        points: Vec::new(),
    };
    SketchView {
        points: vec![
            point("a", [0.0, 0.0]),
            point("b", [2.0, 0.0]),
            point("d", [2.0, 1.0]),
        ],
        curves: vec![
            curve(
                "ab",
                ViewGeometry::Line {
                    a: [0.0, 0.0],
                    b: [2.0, 0.0],
                },
            ),
            curve(
                "ad",
                ViewGeometry::Line {
                    a: [0.0, 0.0],
                    b: [2.0, 1.0],
                },
            ),
            curve(
                "circle",
                ViewGeometry::Circle {
                    center: [5.0, 0.0],
                    radius: 0.5,
                },
            ),
            curve(
                "arc",
                ViewGeometry::Arc {
                    center: [0.0, 5.0],
                    start: [1.0, 5.0],
                    end: [0.0, 6.0],
                },
            ),
        ],
        constraints: Vec::new(),
    }
}

#[test]
fn picks_prefer_points_and_find_curves_by_their_drawn_geometry() {
    let v = fixture();
    assert_eq!(pick(&v, [0.05, 0.02], 0.1), Some(p("a")));
    assert_eq!(pick(&v, [1.0, 0.05], 0.1), Some(c("ab")));
    assert_eq!(pick(&v, [5.0, 0.55], 0.1), Some(c("circle")));
    // On the arc's circle but outside its sweep: nothing.
    assert_eq!(pick(&v, [0.0, 4.0], 0.1), None);
    assert_eq!(
        pick(&v, [0.5f64.sqrt(), 5.0 + 0.5f64.sqrt()], 0.1),
        Some(c("arc"))
    );
}

#[test]
fn the_dimension_tool_waits_for_a_complete_pick_set() {
    let v = fixture();
    assert_eq!(dimension_picks(&v, &[p("a")]), DimPicks::More);
    assert_eq!(dimension_picks(&v, &[c("ab")]), DimPicks::More);
    assert_eq!(
        dimension_picks(&v, &[p("a"), p("b")]),
        DimPicks::Ready {
            refs: vec![p("a"), p("b")],
            linear: true
        }
    );
    assert_eq!(
        dimension_picks(&v, &[c("ab"), p("d")]),
        DimPicks::Ready {
            refs: vec![p("d"), c("ab")],
            linear: false
        }
    );
    assert!(matches!(
        dimension_picks(&v, &[c("ab"), c("ad")]),
        DimPicks::Ready { linear: false, .. }
    ));
    assert!(matches!(
        dimension_picks(&v, &[c("circle")]),
        DimPicks::Ready { .. }
    ));
    assert!(matches!(
        dimension_picks(&v, &[c("circle"), p("a")]),
        DimPicks::Wrong(_)
    ));
}

/// Placed above or below the span a dimension measures along x, beside it
/// along y, and diagonally off both it is aligned (Solidcraft's rule).
#[test]
fn a_linear_dimension_takes_its_direction_from_where_it_is_placed() {
    let v = fixture();
    let ad = [p("a"), p("d")];
    assert_eq!(
        linear_kind(&v, &ad, [1.0, 3.0]),
        DimKind::HorizontalDistance
    );
    assert_eq!(linear_kind(&v, &ad, [-1.0, 0.5]), DimKind::VerticalDistance);
    assert_eq!(linear_kind(&v, &ad, [3.0, 2.0]), DimKind::Distance);
    // A flat span is always its own length.
    assert_eq!(linear_kind(&v, &[c("ab")], [-1.0, 0.0]), DimKind::Distance);
}

#[test]
fn new_dimensions_start_from_the_measured_geometry() {
    let v = fixture();
    let near = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 1e-12);
    assert!(near(
        measure(&v, DimKind::Distance, &[p("a"), p("d")]),
        5f64.sqrt()
    ));
    assert!(near(
        measure(&v, DimKind::HorizontalDistance, &[p("a"), p("d")]),
        2.0
    ));
    assert!(near(
        measure(&v, DimKind::VerticalDistance, &[p("a"), p("d")]),
        1.0
    ));
    assert!(near(measure(&v, DimKind::Distance, &[c("ab")]), 2.0));
    // d is 1 above the line ab.
    assert!(near(
        measure(&v, DimKind::Distance, &[p("d"), c("ab")]),
        1.0
    ));
    assert!(near(measure(&v, DimKind::Diameter, &[c("circle")]), 1.0));
    assert!(near(measure(&v, DimKind::Radius, &[c("arc")]), 1.0));
    assert!(near(
        measure(&v, DimKind::Angle, &[c("ab"), c("ad")]),
        0.5f64.atan().to_degrees()
    ));
}

#[test]
fn constrain_tools_take_the_entities_the_evaluator_accepts() {
    use GeomConstraint as G;
    let v = fixture();
    assert_eq!(
        constrain_picks(&v, G::Horizontal, &[c("ad")]),
        ConstrainPicks::Ready
    );
    assert_eq!(
        constrain_picks(&v, G::Horizontal, &[p("a")]),
        ConstrainPicks::More
    );
    assert_eq!(
        constrain_picks(&v, G::Vertical, &[p("a"), p("d")]),
        ConstrainPicks::Ready
    );
    assert_eq!(
        constrain_picks(&v, G::Parallel, &[c("ab"), c("ad")]),
        ConstrainPicks::Ready
    );
    assert!(matches!(
        constrain_picks(&v, G::Parallel, &[c("circle")]),
        ConstrainPicks::Wrong(_)
    ));
    assert_eq!(
        constrain_picks(&v, G::Symmetric, &[p("a"), p("b")]),
        ConstrainPicks::More
    );
    assert_eq!(
        constrain_picks(&v, G::Symmetric, &[p("a"), p("b"), c("ad")]),
        ConstrainPicks::Ready
    );
    assert_eq!(
        constrain_picks(&v, G::Fixed, &[p("d")]),
        ConstrainPicks::Ready
    );
    assert_eq!(
        constrain_picks(&v, G::Concentric, &[c("circle"), c("arc")]),
        ConstrainPicks::Ready
    );
}

/// The default detail labels each independent parameter once and the
/// unbound driving dimensions; derived, expression-bound and reference
/// dimensions appear with All dims or when the selection touches them.
#[test]
fn key_detail_hides_derived_duplicate_and_expression_dimensions() {
    use pcbcraft_core::library::footprint_source::FootprintParam;
    let k = |id: &str, param: Option<&str>, driven: bool, points: &[&str]| ViewConstraint {
        id: SketchConstraintId(id.into()),
        kind: "LinearDimension",
        value: Some(1.0),
        param: param.map(str::to_string),
        driven,
        points: points.iter().map(|p| SketchPointId((*p).into())).collect(),
        curves: Vec::new(),
        text: None,
    };
    let view = SketchView {
        constraints: vec![
            k("pad", Some("pad_l"), false, &[]),
            k("pad-again", Some("pad_l"), false, &[]),
            k("seed", Some("seed_x"), false, &["anchor"]),
            k("corner", Some("(0.25) * min((pad_l), (pad_w))"), false, &[]),
            k("measured", None, true, &[]),
            k("typed", None, false, &[]),
        ],
        ..SketchView::default()
    };
    let param = |name: &str, expr: &str| FootprintParam {
        name: ParamName(name.into()),
        expr: Expr(expr.into()),
        unit: ParamUnit::Mm,
        comment: String::new(),
    };
    let source = FootprintSource {
        parameters: vec![
            param("pad_l", "0.6"),
            param("row_span", "5.4"),
            param("seed_x", "row_span / 2"),
        ],
        ..FootprintSource::default()
    };
    let ids = |v: Vec<SketchConstraintId>| v.into_iter().map(|i| i.0).collect::<Vec<_>>();
    let none = DimFocus::default();
    assert_eq!(
        ids(shown_dimensions(&view, &source, DimDetail::Key, &none)),
        ["pad", "typed"]
    );
    let focus = DimFocus {
        points: vec![SketchPointId("anchor".into())],
        ..DimFocus::default()
    };
    assert_eq!(
        ids(shown_dimensions(&view, &source, DimDetail::Key, &focus)),
        ["pad", "seed", "typed"]
    );
    assert_eq!(
        shown_dimensions(&view, &source, DimDetail::All, &none).len(),
        6
    );
    // An expression-bound dimension labels its value, not the text.
    assert_eq!(
        label(&k(
            "corner",
            Some("(0.25) * min((pad_l), (pad_w))"),
            false,
            &[]
        )),
        "1"
    );
}

#[test]
fn labels_show_the_parameter_the_value_and_its_kind() {
    let k = |kind: &'static str, value: f64, param: Option<&str>, driven: bool| ViewConstraint {
        id: SketchConstraintId("k".into()),
        kind,
        value: Some(value),
        param: param.map(str::to_string),
        driven,
        points: Vec::new(),
        curves: Vec::new(),
        text: None,
    };
    assert_eq!(
        label(&k("LinearDimension", 1.27, Some("pitch"), false)),
        "pitch = 1.27"
    );
    assert_eq!(label(&k("RadialDimension", 0.5, None, false)), "R0.5");
    assert_eq!(label(&k("DiameterDimension", 1.0, None, false)), "Ø1");
    assert_eq!(label(&k("AngularDimension", 30.0, None, false)), "30°");
    assert_eq!(label(&k("LinearDimension", 2.0, None, true)), "(2)");
    assert_eq!(
        field_text(&k("LinearDimension", 1.27, Some("pitch"), false)),
        "pitch"
    );
    assert_eq!(field_text(&k("LinearDimension", 2.5, None, false)), "2.5");
}

#[test]
fn an_expression_defines_the_bound_parameter_or_the_next_free_name() {
    let mut source = FootprintSource::default();
    for name in ["d1", "d3"] {
        source
            .parameters
            .push(pcbcraft_core::library::footprint_source::FootprintParam {
                name: ParamName(name.into()),
                expr: Expr("1 mm".into()),
                unit: ParamUnit::Mm,
                comment: String::new(),
            });
    }
    assert_eq!(fallback_name(&source, None).0, "d2");
    assert_eq!(fallback_name(&source, Some("pitch")).0, "pitch");
    assert_eq!(fallback_name(&source, Some("pkg_L")).0, "d2");
    // A new dimension's parameter is defined before the dimension binds it.
    let edits = new_dimension_edits(
        DimKind::Distance,
        vec![c("ab")],
        DimInput::Expr(Expr("2*pitch".into())),
        ParamName("d2".into()),
    );
    assert!(matches!(
        edits.as_slice(),
        [
            FootprintEdit::SetParameter { name, .. },
            FootprintEdit::Sketch(SketchEdit::AddDimension {
                value: DimValue::Param(bound),
                ..
            })
        ] if name.0 == "d2" && bound.0 == "d2"
    ));
}

// ---- Canvas and panel gestures ----

const CANVAS: Rect = Rect {
    min: Pos2::ZERO,
    max: Pos2::new(600.0, 400.0),
};

fn button(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn texts(output: &egui::FullOutput) -> Vec<String> {
    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.job.text.clone()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for shape in &output.shapes {
        walk(&shape.shape, &mut out);
    }
    out
}

fn canvas_frame(
    editor: &mut PartEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> Vec<String> {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(CANVAS),
            events,
            ..Default::default()
        },
        |ui| {
            editor.canvas(ui, engine);
        },
    );
    output.textures_delta.clear();
    texts(&output)
}

fn inspector_frame(
    editor: &mut PartEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> Vec<String> {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 4000.0))),
            events,
            ..Default::default()
        },
        |ui| {
            editor.inspector(ui, engine);
        },
    );
    output.textures_delta.clear();
    texts(&output)
}

fn click(editor: &mut PartEditor, engine: &Engine, ctx: &egui::Context, at: Pos2) {
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![button(at, true)],
        vec![button(at, false)],
    ] {
        canvas_frame(editor, engine, ctx, events);
    }
}

/// Send what the editor queued through the engine with the evaluator;
/// whether something was sent, and that it was sent once.
fn send(
    editor: &mut PartEditor,
    engine: &mut Engine,
    ctx: &egui::Context,
) -> pcbcraft_core::Result<bool> {
    let Some(operation) = editor.commit(ctx, engine) else {
        return Ok(false);
    };
    engine.apply_with(engine.revision(), operation, &SketchEvaluator)?;
    assert!(editor.commit(ctx, engine).is_none(), "sent once");
    Ok(true)
}

fn engine_source(engine: &Engine, editor: &PartEditor) -> Option<FootprintSource> {
    let id = editor.draft.as_ref()?.ids.footprint;
    match &engine.drafts().get(id)?.definition {
        Definition::Footprint(f) => f.source.as_deref().cloned(),
        _ => None,
    }
}

/// The engine, the editor, the sketched line and its two end points.
type Sketched = (Engine, PartEditor, SketchCurveId, [SketchPointId; 2]);

/// The demo component with its pads cleared and a sketch of one free line
/// from (-1, 0) to (1, 0.5), drawn through `library.edit_footprint`; the
/// editor on its Footprint tab, fitted to a ±3 mm view.
fn sketched(ctx: &egui::Context) -> Result<Sketched, Box<dyn std::error::Error>> {
    fonts::setup_bundled_fonts(ctx);
    let mut engine = Engine::demo()?;
    let mut editor = PartEditor::default();
    editor.commit(ctx, &engine);
    if let Some(set) = &mut editor.draft {
        set.footprint.pads.clear();
        set.binding.mappings.clear();
    }
    send(&mut editor, &mut engine, ctx)?;
    let draft = editor.draft.as_ref().ok_or("no draft")?.ids.footprint;
    engine.apply_with(
        engine.revision(),
        Operation::Library(pcbcraft_core::LibraryCommand::EditFootprint {
            draft,
            edits: vec![FootprintEdit::Sketch(SketchEdit::AddLine {
                a: [-1.0, 0.0],
                b: [1.0, 0.5],
                construction: false,
            })],
        }),
        &SketchEvaluator,
    )?;
    editor.commit(ctx, &engine);
    editor.on_tab("footprint");
    editor.footprint_view = Some(crate::part_editor::symbol_canvas::ViewState {
        pan: egui::Vec2::ZERO,
        scale: 60.0,
    });
    canvas_frame(&mut editor, &engine, ctx, vec![]);
    let source = engine_source(&engine, &editor).ok_or("no source")?;
    let view = pcbcraft_solidcraft::footprint::view::sketch_view(&source)?;
    let line = view.curves.first().ok_or("a line")?;
    let [a, b] = <[SketchPointId; 2]>::try_from(line.points.clone()).map_err(|_| "two ends")?;
    Ok((engine, editor, line.id.clone(), [a, b]))
}

fn screen(editor: &PartEditor, at: Point) -> Result<Pos2, &'static str> {
    footprint_canvas::screen(editor, CANVAS, at).ok_or("not fitted")
}

fn ends(
    engine: &Engine,
    editor: &PartEditor,
) -> Result<(Point, Point), Box<dyn std::error::Error>> {
    let source = engine_source(engine, editor).ok_or("no source")?;
    let view = pcbcraft_solidcraft::footprint::view::sketch_view(&source)?;
    match view.curves.first().map(|c| &c.geometry) {
        Some(ViewGeometry::Line { a, b }) => Ok((*a, *b)),
        other => Err(format!("{other:?}").into()),
    }
}

/// Dimension tool: pick the line, place the dimension, type
/// `len = 2.5 mm`, press Enter. One commit defines `len` and binds the new
/// dimension to it, and the line solves to 2.5 mm.
#[test]
fn a_dimension_typed_as_a_definition_binds_a_new_parameter() -> TestResult {
    let ctx = egui::Context::default();
    let (mut engine, mut editor, _, _) = sketched(&ctx)?;
    editor.on_tool("fp-dimension", &engine);
    let at = screen(&editor, [0.0, 0.25])?;
    click(&mut editor, &engine, &ctx, at);
    assert_eq!(editor.footprint_dims.picks.len(), 1, "the line is picked");
    // Placing it on empty canvas beyond both of the line's extents (an
    // aligned dimension, as in Solidcraft) opens its field with the
    // measured length.
    let at = screen(&editor, [-1.5, 1.0])?;
    click(&mut editor, &engine, &ctx, at);
    let Some(ValueField::New(field)) = &editor.footprint_dims.field else {
        return Err("a new dimension's field".into());
    };
    assert_eq!(field.kind, DimKind::Distance);
    assert_eq!(field.text, crate::kit::number_text(4.25f64.sqrt()));
    if let Some(field) = &mut editor.footprint_dims.field {
        *field.text_mut() = "len = 2.5 mm".into();
    }
    // The field takes the keyboard, then Enter commits it.
    canvas_frame(&mut editor, &engine, &ctx, vec![]);
    assert!(
        !send(&mut editor, &mut engine, &ctx)?,
        "nothing before Enter"
    );
    canvas_frame(&mut editor, &engine, &ctx, vec![key(egui::Key::Enter)]);
    assert!(editor.footprint_dims.field.is_none(), "the field closed");
    assert!(send(&mut editor, &mut engine, &ctx)?);
    let source = engine_source(&engine, &editor).ok_or("no source")?;
    let len = source
        .parameters
        .iter()
        .find(|p| p.name.0 == "len")
        .ok_or("len defined")?;
    assert_eq!(len.expr.0, "2.5 mm");
    let view = pcbcraft_solidcraft::footprint::view::sketch_view(&source)?;
    let bound = view
        .constraints
        .iter()
        .find(|k| k.param.as_deref() == Some("len"))
        .ok_or("a dimension bound to len")?;
    assert!(bound.value.is_some_and(|v| (v - 2.5).abs() < 1e-9));
    let (a, b) = ends(&engine, &editor)?;
    assert!(((b[0] - a[0]).hypot(b[1] - a[1]) - 2.5).abs() < 1e-6);
    // The canvas shows the bound value.
    let shown = canvas_frame(&mut editor, &engine, &ctx, vec![]);
    assert!(shown.iter().any(|t| t == "len = 2.5"), "{shown:?}");
    Ok(())
}

/// Editing a bound dimension's label to a plain number unbinds it; typing
/// a value that conflicts with the sketch keeps the field open with the
/// solver's refusal and sends nothing.
#[test]
fn a_refused_value_keeps_the_field_and_the_document_unchanged() -> TestResult {
    let ctx = egui::Context::default();
    let (mut engine, mut editor, line, [a, b]) = sketched(&ctx)?;
    let draft = editor.draft.as_ref().ok_or("no draft")?.ids.footprint;
    // The line gets a length of 2 and its x extent a dimension of 1.5.
    engine.apply_with(
        engine.revision(),
        Operation::Library(pcbcraft_core::LibraryCommand::EditFootprint {
            draft,
            edits: vec![
                FootprintEdit::Sketch(SketchEdit::AddDimension {
                    kind: DimKind::Distance,
                    refs: vec![SketchRef::Curve(line)],
                    value: DimValue::Fixed(2.0),
                }),
                FootprintEdit::Sketch(SketchEdit::AddDimension {
                    kind: DimKind::HorizontalDistance,
                    refs: vec![SketchRef::Point(a), SketchRef::Point(b)],
                    value: DimValue::Fixed(1.5),
                }),
            ],
        }),
        &SketchEvaluator,
    )?;
    editor.commit(&ctx, &engine);
    canvas_frame(&mut editor, &engine, &ctx, vec![]);
    canvas_frame(&mut editor, &engine, &ctx, vec![]);
    let before = engine_source(&engine, &editor);
    let horizontal = editor
        .footprint_dims
        .view()
        .and_then(|v| {
            v.constraints
                .iter()
                .find(|k| k.kind == "LinearDimension" && k.points.len() == 2)
        })
        .map(|k| k.id.clone())
        .ok_or("the horizontal dimension")?;
    let label = editor
        .footprint_dims
        .labels
        .iter()
        .find(|(id, _)| *id == horizontal)
        .map(|(_, r)| r.center())
        .ok_or("its label is drawn")?;
    // Double-click the label: its field opens with the value.
    for events in [
        vec![egui::Event::PointerMoved(label)],
        vec![button(label, true)],
        vec![button(label, false)],
        vec![button(label, true)],
        vec![button(label, false)],
    ] {
        canvas_frame(&mut editor, &engine, &ctx, events);
    }
    let Some(ValueField::Existing(field)) = &editor.footprint_dims.field else {
        return Err("the label's field".into());
    };
    assert_eq!(field.text, "1.5");
    // Wider than the line is long: the solver refuses.
    if let Some(field) = &mut editor.footprint_dims.field {
        *field.text_mut() = "3".into();
    }
    canvas_frame(&mut editor, &engine, &ctx, vec![]);
    canvas_frame(&mut editor, &engine, &ctx, vec![key(egui::Key::Enter)]);
    let error = editor
        .footprint_dims
        .field
        .as_ref()
        .and_then(|f| f.error().map(str::to_owned));
    assert!(
        error.is_some_and(|e| e.contains("footprint.")),
        "the field shows why"
    );
    assert!(!send(&mut editor, &mut engine, &ctx)?, "nothing sent");
    assert_eq!(engine_source(&engine, &editor), before);
    // An unknown name keeps the field open and says how to define it.
    if let Some(field) = &mut editor.footprint_dims.field {
        *field.text_mut() = "pad_q".into();
    }
    canvas_frame(&mut editor, &engine, &ctx, vec![]);
    assert!(
        editor.footprint_dims.field.is_some(),
        "a refused value keeps the field open on the next frames"
    );
    canvas_frame(&mut editor, &engine, &ctx, vec![]);
    canvas_frame(&mut editor, &engine, &ctx, vec![key(egui::Key::Enter)]);
    let error = editor
        .footprint_dims
        .field
        .as_ref()
        .and_then(|f| f.error().map(str::to_owned));
    assert!(
        error.is_some_and(|e| e.contains("Unknown parameter pad_q") && e.contains("pad_q = ")),
        "the field names the unknown parameter"
    );
    assert!(!send(&mut editor, &mut engine, &ctx)?, "nothing sent");
    // Esc closes the field; nothing changes.
    canvas_frame(&mut editor, &engine, &ctx, vec![key(egui::Key::Escape)]);
    assert!(editor.footprint_dims.field.is_none());
    assert_eq!(engine_source(&engine, &editor), before);
    Ok(())
}

/// A drag previews the solved sketch every frame without touching the
/// document, and the release commits one `SketchEdit::Drag`.
#[test]
fn dragging_a_free_point_previews_and_commits_once() -> TestResult {
    let ctx = egui::Context::default();
    let (mut engine, mut editor, _, [_, b]) = sketched(&ctx)?;
    let before = engine_source(&engine, &editor);
    let from = screen(&editor, [1.0, 0.5])?;
    let to = screen(&editor, [1.5, -0.5])?;
    for events in [
        vec![egui::Event::PointerMoved(from)],
        vec![button(from, true)],
        vec![egui::Event::PointerMoved(from + (to - from) * 0.5)],
        vec![egui::Event::PointerMoved(to)],
    ] {
        canvas_frame(&mut editor, &engine, &ctx, events);
    }
    let drag = editor.footprint_dims.drag.as_ref().ok_or("dragging")?;
    assert_eq!(drag.point, b);
    let target = editor
        .footprint_view
        .map(|v| crate::symbol_view::View {
            origin: CANVAS.center() + v.pan,
            scale: v.scale,
        })
        .map(|v| v.world(to))
        .ok_or("fitted")?;
    let previewed = drag.view.point(&b).map(|p| p.at).ok_or("b drawn")?;
    assert!((previewed[0] - target[0]).hypot(previewed[1] - target[1]) < 1e-9);
    assert!(editor.footprint_dims.times.frames >= 2, "timed frames");
    assert!(editor.commit(&ctx, &engine).is_none(), "nothing mid-drag");
    assert_eq!(engine_source(&engine, &editor), before);
    canvas_frame(&mut editor, &engine, &ctx, vec![button(to, false)]);
    assert!(send(&mut editor, &mut engine, &ctx)?);
    let (_, end) = ends(&engine, &editor)?;
    assert!(
        (end[0] - target[0]).hypot(end[1] - target[1]) < 1e-6,
        "{end:?}"
    );
    engine.undo()?;
    assert_eq!(engine_source(&engine, &editor), before, "one undo step");
    Ok(())
}

/// Horizontal on the tilted line: one commit, and the line solves level.
/// Esc during a pick set drops it and leaves the document alone.
#[test]
fn a_constrain_tool_commits_once_when_its_picks_are_complete() -> TestResult {
    let ctx = egui::Context::default();
    let (mut engine, mut editor, _, _) = sketched(&ctx)?;
    let before = engine_source(&engine, &editor);
    editor.on_tool("fp-vertical", &engine);
    let at = screen(&editor, [-1.0, 0.0])?;
    click(&mut editor, &engine, &ctx, at);
    assert_eq!(editor.footprint_dims.picks.len(), 1);
    canvas_frame(&mut editor, &engine, &ctx, vec![key(egui::Key::Escape)]);
    assert!(editor.footprint_dims.picks.is_empty());
    assert!(!send(&mut editor, &mut engine, &ctx)?);
    assert_eq!(engine_source(&engine, &editor), before);
    editor.on_tool("fp-horizontal", &engine);
    let at = screen(&editor, [0.0, 0.25])?;
    click(&mut editor, &engine, &ctx, at);
    assert!(send(&mut editor, &mut engine, &ctx)?);
    let (a, b) = ends(&engine, &editor)?;
    assert!((a[1] - b[1]).abs() < 1e-9, "{a:?} {b:?}");
    // The canvas shows the glyph and the remaining freedom.
    let shown = canvas_frame(&mut editor, &engine, &ctx, vec![]);
    assert!(shown.iter().any(|t| t == "-"), "{shown:?}");
    assert!(
        shown.iter().any(|t| t == "3 degrees of freedom"),
        "{shown:?}"
    );
    Ok(())
}

/// The panel lists the package values read-only, then each parameter with
/// its value and readers; a refused expression keeps its text with the
/// reason, an accepted one commits once.
#[test]
fn the_parameters_panel_shows_values_and_readers_and_edits_once() -> TestResult {
    let ctx = egui::Context::default();
    let (mut engine, mut editor, line, _) = sketched(&ctx)?;
    let draft = editor.draft.as_ref().ok_or("no draft")?.ids.footprint;
    engine.apply_with(
        engine.revision(),
        Operation::Library(pcbcraft_core::LibraryCommand::EditFootprint {
            draft,
            edits: vec![
                FootprintEdit::SetParameter {
                    name: ParamName("pitch".into()),
                    expr: Expr("1.25 mm".into()),
                    unit: ParamUnit::Mm,
                },
                FootprintEdit::SetParameter {
                    name: ParamName("span".into()),
                    expr: Expr("2 * pitch".into()),
                    unit: ParamUnit::Mm,
                },
                FootprintEdit::Sketch(SketchEdit::AddDimension {
                    kind: DimKind::Distance,
                    refs: vec![SketchRef::Curve(line)],
                    value: DimValue::Param(ParamName("span".into())),
                }),
            ],
        }),
        &SketchEvaluator,
    )?;
    editor.commit(&ctx, &engine);
    let shown = inspector_frame(&mut editor, &engine, &ctx, vec![]);
    assert!(shown.iter().any(|t| t == "pkg_L"), "{shown:?}");
    assert!(
        shown
            .iter()
            .any(|t| t == "= 1.25 mm · used by 1 dimension, span"),
        "{shown:?}"
    );
    assert!(
        shown.iter().any(|t| t == "= 2.5 mm · used by 1 dimension"),
        "{shown:?}"
    );
    let (a, b) = ends(&engine, &editor)?;
    assert!(((b[0] - a[0]).hypot(b[1] - a[1]) - 2.5).abs() < 1e-6);
    // A self-reference is refused; nothing is sent.
    editor.apply_parameter_change_for_test("pitch", "pitch + 1 mm");
    assert!(editor.footprint_dims.panel_error().is_some());
    assert!(!send(&mut editor, &mut engine, &ctx)?);
    // A new pitch re-solves the bound line: 2 × 1.5 mm.
    editor.apply_parameter_change_for_test("pitch", "1.5 mm");
    assert!(editor.footprint_dims.panel_error().is_none());
    assert!(send(&mut editor, &mut engine, &ctx)?);
    let (a, b) = ends(&engine, &editor)?;
    assert!(((b[0] - a[0]).hypot(b[1] - a[1]) - 3.0).abs() < 1e-6);
    // Removing a parameter a dimension reads is refused.
    editor.remove_parameter_for_test("span");
    let why = editor.footprint_dims.panel_error().ok_or("refused")?;
    assert!(
        why.contains("`span` cannot be removed while it is used by 1 dimension"),
        "{why}"
    );
    assert!(!send(&mut editor, &mut engine, &ctx)?);
    Ok(())
}
