//! The start-page entry opens the form; progress and the result
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
    pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
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
        "Provider",
        "Endpoint",
        "Model",
        "API key",
        "Start",
    ] {
        assert!(
            painted.iter().any(|(t, _)| t == label),
            "{label}: {painted:?}"
        );
    }
    // No prices in the product: no cost cap, no spend.
    assert!(
        !painted
            .iter()
            .any(|(t, _)| t.contains("Cost") || t.contains('$') || t.contains("USD")),
        "{painted:?}"
    );
    // The MCP workflow is offered beside the built-in run.
    assert!(
        painted
            .iter()
            .any(|(t, _)| t.contains("datasheet_to_component")
                && t.contains("Connect AI assistants")),
        "{painted:?}"
    );
    Ok(())
}

/// Demo run 3: the form opens below a long start page; opening it scrolls
/// it into view, its heading carries no count, and its last rows clear the
/// canvas's floating navigation bar.
#[test]
fn the_form_opens_in_view() -> Result<(), String> {
    let ctx = egui::Context::default();
    pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
    let mut assist = DatasheetAssist::default();
    let view = 500.0;
    let page = |ui: &mut Ui, assist: &mut DatasheetAssist| {
        egui::ScrollArea::vertical()
            .id_salt("start-page-test")
            .max_height(view)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // The search and recent lists above it.
                ui.add_space(380.0);
                draw(ui, assist);
                ui.add_space(crate::navigation::bar_band());
            });
    };
    let painted = frame(&ctx, vec![], |ui| page(ui, &mut assist));
    let button = painted
        .iter()
        .find(|(t, r)| t == "Create from datasheet…" && r.bottom() < view)
        .map(|(_, r)| r.center());
    let at = button.ok_or("the button is in view, near the bottom")?;
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
        frame(&ctx, events, |ui| page(ui, &mut assist));
    }
    assert!(assist.open);
    let mut painted = Vec::new();
    // The scroll eases in over a few frames.
    for _ in 0..60 {
        painted = frame(&ctx, vec![], |ui| page(ui, &mut assist));
    }
    let heading = painted
        .iter()
        .find(|(t, _)| t.starts_with("CREATE FROM DATASHEET"))
        .ok_or_else(|| format!("the heading: {painted:?}"))?;
    assert_eq!(heading.0, "CREATE FROM DATASHEET", "no count");
    assert!(
        // Unscrolled it would sit near the bottom, past 400.
        heading.1.top() >= 0.0 && heading.1.top() < view / 2.0,
        "scrolled to the form's heading: {heading:?}"
    );
    let key = painted
        .iter()
        .find(|(t, _)| t == "API key")
        .ok_or_else(|| format!("the key row: {painted:?}"))?;
    assert!(key.1.bottom() <= view, "the key row is in view: {key:?}");
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
        Message::Done(Ok(Finished {
            stop: "review report submitted".into(),
            output_tokens: 1000,
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
    assert!(
        matches!(&assist.result, Some(Ok(f)) if f.review.as_path() == std::path::Path::new("x.review.md"))
    );
}

#[test]
fn an_openai_compatible_model_is_declared_not_detected() -> Result<(), String> {
    let ctx = egui::Context::default();
    pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
    let mut assist = DatasheetAssist {
        open: true,
        provider: Provider::OpenAiCompatible,
        endpoint: "http://localhost:11434/v1".into(),
        model: "qwen3.8:27b".into(),
        ..DatasheetAssist::default()
    };
    let painted = frame(&ctx, vec![], |ui| assist.ui(ui));
    for label in [
        "Context",
        "Takes images (vision)",
        "Reads PDFs itself",
        "API key",
    ] {
        assert!(
            painted.iter().any(|(t, _)| t == label),
            "{label}: {painted:?}"
        );
    }
    assist.vision = true;
    assist.context_tokens = 65_536.0;
    assist.datasheets = "a.pdf".into();
    assist.name = "X".into();
    let plan = assist.plan()?;
    let p = &plan.config.profile;
    assert_eq!(p.provider, Provider::OpenAiCompatible);
    assert_eq!(p.capabilities.context_tokens, 65_536);
    assert!(p.capabilities.vision && !p.capabilities.native_pdf);
    assert!(plan.endpoint.is_loopback());
    assert!(plan.key.is_none());
    Ok(())
}

#[test]
fn a_key_for_plain_http_to_another_computer_is_refused_and_never_kept() {
    let mut assist = DatasheetAssist {
        open: true,
        provider: Provider::OpenAiCompatible,
        endpoint: "http://192.168.1.20:11434/v1".into(),
        model: "m".into(),
        key: "sk-typed".into(),
        datasheets: "a.pdf".into(),
        name: "X".into(),
        out: "x.pcbcraft".into(),
        ..DatasheetAssist::default()
    };
    assist.start();
    assert!(assist.key.is_empty(), "the key leaves the form");
    assert!(assist.job.is_none(), "nothing started");
    assert!(
        matches!(&assist.result, Some(Err(why)) if why.contains("plain HTTP") && !why.contains("sk-typed")),
        "{:?}",
        assist.result
    );
}
