//! Card rows of the sidebar redesign: the tool card, the one-line hint,
//! bounded lists and tables, summary rows, field errors and refusal rows.
//!
//! Slice K4 (docs/sidebar-ia.md, "Contract"). Feature modules show problems
//! only through [`field_error`] and [`refusal`] (`cargo xtask ui-forms`
//! counts the remaining `Tone::Error`/`Tone::Warning` text); help is one
//! [`hint`] line, never a paragraph. Adapted from Solidcraft 4f3ac07a
//! `crates/ui-egui/src/ui_kit/layout.rs` (`summary_row`, `catalog_footer`,
//! `catalog_footer_with_tip`), `reports.rs` (`list_frame`, `empty_box`'s
//! dashed outline), `table.rs` (weighted column shares, header captions) and the
//! feature-panel header (`panel.rs` `header_actions`), MIT option; see
//! ATTRIBUTION.md. Upstream keeps these crate-private, so the geometry is
//! reproduced on the public tokens, fonts and buttons.
#![allow(dead_code, reason = "slice K4 kit; the W slices use it")]

use super::severity::{Level, severity_glyph};
use egui::{Align, Color32, FontId, Layout, Rect, Response, Sense, Ui, UiBuilder, pos2, vec2};
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::{fonts, ui_kit};

/// Rows a list shows before it is bounded with "Show all".
pub const LIST_MAX_ROWS: usize = 8;

/// A tool card's header: what is armed and how it is ended.
pub struct ToolCard<'a> {
    pub icon: &'a str,
    pub name: &'a str,
    /// The tool's key, shown as a key cap; empty for none.
    pub key: &'a str,
    /// One clipped line of help, with the full text on hover.
    pub hint: &'a str,
    pub tip: &'a str,
    /// A pinned footer with Cancel and this primary ("Finish"), for tools
    /// that build an object over several clicks (Polygon). `None` for none.
    pub finish: Option<&'a str>,
}

/// What the tool card's controls did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToolCardResponse {
    /// Done: back to Select and the Properties card.
    pub done: bool,
    /// The footer's Cancel: drop the object being built.
    pub cancel: bool,
    /// The footer's primary: finish the object being built.
    pub finish: bool,
}

/// The tool card that replaces Properties while a tool is armed: a 40 pt
/// header (icon, name, key cap, Done), the tool's `rows`, its hint line and,
/// with [`ToolCard::finish`], a pinned footer.
pub fn tool_card(ui: &mut Ui, card: &ToolCard<'_>, rows: impl FnOnce(&mut Ui)) -> ToolCardResponse {
    let t = Tokens::get();
    let mut out = ToolCardResponse::default();
    let (rect, _) = ui.allocate_exact_size(
        vec2(
            ui.available_width().max(0.0),
            super::dialog::DIALOG_HEADER_HEIGHT,
        ),
        Sense::hover(),
    );
    let mut right = rect.right() - t.space_8;
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(t.space_8, 0.0)))
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            let done = ui_kit::button(ui, "Done", ui_kit::Button::Enabled(true))
                .on_hover_text("Back to Select");
            out.done = done.clicked();
            right = done.rect.left() - t.space_8;
        },
    );
    let icon_center = pos2(
        rect.left() + t.space_12 + t.form_icon_size / 2.0,
        rect.center().y,
    );
    super::icon(ui.painter(), icon_center, t.form_icon_size, card.icon, 1.0);
    let left = icon_center.x + t.form_icon_size / 2.0 + t.space_8;
    let key = (!card.key.is_empty()).then(|| {
        ui.painter().layout_no_wrap(
            card.key.to_owned(),
            fonts::semibold(t.font_small),
            t.form_dim,
        )
    });
    let key_width = key
        .as_ref()
        .map_or(0.0, |g| g.size().x + t.space_8 + t.space_8);
    let name = one_line(
        ui,
        card.name,
        fonts::semibold(t.font_heading),
        t.form_ink,
        (right - left - key_width).max(0.0),
    );
    let name_right = left + name.size().x;
    ui.painter().galley(
        pos2(left, rect.center().y - name.size().y / 2.0),
        name,
        t.form_ink,
    );
    if let Some(galley) = key {
        let cap = Rect::from_min_size(
            pos2(name_right + t.space_8, rect.center().y - t.space_10),
            vec2(galley.size().x + t.space_8, t.space_10 * 2.0),
        );
        if cap.right() <= right {
            ui.painter().rect(
                cap,
                t.radius_sm,
                t.dialog_footer,
                t.tertiary_border(t.dialog_rule),
                egui::StrokeKind::Inside,
            );
            ui.painter()
                .galley(cap.center() - galley.size() / 2.0, galley, t.form_dim);
        }
    }
    t.secondary_hline(ui.painter(), rect.x_range(), rect.bottom(), t.dialog_rule);
    super::form(ui, |ui| {
        rows(ui);
        if !card.hint.is_empty() {
            hint(ui, card.hint, card.tip);
        }
    });
    if let Some(finish) = card.finish {
        let action =
            super::dialog::footer(ui, finish, super::dialog::PrimaryKind::Normal, None, None);
        out.cancel = action == Some(super::dialog::DialogAction::Cancel);
        out.finish = action == Some(super::dialog::DialogAction::Primary);
    }
    out
}

/// `text` laid out on one line of at most `width`, cut with an ellipsis.
pub(super) fn one_line(
    ui: &Ui,
    text: &str,
    font: FontId,
    ink: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, ink, width.max(1.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('\u{2026}');
    ui.painter().layout_job(job)
}

/// Paint `text` on one line in `rect`, vertically centred and cut with an
/// ellipsis. Returns whether it was cut.
pub(super) fn paint_clipped(ui: &Ui, rect: Rect, text: &str, font: FontId, ink: Color32) -> bool {
    let full = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), ink)
        .size()
        .x;
    let galley = one_line(ui, text, font, ink, rect.width());
    ui.painter().galley(
        pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    full > rect.width() + 0.5
}

/// A painted line's hover: `tip` when given, else the full text when it was
/// cut short; screen readers get `text`.
pub(super) fn line_hover(response: Response, text: &str, tip: &str, cut: bool) -> Response {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    if !tip.is_empty() {
        response.on_hover_text(tip)
    } else if cut {
        response.on_hover_text(text)
    } else {
        response
    }
}

/// The height of one small line with its padding.
fn line_height(ui: &Ui, font: &FontId) -> f32 {
    let t = Tokens::get();
    ui.fonts_mut(|f| f.row_height(font)) + t.space_8
}

/// One dim line behind an information glyph, clipped to the row, with `tip`
/// on hover (the full text when `tip` is empty and the line is cut). It
/// replaces help paragraphs.
pub fn hint(ui: &mut Ui, text: &str, tip: &str) -> Response {
    let t = Tokens::get();
    let font = FontId::proportional(t.font_small);
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), line_height(ui, &font)),
        Sense::hover(),
    );
    let glyph = pos2(rect.left() + t.space_6, rect.center().y);
    ui.painter()
        .circle_stroke(glyph, t.space_4 + 0.5, t.control_border_stroke(t.form_dim));
    ui.painter().text(
        glyph,
        egui::Align2::CENTER_CENTER,
        "i",
        fonts::semibold(t.font_badge),
        t.form_dim,
    );
    let text_rect = Rect::from_min_max(pos2(rect.left() + t.space_16, rect.top()), rect.max);
    let cut = paint_clipped(ui, text_rect, text, font, t.form_dim);
    line_hover(response, text, tip, cut)
}

/// What a bounded list did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListFrameResponse {
    /// "Show all ›": open the full list (the Browser or a window).
    pub show_all: bool,
}

/// How many of `len` rows a list bounded at `max_rows` shows.
pub fn shown_rows(len: usize, max_rows: usize) -> usize {
    len.min(max_rows.max(1))
}

/// A bordered list of `len` rows that shows at most `max_rows`, with
/// "n of len" and "Show all ›" when more exist. Nothing is silently capped:
/// the count is always shown. Draws nothing for an empty list.
pub fn list_frame(
    ui: &mut Ui,
    len: usize,
    max_rows: usize,
    mut row: impl FnMut(&mut Ui, usize),
) -> ListFrameResponse {
    let mut out = ListFrameResponse::default();
    if len == 0 {
        return out;
    }
    let t = Tokens::get();
    let shown = shown_rows(len, max_rows);
    frame(ui, |ui| {
        for index in 0..shown {
            ui.push_id(index, |ui| row(ui, index));
        }
        if len > shown {
            divider(ui);
            out.show_all = summary_line(
                ui,
                &format!("{shown} of {len}"),
                t.form_dim,
                Some("Show all \u{203a}"),
            );
        }
    });
    out
}

/// The list border (upstream `reports::list_frame`).
fn frame<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get();
    egui::Frame::NONE
        .stroke(t.container_border(t.dialog_rule))
        .corner_radius(t.control_radius)
        .inner_margin(egui::Margin::symmetric(t.space_10 as i8, t.space_2 as i8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width().max(0.0));
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
}

/// A hairline across the list's text inset (upstream `report_divider`).
fn divider(ui: &mut Ui) {
    let t = Tokens::get();
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width().max(0.0), 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.top(), t.tertiary_border(t.dialog_rule));
}

/// One table column.
pub struct TableColumn<'a> {
    pub title: &'a str,
    /// Share of the row width.
    pub weight: f32,
}

/// What a table did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableResponse {
    /// The dashed add row was clicked.
    pub add: bool,
}

/// Column widths for `width`: shares of the positive weights (equal shares
/// when none is positive).
pub fn column_widths(columns: &[TableColumn<'_>], width: f32) -> Vec<f32> {
    let weight = |c: &TableColumn<'_>| {
        if c.weight.is_finite() {
            c.weight.max(0.0)
        } else {
            0.0
        }
    };
    let total: f32 = columns.iter().map(weight).sum();
    let width = width.max(0.0);
    columns
        .iter()
        .map(|c| {
            if total > 0.0 {
                width * weight(c) / total
            } else {
                width / columns.len().max(1) as f32
            }
        })
        .collect()
}

/// A bordered table of `rows` rows (Parameters, Fields, Mapping, Resolve)
/// under dim column captions, with an optional dashed add row. `cell(ui,
/// row, column)` draws one cell. More than [`LIST_MAX_ROWS`] rows scroll in
/// a frame of that height (the one nested scroll the IA allows), drawing
/// only the visible rows; none is hidden.
pub fn table(
    ui: &mut Ui,
    columns: &[TableColumn<'_>],
    rows: usize,
    mut cell: impl FnMut(&mut Ui, usize, usize),
    add_row: Option<&str>,
) -> TableResponse {
    let t = Tokens::get();
    let mut out = TableResponse::default();
    let row_height = t.control_height + t.space_2;
    let id = ui.id().with("table");
    frame(ui, |ui| {
        let widths = column_widths(
            columns,
            ui.available_width() - t.space_4 * columns.len() as f32,
        );
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), row_height), Sense::hover());
        let mut x = rect.left();
        for (column, w) in columns.iter().zip(&widths) {
            let caption = Rect::from_min_size(pos2(x, rect.top()), vec2(*w, rect.height()));
            paint_clipped(
                ui,
                caption,
                &column.title.to_uppercase(),
                fonts::semibold(t.font_caption),
                t.form_dim,
            );
            x += w + t.space_4;
        }
        divider(ui);
        let mut body = |ui: &mut Ui, range: std::ops::Range<usize>| {
            for row in range {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), row_height), Sense::hover());
                let mut x = rect.left();
                for (index, w) in widths.iter().enumerate() {
                    let cell_rect =
                        Rect::from_min_size(pos2(x, rect.top()), vec2(*w, rect.height()));
                    ui.scope_builder(
                        UiBuilder::new()
                            .id_salt((row, index))
                            .max_rect(cell_rect)
                            .layout(Layout::left_to_right(Align::Center)),
                        |ui| {
                            ui.set_clip_rect(ui.clip_rect().intersect(cell_rect));
                            ui.spacing_mut().text_edit_width = cell_rect.width();
                            cell(ui, row, index);
                        },
                    );
                    x += w + t.space_4;
                }
            }
        };
        if rows > LIST_MAX_ROWS {
            egui::ScrollArea::vertical()
                .id_salt(id)
                .max_height(row_height * LIST_MAX_ROWS as f32)
                .auto_shrink([false, true])
                .show_rows(ui, row_height, rows, |ui, range| body(ui, range));
        } else {
            body(ui, 0..rows);
        }
        if let Some(label) = add_row {
            out.add = dashed_button(ui, label).clicked();
        }
    });
    out
}

/// A full-width dashed outline with a centred dim caption, as a button
/// (upstream `empty_box`'s outline).
fn dashed_button(ui: &mut Ui, label: &str) -> Response {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), t.control_height),
        Sense::click(),
    );
    let rect = rect.shrink2(vec2(0.0, t.space_2));
    if response.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    let r = rect.shrink(0.5);
    let points = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    ui.painter().extend(egui::Shape::dashed_line(
        &points,
        t.control_border_stroke(t.control_border),
        t.space_4,
        t.space_3,
    ));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(t.font_label),
        t.form_dim,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A read-only summary, clipped to one line, with one optional action at
/// the right ("Edit sketch ›", "Choose…"). Returns whether the action was
/// clicked.
pub fn summary_row(ui: &mut Ui, summary: &str, action: Option<&str>) -> bool {
    summary_line(ui, summary, Tokens::get().form_ink, action)
}

fn summary_line(ui: &mut Ui, summary: &str, ink: Color32, action: Option<&str>) -> bool {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), t.control_height),
        Sense::hover(),
    );
    let mut clicked = false;
    let mut right = rect.right();
    if let Some(action) = action {
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("summary", action))
                .max_rect(rect)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                let button = ui_kit::button(ui, action, ui_kit::Button::Enabled(true));
                clicked = button.clicked();
                right = button.rect.left() - t.space_8;
            },
        );
    }
    if !summary.is_empty() {
        let text = Rect::from_min_max(rect.min, pos2(right.max(rect.left()), rect.bottom()));
        let cut = paint_clipped(ui, text, summary, FontId::proportional(t.font_label), ink);
        line_hover(response, summary, "", cut);
    }
    clicked
}

/// Mark `field` invalid: a red ring round it and one red line under it,
/// aligned with the field, clipped with the full text on hover. With
/// `not_stored`, the hover also says the document keeps its last valid
/// value. Call it right after the field's row.
pub fn field_error(ui: &mut Ui, field: &Response, message: &str, not_stored: bool) -> Response {
    let t = Tokens::get();
    ui.painter().rect_stroke(
        field.rect,
        t.radius_sm,
        egui::Stroke::new(1.0, t.error),
        egui::StrokeKind::Outside,
    );
    let font = FontId::proportional(t.font_small);
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), line_height(ui, &font)),
        Sense::hover(),
    );
    let left = field.rect.left().clamp(rect.left(), rect.right());
    let size = t.font_small;
    severity_glyph(
        ui.painter(),
        pos2(left + size / 2.0, rect.center().y),
        size,
        Level::Error,
    );
    let text = Rect::from_min_max(pos2(left + size + t.space_4, rect.top()), rect.max);
    let tip = if not_stored {
        format!("{message}\n{}", super::NOT_STORED)
    } else {
        String::new()
    };
    let cut = paint_clipped(ui, text, message, font, t.error);
    line_hover(response, message, &tip, cut)
}

/// A dialog's refusal: the last body row, an error band with one clipped
/// line and the full text on hover. Shown once; the dialog reveals it.
pub fn refusal(ui: &mut Ui, message: &str) -> Response {
    let t = Tokens::get();
    let font = FontId::proportional(t.font_label);
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), t.control_height),
        Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, t.radius_sm, t.form_error_fill);
    let size = t.font_label;
    severity_glyph(
        ui.painter(),
        pos2(rect.left() + t.space_6 + size / 2.0, rect.center().y),
        size,
        Level::Error,
    );
    let text = Rect::from_min_max(
        pos2(rect.left() + t.space_6 + size + t.space_6, rect.top()),
        rect.max - vec2(t.space_6, 0.0),
    );
    let cut = paint_clipped(ui, text, message, font, t.error);
    line_hover(response, message, "", cut)
}

#[cfg(test)]
#[path = "rows_tests.rs"]
pub(super) mod tests;
