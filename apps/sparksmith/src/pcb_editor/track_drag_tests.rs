//! Dragging a track with Select through the Layout canvas: the other net's
//! track is shoved live during the drag and springs back, release sends one
//! `layout.route_apply` that one undo takes back, Esc cancels, and a drag
//! that cannot move says why on release.
use super::route_tests::{Canvas, TestResult, edits, screen};
use super::*;
use sparksmith_core::layout::RouteRequest;

/// The demo with a 45°-straight-45° run of net 1 and a net 4 track 2 mm
/// above its straight part.
fn setup() -> Result<(Engine, Id, Id), Box<dyn std::error::Error>> {
    let mut engine = Engine::demo()?;
    let mut ids = Vec::new();
    for (net, start, end) in [
        (1, [20.0, 20.0], [25.0, 25.0]),
        (1, [25.0, 25.0], [35.0, 25.0]),
        (1, [35.0, 25.0], [40.0, 20.0]),
        (4, [18.0, 27.0], [42.0, 27.0]),
    ] {
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Route {
                request: RouteRequest {
                    net,
                    start,
                    end,
                    width: 0.3,
                    layer: CopperLayer::Top,
                },
            }),
        )?;
        ids.push(
            engine
                .project()
                .tracks
                .last()
                .map(|t| t.id)
                .ok_or("track")?,
        );
    }
    Ok((engine, ids[1], ids[3]))
}

fn moved(editor: &PcbEditor, engine: &Engine, at: Point) -> egui::Event {
    egui::Event::PointerMoved(screen(editor, engine, at))
}

fn button(editor: &PcbEditor, engine: &Engine, at: Point, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: screen(editor, engine, at),
        button: PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

/// One frame with `event`.
fn send(
    canvas: &mut Canvas,
    editor: &mut PcbEditor,
    engine: &Engine,
    event: egui::Event,
) -> Option<Operation> {
    canvas.frame(editor, engine, vec![event], egui::Modifiers::NONE)
}

/// Press on the straight run and move the pointer to `to`, frame by frame.
fn press_and_move(canvas: &mut Canvas, editor: &mut PcbEditor, engine: &Engine, to: &[Point]) {
    let start = [30.0, 25.0];
    let none = egui::Modifiers::NONE;
    canvas.frame(editor, engine, vec![moved(editor, engine, start)], none);
    canvas.frame(
        editor,
        engine,
        vec![button(editor, engine, start, true)],
        none,
    );
    for at in to {
        canvas.frame(editor, engine, vec![moved(editor, engine, *at)], none);
    }
}

#[test]
fn a_track_drag_shoves_live_springs_back_and_commits_once() -> TestResult {
    let (mut engine, middle, other) = setup()?;
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    canvas.frame(&mut editor, &engine, vec![], egui::Modifiers::NONE);
    let before = engine.project().tracks.clone();
    let revision = engine.revision();
    press_and_move(
        &mut canvas,
        &mut editor,
        &engine,
        &[[30.0, 25.5], [30.0, 26.0], [30.0, 27.0]],
    );
    assert!(editor.track_drag.is_some(), "the track drags");
    assert!(editor.shoved.0.contains(&middle), "drawn moved only");
    assert!(editor.shoved.0.contains(&other), "shoved live");
    assert_eq!(engine.revision(), revision, "the preview changes nothing");
    // Back near where it was: the other track springs back.
    let event = moved(&editor, &engine, [30.0, 25.3]);
    send(&mut canvas, &mut editor, &engine, event);
    assert!(!editor.shoved.0.contains(&other), "springs back");
    let event = moved(&editor, &engine, [30.0, 27.0]);
    send(&mut canvas, &mut editor, &engine, event);
    let event = button(&editor, &engine, [30.0, 27.0], false);
    let release = send(&mut canvas, &mut editor, &engine, event);
    let sent = edits(&release)?;
    assert!(sent.replace.iter().any(|t| t.id == other), "{sent:?}");
    canvas.apply(&mut editor, &mut engine, release)?;
    assert_eq!(engine.revision(), revision + 1, "one transaction");
    let dragged = engine
        .project()
        .tracks
        .iter()
        .find(|t| t.id == middle)
        .ok_or("kept its identity")?;
    assert!((dragged.start[1] - 27.0).abs() < 1e-6, "{dragged:?}");
    engine.undo()?;
    let mut now = engine.project().tracks.clone();
    let mut was = before;
    now.sort_by_key(|t| t.id);
    was.sort_by_key(|t| t.id);
    assert_eq!(now, was, "one undo puts everything back");
    Ok(())
}

#[test]
fn esc_cancels_a_track_drag() -> TestResult {
    let (engine, _, other) = setup()?;
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    canvas.frame(&mut editor, &engine, vec![], egui::Modifiers::NONE);
    press_and_move(
        &mut canvas,
        &mut editor,
        &engine,
        &[[30.0, 26.0], [30.0, 27.0]],
    );
    assert!(editor.shoved.0.contains(&other));
    canvas.key(
        &mut editor,
        &engine,
        [30.0, 27.0],
        egui::Key::Escape,
        egui::Modifiers::NONE,
    );
    assert!(editor.track_drag.is_none(), "cancelled");
    assert!(editor.shoved.0.is_empty(), "nothing drawn moved");
    let event = button(&editor, &engine, [30.0, 27.0], false);
    let release = send(&mut canvas, &mut editor, &engine, event);
    assert!(release.is_none(), "release after Esc sends nothing");
    Ok(())
}

/// A drag of a track that cannot move (here a locked one) sends nothing
/// on release and says why in a toast, not silently.
#[test]
fn a_track_drag_that_cannot_move_says_why() -> TestResult {
    let (mut engine, middle, _) = setup()?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::SetTrackLock {
            id: middle,
            locked: true,
        }),
    )?;
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    canvas.frame(&mut editor, &engine, vec![], egui::Modifiers::NONE);
    let _ = crate::kit::toast::take_posted(canvas.ctx());
    press_and_move(
        &mut canvas,
        &mut editor,
        &engine,
        &[[30.0, 26.0], [30.0, 27.0]],
    );
    assert!(editor.track_drag.is_some(), "the press grips the track");
    let event = button(&editor, &engine, [30.0, 27.0], false);
    let release = send(&mut canvas, &mut editor, &engine, event);
    assert!(release.is_none(), "{release:?}");
    let posted = crate::kit::toast::take_posted(canvas.ctx());
    let texts: Vec<&str> = posted.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, ["Can't drag the track: The track is locked"]);
    Ok(())
}

/// Night finding F7: on the ESP32 board, dragging USB_DN's long run moves
/// USB_DP's run beside it too, live in the preview (both drawn moved) and
/// in the one `layout.route_apply` release sends.
#[test]
fn dragging_one_half_of_the_usb_pair_previews_and_moves_both() -> TestResult {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/esp32-s31-devboard/esp32-s31-devboard.sparksmith"
    );
    let mut engine = Engine::demo()?;
    engine.replace_document(sparksmith_io::read_project(std::path::Path::new(path))?)?;
    // Each half's long run below U3: the longest track on its net.
    let run = |name: &str| {
        let p = engine.project();
        let net = p.nets.iter().find(|n| n.name == name).ok_or("no net")?;
        let length = |t: &&sparksmith_core::model::Track| {
            (t.end[0] - t.start[0]).hypot(t.end[1] - t.start[1])
        };
        p.tracks
            .iter()
            .filter(|t| t.net == net.id)
            .max_by(|a, b| length(a).total_cmp(&length(b)))
            .cloned()
            .ok_or("no run")
    };
    let (dn_run, dp) = (run("USB_DN")?, run("USB_DP")?.id);
    let (dn, x, y) = (dn_run.id, dn_run.start[0], 21.0);
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    let none = egui::Modifiers::NONE;
    canvas.frame(&mut editor, &engine, vec![], none);
    let start = [x, y];
    let hover = moved(&editor, &engine, start);
    canvas.frame(&mut editor, &engine, vec![hover], none);
    let press = button(&editor, &engine, start, true);
    canvas.frame(&mut editor, &engine, vec![press], none);
    for dx in [0.29, 0.79, 1.0] {
        let event = moved(&editor, &engine, [x - dx, y]);
        send(&mut canvas, &mut editor, &engine, event);
    }
    assert_eq!(editor.track_drag.as_ref().map(|d| d.track), Some(dn));
    assert!(editor.shoved.0.contains(&dn), "{:?}", editor.shoved.0);
    assert!(editor.shoved.0.contains(&dp), "{:?}", editor.shoved.0);
    let event = button(&editor, &engine, [x - 1.0, y], false);
    let release = send(&mut canvas, &mut editor, &engine, event);
    let sent = edits(&release)?;
    for id in [dn, dp] {
        assert!(sent.replace.iter().any(|t| t.id == id), "{sent:?}");
    }
    Ok(())
}

/// ⌥-drag on USB_DN's long run moves that half alone: USB_DP is neither
/// drawn moved nor in the one `layout.route_apply` release sends, and the
/// canvas hint names ⌥ during the drag.
#[test]
fn an_option_drag_moves_one_pair_half_alone() -> TestResult {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/esp32-s31-devboard/esp32-s31-devboard.sparksmith"
    );
    let mut engine = Engine::demo()?;
    engine.replace_document(sparksmith_io::read_project(std::path::Path::new(path))?)?;
    let run = |name: &str| {
        let p = engine.project();
        let net = p.nets.iter().find(|n| n.name == name).ok_or("no net")?;
        let length = |t: &&sparksmith_core::model::Track| {
            (t.end[0] - t.start[0]).hypot(t.end[1] - t.start[1])
        };
        p.tracks
            .iter()
            .filter(|t| t.net == net.id)
            .max_by(|a, b| length(a).total_cmp(&length(b)))
            .cloned()
            .ok_or("no run")
    };
    let (dn_run, dp) = (run("USB_DN")?, run("USB_DP")?.id);
    let (dn, x, y) = (dn_run.id, dn_run.start[0], 21.0);
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    let (none, alt) = (egui::Modifiers::NONE, egui::Modifiers::ALT);
    canvas.frame(&mut editor, &engine, vec![], none);
    let start = [x, y];
    let hover = moved(&editor, &engine, start);
    canvas.frame(&mut editor, &engine, vec![hover], none);
    let press = button(&editor, &engine, start, true);
    canvas.frame(&mut editor, &engine, vec![press], none);
    for dx in [0.29, 0.79, 1.0] {
        let event = moved(&editor, &engine, [x - dx, y]);
        canvas.frame(&mut editor, &engine, vec![event], alt);
    }
    assert!(editor.dragging_pair_half(engine.project()));
    assert!(editor.shoved.0.contains(&dn), "{:?}", editor.shoved.0);
    assert!(!editor.shoved.0.contains(&dp), "{:?}", editor.shoved.0);
    let event = button(&editor, &engine, [x - 1.0, y], false);
    let release = canvas.frame(&mut editor, &engine, vec![event], alt);
    let sent = edits(&release)?;
    assert!(sent.replace.iter().any(|t| t.id == dn), "{sent:?}");
    assert!(!sent.replace.iter().any(|t| t.id == dp), "{sent:?}");
    Ok(())
}
