//! Copper fills in Layout: drawing (solid, hatched, outlines), picking,
//! the Fill card, the Fill tool and the outline handles, each through the
//! editor's own canvas and card as a person drives them.
use super::fill_paint::{FillDisplay, FillMesh};
use super::*;
use pcbcraft_core::layout::fill::{FillCommand, RefillTarget};
use pcbcraft_core::library::Segment;
use pcbcraft_core::model::fill::{FilledCopper, FilledRegion};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SCREEN: [f32; 2] = [900.0, 700.0];

/// A counter-clockwise rectangle.
fn rect(lo: Point, hi: Point) -> Vec<Point> {
    vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
}

fn copper(regions: Vec<FilledRegion>) -> FilledCopper {
    FilledCopper {
        key: "test".into(),
        regions,
        removed_islands: 0,
    }
}

/// The showcase with copper on its bottom pour: a rectangle at the
/// bottom-right corner with a square hole. The inner plane stays unfilled.
fn filled_showcase() -> Result<(Engine, Id), Box<dyn std::error::Error>> {
    let engine = Engine::showcase()?;
    let mut project = engine.project().clone();
    // The showcase refills its pours; the inner plane is left unfilled
    // here so its dashed outline shows.
    for fill in &mut project.fills {
        if fill.layer != CopperLayer::Bottom {
            fill.computed = None;
        }
    }
    let pour = project
        .fills
        .iter_mut()
        .find(|f| f.layer == CopperLayer::Bottom)
        .ok_or("bottom pour")?;
    let id = pour.id;
    pour.computed = Some(copper(vec![FilledRegion {
        outer: rect([30.0, 1.0], [40.0, 4.0]),
        holes: vec![rect([31.5, 2.0], [32.5, 3.0]).into_iter().rev().collect()],
    }]));
    let mut file = engine.to_file();
    file.project = project;
    Ok((Engine::from_file(file)?, id))
}

fn canvas(
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
        |ui| {
            command = editor.canvas(ui, engine);
        },
    );
    output.textures_delta.clear();
    (output, command)
}

fn card(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<Operation>) {
    let mut command = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700.0, 900.0))),
            events,
            ..Default::default()
        },
        |ui| {
            command = editor.subject_slot(ui, engine).output;
            let _ = refusal::take_mark(ui.ctx());
        },
    );
    output.textures_delta.clear();
    (output, command)
}

fn view_of(editor: &PcbEditor, engine: &Engine) -> BoardView {
    board_view(
        Rect::from_min_size(Pos2::ZERO, egui::vec2(SCREEN[0], SCREEN[1])),
        engine.project().board_size,
        editor.zoom,
        editor.pan,
        editor.from_bottom,
    )
}

fn button(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
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

/// A click at screen point `at` on the canvas: the operation it made.
fn click(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    at: Pos2,
) -> Option<Operation> {
    canvas(editor, engine, ctx, vec![egui::Event::PointerMoved(at)]);
    let pressed = canvas(
        editor,
        engine,
        ctx,
        vec![egui::Event::PointerMoved(at), button(at, true)],
    )
    .1;
    let released = canvas(editor, engine, ctx, vec![button(at, false)]).1;
    pressed.or(released)
}

/// Every shape of a frame, flattened, in paint order.
fn shapes(output: &egui::FullOutput) -> Vec<Shape> {
    fn walk(shape: &Shape, out: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            other => out.push(other.clone()),
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

fn mesh_colored(shape: &Shape, color: Color32) -> bool {
    matches!(shape, Shape::Mesh(m) if m.vertices.iter().any(|v| v.color == color))
}

fn segment_colored(shape: &Shape, color: Color32) -> bool {
    matches!(shape, Shape::LineSegment { stroke, .. } if stroke.color == color)
}

fn path_filled(shape: &Shape, color: Color32) -> bool {
    matches!(shape, Shape::Path(p) if p.fill == color)
}

/// A region's triangles cover exactly its area less its holes, and the
/// mesh picks copper, not holes.
#[test]
fn fill_copper_triangulates_with_its_holes() {
    let hole = |lo: Point, hi: Point| -> Vec<Point> { rect(lo, hi).into_iter().rev().collect() };
    let regions = vec![
        FilledRegion {
            outer: rect([0.0, 0.0], [10.0, 6.0]),
            holes: vec![hole([1.0, 1.0], [3.0, 3.0]), hole([5.0, 2.0], [9.0, 5.0])],
        },
        // An island inside the second hole.
        FilledRegion {
            outer: rect([6.0, 3.0], [7.0, 4.0]),
            holes: vec![],
        },
    ];
    let mesh = FillMesh::of(0, &regions);
    assert!(
        (mesh.area() - (60.0 - 4.0 - 12.0 + 1.0)).abs() < 1e-9,
        "{}",
        mesh.area()
    );
    assert!(mesh.triangle_count() > 0);
    assert!(mesh.contains([0.5, 0.5]));
    assert!(!mesh.contains([2.0, 2.0]), "in a hole");
    assert!(mesh.contains([6.5, 3.5]), "on the island");
    assert!(!mesh.contains([5.5, 4.5]), "in the hole round the island");
    assert!(!mesh.contains([11.0, 1.0]));
}

/// Fill copper draws under its layer's tracks in the layer's one opaque
/// colour; a fill without copper draws its dashed outline. Outlines-only
/// and hatched views keep the copper out of solid fill.
#[test]
fn layout_draws_fill_copper_under_tracks_and_outlines_unfilled_fills() -> TestResult {
    let (engine, _) = filled_showcase()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    // Seen from the bottom with the bottom layer active.
    let mut editor = PcbEditor {
        from_bottom: true,
        layer: CopperLayer::Bottom,
        ..Default::default()
    };
    let ink = BoardInk::get();
    let colors = crate::palette::board();
    let bottom = ink.on_board(colors.copper_layer(CopperLayer::Bottom), 1.0);
    let inner = ink.on_board(colors.copper_layer(CopperLayer::Inner(1)), OTHER_SHADE);

    let (output, _) = canvas(&mut editor, &engine, &ctx, vec![]);
    let all = shapes(&output);
    let fill = all
        .iter()
        .position(|s| mesh_colored(s, bottom))
        .ok_or("the pour's copper is drawn in the bottom colour")?;
    // The bottom OUT track along y = 24.
    let on_track = view_of(&editor, &engine).screen([17.0, 24.0]);
    let track = all
        .iter()
        .position(|s| {
            path_filled(s, bottom)
                && matches!(s, Shape::Path(p) if Rect::from_points(&p.points).contains(on_track))
        })
        .ok_or("the bottom track")?;
    assert!(fill < track, "fill copper lies under the tracks");
    assert!(
        all.iter().any(|s| segment_colored(s, inner)),
        "the unfilled inner plane shows its dashed outline"
    );

    editor.fill_display = FillDisplay::Outline;
    let all = shapes(&canvas(&mut editor, &engine, &ctx, vec![]).0);
    assert!(!all.iter().any(|s| mesh_colored(s, bottom)));
    assert!(all.iter().any(|s| segment_colored(s, bottom)));

    editor.fill_display = FillDisplay::Hatched;
    let all = shapes(&canvas(&mut editor, &engine, &ctx, vec![]).0);
    assert!(!all.iter().any(|s| mesh_colored(s, bottom)));
    let hatch = all.iter().filter(|s| segment_colored(s, bottom)).count();
    assert!(hatch > 10, "{hatch} hatch lines");

    // Hidden with its layer.
    editor.fill_display = FillDisplay::Solid;
    editor.layers.copper_bottom = false;
    let all = shapes(&canvas(&mut editor, &engine, &ctx, vec![]).0);
    assert!(!all.iter().any(|s| mesh_colored(s, bottom)));
    Ok(())
}

/// Clicking fill copper selects the fill and lights its net; a click in
/// its hole does not. The Fill card commits an edit as one `fill.edit`
/// (one undo step) and Refill refills it.
#[test]
fn clicking_fill_copper_selects_it_and_the_card_edits_it() -> TestResult {
    let (mut engine, pour) = filled_showcase()?;
    let gnd = engine.project().fill(pour).and_then(|f| f.net);
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor {
        from_bottom: true,
        ..Default::default()
    };
    canvas(&mut editor, &engine, &ctx, vec![]);
    let view = view_of(&editor, &engine);
    click(&mut editor, &engine, &ctx, view.screen([32.0, 2.5]));
    assert_eq!(editor.fill, None, "the hole is not copper");
    click(&mut editor, &engine, &ctx, view.screen([36.0, 2.5]));
    assert_eq!(editor.fill, Some(pour));
    assert_eq!(editor.highlighted_net(), gnd);
    assert_eq!(editor.inspected(engine.project()), Inspected::Fill(pour));
    let header = editor.header(&engine);
    assert!(header.title.contains("GND"), "{}", header.title);

    let (output, op) = card(&mut editor, &engine, &ctx, vec![]);
    assert!(op.is_none(), "showing the card sends nothing");
    let texts: Vec<String> = shapes(&output)
        .iter()
        .filter_map(|s| match s {
            Shape::Text(t) => Some(t.galley.job.text.clone()),
            _ => None,
        })
        .collect();
    for label in [
        "Priority",
        "Clearance",
        "Thermals",
        "Remove islands",
        "Refill",
    ] {
        assert!(texts.iter().any(|t| t == label), "{label} in {texts:?}");
    }
    // A staged change commits once nothing has focus.
    if let Some(draft) = editor.fill_card.as_mut() {
        draft_priority(draft, 7);
    }
    let (_, op) = card(&mut editor, &engine, &ctx, vec![]);
    let Some(Operation::Fill(FillCommand::Edit { fill })) = op.clone() else {
        return Err(format!("expected fill.edit, got {op:?}").into());
    };
    assert_eq!((fill.id, fill.priority), (pour, 7));
    let before = engine.project().clone();
    engine.apply(
        engine.revision(),
        Operation::Fill(FillCommand::Edit { fill }),
    )?;
    assert_eq!(engine.project().fill(pour).map(|f| f.priority), Some(7));
    let (_, again) = card(&mut editor, &engine, &ctx, vec![]);
    assert!(again.is_none(), "the committed value is not resent");
    engine.undo()?;
    assert_eq!(engine.project(), &before, "one undo step");

    // Refill in the footer.
    let (output, _) = card(&mut editor, &engine, &ctx, vec![]);
    let at = shapes(&output)
        .iter()
        .find_map(|s| match s {
            Shape::Text(t) if t.galley.job.text == "Refill" => Some(t.pos + t.galley.size() * 0.5),
            _ => None,
        })
        .ok_or("Refill button")?;
    card(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(at)],
    );
    card(&mut editor, &engine, &ctx, vec![button(at, true)]);
    let (_, op) = card(&mut editor, &engine, &ctx, vec![button(at, false)]);
    assert!(
        matches!(
            op,
            Some(Operation::Fill(FillCommand::Refill {
                target: RefillTarget::One(id)
            })) if id == pour
        ),
        "{op:?}"
    );
    Ok(())
}

fn draft_priority(draft: &mut super::fill_card::FillDraft, priority: u32) {
    draft.set_priority(priority);
}

/// The Fill tool draws an outline of lines and an arc on the routing
/// layer, closes on its first point and adds the fill in one `fill.add`.
#[test]
fn the_fill_tool_draws_lines_and_an_arc_and_adds_the_fill() -> TestResult {
    let mut engine = Engine::showcase()?;
    let gnd = engine
        .project()
        .nets
        .iter()
        .find(|n| n.name == "GND")
        .map(|n| n.id)
        .ok_or("GND")?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    editor.on_tool("fill", &engine);
    assert!(editor.subject() == Subject::Tool);
    editor.layer = CopperLayer::Top;
    editor.fill_draw.net = Some(gnd);
    canvas(&mut editor, &engine, &ctx, vec![]);
    let view = view_of(&editor, &engine);
    let mut op = None;
    for p in [[5.0, 2.0], [15.0, 2.0]] {
        op = op.or(click(&mut editor, &engine, &ctx, view.screen(p)));
    }
    let at = view.screen([15.0, 2.0]);
    canvas(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(at), key(egui::Key::A)],
    );
    assert!(editor.fill_draw.arc);
    // An arc to (15, 6) through (17, 4).
    for p in [[15.0, 6.0], [17.0, 4.0]] {
        op = op.or(click(&mut editor, &engine, &ctx, view.screen(p)));
    }
    canvas(&mut editor, &engine, &ctx, vec![key(egui::Key::A)]);
    assert!(!editor.fill_draw.arc);
    op = op.or(click(&mut editor, &engine, &ctx, view.screen([5.0, 6.0])));
    assert!(op.is_none(), "not closed yet");
    let op = click(&mut editor, &engine, &ctx, view.screen([5.0, 2.0])).ok_or("closing adds")?;
    let Operation::Fill(FillCommand::Add { fill }) = &op else {
        return Err(format!("expected fill.add, got {op:?}").into());
    };
    assert_eq!((fill.net, fill.layer), (Some(gnd), CopperLayer::Top));
    assert_eq!(fill.outline.start, [5.0, 2.0]);
    assert!(fill.outline.closed());
    assert_eq!(fill.outline.segments.len(), 4);
    let Segment::Arc { to, bulge } = fill.outline.segments[1] else {
        return Err("the second segment is an arc".into());
    };
    assert_eq!(to, [15.0, 6.0]);
    assert!((bulge - 1.0).abs() < 1e-9, "a half circle: {bulge}");
    let fills = engine.project().fills.len();
    engine.apply(engine.revision(), op)?;
    assert_eq!(engine.project().fills.len(), fills + 1);
    engine.undo()?;
    assert_eq!(engine.project().fills.len(), fills, "one undo step");
    Ok(())
}

/// Dragging a vertex handle of the selected fill moves the vertex in one
/// `fill.edit` on release; nothing changes before it.
#[test]
fn dragging_an_outline_handle_moves_the_vertex_in_one_edit() -> TestResult {
    let (mut engine, pour) = filled_showcase()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor {
        from_bottom: true,
        ..Default::default()
    };
    editor.select_fill(pour, None);
    canvas(&mut editor, &engine, &ctx, vec![]);
    let view = view_of(&editor, &engine);
    let outline = engine
        .project()
        .fill(pour)
        .map(|f| f.outline.clone())
        .ok_or("pour")?;
    let corner = outline.segments[0].to();
    let from = view.screen(corner);
    let to = view.screen([corner[0] - 2.0, corner[1] + 1.0]);
    canvas(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(from)],
    );
    canvas(&mut editor, &engine, &ctx, vec![button(from, true)]);
    let revision = engine.revision();
    let mut op = None;
    for k in 1..=6 {
        let p = from + (to - from) * (k as f32 / 6.0);
        op = op.or(canvas(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::PointerMoved(p)],
        )
        .1);
    }
    assert!(op.is_none(), "nothing before release");
    assert!(
        editor.fill_drag.is_some(),
        "the handle is dragged, not a part"
    );
    let op = canvas(&mut editor, &engine, &ctx, vec![button(to, false)])
        .1
        .ok_or("release edits")?;
    let Operation::Fill(FillCommand::Edit { fill }) = &op else {
        return Err(format!("expected fill.edit, got {op:?}").into());
    };
    let moved = fill.outline.segments[0].to();
    assert_eq!(moved, snap([corner[0] - 2.0, corner[1] + 1.0]));
    assert_eq!(engine.revision(), revision);
    engine.apply(engine.revision(), op)?;
    assert_eq!(
        engine
            .project()
            .fill(pour)
            .map(|f| f.outline.segments[0].to()),
        Some(moved)
    );
    Ok(())
}
