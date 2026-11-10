//! Select clicks on the ESP32 board: a click a few pixels beside a thin
//! track on the active layer picks the track, not a plane on an inner
//! layer under it, and Esc clears the selection.
use super::via_tests::{click, frame, key, screen};
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
            let clear = editor.pick_pad(at, |p| layers.pad(p)).is_none()
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
#[test]
fn a_click_just_beside_a_top_track_selects_it_and_esc_clears() -> TestResult {
    let engine = esp32()?;
    let ctx = egui::Context::default();
    let mut editor = PcbEditor::default();
    frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(editor.layer, CopperLayer::Top);
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
