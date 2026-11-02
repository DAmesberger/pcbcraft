//! The router modes through the Layout canvas: M cycles Shove, Walkaround
//! and Ignore; a Shove click sends one `layout.route_apply` that moves the
//! other net's track (the preview draws it at its new place only);
//! Backspace puts it back; Walkaround stops short of a target inside the
//! clearance and Ignore is refused by the engine. Every operation the
//! editor sends is applied through the engine, as the app does.
use super::route_tests::{Canvas, TestResult, close, edits};
use super::*;
use pcbcraft_core::Track;
use pcbcraft_core::layout::RouteRequest;
use pcbcraft_core::layout::router::RouteMode;

/// R1 pad 2, on the demo's net 2.
const R1_2: Point = [8.8, 8.0];
/// The demo's net 4.
const OTHER: Id = 4;

/// The demo with a track of net 4 above R1 and R2, and the Route tool
/// started on R1 pad 2.
fn setup(
    canvas: &mut Canvas,
    editor: &mut PcbEditor,
) -> Result<(Engine, Track), Box<dyn std::error::Error>> {
    let mut engine = Engine::demo()?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net: OTHER,
                start: [10.0, 10.8],
                end: [14.0, 10.8],
                width: 0.3,
                layer: CopperLayer::Top,
            },
        }),
    )?;
    let track = engine.project().tracks.last().cloned().ok_or("no track")?;
    editor.on_tool("route", &engine);
    canvas.frame(editor, &engine, vec![], egui::Modifiers::NONE);
    assert!(canvas.click(editor, &engine, R1_2).is_none());
    if editor.route_start.is_none_or(|head| !close(head, R1_2)) {
        return Err("the route did not start on R1 pad 2".into());
    }
    Ok((engine, track))
}

/// M steps Shove → Walkaround → Ignore → Shove; Shove is the default and
/// the key is listed with the Route keys.
#[test]
fn m_cycles_the_router_mode() -> TestResult {
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    let (engine, _) = setup(&mut canvas, &mut editor)?;
    assert_eq!(editor.route.mode, RouteMode::Shove);
    let mut seen = Vec::new();
    for _ in 0..3 {
        canvas.key(
            &mut editor,
            &engine,
            [12.0, 9.0],
            egui::Key::M,
            egui::Modifiers::NONE,
        );
        seen.push(editor.route.mode);
    }
    assert_eq!(
        seen,
        [RouteMode::Walkaround, RouteMode::Ignore, RouteMode::Shove]
    );
    assert!(ROUTE_KEYS.iter().any(|(key, _)| *key == "M"));
    Ok(())
}

/// A step whose elbow ends too close to the other net's track shoves it:
/// the click sends the track's replacement in the same `route_apply`, the
/// preview hides its old place, one undo puts everything back, and
/// Backspace does too.
#[test]
fn a_shove_click_moves_the_track_and_backspace_restores_it() -> TestResult {
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    let (mut engine, track) = setup(&mut canvas, &mut editor)?;
    let before = engine.project().tracks.clone();
    let step = canvas.click(&mut editor, &engine, [12.0, 10.5]);
    let sent = edits(&step)?;
    assert!(
        sent.replace.iter().any(|t| t.id == track.id),
        "the track is shoved: {sent:?}"
    );
    assert!(editor.shoved.0.contains(&track.id), "drawn moved only");
    canvas.apply(&mut editor, &mut engine, step)?;
    let moved = engine
        .project()
        .tracks
        .iter()
        .find(|t| t.id == track.id)
        .cloned()
        .ok_or("the track keeps its identity")?;
    assert_ne!(moved, track);
    assert!(close(
        editor.route_start.ok_or("route ended")?,
        [12.0, 10.5]
    ));
    let after = engine.project().tracks.clone();
    // Backspace takes the step back and puts the track where it was.
    let unwind = canvas.key(
        &mut editor,
        &engine,
        [12.0, 10.5],
        egui::Key::Backspace,
        egui::Modifiers::NONE,
    );
    assert!(edits(&unwind)?.replace.contains(&track));
    canvas.apply(&mut editor, &mut engine, unwind)?;
    let mut now = engine.project().tracks.clone();
    let mut was = before.clone();
    now.sort_by_key(|t| t.id);
    was.sort_by_key(|t| t.id);
    assert_eq!(now, was);
    assert!(close(editor.route_start.ok_or("route ended")?, R1_2));
    // Undo takes back the unwind, then the step with its shove: one undo
    // step each.
    engine.undo()?;
    assert_eq!(engine.project().tracks, after);
    engine.undo()?;
    assert_eq!(engine.project().tracks, before);
    Ok(())
}

/// Walkaround cannot reach a target inside the track's clearance: the
/// head stops at the last legal point and the route continues from there.
/// Ignore sends the plain elbow, which the engine refuses.
#[test]
fn walkaround_stops_short_and_ignore_is_refused() -> TestResult {
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    let (mut engine, track) = setup(&mut canvas, &mut editor)?;
    editor.route.mode = RouteMode::Walkaround;
    let step = canvas.click(&mut editor, &engine, [12.0, 10.5]);
    let sent = edits(&step)?;
    assert!(sent.replace.is_empty(), "nothing shoved");
    let end = sent.add.last().map(|r| r.end).ok_or("no legs")?;
    assert!(!close(end, [12.0, 10.5]), "stopped short at {end:?}");
    canvas.apply(&mut editor, &mut engine, step)?;
    assert!(close(editor.route_start.ok_or("route ended")?, end));
    assert!(engine.project().tracks.contains(&track));
    // From there straight on along the track, inside its clearance.
    editor.route.mode = RouteMode::Ignore;
    let step = canvas.click(&mut editor, &engine, [13.0, 10.5]);
    let plain = edits(&step)?.clone();
    assert!(plain.replace.is_empty());
    let revision = engine.revision();
    let refused = engine.apply(revision, step.ok_or("nothing sent")?);
    assert!(refused.is_err(), "the engine refuses the violation");
    assert_eq!(engine.revision(), revision);
    Ok(())
}
