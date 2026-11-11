//! A part's placement state in Layout (docs/e2e-ui-run.md findings 5, 8, 9
//! and 10), in headless egui frames on the engine: the canvas marks of
//! locked and dimension-held parts, the refusal of a drag of either in the
//! engine's words, a drag that slides a part along the one edge it is
//! dimensioned to, the Placement card's state row and Dimensions section in
//! a short card, and a refused board parameter going back to its value with
//! the whole reason.
//!
//! With `PCBCRAFT_PICTURES` set, `hold_pictures` writes the canvas and the
//! cards as PNGs there.
use super::hold_mark::{GLYPH, Hold};
use super::raster_tests::{picture, picture_of, view_of};
use super::*;
use crate::kit::toast;
use pcbcraft_core::layout::dimensions::{self, DimensionCommand};
use pcbcraft_core::layout::outline::BoardCommand;
use pcbcraft_core::library::footprint_source::{DimKind, DimValue, SketchRef};
use pcbcraft_core::model::board::ConstraintTarget;
use pcbcraft_core::model::dimension::{BoardRef, FeatureRef, PartDimensionSpec, PartRelation};

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Apply with the app's host board evaluator, as the desktop does.
fn hosted(
    engine: &mut Engine,
    operation: Operation,
) -> pcbcraft_core::Result<pcbcraft_core::Outcome> {
    engine.apply_hosted(
        engine.revision(),
        operation,
        pcbcraft_core::Evaluators {
            footprint: None,
            board: board_shape::host_evaluator(),
        },
    )
}

/// A distance from a part's origin to a named board edge.
fn to_edge(engine: &Engine, part: Id, edge: &str, mm: f64) -> Fallible<PartDimensionSpec> {
    let ev = board_shape::host_evaluator().ok_or("no evaluator")?;
    let board = dimensions::board_sketch(engine.project(), ev)?;
    let curve = board
        .edges
        .iter()
        .find(|e| e.name.0 == edge)
        .map(|e| e.curve.clone())
        .ok_or("no edge")?;
    Ok(PartDimensionSpec {
        kind: PartRelation::Dim(DimKind::Distance),
        refs: vec![
            BoardRef::Part(FeatureRef::Origin {
                target: ConstraintTarget::Placement(part),
            }),
            BoardRef::Board(SketchRef::Curve(curve)),
        ],
        value: Some(DimValue::Fixed(mm)),
        reference: false,
    })
}

fn dimension(engine: &mut Engine, spec: PartDimensionSpec) -> Fallible {
    hosted(
        engine,
        Operation::Dimension(DimensionCommand::Add { dimension: spec }),
    )?;
    Ok(())
}

/// The demo board with R1 held 5 mm from the right edge and 10 mm from
/// the bottom edge, and R2 locked; any other part is free.
fn held_board() -> Fallible<(Engine, Id, Id)> {
    let mut engine = Engine::demo()?;
    let r1 = engine
        .project()
        .instance_by_designator("R1")
        .map(|i| i.id)
        .ok_or("no R1")?;
    let r2 = engine
        .project()
        .instance_by_designator("R2")
        .map(|i| i.id)
        .ok_or("no R2")?;
    let right = to_edge(&engine, r1, "right", 5.0)?;
    dimension(&mut engine, right)?;
    let bottom = to_edge(&engine, r1, "bottom", 10.0)?;
    dimension(&mut engine, bottom)?;
    hosted(
        &mut engine,
        Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: r2,
            locked: true,
        }),
    )?;
    Ok((engine, r1, r2))
}

fn canvas_frame(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<Operation>) {
    let mut command = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 700.0))),
            events,
            ..Default::default()
        },
        |ui| command = editor.canvas(ui, engine),
    );
    output.textures_delta.clear();
    (output, command)
}

/// Every glyph-sized box painted with a rim in `ink`, by its centre.
fn glyphs(output: &egui::FullOutput, ink: Color32) -> Vec<Pos2> {
    fn walk(shape: &Shape, ink: Color32, out: &mut Vec<Pos2>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, ink, out)),
            Shape::Rect(r)
                if r.stroke.color == ink
                    && (r.rect.width() - GLYPH).abs() < 0.01
                    && (r.rect.height() - GLYPH).abs() < 0.01 =>
            {
                out.push(r.rect.center());
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, ink, &mut out);
    }
    out
}

/// Every filled circle in `ink`, by its centre.
fn dots(output: &egui::FullOutput, ink: Color32) -> Vec<Pos2> {
    fn walk(shape: &Shape, ink: Color32, out: &mut Vec<Pos2>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, ink, out)),
            Shape::Circle(c) if c.fill == ink => out.push(c.center),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, ink, &mut out);
    }
    out
}

/// Where a part's mark belongs on screen: just outside the top-right
/// corner of its envelope, or on it when the part is smaller than a glyph.
fn corner(editor: &PcbEditor, engine: &Engine, instance: Id) -> Fallible<Pos2> {
    let project = engine.project();
    let placement = project.placement_of(instance).ok_or("not placed")?;
    let corners = body_polygon(project, placement).ok_or("no envelope")?;
    let view = view_of(editor, engine);
    Ok(super::hold_mark::mark_at(Rect::from_points(&corners.map(|p| view.screen(p)))).0)
}

fn screen_of(editor: &PcbEditor, engine: &Engine, instance: Id) -> Fallible<Pos2> {
    let placement = engine
        .project()
        .placement_of(instance)
        .ok_or("not placed")?;
    Ok(view_of(editor, engine).screen(placement.position))
}

/// Unselected, a held part shows the Dimension glyph with a dimension
/// rim, a locked part the Fix glyph with a locked rim, each on its
/// envelope's top-right corner; a free part shows nothing. Zoomed far out
/// the marks stay, as dots of the same colours.
#[test]
fn held_and_locked_parts_are_marked_and_free_parts_are_not() -> Fallible {
    let (engine, r1, r2) = held_board()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    assert_eq!(editor.instance, None, "nothing selected");
    let (output, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
    let t = Tokens::get();
    assert_ne!(
        t.dimension, t.sketch_locked,
        "the two states differ in colour"
    );
    let held = glyphs(&output, Hold::Constrained.ink());
    let locked = glyphs(&output, Hold::Locked.ink());
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(locked.len(), 1, "{locked:?}");
    assert!(held[0].distance(corner(&editor, &engine, r1)?) < 0.5);
    assert!(locked[0].distance(corner(&editor, &engine, r2)?) < 0.5);
    // Free parts: no mark at their corners.
    let free = engine
        .project()
        .placements
        .iter()
        .filter(|p| p.instance != r1 && p.instance != r2)
        .count();
    let all =
        glyphs(&output, Hold::Constrained.ink()).len() + glyphs(&output, Hold::Locked.ink()).len();
    assert_eq!(all, 2, "{free} free parts carry no mark");
    // Selecting a part does not change the marks.
    editor.set_selection(Some(r1), None, None);
    let (output, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(glyphs(&output, Hold::Constrained.ink()).len(), 1);
    // Zoomed out until the parts are smaller than the glyph: dots.
    editor.set_selection(None, None, None);
    editor.zoom = 0.2;
    let (output, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
    assert!(glyphs(&output, Hold::Constrained.ink()).is_empty());
    let held = dots(&output, Hold::Constrained.ink());
    let locked = dots(&output, Hold::Locked.ink());
    assert_eq!((held.len(), locked.len()), (1, 1), "{held:?} {locked:?}");
    assert!(held[0].distance(corner(&editor, &engine, r1)?) < 0.5);
    Ok(())
}

fn drag(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    from: Pos2,
) -> Option<Operation> {
    let to = from + egui::vec2(40.0, 25.0);
    let mut last = None;
    for events in [
        vec![
            egui::Event::PointerMoved(from),
            egui::Event::PointerButton {
                pos: from,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        vec![egui::Event::PointerMoved(from + egui::vec2(10.0, 6.0))],
        vec![egui::Event::PointerMoved(to)],
        vec![egui::Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    ] {
        let (_, op) = canvas_frame(editor, engine, ctx, events);
        last = last.or(op);
    }
    last
}

/// Dragging a held or a locked part moves nothing and says why, in the
/// words the engine refuses `layout.move` with (the CLI and MCP get the
/// same); a free part still drags into one `layout.move`.
#[test]
fn dragging_a_fixed_part_is_refused_with_the_engines_reason() -> Fallible {
    let (mut engine, r1, r2) = held_board()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    canvas_frame(&mut editor, &engine, &ctx, vec![]);
    let _ = toast::take_posted(&ctx);
    for (part, want) in [
        (
            r1,
            "R1 is held by its dimensions; edit or delete one to move it",
        ),
        (r2, "R2 is locked; unlock it to move it"),
    ] {
        let at = screen_of(&editor, &engine, part)?;
        let op = drag(&mut editor, &engine, &ctx, at);
        assert!(op.is_none(), "{op:?}");
        assert_eq!(editor.instance, Some(part), "the press selects it");
        let posted = toast::take_posted(&ctx);
        let texts: Vec<&str> = posted.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(texts, [want]);
        assert_eq!(posted[0].level, crate::kit::severity::Level::Error);
        // The engine refuses a typed move of the part in the same words.
        let placement = engine
            .project()
            .placement_of(part)
            .cloned()
            .ok_or("placed")?;
        let refused = hosted(
            &mut engine,
            Operation::Layout(LayoutCommand::Move {
                instance: part,
                position: [placement.position[0] + 2.0, placement.position[1]],
                rotation_deg: placement.rotation_deg,
                layer: placement.layer,
            }),
        )
        .err()
        .ok_or("the engine moved it")?;
        assert_eq!(refusal::shown_error(&refused), want);
    }
    // Unlocked, R2 drags again.
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: r2,
            locked: false,
        }),
    )?;
    let at = screen_of(&editor, &engine, r2)?;
    let op = drag(&mut editor, &engine, &ctx, at);
    assert!(
        matches!(op, Some(Operation::Layout(LayoutCommand::Move { instance, .. })) if instance == r2),
        "{op:?}"
    );
    assert!(toast::take_posted(&ctx).is_empty());
    Ok(())
}

/// A part dimensioned to one edge only slides along it: the drag previews
/// and commits the move the engine lands, its distance to the right edge
/// kept while it follows the pointer up and down.
#[test]
fn dragging_slides_a_part_along_its_one_edge_dimension() -> Fallible {
    let mut engine = Engine::demo()?;
    let r2 = engine
        .project()
        .instance_by_designator("R2")
        .map(|i| i.id)
        .ok_or("no R2")?;
    let before = engine.project().placement_of(r2).cloned().ok_or("placed")?;
    let held = to_edge(&engine, r2, "right", 60.0 - before.position[0])?;
    dimension(&mut engine, held)?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    canvas_frame(&mut editor, &engine, &ctx, vec![]);
    let at = screen_of(&editor, &engine, r2)?;
    let op = drag(&mut editor, &engine, &ctx, at);
    let Some(op @ Operation::Layout(LayoutCommand::Move { position, .. })) = op else {
        return Err(format!("no layout.move: {op:?}").into());
    };
    assert!(
        (position[0] - before.position[0]).abs() < 1e-6,
        "x is held: {position:?}"
    );
    assert!(
        (position[1] - before.position[1]).abs() > 1.0,
        "y follows the pointer: {position:?}"
    );
    assert!(toast::take_posted(&ctx).is_empty());
    hosted(&mut engine, op)?;
    let placed = engine.project().placement_of(r2).ok_or("placed")?;
    assert!((placed.position[0] - before.position[0]).abs() < 1e-6);
    assert!((placed.position[1] - position[1]).abs() < 1e-6);
    Ok(())
}

/// The right column's subject card while Issues is expanded: header and
/// about ten rows (`kit::card::subject_rows_height`), as the founder saw it.
const SHORT_CARD: [f32; 2] = [280.0, 420.0];

fn short_card(
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
                egui::vec2(SHORT_CARD[0], SHORT_CARD[1]),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            let height = crate::kit::card::subject_rows_height().min(SHORT_CARD[1]);
            let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(SHORT_CARD[0], height));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            child.set_clip_rect(rect);
            command = editor.subject_slot(&mut child, engine).output;
            let _ = refusal::take_mark(ui.ctx());
        },
    );
    output.textures_delta.clear();
    (output, command)
}

/// Whether `label` is painted whole: inside its clip rect and not cut
/// short with "…".
fn painted_whole(output: &egui::FullOutput, label: &str) -> bool {
    fn walk(shape: &Shape, clip: Rect, label: &str) -> bool {
        match shape {
            Shape::Vec(shapes) => shapes.iter().any(|s| walk(s, clip, label)),
            Shape::Text(text) if text.galley.job.text == label => {
                let rect = text.galley.rect.translate(text.pos.to_vec2());
                let cut = text
                    .galley
                    .rows
                    .iter()
                    .any(|r| r.glyphs.iter().any(|g| g.chr == '…'));
                !cut && rect.right() <= clip.right() + 0.5
            }
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|c| walk(&c.shape, c.clip_rect, label))
}

/// Every text painted.
fn collect_texts(shape: &Shape, out: &mut Vec<String>) {
    match shape {
        Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_texts(s, out)),
        Shape::Text(text) => out.push(text.galley.job.text.clone()),
        _ => {}
    }
}

/// Texts painted inside the card's visible height, by centre.
fn visible_texts(output: &egui::FullOutput) -> Vec<(String, Pos2)> {
    fn walk(shape: &Shape, clip: Rect, out: &mut Vec<(String, Pos2)>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, clip, out)),
            Shape::Text(text) => {
                let at = text.pos + text.galley.size() * 0.5;
                if clip.contains(at) {
                    out.push((text.galley.job.text.clone(), at));
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, clipped.clip_rect, &mut out);
    }
    out
}

fn click(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    label: &str,
) -> Fallible<Option<Operation>> {
    let (output, _) = short_card(editor, engine, ctx, vec![]);
    let at = visible_texts(&output)
        .into_iter()
        .find(|(t, _)| t == label)
        .map(|(_, p)| p)
        .ok_or_else(|| format!("{label} is not visible on the card"))?;
    short_card(
        editor,
        engine,
        ctx,
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    Ok(short_card(
        editor,
        engine,
        ctx,
        vec![egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    )
    .1)
}

/// In a card as short as the founder's, the Placement section opens with
/// the part's state, visible at once: a free part says so and offers
/// "Dimension to edge", a held part says where it is held (and offers
/// "Free part" when one dimension holds it), a locked part says it is
/// locked; the canvas glyph sits beside the words.
#[test]
fn the_placement_card_states_the_hold_at_once() -> Fallible {
    let (mut engine, r1, r2) = held_board()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    for (part, words, action) in [
        (
            r1,
            "Held 5 mm from the right edge and 10 mm from the bottom edge",
            None::<&str>,
        ),
        (r2, "Locked in place", None),
    ] {
        editor.set_selection(Some(part), None, None);
        short_card(&mut editor, &engine, &ctx, vec![]);
        let (output, _) = short_card(&mut editor, &engine, &ctx, vec![]);
        let shown = visible_texts(&output);
        assert!(shown.iter().any(|(t, _)| t == words), "{words}: {shown:?}");
        assert!(painted_whole(&output, words), "{words} is cut short");
        if let Some(action) = action {
            assert!(
                shown.iter().any(|(t, _)| t == action),
                "{action}: {shown:?}"
            );
        }
    }
    // Free part: unlock R2, and the card offers to dimension it to its
    // nearest edge, in place, as one layout.dimension_add.
    hosted(
        &mut engine,
        Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: r2,
            locked: false,
        }),
    )?;
    editor.set_selection(Some(r2), None, None);
    let (output, _) = short_card(&mut editor, &engine, &ctx, vec![]);
    let shown = visible_texts(&output);
    assert!(
        shown.iter().any(|(t, _)| t.starts_with("Free · nearest: ")),
        "{shown:?}"
    );
    let free = shown
        .iter()
        .map(|(t, _)| t.as_str())
        .find(|t| t.starts_with("Free · "))
        .ok_or("no free state")?;
    assert!(painted_whole(&output, free), "{free} is cut short");
    let op = click(&mut editor, &engine, &ctx, "Dimension to edge")?;
    let Some(op @ Operation::Dimension(DimensionCommand::Add { .. })) = op else {
        return Err(format!("no layout.dimension_add: {op:?}").into());
    };
    let before = engine.project().placement_of(r2).cloned();
    hosted(&mut engine, op)?;
    assert_eq!(
        engine.project().placement_of(r2).cloned(),
        before,
        "in place"
    );
    let (output, _) = short_card(&mut editor, &engine, &ctx, vec![]);
    let shown = visible_texts(&output);
    assert!(
        shown
            .iter()
            .any(|(t, _)| t == "Held 8 mm from the bottom edge"),
        "{shown:?}"
    );
    // The canvas draws the selected part's distance to its edge, labelled
    // as 3D labels it (the one dimension drawing, §20).
    let (output, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
    let mut painted = Vec::new();
    for clipped in &output.shapes {
        collect_texts(&clipped.shape, &mut painted);
    }
    assert!(painted.iter().any(|t| t == "8.00 mm"), "{painted:?}");
    // Its Dimensions section (in a taller card) lists the dimension with its
    // value and its delete icon.
    let id = super::edge_card::dimensions_of(engine.project(), r2)
        .first()
        .map(|d| d.id)
        .ok_or("no dimension")?;
    ctx.enable_accesskit();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(SHORT_CARD[0], 900.0),
            )),
            ..Default::default()
        },
        |ui| {
            editor.subject_slot(ui, &engine);
            let _ = refusal::take_mark(ui.ctx());
        },
    );
    output.textures_delta.clear();
    let mut painted = Vec::new();
    for clipped in &output.shapes {
        collect_texts(&clipped.shape, &mut painted);
    }
    for want in ["DIMENSIONS", "Origin → bottom edge"] {
        assert!(painted.iter().any(|t| t == want), "{want}: {painted:?}");
    }
    // Its delete is an icon at the row's end, named for screen readers
    // and the keyboard by what it deletes.
    let named: Vec<String> = output
        .platform_output
        .accesskit_update
        .as_ref()
        .map(|u| {
            u.nodes
                .iter()
                .filter(|(_, n)| n.role() == egui::accesskit::Role::Button)
                .filter_map(|(_, n)| n.label().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        named
            .iter()
            .any(|n| n == "Delete dimension Origin → bottom edge"),
        "{named:?}"
    );
    // "Free part" removes the one dimension as one step, in place.
    let held = engine.project().placement_of(r2).cloned();
    let op = click(&mut editor, &engine, &ctx, "Free part")?;
    let Some(op @ Operation::Dimension(DimensionCommand::Delete { id: deleted })) = op else {
        return Err(format!("no layout.dimension_delete: {op:?}").into());
    };
    assert_eq!(deleted, id);
    hosted(&mut engine, op)?;
    assert!(super::edge_card::dimensions_of(engine.project(), r2).is_empty());
    assert_eq!(engine.project().placement_of(r2).cloned(), held);
    Ok(())
}

/// The demo board as a sketch driven by `width` and `height`, solved by
/// the app's host evaluator.
fn sketched_demo() -> Fallible<Engine> {
    let mut engine = Engine::demo()?;
    engine.apply_hosted(
        engine.revision(),
        Operation::Board(BoardCommand::StartSketch),
        pcbcraft_core::Evaluators {
            footprint: None,
            board: board_shape::host_evaluator(),
        },
    )?;
    Ok(engine)
}

/// A refused board parameter goes back to its stored value at once, is not
/// marked invalid (what it shows is valid), and shows the engine's whole
/// reason under it until the document changes. Parts may sit off the
/// board, but track copper may not, so a track 20–30 mm in refuses an
/// 8 mm wide board.
#[test]
fn a_refused_board_parameter_keeps_its_value_and_shows_the_whole_reason() -> Fallible {
    let mut engine = sketched_demo()?;
    let net = engine.project().nets.first().map(|n| n.id).ok_or("a net")?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: pcbcraft_core::layout::RouteRequest {
                net,
                start: [20.0, 30.0],
                end: [30.0, 30.0],
                width: 0.3,
                layer: pcbcraft_core::Layer::Top.into(),
            },
        }),
    )?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    let frame = |editor: &mut PcbEditor, engine: &Engine, events| {
        let mut command = None;
        let mut mark = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(320.0, 1200.0))),
                events,
                ..Default::default()
            },
            |ui| {
                command = editor.subject_slot(ui, engine).output;
                mark = refusal::take_mark(ui.ctx());
            },
        );
        output.textures_delta.clear();
        (output, command, mark)
    };
    let width = engine.project().board_size[0];
    let stored = format!("{} mm", kit::number_text(width));
    let (output, _, _) = frame(&mut editor, &engine, vec![]);
    let field = visible_texts(&output)
        .into_iter()
        .find(|(t, _)| *t == stored)
        .map(|(_, p)| p)
        .ok_or_else(|| format!("no width field showing {stored}"))?;
    frame(
        &mut editor,
        &engine,
        vec![
            egui::Event::PointerMoved(field),
            egui::Event::PointerButton {
                pos: field,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: field,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    frame(
        &mut editor,
        &engine,
        vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            },
            egui::Event::Text("8 mm".into()),
        ],
    );
    let (_, op, mark) = frame(
        &mut editor,
        &engine,
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let op = op.ok_or("Enter did not commit the width")?;
    let origin = mark.ok_or("the commit was not marked")?;
    let revision = engine.revision();
    let error = engine
        .apply_hosted(
            revision,
            op,
            pcbcraft_core::Evaluators {
                footprint: None,
                board: board_shape::host_evaluator(),
            },
        )
        .err()
        .ok_or("an 8 mm board took the track")?;
    assert_eq!(engine.revision(), revision);
    let reason = refusal::shown_error(&error);
    // It names the copper in the way and the size asked for.
    assert!(
        reason.contains("1 track on") && reason.contains("would lie outside 8 × "),
        "{reason}"
    );
    // The app hands the refusal back to the field on the next frame.
    refusal::hand_back(&ctx, refusal::Refused { origin, error }, false);
    frame(&mut editor, &engine, vec![]);
    let (output, _, _) = frame(&mut editor, &engine, vec![]);
    let shown: Vec<String> = visible_texts(&output).into_iter().map(|(t, _)| t).collect();
    assert!(shown.contains(&stored), "back to {stored}: {shown:?}");
    assert!(!shown.iter().any(|t| t == "8 mm"), "{shown:?}");
    assert!(shown.contains(&reason), "the whole reason: {shown:?}");
    assert!(
        !red_rings(&output),
        "the field holds a valid value and is not marked"
    );
    // Another change to the document clears it.
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: engine
                .project()
                .placements
                .first()
                .map(|p| p.instance)
                .ok_or("a part")?,
            locked: true,
        }),
    )?;
    frame(&mut editor, &engine, vec![]);
    let (output, _, _) = frame(&mut editor, &engine, vec![]);
    let shown: Vec<String> = visible_texts(&output).into_iter().map(|(t, _)| t).collect();
    assert!(!shown.contains(&reason), "{shown:?}");
    Ok(())
}

/// An invalid board parameter is marked; typing the stored, valid value
/// back clears the mark and its note, though nothing is committed.
#[test]
fn a_valid_value_clears_a_board_parameter_error() -> Fallible {
    let engine = sketched_demo()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    let frame = |editor: &mut PcbEditor, events| {
        let mut command = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(320.0, 1200.0))),
                events,
                ..Default::default()
            },
            |ui| command = editor.subject_slot(ui, &engine).output,
        );
        output.textures_delta.clear();
        (output, command)
    };
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    let stored = format!("{} mm", kit::number_text(engine.project().board_size[0]));
    let (output, _) = frame(&mut editor, vec![]);
    let field = visible_texts(&output)
        .into_iter()
        .find(|(t, _)| *t == stored)
        .map(|(_, p)| p)
        .ok_or_else(|| format!("no width field showing {stored}"))?;
    let enter = |editor: &mut PcbEditor, text: &str| {
        frame(
            editor,
            vec![
                egui::Event::PointerMoved(field),
                egui::Event::PointerButton {
                    pos: field,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: field,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(
            editor,
            vec![
                key(egui::Key::A, egui::Modifiers::COMMAND),
                egui::Event::Text(text.into()),
            ],
        );
        let (_, op) = frame(editor, vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
        let (output, _) = frame(editor, vec![]);
        (output, op)
    };
    let (output, op) = enter(&mut editor, "abc");
    assert!(op.is_none(), "an invalid value is not committed");
    let shown: Vec<String> = visible_texts(&output).into_iter().map(|(t, _)| t).collect();
    assert!(shown.iter().any(|t| t.contains("abc")), "{shown:?}");
    assert!(red_rings(&output), "the field is marked");
    let (output, op) = enter(&mut editor, &stored);
    assert!(op.is_none(), "the stored value needs no commit");
    let shown: Vec<String> = visible_texts(&output).into_iter().map(|(t, _)| t).collect();
    assert!(!shown.iter().any(|t| t.contains("abc")), "{shown:?}");
    assert!(!red_rings(&output), "the mark went");
    Ok(())
}

/// Demo run item 8: on a plain rectangle board a part dimension's value
/// reads `height / 2` (the board's own parameters, as the dimension solve
/// reads them); an invalid value after a committed one is marked, and an
/// undo, which puts the document's values back in the fields, clears it.
#[test]
fn a_rectangle_boards_parameters_read_and_undo_clears_a_dimension_error() -> Fallible {
    let (mut engine, r1, _) = held_board()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    editor.set_selection(Some(r1), None, None);
    let frame = |editor: &mut PcbEditor, engine: &Engine, events| {
        let mut command = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(320.0, 1200.0))),
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
    };
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    let enter = |editor: &mut PcbEditor, engine: &Engine, field: Pos2, text: &str| {
        let click = |pressed| egui::Event::PointerButton {
            pos: field,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            editor,
            engine,
            vec![egui::Event::PointerMoved(field), click(true), click(false)],
        );
        frame(
            editor,
            engine,
            vec![
                key(egui::Key::A, egui::Modifiers::COMMAND),
                egui::Event::Text(text.into()),
            ],
        );
        frame(
            editor,
            engine,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        )
        .1
    };
    frame(&mut editor, &engine, vec![]);
    let (output, _) = frame(&mut editor, &engine, vec![]);
    let field = visible_texts(&output)
        .into_iter()
        .find(|(t, _)| t == "5 mm")
        .map(|(_, p)| p)
        .ok_or("no dimension field showing 5 mm")?;
    let op = enter(&mut editor, &engine, field, "height / 2").ok_or("height / 2 was refused")?;
    hosted(&mut engine, op)?;
    let (output, _) = frame(&mut editor, &engine, vec![]);
    assert!(!red_rings(&output), "a valid expression is marked");
    assert!(enter(&mut editor, &engine, field, "abc").is_none());
    let (output, _) = frame(&mut editor, &engine, vec![]);
    assert!(red_rings(&output), "the invalid value is marked");
    engine.undo()?;
    let (output, _) = frame(&mut editor, &engine, vec![]);
    let shown: Vec<String> = visible_texts(&output).into_iter().map(|(t, _)| t).collect();
    assert!(shown.iter().any(|t| t == "5 mm"), "{shown:?}");
    assert!(!red_rings(&output), "the undone field stays marked");
    Ok(())
}

/// Whether a red ring (a `kit::field_error` mark) is painted.
fn red_rings(output: &egui::FullOutput) -> bool {
    fn walk(shape: &Shape, red: Color32) -> bool {
        match shape {
            Shape::Vec(shapes) => shapes.iter().any(|s| walk(s, red)),
            Shape::Rect(r) => r.stroke.color == red && r.fill == Color32::TRANSPARENT,
            _ => false,
        }
    }
    let red = Tokens::get().error;
    output.shapes.iter().any(|c| walk(&c.shape, red))
}

/// Evidence pictures, written only with `PCBCRAFT_PICTURES` set: the
/// canvas at the fitted zoom, close and far out, and the three cards.
#[test]
fn hold_pictures() -> Fallible {
    let Some(dir) = std::env::var_os("PCBCRAFT_PICTURES") else {
        return Ok(());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let (engine, r1, r2) = held_board()?;
    let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 700.0));
    let board = engine.project().board_size;
    let around = |part: Id| -> Fallible<[Point; 2]> {
        let p = engine
            .project()
            .placement_of(part)
            .ok_or("placed")?
            .position;
        Ok([[p[0] - 4.0, p[1] - 3.0], [p[0] + 4.0, p[1] + 3.0]])
    };
    let shots = [
        ("hold-canvas", None),
        ("hold-canvas-held-close", Some(around(r1)?)),
        ("hold-canvas-locked-close", Some(around(r2)?)),
        ("hold-canvas-far", None),
    ];
    for (name, bounds) in shots {
        let mut editor = PcbEditor::default();
        if let Some(bounds) = bounds {
            (editor.zoom, editor.pan) = frame_view(rect, board, bounds, false);
        } else if name.ends_with("far") {
            editor.zoom = 0.2;
        }
        let png = picture(&mut editor, &engine).png().ok_or("png")?;
        std::fs::write(dir.join(format!("{name}.png")), png)?;
    }
    // The demo has two parts: a copy with R2 unlocked shows a free one.
    let (mut freed, _, _) = held_board()?;
    hosted(
        &mut freed,
        Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: r2,
            locked: false,
        }),
    )?;
    let cards = [
        ("hold-card-held", r1, &engine),
        ("hold-card-locked", r2, &engine),
        ("hold-card-free", r2, &freed),
    ];
    for (name, part, engine) in cards {
        let mut editor = PcbEditor::default();
        editor.set_selection(Some(part), None, None);
        let height = crate::kit::card::subject_rows_height();
        let png = picture_of([SHORT_CARD[0], height], Tokens::get().panel, |ui| {
            editor.subject_slot(ui, engine);
            let _ = refusal::take_mark(ui.ctx());
        })
        .png()
        .ok_or("png")?;
        std::fs::write(dir.join(format!("{name}.png")), png)?;
    }
    Ok(())
}

/// Align with the host's evaluator: a part its dimensions hold on the axis
/// it would move along, or a locked one, refuses the command in words that
/// name it, and nothing changes; a part held on the other axis only lines
/// up on this one.
#[test]
fn aligning_a_held_or_locked_part_is_refused_and_names_it() -> Fallible {
    use pcbcraft_core::layout::arrange::AlignTo;
    let (mut engine, r1, r2) = held_board()?;
    let align = |to| {
        Operation::Layout(LayoutCommand::Align {
            instances: vec![r1, r2],
            to,
        })
    };
    let refused = |engine: &mut Engine, op: Operation| -> Fallible<String> {
        let (before, revision) = (engine.project().clone(), engine.revision());
        let error = hosted(engine, op).err().ok_or("not refused")?;
        assert_eq!(engine.project(), &before);
        assert_eq!(engine.revision(), revision);
        Ok(refusal::shown_error(&error))
    };
    // R1 is right of R2 and held 5 mm from the right edge: aligning left
    // edges would move R1 along x.
    let x_of = |engine: &Engine, id| engine.project().placement_of(id).map(|p| p.position[0]);
    let (r1_x, r2_x) = (
        x_of(&engine, r1).ok_or("R1")?,
        x_of(&engine, r2).ok_or("R2")?,
    );
    assert!(r1_x > r2_x, "{r1_x} {r2_x}");
    let lock = |locked| {
        Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: r2,
            locked,
        })
    };
    hosted(&mut engine, lock(false))?;
    let why = refused(&mut engine, align(AlignTo::Left))?;
    assert!(
        why.starts_with("R1 is held") && why.contains("align it"),
        "{why}"
    );
    // Aligning right edges moves R2, locked again.
    hosted(&mut engine, lock(true))?;
    let why = refused(&mut engine, align(AlignTo::Right))?;
    assert_eq!(why, "R2 is locked; unlock it to align it");

    // Held only on x (to the right edge), R1 lines up along y.
    let mut engine = Engine::demo()?;
    let right = to_edge(&engine, r1, "right", 5.0)?;
    dimension(&mut engine, right)?;
    hosted(
        &mut engine,
        Operation::Layout(LayoutCommand::Move {
            instance: r2,
            position: [16.0, 14.0],
            rotation_deg: 0.0,
            layer: Layer::Top,
        }),
    )?;
    let x = x_of(&engine, r1).ok_or("R1")?;
    hosted(&mut engine, align(AlignTo::Top))?;
    let [a, b] = [r1, r2].map(|id| engine.project().placement_of(id).map(|p| p.position));
    let (a, b) = (a.ok_or("R1")?, b.ok_or("R2")?);
    assert!(
        (a[1] - b[1]).abs() < 1e-9 && (a[1] - 14.0).abs() < 1e-9,
        "{a:?} {b:?}"
    );
    assert!((a[0] - x).abs() < 1e-9, "x stays where it is held");
    let why = refused(&mut engine, align(AlignTo::Left))?;
    assert!(why.starts_with("R1 is held"), "{why}");
    Ok(())
}
