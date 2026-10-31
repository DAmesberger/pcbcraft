//! Card rows of the sidebar redesign: the tool card, the one-line hint,
//! bounded lists and tables, summary rows, field errors and refusal rows.
//!
//! Contract from slice S0 (docs/sidebar-ia.md, "Kit"); slice K4 owns the
//! final look. Feature modules show problems only through [`field_error`]
//! and [`refusal`]; help is one [`hint`] line, never a paragraph. Adapted
//! from Solidcraft 4f3ac07a `crates/ui-egui/src/ui_kit/layout.rs`
//! (`summary_row`, `catalog_footer_with_tip`), `reports.rs` (`list_frame`),
//! `name_list.rs`, `table.rs` and the feature-panel header, MIT option; see
//! ATTRIBUTION.md.
#![allow(dead_code, reason = "S0 contract; slice K4 and the W slices use it")]

use egui::{Response, RichText, Ui, vec2};
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::ui_kit;

/// Rows a list shows before it is bounded with "Show all".
pub const LIST_MAX_ROWS: usize = 8;

/// A tool card's header: what is armed and how it is ended.
pub struct ToolCard<'a> {
    pub icon: &'a str,
    pub name: &'a str,
    /// The tool's key, shown as a key cap.
    pub key: &'a str,
    /// One clipped line of help, with the full text on hover.
    pub hint: &'a str,
    pub tip: &'a str,
}

/// What the tool card's controls did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToolCardResponse {
    /// Done: back to Select and the Properties card.
    pub done: bool,
}

/// The tool card that replaces Properties while a tool is armed: header
/// with Done, the tool's `rows`, then its hint line.
pub fn tool_card(ui: &mut Ui, card: &ToolCard<'_>, rows: impl FnOnce(&mut Ui)) -> ToolCardResponse {
    let mut out = ToolCardResponse::default();
    ui.horizontal(|ui| {
        super::inspector_header(ui, card.icon, card.name, card.key);
        out.done = ui_kit::button(ui, "Done", ui_kit::Button::Enabled(true)).clicked();
    });
    super::form(ui, rows);
    hint(ui, card.hint, card.tip);
    out
}

/// One dim line, clipped to the row, with `tip` on hover. It replaces help
/// paragraphs.
pub fn hint(ui: &mut Ui, text: &str, tip: &str) -> Response {
    let t = Tokens::get();
    let response = ui.add(
        egui::Label::new(RichText::new(text).size(t.font_small).color(t.form_dim))
            .truncate()
            .selectable(false),
    );
    if tip.is_empty() {
        response
    } else {
        response.on_hover_text(tip)
    }
}

/// What a bounded list did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListFrameResponse {
    /// "Show all ›": open the full list (the Browser or a window).
    pub show_all: bool,
}

/// A list of `len` rows that shows at most `max_rows`, with "Show all ›"
/// when more exist. Nothing is silently capped: the count is always shown.
pub fn list_frame(
    ui: &mut Ui,
    len: usize,
    max_rows: usize,
    mut row: impl FnMut(&mut Ui, usize),
) -> ListFrameResponse {
    let shown = len.min(max_rows);
    for index in 0..shown {
        row(ui, index);
    }
    let mut out = ListFrameResponse::default();
    if len > shown {
        out.show_all = summary_row(ui, &format!("{shown} of {len}"), Some("Show all ›"));
    }
    out
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

/// A table of `rows` rows (Parameters, Fields, Mapping, Resolve) with an
/// optional dashed add row. `cell(ui, row, column)` draws one cell.
pub fn table(
    ui: &mut Ui,
    columns: &[TableColumn<'_>],
    rows: usize,
    mut cell: impl FnMut(&mut Ui, usize, usize),
    add_row: Option<&str>,
) -> TableResponse {
    let t = Tokens::get();
    let total: f32 = columns
        .iter()
        .map(|c| c.weight.max(0.0))
        .sum::<f32>()
        .max(1.0);
    let width = ui.available_width();
    let widths: Vec<f32> = columns
        .iter()
        .map(|c| width * c.weight.max(0.0) / total)
        .collect();
    ui.horizontal(|ui| {
        for (column, w) in columns.iter().zip(&widths) {
            ui.add_sized(
                vec2(*w, t.control_height),
                egui::Label::new(
                    RichText::new(column.title)
                        .size(t.font_small)
                        .color(t.form_dim),
                ),
            );
        }
    });
    for row in 0..rows {
        ui.horizontal(|ui| {
            for (index, w) in widths.iter().enumerate() {
                ui.allocate_ui(vec2(*w, t.control_height), |ui| cell(ui, row, index));
            }
        });
    }
    let mut out = TableResponse::default();
    if let Some(label) = add_row {
        out.add = ui
            .add_sized(
                vec2(width, t.control_height),
                egui::Button::new(RichText::new(label).size(t.font_label).color(t.form_dim)),
            )
            .clicked();
    }
    out
}

/// A read-only summary with one optional action. Returns whether the
/// action was clicked.
pub fn summary_row(ui: &mut Ui, summary: &str, action: Option<&str>) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        hint(ui, summary, summary);
        if let Some(action) = action {
            clicked = ui_kit::button(ui, action, ui_kit::Button::Enabled(true)).clicked();
        }
    });
    clicked
}

/// Mark `field` invalid: a red ring and one line under it. With
/// `not_stored`, the line also says the document keeps its last valid
/// value.
pub fn field_error(ui: &mut Ui, field: &Response, message: &str, not_stored: bool) -> Response {
    let t = Tokens::get();
    ui.painter().rect_stroke(
        field.rect,
        t.radius_sm,
        egui::Stroke::new(1.0, t.error),
        egui::StrokeKind::Outside,
    );
    let response = super::status(ui, message, super::Tone::Error);
    if not_stored {
        response.on_hover_text(super::NOT_STORED)
    } else {
        response
    }
}

/// A dialog's refusal: the last body row, shown once.
pub fn refusal(ui: &mut Ui, message: &str) -> Response {
    super::status(ui, message, super::Tone::Error)
}
