//! Text that may not fit: the one way every card, list and row shows it.
//!
//! Where there is room the text wraps ([`wrapped`]); where a
//! row needs a single line it is shortened with "…" ([`paint_line`]) and
//! the full text shows on hover ([`hover`]). A widget gives its text one
//! hover: the full text when it was cut, then its own tip, in one tooltip
//! on the widget's own response, so tooltips never stack and close when
//! the pointer leaves that widget. Painting a shortened galley anywhere
//! else needs the same pairing; `kit::text` tests hover every cut text of
//! the kit and of the cards that use it, and fail when one has no hover.
use egui::text::LayoutJob;
use egui::{Color32, FontId, Galley, Rect, Response, Sense, Ui, pos2};
use std::sync::Arc;

/// The ellipsis that marks shortened text.
pub const ELLIPSIS: char = '\u{2026}';

/// `text` laid out on one line of at most `width`, cut with an ellipsis
/// (`galley.elided` says whether it was).
pub fn one_line(ui: &Ui, text: &str, font: FontId, ink: Color32, width: f32) -> Arc<Galley> {
    wrapped(ui, text, font, ink, width, 1)
}

/// `text` wrapped to `width`, at most `max_rows` rows; the last kept row
/// ends with an ellipsis when more was cut (`galley.elided`).
pub fn wrapped(
    ui: &Ui,
    text: &str,
    font: FontId,
    ink: Color32,
    width: f32,
    max_rows: usize,
) -> Arc<Galley> {
    let mut job = LayoutJob::simple(text.to_owned(), font, ink, width.max(1.0));
    job.wrap.max_rows = max_rows.max(1);
    job.wrap.break_anywhere = max_rows <= 1;
    job.wrap.overflow_character = Some(ELLIPSIS);
    ui.painter().layout_job(job)
}

/// Paint `text` on one line in `rect`, vertically centred and cut with an
/// ellipsis. Returns whether it was cut; pass that to [`hover`].
pub fn paint_line(ui: &Ui, rect: Rect, text: &str, font: FontId, ink: Color32) -> bool {
    let galley = one_line(ui, text, font, ink, rect.width());
    let cut = galley.elided;
    ui.painter().galley(
        pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    cut
}

/// The one tooltip of a widget showing `text`: the full text when it was
/// `cut`, then `tip` when there is one, or nothing. Screen readers get
/// `text`. Attach it to the widget's own response, never to a second
/// response over the same place.
pub fn hover(response: Response, text: &str, tip: &str, cut: bool) -> Response {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
    match (cut, tip.is_empty()) {
        (true, true) => response.on_hover_text(text),
        (true, false) => response.on_hover_text(format!("{text}\n{tip}")),
        (false, false) => response.on_hover_text(tip),
        (false, true) => response,
    }
}

/// egui's tooltip width (500 points), never more than fits the window:
/// tooltips wrap at it, and a tooltip wider than the window runs off its
/// edge. Call once per frame, before anything shows a tooltip.
pub fn fit_tooltips(ctx: &egui::Context) {
    const TOOLTIP_WIDTH: f32 = 500.0;
    // The tooltip's frame and its distance from the window's edges.
    const ROOM: f32 = 32.0;
    let width = TOOLTIP_WIDTH.min((ctx.content_rect().width() - ROOM).max(ROOM));
    if (ctx.global_style().spacing.tooltip_width - width).abs() > 0.5 {
        ctx.global_style_mut(|s| s.spacing.tooltip_width = width);
    }
}

/// Paint `text` on one line in `rect` with its own hover (`id`): the full
/// text when it was cut, else `tip`. For text with no widget of its own
/// (a header title, a footer status).
pub fn line(
    ui: &Ui,
    id: egui::Id,
    rect: Rect,
    text: &str,
    tip: &str,
    font: FontId,
    ink: Color32,
) -> Response {
    let cut = paint_line(ui, rect, text, font, ink);
    let response = ui.interact(rect, id, Sense::hover());
    hover(response, text, tip, cut)
}

#[cfg(test)]
#[path = "text_tests.rs"]
pub(crate) mod tests;
