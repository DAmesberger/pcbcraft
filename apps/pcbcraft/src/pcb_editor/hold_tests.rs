//! A part's placement state in Layout (docs/e2e-ui-run.md findings 5, 8, 9
//! and 10), in headless egui frames on the engine: the canvas marks of
//! locked and edge-held parts, the refusal of a drag of either in the
//! engine's words, the Placement card's state row in a short card, and a
//! refused board parameter going back to its value with the whole reason.
//!
//! With `PCBCRAFT_PICTURES` set, `hold_pictures` writes the canvas and the
//! cards as PNGs there.
use super::hold_mark::{GLYPH, Hold};
use super::raster_tests::{picture, picture_of, view_of};
use super::*;
use crate::kit::toast;
use pcbcraft_core::layout::outline::BoardCommand;
use pcbcraft_core::library::footprint_source::Expr;
use pcbcraft_core::model::board::{
    ConstraintAlign, ConstraintTarget, EdgeAnchor, EdgeConstraintSpec, EdgeName,
};

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The demo board with R1 held 5 mm from the right edge (10 mm along it)
/// and R2 locked; any other part is free.
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
    engine.apply(
        engine.revision(),
        Operation::Board(BoardCommand::Constrain {
            constraint: EdgeConstraintSpec {
                target: ConstraintTarget::Placement(r1),
                edge: EdgeName("right".into()),
                anchor: EdgeAnchor::Start,
                along: Expr("10 mm".into()),
                offset: Expr("5 mm".into()),
                align: ConstraintAlign::Origin,
                follow_edge: false,
                rotation_deg: 0.0,
            },
        }),
    )?;
    engine.apply(
        engine.revision(),
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
            "R1 is held 5 mm from the right edge; edit or remove its constraint to move it",
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
        let refused = engine
            .apply(
                engine.revision(),
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
/// "Constrain to edge", a held part says where it is held and offers
/// "Free part", a locked part says it is locked; the canvas glyph sits
/// beside the words.
#[test]
fn the_placement_card_states_the_hold_at_once() -> Fallible {
    let (mut engine, r1, r2) = held_board()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    for (part, words, action) in [
        (r1, "Held 5 mm from the right edge", Some("Free part")),
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
    // Free part: unlock R2, and the card offers to hold it to its nearest
    // edge, in place, as one board.constrain.
    engine.apply(
        engine.revision(),
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
    let op = click(&mut editor, &engine, &ctx, "Constrain to edge")?;
    let Some(op @ Operation::Board(BoardCommand::Constrain { .. })) = op else {
        return Err(format!("no board.constrain: {op:?}").into());
    };
    let before = engine.project().placement_of(r2).cloned();
    engine.apply(engine.revision(), op)?;
    assert_eq!(
        engine.project().placement_of(r2).cloned(),
        before,
        "in place"
    );
    let (output, _) = short_card(&mut editor, &engine, &ctx, vec![]);
    let shown = visible_texts(&output);
    assert!(
        shown.iter().any(|(t, _)| t.starts_with("Held ")),
        "{shown:?}"
    );
    // "Free part" removes the constraint as one step.
    let op = click(&mut editor, &engine, &ctx, "Free part")?;
    let Some(op @ Operation::Board(BoardCommand::DeleteConstraint { .. })) = op else {
        return Err(format!("no board.constraint_delete: {op:?}").into());
    };
    engine.apply(engine.revision(), op)?;
    assert!(super::edge_card::constraint_of(engine.project(), r2).is_none());
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
    assert!(
        reason.to_lowercase().contains("track copper lies outside"),
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
    freed.apply(
        freed.revision(),
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
