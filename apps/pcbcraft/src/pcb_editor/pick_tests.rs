//! Pointing at the ESP32 board: a click a few pixels beside a thin track
//! on the active layer picks the track, not a plane on an inner layer
//! under it, Esc clears the selection, and the Tune readout at the
//! pointer stays inside the canvas.
use super::via_tests::{SCREEN, click, frame, key, screen};
use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn esp32() -> Result<Engine, Box<dyn std::error::Error>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft"
    );
    let mut engine = Engine::demo()?;
    engine.replace_document(pcbcraft_io::read_project(std::path::Path::new(path))?)?;
    Ok(engine)
}

/// A point `px` screen pixels beside the copper edge of a Top USB_DN track,
/// over a fill on another layer and clear of every other pick, and the
/// track.
fn beside_a_usb_track(
    editor: &PcbEditor,
    engine: &Engine,
    px: f64,
) -> Result<(Point, Id), Box<dyn std::error::Error>> {
    let project = engine.project();
    let net = project
        .nets
        .iter()
        .find(|n| n.name == "USB_DN")
        .map(|n| n.id)
        .ok_or("USB_DN")?;
    let scale =
        f64::from(screen(editor, engine, [1.0, 0.0]).x - screen(editor, engine, [0.0, 0.0]).x)
            .abs();
    let layers = editor.seen();
    for track in project
        .tracks
        .iter()
        .filter(|t| t.net == net && t.layer == CopperLayer::Top)
    {
        let (dx, dy) = (track.end[0] - track.start[0], track.end[1] - track.start[1]);
        let length = dx.hypot(dy);
        if length * scale < 40.0 {
            continue;
        }
        let normal = [-dy / length, dx / length];
        let mid = [
            (track.start[0] + track.end[0]) / 2.0,
            (track.start[1] + track.end[1]) / 2.0,
        ];
        for side in [1.0, -1.0] {
            let off = track.width / 2.0 + px / scale;
            let at = [
                mid[0] + side * normal[0] * off,
                mid[1] + side * normal[1] * off,
            ];
            let fill = editor.pick_fill(project, at, 0.0);
            let shown = screen(editor, engine, at);
            let on_canvas = (20.0..SCREEN[0] - 20.0).contains(&shown.x)
                && (20.0..SCREEN[1] - 20.0).contains(&shown.y);
            let clear = on_canvas
                && editor.pick_pad(at, |p| layers.pad(p)).is_none()
                && editor.pick_via(project, at).is_none()
                && !project
                    .tracks
                    .iter()
                    .any(|t| t.id != track.id && layout::track_contains(t, at, 6.0 / scale));
            if clear && fill.is_some_and(|f| f.layer != CopperLayer::Top) {
                return Ok((at, track.id));
            }
        }
    }
    Err("no USB_DN track beside an inner-layer fill".into())
}

/// N1: with Top active, a click 3 px beside a Top track selects the track,
/// not the GND plane on Inner 1 under it; Esc then clears the selection.
/// Zoomed in twice: the USB pair's gap under the board's JLCPCB rules is
/// within the test's 6 px pick margin at the fitted zoom.
#[test]
fn a_click_just_beside_a_top_track_selects_it_and_esc_clears() -> TestResult {
    let engine = esp32()?;
    let ctx = egui::Context::default();
    let mut editor = PcbEditor::default();
    frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(editor.layer, CopperLayer::Top);
    editor.zoom *= 2.0;
    frame(&mut editor, &engine, &ctx, vec![]);
    let (at, track) = beside_a_usb_track(&editor, &engine, 3.0)?;
    let point = screen(&editor, &engine, at);
    frame(&mut editor, &engine, &ctx, click(point));
    assert_eq!(editor.selected_track(), Some(track));
    assert_eq!(editor.fill, None);
    frame(&mut editor, &engine, &ctx, key(point, egui::Key::Escape));
    assert_eq!(editor.selected_track(), None);
    assert_eq!(editor.highlighted_net(), None);
    // Esc clears a fill selection too.
    let fill = engine.project().fills.first().ok_or("a fill")?;
    editor.select_fill(fill.id, fill.net);
    frame(&mut editor, &engine, &ctx, key(point, egui::Key::Escape));
    assert_eq!(editor.fill, None);
    Ok(())
}

/// The Tune tool's length readout at the pointer sits inside the canvas:
/// near the right edge it is not cut off.
#[test]
fn the_tune_readout_stays_inside_the_canvas() -> TestResult {
    let engine = esp32()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let project = engine.project();
    let net = project
        .nets
        .iter()
        .find(|n| n.name == "USB_DN")
        .map(|n| n.id)
        .ok_or("USB_DN")?;
    let track = project
        .tracks
        .iter()
        .filter(|t| t.net == net && t.layer == CopperLayer::Top)
        .max_by(|a, b| {
            let l = |t: &pcbcraft_core::Track| pcbcraft_core::geometry::distance(t.start, t.end);
            l(a).total_cmp(&l(b))
        })
        .ok_or("a USB_DN track")?;
    let mid = [
        (track.start[0] + track.end[0]) / 2.0,
        (track.start[1] + track.end[1]) / 2.0,
    ];
    let mut editor = PcbEditor {
        tool: Tool::Tune,
        zoom: 4.0,
        ..Default::default()
    };
    frame(&mut editor, &engine, &ctx, vec![]);
    let at = Pos2::new(SCREEN[0] - 40.0, 300.0);
    editor.pan += at - screen(&editor, &engine, mid);
    frame(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(at)],
    );
    let (output, _) = frame(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(at)],
    );
    let readout = output
        .shapes
        .iter()
        .find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.job.text.starts_with("USB_DN ") => {
                Some(Rect::from_min_size(t.pos, t.galley.size()))
            }
            _ => None,
        })
        .ok_or("no readout")?;
    assert!(
        readout.right() <= SCREEN[0] && readout.left() >= 0.0,
        "readout at {readout:?}"
    );
    Ok(())
}

/// Night round 3 (R3-5): a Tune click on a net already at its target sends
/// nothing and says so as information, not as a red refusal.
#[test]
fn tuning_a_net_at_its_target_is_information() -> TestResult {
    let engine = esp32()?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor {
        tool: Tool::Tune,
        zoom: 4.0,
        ..Default::default()
    };
    let project = engine.project();
    let net = project
        .nets
        .iter()
        .find(|n| n.name == "USB_DN")
        .map(|n| n.id)
        .ok_or("USB_DN")?;
    let track = project
        .tracks
        .iter()
        .filter(|t| t.net == net && t.layer == CopperLayer::Top)
        .max_by(|a, b| {
            let l = |t: &pcbcraft_core::Track| pcbcraft_core::geometry::distance(t.start, t.end);
            l(a).total_cmp(&l(b))
        })
        .ok_or("a USB_DN track")?;
    let mid = [
        (track.start[0] + track.end[0]) / 2.0,
        (track.start[1] + track.end[1]) / 2.0,
    ];
    editor.tune.target = Some(layout::length::signal_length(project, net));
    frame(&mut editor, &engine, &ctx, vec![]);
    let point = Pos2::new(400.0, 300.0);
    editor.pan += point - screen(&editor, &engine, mid);
    frame(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::PointerMoved(point)],
    );
    let _ = crate::kit::toast::take_posted(&ctx);
    let (_, operation) = frame(&mut editor, &engine, &ctx, click(point));
    assert!(operation.is_none(), "{operation:?}");
    let posted = crate::kit::toast::take_posted(&ctx);
    let said: Vec<_> = posted.iter().map(|p| (p.level, p.text.as_str())).collect();
    assert_eq!(
        said,
        [(
            crate::kit::severity::Level::Info,
            "USB_DN is already at or past its target"
        )]
    );
    Ok(())
}
