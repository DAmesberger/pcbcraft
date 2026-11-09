//! The founder's hook: a route into a fine-pitch USB-C pin through a
//! corner on the 0.5 mm grid. The pins sit at 0.25 mm off the grid, so
//! the corner click lands past the pin's column; the step from there used
//! to run back into the pin (a 45° hook flipped, a knee past the pin
//! unflipped). The step now moves the corner back along the incoming
//! track and enters the pin straight. Driven through the Layout canvas on
//! the shipped ESP32 board, whose J1 is the USB-C receptacle template.
use super::route_tests::{Canvas, TestResult, close, edits};
use super::*;
use pcbcraft_core::layout::RouteEditSet;

const R5_1: Point = [24.0, 10.85];
const B5: Point = [21.75, 7.355];

/// The turn at each corner of `points`, degrees.
fn turns(points: &[Point]) -> Vec<f64> {
    points
        .windows(3)
        .map(|w| {
            let u = [w[1][0] - w[0][0], w[1][1] - w[0][1]];
            let v = [w[2][0] - w[1][0], w[2][1] - w[1][1]];
            let cos = (u[0] * v[0] + u[1] * v[1]) / (u[0].hypot(u[1]) * v[0].hypot(v[1]));
            cos.clamp(-1.0, 1.0).acos().to_degrees()
        })
        .collect()
}

/// The tracks of `net` followed end to end from `from`.
fn polyline(project: &Project, net: Id, from: Point) -> Vec<Point> {
    let mut used = vec![false; project.tracks.len()];
    let mut points = vec![from];
    while let Some(at) = points.last().copied() {
        let next = project.tracks.iter().enumerate().find_map(|(i, t)| {
            if used.get(i).copied().unwrap_or(true) || t.net != net {
                None
            } else if close(t.start, at) {
                Some((i, t.end))
            } else if close(t.end, at) {
                Some((i, t.start))
            } else {
                None
            }
        });
        let Some((i, end)) = next else { break };
        if let Some(u) = used.get_mut(i) {
            *u = true;
        }
        points.push(end);
    }
    points
}

/// The ESP32 board with CC2 (R5 pad 1 to J1 B5) unrouted, and its net.
fn unrouted_cc2() -> Result<(Engine, Id), Box<dyn std::error::Error>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft"
    );
    let mut engine = Engine::demo()?;
    engine.replace_document(pcbcraft_io::read_project(std::path::Path::new(path))?)?;
    let net = engine
        .project()
        .nets
        .iter()
        .find(|n| n.name == "CC2")
        .map(|n| n.id)
        .ok_or("CC2")?;
    let remove = engine
        .project()
        .tracks
        .iter()
        .filter(|t| t.net == net)
        .map(|t| t.id)
        .collect();
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::ApplyRoute {
            edits: RouteEditSet {
                remove,
                ..Default::default()
            },
        }),
    )?;
    Ok((engine, net))
}

/// R5 pad 1, a corner click past B5's column (it snaps to [21.5, 9.5]),
/// then B5, with the last step's elbow flipped or not. Each way the route
/// enters B5 straight up from the corner, moved back to the pin's column:
/// no leg runs back and no corner turns by more than 90°.
#[test]
fn a_route_into_a_usb_c_pin_past_its_column_enters_it_straight() -> TestResult {
    for flip_last in [true, false] {
        let (mut engine, net) = unrouted_cc2()?;
        let mut canvas = Canvas::new();
        let mut editor = PcbEditor::default();
        editor.on_tool("route", &engine);
        canvas.frame(&mut editor, &engine, vec![], egui::Modifiers::NONE);
        // Diagonal first, so the first step ends running left along y 9.5.
        canvas.key(
            &mut editor,
            &engine,
            R5_1,
            egui::Key::F,
            egui::Modifiers::NONE,
        );
        assert!(canvas.click(&mut editor, &engine, R5_1).is_none());
        assert_eq!(editor.net, Some(net));
        let corner = canvas.click(&mut editor, &engine, [21.6, 9.45]);
        let legs = &edits(&corner)?.add;
        // (The step shoves the VBUS via beside it; its copper follows.)
        let end = legs
            .iter()
            .rfind(|r| r.net == net)
            .map(|r| r.end)
            .ok_or("no legs")?;
        assert!(close(end, [21.5, 9.5]), "{legs:?}");
        canvas.apply(&mut editor, &mut engine, corner)?;
        if !flip_last {
            canvas.key(
                &mut editor,
                &engine,
                B5,
                egui::Key::F,
                egui::Modifiers::NONE,
            );
        }
        let last = canvas.click(&mut editor, &engine, [21.75, 7.1]);
        let step = edits(&last)?;
        // The incoming track is shortened to the pin's column.
        let shortened: Vec<_> = step.replace.iter().filter(|t| t.net == net).collect();
        assert_eq!(shortened.len(), 1, "{step:?}");
        assert!(close(shortened[0].end, [21.75, 9.5]), "{step:?}");
        canvas.apply(&mut editor, &mut engine, last)?;
        assert_eq!(editor.route_start, None, "finished on the pin");
        let points = polyline(engine.project(), net, R5_1);
        assert!(close(*points.last().ok_or("no route")?, B5), "{points:?}");
        assert_eq!(points.len(), 4, "{points:?}");
        assert!(close(points[2], [21.75, 9.5]), "{points:?}");
        assert!(
            turns(&points).iter().all(|t| *t <= 90.0 + 1e-6),
            "flip {flip_last}: {points:?}"
        );
    }
    Ok(())
}

/// A step back past the head's corner moves the corner instead, in one
/// undo step that Backspace also takes back whole: the shortened track
/// gets its old end again.
#[test]
fn backspace_restores_a_moved_corner() -> TestResult {
    let mut engine = Engine::demo()?;
    let mut canvas = Canvas::new();
    let mut editor = PcbEditor::default();
    editor.on_tool("route", &engine);
    canvas.frame(&mut editor, &engine, vec![], egui::Modifiers::NONE);
    let r1_2 = [8.8, 8.0];
    assert!(canvas.click(&mut editor, &engine, r1_2).is_none());
    let first = canvas.click(&mut editor, &engine, [11.0, 8.0]);
    canvas.apply(&mut editor, &mut engine, first)?;
    let after_first = engine.project().tracks.clone();
    // Down and back left: the corner moves back to x 10, then straight down.
    let second = canvas.click(&mut editor, &engine, [10.0, 10.0]);
    let step = edits(&second)?;
    assert_eq!(step.replace.len(), 1, "{step:?}");
    assert!(close(step.replace[0].end, [10.0, 8.0]), "{step:?}");
    let legs: Vec<_> = step.add.iter().map(|r| (r.start, r.end)).collect();
    assert_eq!(legs.len(), 1, "{legs:?}");
    assert!(close(legs[0].0, [10.0, 8.0]) && close(legs[0].1, [10.0, 10.0]));
    canvas.apply(&mut editor, &mut engine, second)?;
    assert_eq!(editor.route_start, Some([10.0, 10.0]));
    let unwind = canvas.key(
        &mut editor,
        &engine,
        [10.0, 10.0],
        egui::Key::Backspace,
        egui::Modifiers::NONE,
    );
    canvas.apply(&mut editor, &mut engine, unwind)?;
    assert_eq!(engine.project().tracks, after_first);
    assert_eq!(editor.route_start, Some([11.0, 8.0]));
    Ok(())
}
