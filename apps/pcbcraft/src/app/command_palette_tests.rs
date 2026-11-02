use super::*;

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn row(id: &str, enabled: bool) -> PaletteRow {
    PaletteRow {
        id: id.to_owned(),
        title: id.to_owned(),
        icon: "params",
        detail: String::new(),
        shortcut: None,
        enabled,
        recent: false,
    }
}

/// One headless frame of the palette over fixed rows; what it returned,
/// and the queries it ranked.
fn frame(
    ctx: &egui::Context,
    palette: &mut CommandPalette,
    rows: &[PaletteRow],
    events: Vec<egui::Event>,
) -> (Option<PaletteEvent>, Vec<String>) {
    let mut out = None;
    let mut queries = Vec::new();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            out = palette.show(ui.ctx(), |q| {
                queries.push(q.to_owned());
                rows.iter()
                    .filter(|r| r.id.contains(q.trim()))
                    .cloned()
                    .collect()
            });
        },
    );
    output.textures_delta.clear();
    (out, queries)
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
    ctx
}

/// ↓ and ↑ move over enabled rows only, wrapping; Enter runs the
/// highlighted row and closes the palette.
#[test]
fn arrows_skip_disabled_rows_and_enter_runs_the_highlighted_one() {
    let ctx = context();
    let rows = [row("alpha", true), row("beta", false), row("gamma", true)];
    let mut palette = CommandPalette::default();
    palette.open();
    frame(&ctx, &mut palette, &rows, vec![]);
    assert_eq!(palette.highlighted, 0);
    frame(&ctx, &mut palette, &rows, vec![key(egui::Key::ArrowDown)]);
    assert_eq!(palette.highlighted, 2, "beta is disabled");
    frame(&ctx, &mut palette, &rows, vec![key(egui::Key::ArrowDown)]);
    assert_eq!(palette.highlighted, 0, "wraps");
    frame(&ctx, &mut palette, &rows, vec![key(egui::Key::ArrowUp)]);
    assert_eq!(palette.highlighted, 2);
    let (event, _) = frame(&ctx, &mut palette, &rows, vec![key(egui::Key::Enter)]);
    assert_eq!(event, Some(PaletteEvent::Run("gamma".into())));
    assert!(!palette.is_open());
}

/// Typing filters through the caller's ranking; a disabled first row is
/// skipped, and Enter with only disabled rows runs nothing.
#[test]
fn typing_ranks_the_query_and_disabled_rows_never_run() {
    let ctx = context();
    let rows = [row("beta", false), row("alpha", true)];
    let mut palette = CommandPalette::default();
    palette.open();
    frame(&ctx, &mut palette, &rows, vec![]);
    let (_, queries) = frame(
        &ctx,
        &mut palette,
        &rows,
        vec![egui::Event::Text("bet".into())],
    );
    assert_eq!(queries.last().map(String::as_str), Some("bet"));
    let (event, _) = frame(&ctx, &mut palette, &rows, vec![key(egui::Key::Enter)]);
    assert_eq!(event, None, "the only match is disabled");
    assert!(palette.is_open());
    // Back to everything: the enabled row is highlighted.
    frame(
        &ctx,
        &mut palette,
        &rows,
        vec![
            key(egui::Key::Backspace),
            key(egui::Key::Backspace),
            key(egui::Key::Backspace),
        ],
    );
    assert_eq!(palette.highlighted, 1);
    let (event, _) = frame(&ctx, &mut palette, &rows, vec![key(egui::Key::Enter)]);
    assert_eq!(event, Some(PaletteEvent::Run("alpha".into())));
}

#[test]
fn escape_closes_without_running() {
    let ctx = context();
    let rows = [row("alpha", true)];
    let mut palette = CommandPalette::default();
    palette.open();
    frame(&ctx, &mut palette, &rows, vec![]);
    let (event, _) = frame(&ctx, &mut palette, &rows, vec![key(egui::Key::Escape)]);
    assert_eq!(event, None);
    assert!(!palette.is_open());
    // Closed, it draws and ranks nothing.
    let (_, queries) = frame(&ctx, &mut palette, &rows, vec![key(egui::Key::Enter)]);
    assert!(queries.is_empty());
}

#[test]
fn recent_commands_are_kept_most_recent_first_and_bounded() {
    let mut palette = CommandPalette::default();
    for n in 0..20 {
        palette.used(&format!("c{n}"));
    }
    palette.used("c5");
    assert_eq!(palette.recent().len(), RECENT);
    assert_eq!(palette.recent().first().map(String::as_str), Some("c5"));
    assert_eq!(palette.recent().get(1).map(String::as_str), Some("c19"));
    assert_eq!(palette.recent().iter().filter(|r| *r == "c5").count(), 1);
}
