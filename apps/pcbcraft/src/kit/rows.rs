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

use super::severity::{Level, severity_glyph};
use super::text;
use egui::{Align, Color32, FontId, Layout, Rect, Response, Sense, Ui, UiBuilder, pos2, vec2};
#[cfg(test)]
use pcbcraft_ui::fonts;
use pcbcraft_ui::theme::Tokens;
use pcbcraft_ui::ui_kit;
use pcbcraft_ui::ui_kit::type_face;

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
    let slot = ui.available_rect_before_wrap();
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
            type_face::semibold(t.font_small),
            t.form_dim,
        )
    });
    let key_width = key
        .as_ref()
        .map_or(0.0, |g| g.size().x + t.space_8 + t.space_8);
    let name = text::one_line(
        ui,
        card.name,
        type_face::semibold(t.font_heading),
        t.form_ink,
        (right - left - key_width).max(0.0),
    );
    let name_right = left + name.size().x;
    let name_cut = name.elided;
    let name_rect = Rect::from_min_size(
        pos2(left, rect.center().y - name.size().y / 2.0),
        name.size(),
    );
    ui.painter().galley(name_rect.min, name, t.form_ink);
    text::hover(
        ui.interact(name_rect, ui.scope_id().with("tool_name"), Sense::hover()),
        card.name,
        "",
        name_cut,
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
    // The rows scroll within the slot, so a tall card never grows past it
    // (its area would otherwise be pushed up over the ribbon).
    let footer = if card.finish.is_some() {
        super::card::FOOTER_HEIGHT
    } else {
        0.0
    };
    let room = if slot.bottom().is_finite() {
        (slot.bottom() - ui.cursor().top() - footer).max(0.0)
    } else {
        f32::INFINITY
    };
    egui::ScrollArea::vertical()
        .id_salt(("tool_card_body", card.name))
        .min_scrolled_height(0.0)
        .max_height(room)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            super::form(ui, |ui| {
                rows(ui);
                if !card.hint.is_empty() {
                    hint(ui, card.hint, card.tip);
                }
            });
        });
    if let Some(finish) = card.finish {
        let action =
            super::dialog::footer(ui, finish, super::dialog::PrimaryKind::Normal, None, None);
        out.cancel = action == Some(super::dialog::DialogAction::Cancel);
        out.finish = action == Some(super::dialog::DialogAction::Primary);
    }
    out
}

/// The height of one small line with its padding.
fn line_height(ui: &Ui, font: &FontId) -> f32 {
    let t = Tokens::get();
    ui.fonts_mut(|f| f.row_height(font)) + t.space_8
}

/// Rows a hint wraps to before it is cut (with the full text on hover).
const HINT_MAX_ROWS: usize = 4;

/// One dim sentence behind an information glyph, wrapped to the card's
/// width, with `tip` on hover. It replaces help paragraphs.
pub fn hint(ui: &mut Ui, text: &str, tip: &str) -> Response {
    let t = Tokens::get();
    let font = FontId::proportional(t.font_small);
    let indent = t.space_16;
    let width = (ui.available_width() - indent).max(1.0);
    let galley = text::wrapped(ui, text, font.clone(), t.form_dim, width, HINT_MAX_ROWS);
    let cut = galley.elided;
    let first = line_height(ui, &font);
    let pad = (first - ui.fonts_mut(|f| f.row_height(&font))) / 2.0;
    let (rect, response) = ui.allocate_exact_size(
        vec2(
            ui.available_width().max(0.0),
            (galley.size().y + 2.0 * pad).max(first),
        ),
        Sense::hover(),
    );
    let glyph = pos2(rect.left() + t.space_6, rect.top() + first / 2.0);
    ui.painter()
        .circle_stroke(glyph, t.space_4 + 0.5, t.control_border_stroke(t.form_dim));
    ui.painter().text(
        glyph,
        egui::Align2::CENTER_CENTER,
        "i",
        type_face::semibold(t.font_badge),
        t.form_dim,
    );
    ui.painter().galley(
        pos2(rect.left() + indent, rect.top() + pad),
        galley,
        t.form_dim,
    );
    text::hover(response, text, tip, cut)
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
                None,
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
    /// "Show all ›" of a [`table_bounded`] table was clicked.
    pub show_all: bool,
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
    cell: impl FnMut(&mut Ui, usize, usize),
    add_row: Option<&str>,
) -> TableResponse {
    table_with(ui, columns, rows, None, None, cell, add_row)
}

/// A [`table`] whose scroll frame (more than [`LIST_MAX_ROWS`] rows) is
/// scrolled this frame to show row `reveal`, so a cell drawn only when
/// visible can be revealed ("Show rule").
pub fn table_revealing(
    ui: &mut Ui,
    columns: &[TableColumn<'_>],
    rows: usize,
    reveal: Option<usize>,
    cell: impl FnMut(&mut Ui, usize, usize),
    add_row: Option<&str>,
) -> TableResponse {
    table_with(ui, columns, rows, None, reveal, cell, add_row)
}

/// A [`table`] in a card that shows at most `max_rows` rows, then "n of
/// rows" and "Show all ›" as [`list_frame`] does, instead of scrolling
/// inside the card. Raise `max_rows` to `rows` once Show all was clicked.
pub fn table_bounded(
    ui: &mut Ui,
    columns: &[TableColumn<'_>],
    rows: usize,
    max_rows: usize,
    cell: impl FnMut(&mut Ui, usize, usize),
    add_row: Option<&str>,
) -> TableResponse {
    table_with(ui, columns, rows, Some(max_rows), None, cell, add_row)
}

fn table_with(
    ui: &mut Ui,
    columns: &[TableColumn<'_>],
    rows: usize,
    bound: Option<usize>,
    reveal: Option<usize>,
    mut cell: impl FnMut(&mut Ui, usize, usize),
    add_row: Option<&str>,
) -> TableResponse {
    let t = Tokens::get();
    let mut out = TableResponse::default();
    let row_height = t.control_height + t.space_2;
    let id = ui.scope_id().with("table");
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
            text::line(
                ui,
                ui.scope_id().with(("caption", column.title)),
                caption,
                &column.title.to_uppercase(),
                "",
                type_face::semibold(t.font_caption),
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
                // A cell that draws nothing leaves the cursor at its
                // centre; the next row starts under this one.
                ui.advance_cursor_after_rect(rect);
            }
        };
        if let Some(max_rows) = bound {
            let shown = shown_rows(rows, max_rows);
            body(ui, 0..shown);
            if rows > shown {
                divider(ui);
                out.show_all = summary_line(
                    ui,
                    &format!("{shown} of {rows}"),
                    t.form_dim,
                    Some("Show all \u{203a}"),
                    None,
                );
            }
        } else if rows > LIST_MAX_ROWS {
            let mut area = egui::ScrollArea::vertical()
                .id_salt(id)
                .max_height(row_height * LIST_MAX_ROWS as f32)
                .auto_shrink([false, true]);
            if let Some(row) = reveal.filter(|r| *r < rows) {
                // The row near the frame's middle.
                let pitch = row_height + ui.spacing().item_spacing.y;
                let above = row.saturating_sub(LIST_MAX_ROWS / 2);
                area = area.vertical_scroll_offset(above as f32 * pitch);
            }
            area.show_rows(ui, row_height, rows, |ui, range| body(ui, range));
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
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A read-only summary, clipped to one line, with one optional action at
/// the right ("Edit sketch ›", "Choose…"). Returns whether the action was
/// clicked.
pub fn summary_row(ui: &mut Ui, summary: &str, action: Option<&str>) -> bool {
    summary_line(ui, summary, Tokens::get().form_ink, action, None)
}

/// One action at the right of a row with no summary, saying what it does
/// on hover ("Delete sheet": what goes with it). Returns whether it was
/// clicked.
pub fn action_row(ui: &mut Ui, action: &str, hover: &str) -> bool {
    summary_line(ui, "", Tokens::get().form_ink, Some(action), Some(hover))
}

fn summary_line(
    ui: &mut Ui,
    summary: &str,
    ink: Color32,
    action: Option<&str>,
    hover: Option<&str>,
) -> bool {
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
                let mut button = ui_kit::button(ui, action, ui_kit::Button::Enabled(true));
                if let Some(hover) = hover {
                    button = button.on_hover_text(hover);
                }
                clicked = button.clicked();
                right = button.rect.left() - t.space_8;
            },
        );
    }
    if !summary.is_empty() {
        // The hover covers the text only, so the action keeps its own.
        let area = Rect::from_min_max(rect.min, pos2(right.max(rect.left()), rect.bottom()));
        text::line(
            ui,
            response.id.with("text"),
            area,
            summary,
            "",
            FontId::proportional(t.font_label),
            ink,
        );
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
    let tip = if not_stored { super::NOT_STORED } else { "" };
    let cut = text::paint_line(ui, text, message, font, t.error);
    text::hover(response, message, tip, cut)
}

/// A refused commit of `field` whose field went back to the stored value:
/// no red ring (what the field shows is valid), and the whole reason under
/// it, wrapped, so a refusal that names parts and distances is read in
/// full. Call it right after the field's row; it shows until the field is
/// edited again or the document changes.
#[allow(
    dead_code,
    reason = "Board parameters show refusals through kit::list; other cards may again"
)]
pub fn field_refused(ui: &mut Ui, field: &Response, message: &str) -> Response {
    let t = Tokens::get();
    let size = t.font_small;
    let left = field.rect.left().max(ui.max_rect().left());
    let width = (ui.max_rect().right() - left - size - t.space_4).max(24.0);
    let galley = ui.painter().layout(
        message.to_owned(),
        FontId::proportional(t.font_small),
        t.error,
        width,
    );
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), galley.size().y),
        Sense::hover(),
    );
    let first = line_height(ui, &FontId::proportional(t.font_small));
    severity_glyph(
        ui.painter(),
        pos2(left + size / 2.0, rect.top() + first / 2.0),
        size,
        Level::Error,
    );
    ui.painter()
        .galley(pos2(left + size + t.space_4, rect.top()), galley, t.error);
    response
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
    let cut = text::paint_line(ui, text, message, font, t.error);
    text::hover(response, message, "", cut)
}

/// Rows a tool card's refusal wraps to before it is cut (with the full
/// text on hover).
const REFUSAL_MAX_ROWS: usize = 8;

/// A tool card's refusal: the error band of [`refusal`], with the whole
/// reason wrapped to the card's width, so a sentence that names parts and
/// distances is read where it shows, not in a tooltip over the rows below.
pub fn refusal_wrapped(ui: &mut Ui, message: &str) -> Response {
    let t = Tokens::get();
    let font = FontId::proportional(t.font_label);
    let size = t.font_label;
    let indent = t.space_6 + size + t.space_6;
    let width = (ui.available_width() - indent - t.space_6).max(1.0);
    let galley = text::wrapped(ui, message, font.clone(), t.error, width, REFUSAL_MAX_ROWS);
    let cut = galley.elided;
    let row = ui.fonts_mut(|f| f.row_height(&font));
    let pad = ((t.control_height - row) / 2.0).max(0.0);
    let (rect, response) = ui.allocate_exact_size(
        vec2(
            ui.available_width().max(0.0),
            (galley.size().y + 2.0 * pad).max(t.control_height),
        ),
        Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, t.radius_sm, t.form_error_fill);
    severity_glyph(
        ui.painter(),
        pos2(
            rect.left() + t.space_6 + size / 2.0,
            rect.top() + t.control_height / 2.0,
        ),
        size,
        Level::Error,
    );
    ui.painter().galley(
        pos2(rect.left() + indent, rect.top() + pad),
        galley,
        t.error,
    );
    text::hover(response, message, "", cut)
}

#[cfg(test)]
#[path = "rows_tests.rs"]
pub(super) mod tests;
