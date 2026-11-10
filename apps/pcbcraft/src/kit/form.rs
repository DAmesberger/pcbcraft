//! Form rows and controls (upstream `ui_kit/layout.rs` and `fields.rs`).
use egui::{Align, Layout, Response, RichText, Ui, UiBuilder, vec2};
use solidcraft_theme::Tokens;
#[cfg(test)]
use solidcraft_ui_egui::fonts;
use solidcraft_ui_egui::ui_kit;
use solidcraft_ui_egui::ui_kit::type_face;

/// Form scope: tokenised spacing, type and control looks for every row
/// inside `add` (upstream `layout::scope`).
pub fn form<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        let t = Tokens::get();
        ui.spacing_mut().item_spacing = vec2(t.space_4, t.form_gap_y);
        ui.spacing_mut().interact_size.y = t.control_height;
        ui.spacing_mut().button_padding = vec2(t.space_8, t.space_4);
        let styles = &mut ui.style_mut().text_styles;
        styles.insert(
            egui::TextStyle::Body,
            egui::FontId::proportional(t.font_label),
        );
        styles.insert(
            egui::TextStyle::Button,
            egui::FontId::proportional(t.font_label),
        );
        styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::monospace(t.font_body),
        );
        let visuals = ui.visuals_mut();
        visuals.weak_text_color = Some(t.form_dim);
        visuals.selection.stroke = egui::Stroke::new(1.0, t.form_primary);
        visuals.selection.bg_fill = t.selected_bg;
        visuals.widgets.noninteractive.bg_stroke = t.tertiary_border(t.dialog_rule);
        for state in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            state.bg_fill = t.control_fill;
            state.weak_bg_fill = t.control_fill;
            state.bg_stroke = t.control_border_stroke(t.control_border);
            state.corner_radius = t.control_radius.into();
            state.expansion = 0.0;
            state.fg_stroke.color = t.form_ink;
        }
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        add(ui)
    })
    .inner
}

/// Aligned label/control row of one control height: the label sits in the
/// shared label column, the control fills the rest.
pub fn row<R>(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get();
    let width = ui.available_width().max(0.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, t.control_height), egui::Sense::hover());
    let label_width = t.form_label_width.min(width * 0.45);
    let label_rect = egui::Rect::from_min_size(rect.min, vec2(label_width, rect.height()));
    let label_id = ui
        .scope_builder(
            UiBuilder::new()
                .max_rect(label_rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(label)
                            .size(t.font_label)
                            .color(t.dialog_label),
                    )
                    .truncate(),
                )
                .id
            },
        )
        .inner;
    let control_rect = egui::Rect::from_min_max(
        egui::pos2(
            (label_rect.right() + t.space_8).min(rect.right()),
            rect.top(),
        ),
        rect.max,
    );
    ui.scope_builder(
        UiBuilder::new()
            .id_salt(label)
            .max_rect(control_rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(control_rect));
            ui.spacing_mut().text_edit_width = control_rect.width();
            // The row's label names its field ([`named_by_row`]).
            let named = !label.is_empty();
            if named {
                ui.data_mut(|d| d.insert_temp(row_label_key(), label_id));
            }
            let inner = control(ui);
            if named {
                ui.data_mut(|d| d.remove::<egui::Id>(row_label_key()));
            }
            inner
        },
    )
    .inner
}

/// Temp memory: the label of the form row whose control is being drawn.
pub(super) fn row_label_key() -> egui::Id {
    egui::Id::unique("pcbcraft-form-row-label")
}

/// A field drawn in a [`row`] is named by the row's label in the
/// accessibility tree, as screen readers and UI automation read it.
pub(super) fn named_by_row(ui: &Ui, response: Response) -> Response {
    match ui.data(|d| d.get_temp::<egui::Id>(row_label_key())) {
        Some(label) => response.labelled_by(label),
        None => response,
    }
}

/// Plain text row with the upstream cell editor. Returns the field's response.
pub fn text_row(ui: &mut Ui, label: &str, value: &mut String) -> Response {
    row(ui, label, |ui| text_field(ui, label, value, "", false))
}

/// Several lines of text in the control column: Enter breaks the line,
/// leaving the field ends the edit. Returns the field's response.
pub fn multiline_row(ui: &mut Ui, label: &str, value: &mut String, invalid: bool) -> Response {
    row(ui, label, |ui| {
        let t = Tokens::get();
        let edit = egui::TextEdit::multiline(value)
            .id(ui.scope_id().with(("multiline", label)))
            .font(egui::TextStyle::Monospace)
            .desired_rows(3)
            .desired_width(ui.available_width())
            .text_color(if invalid { t.error } else { t.text });
        let response = ui.add(edit);
        let response = named_by_row(ui, response);
        if invalid {
            not_stored(response)
        } else {
            response
        }
    })
}

/// Text row whose content the caller validates; `invalid` shows the error look.
pub fn checked_text_row(
    ui: &mut Ui,
    label: &str,
    value: &mut String,
    hint: &str,
    invalid: bool,
) -> Response {
    row(ui, label, |ui| text_field(ui, label, value, hint, invalid))
}

/// The upstream cell editor (monospace text in the control fill).
pub fn text_field(
    ui: &mut Ui,
    id: &str,
    value: &mut String,
    hint: &str,
    invalid: bool,
) -> Response {
    let id = ui.scope_id().with(("text", id));
    let response = ui_kit::cell_editor(ui, id, value, hint, invalid);
    let response = named_by_row(ui, response);
    if invalid {
        not_stored(response)
    } else {
        shortened(ui, response, value)
    }
}

/// A field not being edited whose text does not fit ends in "…" and
/// shows its whole text on hover, as every cut text does (`kit::text`);
/// editing shows it all again.
fn shortened(ui: &Ui, response: Response, value: &str) -> Response {
    if response.has_focus() || value.is_empty() {
        return response;
    }
    let t = Tokens::get();
    // The cell editor's text: monospace at the label size, inset.
    let font = egui::FontId::monospace(t.font_label);
    let inner = response.rect.shrink2(vec2(t.form_inset, 1.0));
    let width = ui
        .painter()
        .layout_no_wrap(value.to_owned(), font.clone(), t.form_ink)
        .size()
        .x;
    if width <= inner.width() {
        return response;
    }
    if ui.is_enabled() {
        ui.painter().rect_filled(inner, 0.0, t.input_fill());
        super::text::paint_line(ui, inner, value, font, t.form_ink);
    }
    response.on_hover_text(value)
}

/// The hover text of a field whose text is invalid: what it shows is not
/// what the document holds (plan principle 13: invalid input never
/// commits and is marked).
pub const NOT_STORED: &str = "Not stored: this text is invalid, so the document keeps its last valid value. Fix the text to store it.";

/// Mark an invalid field's response as not stored.
pub fn not_stored(response: Response) -> Response {
    response.on_hover_text(NOT_STORED)
}

/// Units written directly after the number ("45°"); others take a space ("6 mm").
fn unit_gap(unit: &str) -> &'static str {
    if matches!(unit, "°" | "%") { "" } else { " " }
}

/// Format a number without trailing zeros, at most four decimals.
pub fn number_text(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// Parse a number with an optional `unit` suffix (`"2.5"`, `"2.5 mm"`).
pub fn parse_number(text: &str, unit: &str) -> Option<f64> {
    let text = text.trim();
    let text = if unit.is_empty() {
        text
    } else {
        text.strip_suffix(unit).unwrap_or(text).trim_end()
    };
    if text.len() > 32 {
        return None;
    }
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A number with its unit in the text (`"6 mm"`), like Solidcraft's
/// expression fields. Out-of-range or malformed text shows the error look
/// and leaves `value` unchanged; it stays in the field after focus leaves,
/// still marked, until it is fixed or Esc restores the value's text.
/// Returns whether `value` changed.
pub fn number_field(
    ui: &mut Ui,
    id: &str,
    value: &mut f64,
    unit: &str,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    number_input(ui, id, value, unit, range).changed
}

/// What [`number_input`] did this frame.
pub struct NumberInput {
    /// The value changed (one commit per change).
    pub changed: bool,
    /// The field's response.
    pub response: Response,
    /// Why the shown text is not stored, while it is invalid.
    pub invalid: Option<String>,
}

/// [`number_field`] with its response and why its text is invalid, so a
/// row can mark it with [`super::field_error`].
pub fn number_input(
    ui: &mut Ui,
    id: &str,
    value: &mut f64,
    unit: &str,
    range: std::ops::RangeInclusive<f64>,
) -> NumberInput {
    let key = ui.scope_id().with(("number", id));
    let shown = || {
        if unit.is_empty() {
            number_text(*value)
        } else {
            format!("{}{}{unit}", number_text(*value), unit_gap(unit))
        }
    };
    let mut text = ui.data(|d| d.get_temp::<String>(key)).unwrap_or_else(shown);
    let parsed = parse_number(&text, unit).filter(|v| range.contains(v));
    let response = ui_kit::cell_editor(ui, key.with("edit"), &mut text, "", parsed.is_none());
    let mut response = named_by_row(ui, response);
    let parsed = parse_number(&text, unit).filter(|v| range.contains(v));
    let escaped = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
    let mut changed = false;
    if escaped {
        ui.data_mut(|d| d.remove::<String>(key));
        return NumberInput {
            changed,
            response,
            invalid: None,
        };
    }
    if response.has_focus() {
        if let Some(v) = parsed
            && (v - *value).abs() > f64::EPSILON
        {
            *value = v;
            changed = true;
        }
        ui.data_mut(|d| d.insert_temp(key, text.clone()));
    } else if parsed.is_some() {
        ui.data_mut(|d| d.remove::<String>(key));
    } else {
        // Invalid text is kept and marked, never silently replaced.
        ui.data_mut(|d| d.insert_temp(key, text.clone()));
    }
    let invalid = parsed
        .is_none()
        .then(|| number_message(&text, unit, &range));
    if invalid.is_some() {
        response = not_stored(response);
    }
    NumberInput {
        changed,
        response,
        invalid,
    }
}

/// Why `text` is not a number of `range`, short enough for the line under
/// a half-width field: "Enter a number in mm", or "Enter 0 to 1000 mm".
fn number_message(text: &str, unit: &str, range: &std::ops::RangeInclusive<f64>) -> String {
    let gap = unit_gap(unit);
    if parse_number(text, unit).is_none() {
        if unit.is_empty() {
            "Enter a number".to_owned()
        } else {
            format!("Enter a number in {unit}")
        }
    } else {
        format!(
            "Enter {} to {}{gap}{unit}",
            number_text(*range.start()),
            number_text(*range.end())
        )
    }
}

/// One labelled number row. Invalid text is marked with one error line
/// under the row ([`super::field_error`]) and never committed.
pub fn number_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f64,
    unit: &str,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let input = row(ui, label, |ui| number_input(ui, label, value, unit, range));
    if let Some(message) = &input.invalid {
        super::field_error(ui, &input.response, message, true);
    }
    input.changed
}

/// X and Y of one point in one row, each half of the control column.
pub fn pair_row(
    ui: &mut Ui,
    label: &str,
    value: &mut [f64; 2],
    unit: &str,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    row(ui, label, |ui| {
        let t = Tokens::get();
        let gap = t.space_4;
        let half = ((ui.available_width() - gap) / 2.0).max(0.0);
        let mut changed = false;
        for (index, axis) in ["X", "Y"].into_iter().enumerate() {
            ui.allocate_ui_with_layout(
                vec2(half, t.control_height),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.spacing_mut().text_edit_width = half;
                    changed |= number_field(
                        ui,
                        &format!("{label}-{axis}"),
                        &mut value[index],
                        unit,
                        range.clone(),
                    );
                },
            );
        }
        changed
    })
}

/// A full-width dropdown with the kit chevron. `options` pairs values with
/// labels; returns whether the value changed.
pub fn dropdown<T: PartialEq + Clone>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut T,
    options: &[(T, String)],
) -> bool {
    let current = options
        .iter()
        .find(|(v, _)| v == value)
        .map_or("—", |(_, label)| label.as_str())
        .to_owned();
    let mut changed = false;
    dropdown_menu(ui, id, &current, |ui| {
        for (option, label) in options {
            if ui
                .selectable_label(option == value, label.as_str())
                .clicked()
                && option != value
            {
                *value = option.clone();
                changed = true;
            }
        }
    });
    changed
}

/// A full-width dropdown whose menu the caller fills.
pub fn dropdown_menu(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    current: &str,
    add: impl FnOnce(&mut Ui),
) -> Response {
    let t = Tokens::get();
    let response = egui::ComboBox::from_id_salt(ui.scope_id().with(id))
        .width(ui.available_width().max(0.0))
        .height(t.space_24 * 14.0)
        .selected_text(current)
        .icon(|ui, rect, _, _| {
            super::chevron(ui.painter(), rect, true, Tokens::get().text_dim);
        })
        .show_ui(ui, add)
        .response;
    t.button_bevel(ui, &response, false);
    response
}

/// One labelled dropdown row.
pub fn choice_row<T: PartialEq + Clone>(
    ui: &mut Ui,
    label: &str,
    value: &mut T,
    options: &[(T, String)],
) -> bool {
    row(ui, label, |ui| dropdown(ui, label, value, options))
}

/// A checkbox in the control column, aligned with the rows above.
pub fn check_row(ui: &mut Ui, label: &str, value: &mut bool) -> bool {
    row(ui, "", |ui| ui.checkbox(value, label).changed())
}

/// Rows a read-only value wraps to before it is cut.
const VALUE_MAX_ROWS: usize = 3;

/// A row of read-only text in the control column. A long value wraps (the
/// row grows, its label stays with the first line); past three lines it
/// is cut and the full text shows on hover, as does a cut label. The
/// tooltip belongs to the text, never to a value that fits: one that
/// repeats the shown value lingers over it while the panel scrolls.
pub fn value_row(ui: &mut Ui, label: &str, value: &str) {
    let t = Tokens::get();
    let width = ui.available_width().max(0.0);
    let label_width = t.form_label_width.min(width * 0.45);
    let value_left = (label_width + t.space_8).min(width);
    let font = egui::FontId::proportional(t.font_label);
    let galley = super::text::wrapped(
        ui,
        value,
        font.clone(),
        t.form_ink,
        (width - value_left).max(1.0),
        VALUE_MAX_ROWS,
    );
    let line = ui.fonts_mut(|f| f.row_height(&font));
    let pad = ((t.control_height - line) / 2.0).max(0.0);
    let height = (galley.size().y + 2.0 * pad).max(t.control_height);
    let (rect, row) = ui.allocate_exact_size(vec2(width, height), egui::Sense::hover());
    let label_rect = egui::Rect::from_min_size(rect.min, vec2(label_width, t.control_height));
    super::text::line(
        ui,
        row.id.with("label"),
        label_rect,
        label,
        "",
        font,
        t.dialog_label,
    );
    let value_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + value_left, rect.top() + pad),
        galley.size(),
    );
    let cut = galley.elided;
    ui.painter().galley(value_rect.min, galley, t.form_ink);
    let response = ui.interact(value_rect, row.id.with("value"), egui::Sense::hover());
    super::text::hover(response, value, "", cut);
}

/// Semantic ink of a form message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Dim,
    Note,
    Warning,
    Error,
}

fn tone_ink(tone: Tone) -> egui::Color32 {
    let t = Tokens::get();
    match tone {
        Tone::Dim => t.text_dim,
        Tone::Note => t.form_primary,
        Tone::Warning => t.warning,
        Tone::Error => t.error,
    }
}

/// Full-width wrapping message outside any row.
pub fn status(ui: &mut Ui, text: &str, tone: Tone) -> Response {
    let t = Tokens::get();
    ui.add(egui::Label::new(RichText::new(text).size(t.font_label).color(tone_ink(tone))).wrap())
}

/// A small dim note, for guidance under a group of rows.
pub fn note(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::get();
    ui.add(egui::Label::new(RichText::new(text).size(t.font_small).color(t.form_dim)).wrap())
}

/// Persisted disclosure with an uppercase caption, a rule and an optional
/// count badge (upstream `layout::section_count`). Returns `add`'s result
/// when open.
pub fn section<R>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    title: &str,
    default_open: bool,
    count: Option<usize>,
    add: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    section_with(ui, id, title, default_open, count, None, add)
}

/// A [`section`] that shows `summary` (one dim line, such as "0, 0, 0 mm ·
/// 0°") after its caption while folded, so a folded section still says
/// what it holds.
pub fn summary_section<R>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    title: &str,
    default_open: bool,
    summary: &str,
    add: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    section_with(ui, id, title, default_open, None, Some(summary), add)
}

fn section_with<R>(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    title: &str,
    default_open: bool,
    count: Option<usize>,
    summary: Option<&str>,
    add: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let t = Tokens::get();
    ui.add_space(t.space_2);
    let id = ui.make_persistent_id(("section", id));
    let mut open = ui
        .ctx()
        .data_mut(|d| d.get_persisted::<bool>(id))
        .unwrap_or(default_open);
    let width = ui.available_width().max(0.0);
    let (rect, response) =
        ui.allocate_exact_size(vec2(width, t.control_height), egui::Sense::click());
    if response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect.shrink2(vec2(0.0, 1.0)), t.radius_sm, t.hover);
    }
    let icon_center = egui::pos2(
        rect.left() + t.form_section_icon_size / 2.0,
        rect.center().y,
    );
    super::disclosure(ui.painter(), icon_center, open, t.form_dim);
    let caption = title.to_uppercase();
    let mut format = egui::TextFormat::simple(type_face::semibold(t.font_caption), t.form_dim);
    format.extra_letter_spacing = t.font_caption * t.font_section_tracking;
    let galley = ui
        .painter()
        .layout_job(egui::text::LayoutJob::single_section(caption, format));
    let text_left = icon_center.x + t.form_section_icon_size / 2.0 + t.form_gap_y;
    let title_right = text_left + galley.size().x;
    ui.painter().galley(
        egui::pos2(text_left, rect.center().y - galley.size().y / 2.0),
        galley,
        t.form_dim,
    );
    let mut rule_left = title_right + t.space_12;
    if let Some(count) = count {
        let text = count.to_string();
        let galley =
            ui.painter()
                .layout_no_wrap(text, egui::FontId::proportional(t.font_small), t.form_dim);
        let badge = egui::Rect::from_min_size(
            egui::pos2(rule_left, rect.center().y - t.space_10),
            vec2(galley.size().x + t.space_12, t.space_10 * 2.0),
        );
        if badge.right() < rect.right() {
            ui.painter()
                .rect_filled(badge, t.radius_xl, t.dialog_footer);
            ui.painter()
                .galley(badge.center() - galley.size() / 2.0, galley, t.form_dim);
            rule_left = badge.right() + t.space_12;
        }
    }
    let mut summary_cut = false;
    if let Some(summary) = summary.filter(|s| !open && !s.is_empty()) {
        let galley = super::text::one_line(
            ui,
            summary,
            egui::FontId::proportional(t.font_small),
            t.form_dim,
            (rect.right() - rule_left).max(0.0),
        );
        summary_cut = galley.elided;
        let width = galley.size().x;
        ui.painter().galley(
            egui::pos2(rule_left, rect.center().y - galley.size().y / 2.0),
            galley,
            t.form_dim,
        );
        rule_left += width + t.space_12;
    }
    if rule_left < rect.right() {
        t.secondary_hline(
            ui.painter(),
            rule_left..=rect.right(),
            rect.center().y,
            t.dialog_rule,
        );
    }
    response.widget_info(|| {
        let name = match summary.filter(|s| !open && !s.is_empty()) {
            Some(summary) => format!("{title}: {summary}"),
            None => title.to_owned(),
        };
        egui::WidgetInfo::selected(egui::Role::DisclosureTriangle, true, open, &name)
    });
    let response = match summary {
        Some(summary) if summary_cut => response.on_hover_text(summary),
        _ => response,
    };
    if response.clicked() {
        open = !open;
    }
    ui.ctx().data_mut(|d| d.insert_persisted(id, open));
    open.then(|| ui.push_id(id, add).inner)
}

/// Right-aligned action buttons under a form, primary last.
pub fn actions(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let t = Tokens::get();
    ui.add_space(t.space_4);
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), t.control_height),
        Layout::right_to_left(Align::Center),
        add,
    );
}

/// The dialog's primary action. Without an action to take it is a plain
/// disabled button: a disabled accent fill reads as available.
pub fn primary(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    ui_kit::button(
        ui,
        label,
        if enabled {
            ui_kit::Button::Primary
        } else {
            ui_kit::Button::Enabled(false)
        },
    )
}

/// A rule between groups of rows.
pub fn rule(ui: &mut Ui) {
    let t = Tokens::get();
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), t.space_8), egui::Sense::hover());
    t.secondary_hline(ui.painter(), rect.x_range(), rect.center().y, t.dialog_rule);
}

/// One form row (`add`) with `key`, the key of the command that steps
/// this setting as the keymap has it, as a chip at the right of the
/// label column; no chip without a key. Settings rows of a tool card use
/// it so the key that changes a setting shows beside it.
pub fn keyed<R>(ui: &mut Ui, key: Option<String>, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get();
    let start = ui.cursor().min;
    let width = ui.available_width().max(0.0);
    let inner = add(ui);
    if let Some(key) = key {
        let label_width = t.form_label_width.min(width * 0.45);
        let column = egui::Rect::from_min_size(start, vec2(label_width, t.control_height));
        key_chip(ui, column, &key);
    }
    inner
}

/// A key chip right-aligned and centred in `column`.
fn key_chip(ui: &Ui, column: egui::Rect, key: &str) {
    let t = Tokens::get();
    let galley = ui.painter().layout_no_wrap(
        key.to_owned(),
        egui::FontId::proportional(t.font_small),
        t.form_dim,
    );
    let pad = vec2(t.space_4, t.space_4 / 2.0);
    let size = galley.size() + 2.0 * pad;
    let chip = egui::Rect::from_min_size(
        egui::pos2(column.right() - size.x, column.center().y - size.y / 2.0),
        size,
    );
    ui.painter().rect_stroke(
        chip,
        t.control_radius,
        egui::Stroke::new(1.0, t.border),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(chip.min + pad, galley, t.form_dim);
}

/// The pointer over a canvas: a crosshair while a drawing or placing
/// tool is armed, so every editor shows an armed tool the same way.
pub fn tool_cursor(ui: &Ui, canvas: &Response, armed: bool) {
    if armed && canvas.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B-h: invalid text kept in a field it lost focus from keeps the error
    /// look and says on hover that it is not stored; valid text does not.
    #[test]
    fn an_invalid_field_says_it_is_not_stored() {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut time = 0.0;
        let mut frame = |text: &mut String, invalid: bool, events: Vec<egui::Event>| {
            time += 0.4;
            let mut rect = egui::Rect::NOTHING;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        vec2(360.0, 200.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| rect = text_field(ui, "value", text, "", invalid).rect,
            );
            output.textures_delta.clear();
            fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
                match shape {
                    egui::Shape::Text(t) => out.push(t.galley.job.text.clone()),
                    egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                    _ => {}
                }
            }
            let mut out = Vec::new();
            for shape in &output.shapes {
                texts(&shape.shape, &mut out);
            }
            (rect, out)
        };
        for invalid in [true, false] {
            let mut text = "ten kilo".to_owned();
            let (rect, _) = frame(&mut text, invalid, vec![]);
            let mut shown = Vec::new();
            frame(
                &mut text,
                invalid,
                vec![egui::Event::PointerMoved(rect.center())],
            );
            for _ in 0..4 {
                shown = frame(&mut text, invalid, vec![]).1;
            }
            assert_eq!(
                shown.iter().any(|t| t == NOT_STORED),
                invalid,
                "invalid {invalid}: {shown:?}"
            );
        }
    }

    #[test]
    fn numbers_read_with_or_without_their_unit_and_print_without_trailing_zeros() {
        assert_eq!(parse_number("2.5 mm", "mm"), Some(2.5));
        assert_eq!(parse_number(" -4 ", "mm"), Some(-4.0));
        assert_eq!(parse_number("2.5mm", "mm"), Some(2.5));
        assert_eq!(parse_number("abc", "mm"), None);
        assert_eq!(parse_number("inf", ""), None);
        assert_eq!(number_text(6.0), "6");
        assert_eq!(number_text(-0.00001), "0");
        assert_eq!(number_text(0.25), "0.25");
    }
}
