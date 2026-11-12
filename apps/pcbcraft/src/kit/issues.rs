//! The Issues surface widgets besides the list (`kit::issues_list`): the
//! card's one-line issue slot, the always-visible bar and the canvas count
//! badge.
//!
//! docs/sidebar-ia.md, "Issues": these widgets lay out what core's
//! `project.issues` returns and never change a count. Counts are drawn
//! glyphs followed by numbers ("× 4 ! 2 ○ 36"), with the plain words as
//! the accessible name.
//!
//! Adapted from Solidcraft 4f3ac07a `crates/ui-egui/src/ui_kit/status.rs`
//! (`check_status_line`) and `checks.rs`, MIT option; see ATTRIBUTION.md.
//! Upstream keeps these crate-private, so the geometry is reproduced here
//! on the public tokens and fonts.
use super::severity::{Level, severity_glyph};
use egui::{
    Color32, CursorIcon, FontId, Galley, Rect, Response, Role, Sense, Ui, WidgetInfo, pos2, vec2,
};
use pcbcraft_core::issues::IssueCounts;
use solidcraft_theme::Tokens;
#[cfg(test)]
use solidcraft_ui_egui::fonts;
use solidcraft_ui_egui::ui_kit::type_face;
use std::sync::Arc;

/// The Issues bar's height while collapsed, in points.
pub const ISSUES_BAR_HEIGHT: f32 = 28.0;

/// One pick row (`kit::pick`), in points: the Browser row height, so both
/// lists read alike.
pub const ISSUE_ROW_HEIGHT: f32 = super::TREE_ROW_HEIGHT;

/// The levels a count summary shows, most severe first, skipping zeros.
fn segments(counts: &IssueCounts, to_do: bool, info: bool) -> Vec<(Level, usize)> {
    [
        (Level::Error, counts.error),
        (Level::Warning, counts.warning),
        (Level::ToDo, if to_do { counts.to_do } else { 0 }),
        (Level::Info, if info { counts.info } else { 0 }),
    ]
    .into_iter()
    .filter(|(_, n)| *n > 0)
    .collect()
}

/// The counts in words, the accessible name of a count summary:
/// "4 errors, 2 warnings, 36 to do", or "No issues".
pub fn counts_words(counts: &IssueCounts, to_do: bool, info: bool) -> String {
    let parts: Vec<String> = segments(counts, to_do, info)
        .into_iter()
        .map(|(level, n)| level.count_words(n))
        .collect();
    if parts.is_empty() {
        Level::Ok.count_words(0)
    } else {
        parts.join(", ")
    }
}

/// Glyph and number pairs laid out from `left` at height `cy`; returns the
/// right edge. With nothing to count it draws the passed glyph alone.
fn paint_counts(ui: &Ui, left: f32, cy: f32, segs: &[(Level, usize)], font: &FontId) -> f32 {
    let t = Tokens::get();
    let glyph = font.size;
    if segs.is_empty() {
        severity_glyph(ui.painter(), pos2(left + glyph / 2.0, cy), glyph, Level::Ok);
        return left + glyph;
    }
    let mut x = left;
    for (i, (level, n)) in segs.iter().enumerate() {
        if i > 0 {
            x += t.space_10;
        }
        severity_glyph(ui.painter(), pos2(x + glyph / 2.0, cy), glyph, *level);
        x += glyph + t.space_4;
        let number = galley(ui, &super::severity::compact(*n), font.clone(), t.text);
        let width = number.size().x;
        ui.painter()
            .galley(pos2(x, cy - number.size().y / 2.0), number, t.text);
        x += width;
    }
    x
}

/// The width [`paint_counts`] takes.
fn counts_width(ui: &Ui, segs: &[(Level, usize)], font: &FontId) -> f32 {
    let t = Tokens::get();
    let glyph = font.size;
    if segs.is_empty() {
        return glyph;
    }
    segs.iter()
        .map(|(_, n)| glyph + t.space_4 + galley(ui, &n.to_string(), font.clone(), t.text).size().x)
        .sum::<f32>()
        + t.space_10 * (segs.len().saturating_sub(1)) as f32
}

/// The card's reserved one-line object-issue slot ("× 1 ! 1 on R1 ›").
/// It always takes its row, so the card does not reflow when issues appear;
/// it is blank and inert while the object has none. A click opens Issues
/// scoped to this object.
pub fn issue_line(ui: &mut Ui, counts: &IssueCounts, label: &str) -> Response {
    let t = Tokens::get();
    let segs = segments(counts, true, true);
    let size = vec2(ui.available_width().max(0.0), t.control_height);
    if segs.is_empty() {
        return ui.allocate_exact_size(size, Sense::hover()).1;
    }
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    let cy = rect.center().y;
    let font = FontId::proportional(t.font_label);
    let end = paint_counts(ui, rect.left() + t.space_6, cy, &segs, &font);
    let chevron_size = t.form_section_icon_size;
    let chevron = Rect::from_center_size(
        pos2(rect.right() - t.space_6 - chevron_size / 2.0, cy),
        vec2(chevron_size, chevron_size),
    );
    super::chevron(ui.painter(), chevron, false, t.text_dim);
    let words = clipped(
        ui,
        label,
        font,
        t.text_dim,
        chevron.left() - t.space_6 - end - t.space_8,
    );
    ui.painter().galley(
        pos2(end + t.space_8, cy - words.size().y / 2.0),
        words,
        t.text_dim,
    );
    let name = format!("{} {label}", counts_words(counts, true, true));
    response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), &name));
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Show in Issues")
}

/// The always-visible Issues bar: disclosure, title and every count (the
/// bar never hides To do), then the `elsewhere` blocker line ("Component:
/// 2 block publishing") at its right. A click expands or collapses the
/// card.
pub fn issues_bar(
    ui: &mut Ui,
    counts: &IssueCounts,
    expanded: bool,
    elsewhere: Option<&str>,
) -> Response {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), ISSUES_BAR_HEIGHT),
        Sense::click(),
    );
    if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    t.secondary_hline(ui.painter(), rect.x_range(), rect.top(), t.dialog_rule);
    let cy = rect.center().y;
    let disclosure = Rect::from_min_size(
        pos2(rect.left() + t.space_8, cy - t.form_section_icon_size / 2.0),
        vec2(t.form_section_icon_size, t.form_section_icon_size),
    );
    super::chevron(ui.painter(), disclosure, expanded, t.text_dim);
    let title = galley(ui, "Issues", type_face::semibold(t.font_label), t.text);
    let mut x = disclosure.right() + t.space_6;
    let title_width = title.size().x;
    let segs = segments(counts, true, true);
    let font = FontId::proportional(t.font_label);
    // A narrow column drops the word "Issues" before it clips the
    // blockers elsewhere; the counts and the note always show.
    let note = elsewhere.filter(|e| !e.is_empty()).map_or(0.0, |e| {
        galley(ui, e, FontId::proportional(t.font_small), t.text_dim)
            .size()
            .x
            + t.space_16
            + t.space_10
    });
    // With nothing to count the bar says "No issues" after the glyph.
    let none_width = if segs.is_empty() {
        t.space_4
            + galley(ui, &Level::Ok.count_words(0), font.clone(), t.text_dim)
                .size()
                .x
    } else {
        0.0
    };
    let fits = x + title_width + t.space_10 + counts_width(ui, &segs, &font) + none_width + note
        <= rect.right() - t.space_8;
    if fits || note == 0.0 {
        ui.painter()
            .galley(pos2(x, cy - title.size().y / 2.0), title, t.text);
        x += title_width + t.space_10;
    }
    x = paint_counts(ui, x, cy, &segs, &font);
    if segs.is_empty() {
        let none = galley(ui, &Level::Ok.count_words(0), font, t.text_dim);
        let width = none.size().x;
        ui.painter().galley(
            pos2(x + t.space_4, cy - none.size().y / 2.0),
            none,
            t.text_dim,
        );
        // The elsewhere note clips against the end of "No issues".
        x += t.space_4 + width;
    }
    let mut name = format!("Issues: {}", counts_words(counts, true, true));
    if let Some(elsewhere) = elsewhere.filter(|e| !e.is_empty()) {
        let right = rect.right() - t.space_8;
        let dot = t.space_6;
        let text = clipped(
            ui,
            elsewhere,
            FontId::proportional(t.font_small),
            t.text_dim,
            (right - x - t.space_16 - dot - t.space_4).max(0.0),
        );
        let text_x = right - text.size().x;
        ui.painter().circle_filled(
            pos2(text_x - t.space_4 - dot / 2.0, cy),
            dot / 2.0,
            super::severity::ink(Level::Error),
        );
        ui.painter()
            .galley(pos2(text_x, cy - text.size().y / 2.0), text, t.text_dim);
        name = format!("{name}; {elsewhere}");
    }
    response.widget_info(|| WidgetInfo::selected(Role::Button, ui.is_enabled(), expanded, &name));
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(&name)
}

/// The counts as a compact badge (ribbon toggle, canvas). Info is not
/// counted here. To do is hidden while errors exist when
/// `hide_to_do_with_errors`; the bar never hides it.
pub fn count_badge(ui: &mut Ui, counts: &IssueCounts, hide_to_do_with_errors: bool) -> Response {
    let t = Tokens::get();
    let to_do = !(hide_to_do_with_errors && counts.error > 0);
    let segs = segments(counts, to_do, false);
    let font = FontId::proportional(t.font_small);
    let size = vec2(
        counts_width(ui, &segs, &font) + t.space_8 * 2.0,
        t.space_16 + t.space_6,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if response.hovered() { t.hover } else { t.panel };
    ui.painter().rect(
        rect,
        t.radius_xl,
        fill,
        t.control_border_stroke(t.border),
        egui::StrokeKind::Inside,
    );
    paint_counts(ui, rect.left() + t.space_8, rect.center().y, &segs, &font);
    let name = format!("Issues: {}", counts_words(counts, to_do, false));
    response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), &name));
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(&name)
}

fn galley(ui: &Ui, text: &str, font: FontId, color: Color32) -> Arc<Galley> {
    ui.painter().layout_no_wrap(text.to_owned(), font, color)
}

/// One line clipped to `width` with an ellipsis; `elided` says whether it
/// was cut (`kit::text`).
fn clipped(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    super::text::one_line(ui, text, font, color, width.max(0.0))
}

#[cfg(test)]
#[path = "issues_tests.rs"]
mod tests;
