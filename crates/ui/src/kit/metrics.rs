//! Metrics elements for dense cards; see `docs/ui-kit.md` for tokens and usage.
use crate::tokens::Tokens;
use egui::Ui;

/// Row height for controls. Tokens: `control_height`.
pub fn row_height() -> f32 {
    Tokens::get().control_height
}

/// Feature heading height. Tokens: `form_header_height`.
pub fn header_height() -> f32 {
    Tokens::get().form_header_height
}

/// Sanitize an input dimension. Tokens: `none`.
pub fn dimension(value: f32) -> f32 {
    if value.is_finite() { value.max(0.0) } else { 0.0 }
}

/// A label as drawn: the leading indent kept, the text itself translated (a nested row such as
/// `"  Of"` lines up under the one above and still translates). Measuring (`label_width`) and the
/// row label (`sized_row`) both prepare their text with this, so a label is translated exactly
/// once and the label column and the captions line up on the same text.
pub(super) fn indented_label(text: &str) -> String {
    let indent = text.len() - text.trim_start().len();
    format!("{}{}", &text[..indent], solidcraft_ui_text::tr_label(text.trim_start()))
}

/// Measured width of a label in the form label font. Tokens: `font_label, dialog_label`.
pub(super) fn label_width(ui: &Ui, text: &str) -> f32 {
    let t = Tokens::get();
    ui.painter().layout_no_wrap(indented_label(text), egui::FontId::proportional(t.font_label), t.dialog_label).size().x.ceil()
}

/// Minimum content widths for table controls, including their token/native control metrics.
pub struct ContentWidth;

impl ContentWidth {
    pub fn label(ui: &Ui, text: &str) -> f32 {
        label_width(ui, text)
    }
}
