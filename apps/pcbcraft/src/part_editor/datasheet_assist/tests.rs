//! The start-page entry opens the form; progress, spend and the result
//! arrive from the background thread.
use super::*;

/// Run one headless frame drawing `add`; the painted texts.
fn frame(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    mut add: impl FnMut(&mut Ui),
) -> Vec<(String, egui::Rect)> {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 900.0),
            )),
            events,
            ..Default::default()
        },
        |ui| add(ui),
    );
    output.textures_delta.clear();
    fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(t) => out.push((
                t.galley.text().to_owned(),
                t.galley.rect.translate(t.pos.to_vec2()),
            )),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for s in &output.shapes {
        walk(&s.shape, &mut out);
    }
    out
}

fn draw(ui: &mut Ui, assist: &mut DatasheetAssist) {
    super::super::start::start_new_for_tests(ui, assist);
    assist.ui(ui);
}

#[test]
fn the_start_page_button_opens_the_form() -> Result<(), String> {
    let ctx = egui::Context::default();
    solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
    let mut assist = DatasheetAssist::default();
    let painted = frame(&ctx, vec![], |ui| draw(ui, &mut assist));
    let (_, rect) = painted
        .iter()
        .find(|(t, _)| t == "Create from datasheet…")
        .ok_or_else(|| format!("the button: {painted:?}"))?;
    assert!(
        !painted.iter().any(|(t, _)| t == "Datasheets"),
        "closed at first"
    );
    let at = rect.center();
    let press = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![press(true)],
        vec![press(false)],
    ] {
        frame(&ctx, events, |ui| draw(ui, &mut assist));
    }
    assert!(assist.open);
    let painted = frame(&ctx, vec![], |ui| draw(ui, &mut assist));
    for label in [
        "Datasheets",
        "Part name",
        "Save as",
        "Model",
        "Cost cap",
        "Start",
    ] {
        assert!(
            painted.iter().any(|(t, _)| t == label),
            "{label}: {painted:?}"
        );
    }
    Ok(())
}

#[test]
fn progress_and_the_result_arrive_from_the_thread() {
    let (tx, rx) = channel();
    let mut assist = DatasheetAssist {
        open: true,
        job: Some(Job {
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
        }),
        ..DatasheetAssist::default()
    };
    let sent = [
        Message::Line("Request 1".into()),
        Message::Spent(0.42),
        Message::Done(Ok(Finished {
            stop: "review report submitted".into(),
            spent_usd: 0.42,
            project: PathBuf::from("x.pcbcraft"),
            review: PathBuf::from("x.review.md"),
        })),
    ];
    for message in sent {
        assert!(tx.send(message).is_ok());
    }
    assist.poll();
    assert!(assist.job.is_none(), "the run is over");
    assert_eq!(assist.log, ["Request 1"]);
    assert!((assist.spent - 0.42).abs() < 1e-12);
    assert!(matches!(&assist.result, Some(Ok(f)) if f.review == PathBuf::from("x.review.md")));
}
