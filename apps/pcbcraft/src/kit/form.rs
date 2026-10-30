//! Form rows and controls (upstream `ui_kit/layout.rs` and `fields.rs`).
use egui::{Align, Layout, Response, RichText, Ui, UiBuilder, vec2};
use solidcraft_ui_egui::{fonts, theme::Tokens, ui_kit};

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
    ui.scope_builder(
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
            );
        },
    );
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
            control(ui)
        },
    )
    .inner
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
            .id(ui.id().with(("multiline", label)))
            .font(egui::TextStyle::Monospace)
            .desired_rows(3)
            .desired_width(ui.available_width())
            .text_color(if invalid { t.error } else { t.text });
        ui.add(edit)
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
    let id = ui.id().with(("text", id));
    ui_kit::cell_editor(ui, id, value, hint, invalid)
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
/// expression fields. Out-of-range or malformed text shows the error look and
/// leaves `value` unchanged; leaving the field restores the value's text.
/// Returns whether `value` changed.
pub fn number_field(
    ui: &mut Ui,
    id: &str,
    value: &mut f64,
    unit: &str,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let key = ui.id().with(("number", id));
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
    let mut changed = false;
    if response.has_focus() {
        if let Some(v) = parse_number(&text, unit).filter(|v| range.contains(v))
            && (v - *value).abs() > f64::EPSILON
        {
            *value = v;
            changed = true;
        }
        ui.data_mut(|d| d.insert_temp(key, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(key));
    }
    changed
}

/// One labelled number row.
pub fn number_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f64,
    unit: &str,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    row(ui, label, |ui| number_field(ui, label, value, unit, range))
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
    let response = egui::ComboBox::from_id_salt(ui.id().with(id))
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

/// A row of read-only text in the control column.
pub fn value_row(ui: &mut Ui, label: &str, value: &str) {
    row(ui, label, |ui| {
        let t = Tokens::get();
        ui.add(egui::Label::new(RichText::new(value).color(t.form_ink)).truncate())
            .on_hover_text(value);
    });
}

/// Semantic ink of a form message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Dim,
    Note,
    Warning,
    Error,
}

/// Full-width wrapping message outside any row.
pub fn status(ui: &mut Ui, text: &str, tone: Tone) -> Response {
    let t = Tokens::get();
    let ink = match tone {
        Tone::Dim => t.text_dim,
        Tone::Note => t.form_primary,
        Tone::Warning => t.warning,
        Tone::Error => t.error,
    };
    ui.add(egui::Label::new(RichText::new(text).size(t.font_label).color(ink)).wrap())
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
    let mut format = egui::TextFormat::simple(fonts::semibold(t.font_caption), t.form_dim);
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
    if rule_left < rect.right() {
        t.secondary_hline(
            ui.painter(),
            rule_left..=rect.right(),
            rect.center().y,
            t.dialog_rule,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, title)
    });
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

#[cfg(test)]
mod tests {
    use super::*;

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
