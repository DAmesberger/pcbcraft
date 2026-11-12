//! Layout elements for dense cards; see `docs/ui-kit.md` for tokens and usage.
use super::{metrics::*, text::paint_text};
use crate::kit::BackingTooltipExt as _;
use crate::tokens::Tokens;
use egui::{Response, RichText, Ui, vec2};

pub(super) const FORM: &str = "sc_dense_form";

pub(super) const LABEL: &str = "sc_form_label";
pub(super) const GROW_LABEL: &str = "sc_grow_label_column";

/// Mark a form without global state: children inherit this tag through their UI stack.
/// Inherited form styling and label-column identity. Tokens: `space_4/8, form_gap_y, control_height, font_label, accent, selected_bg, dialog_rule, control_fill/border/radius, text`.
pub fn scope<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    scope_inner(ui, None, add)
}

fn scope_inner<R>(ui: &mut Ui, grow: Option<egui::Id>, add: impl FnOnce(&mut Ui) -> R) -> R {
    let mut info = egui::UiStackInfo::default().with_tag(FORM);
    if let Some(key) = grow {
        info = info.with_tag_value(GROW_LABEL, key);
    }
    ui.scope_builder(egui::UiBuilder::new().ui_stack_info(info), |ui| {
        let t = Tokens::get();
        ui.spacing_mut().item_spacing = vec2(t.space_4, t.form_gap_y);
        ui.spacing_mut().interact_size.y = row_height();
        ui.spacing_mut().button_padding = vec2(t.space_8, t.space_4);
        ui.style_mut().text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(t.font_label));
        ui.style_mut().text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(t.font_label));
        ui.style_mut().text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(t.font_body));
        let visuals = ui.visuals_mut();
        visuals.weak_text_color = Some(t.form_dim);
        visuals.selection.stroke = egui::Stroke::new(1.0, selection_ink(&t));
        visuals.selection.bg_fill = t.selected_bg;
        visuals.widgets.noninteractive.bg_stroke = t.tertiary_border(t.dialog_rule);
        for state in [&mut visuals.widgets.inactive, &mut visuals.widgets.hovered, &mut visuals.widgets.active, &mut visuals.widgets.open] {
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

/// The colour egui draws selected text, a selected button's label and a focused field's outline
/// in, on a form: the form's primary colour, unless it all but vanishes on `selected_bg` (below
/// 1.5:1), as in SolidCraft 95 and CGA where they are the same colour; then the theme's ink for
/// selected elements (#11804). Every other theme keeps its look. Tokens: `form_primary,
/// selected_bg, selected_ink`.
pub fn selection_ink(t: &Tokens) -> egui::Color32 {
    if crate::tokens::contrast_ratio(t.form_primary, t.selected_bg) >= SELECTION_INK_MIN_CONTRAST { t.form_primary } else { t.selected_ink }
}

/// Below this contrast with `selected_bg`, a form's primary colour counts as the selection's own
/// colour (`selection_ink`).
const SELECTION_INK_MIN_CONTRAST: f64 = 1.5;

pub(super) fn in_catalog(ui: &Ui) -> bool {
    ui.stack().iter().any(|node| node.tags().contains("sc_catalog"))
}

/// Read the enclosing row accessibility label. Tokens: `none`.
pub fn label(ui: &Ui) -> &str {
    ui.stack().iter().find_map(|node| node.tags().get_downcast::<String>(LABEL)).map(String::as_str).unwrap_or("")
}

/// Publish accessibility and scenario geometry. Tokens: `none`.
pub(super) fn publish(ui: &Ui, response: &Response, kind: egui::Role) {
    let t = Tokens::get();
    match kind {
        egui::Role::TextInput => {
            t.input_bevel(ui, response);
            if response.has_focus() {
                super::settings_search::note_key_sink(ui.ctx());
            }
        }
        egui::Role::ComboBox | egui::Role::Button => t.button_bevel(ui, response, false),
        _ => {}
    }
    let label = label(ui);
    response.widget_info(|| egui::WidgetInfo::labeled(kind, ui.is_enabled(), solidcraft_ui_text::tr_label(label)));
    publish_geometry(ui, response);
}

/// Semantic ink for a form's explanatory message.
#[derive(Clone, Copy)]
pub enum StatusTone {
    Dim,
    Warning,
    Error,
}

/// Where a [`figure_rows`] field went (in the order given): the field's rect, and whether it is
/// hovered or has the keyboard (its part of the figure lights up).
pub struct FigureSlot {
    pub rect: egui::Rect,
    pub hot: bool,
}

#[allow(dead_code, reason = "composed by the independent Preferences panels of #9863")]
/// Memory belongs to the enclosing dialog ID; animation is avoided so each row keeps its fixed height.
/// Persisted disclosure with a rule and uppercase title. Tokens: `control_height, space_2, radius_sm, hover, dialog_rule, form_section_icon_size, form_gap_y, font_caption, font_section_tracking, text_dim`.
/// Static section caption for a group that stays expanded.
pub fn section_heading(ui: &mut Ui, title: &str) {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(vec2(dimension(ui.available_width()), t.space_16), egui::Sense::hover());
    paint_text(ui, rect, &solidcraft_ui_text::tr_label(title).to_uppercase(), crate::kit::type_face::semibold(t.font_caption), t.form_dim, true);
    solidcraft_ui_registry::publish_rect(&format!("dialog:heading:{title}"), rect);
}

/// Publish geometry without replacing native accessibility. Tokens: `none`.
pub(super) fn publish_geometry(ui: &Ui, response: &Response) {
    let label = label(ui);
    solidcraft_ui_registry::publish_rect(&format!("dialog:widget:{label}"), response.rect);
    // A click lands where the field shows: a field scrolled out of a dialog's field area, under
    // its footer, has no handle (a click at its centre would press the button over it).
    let shown = response.rect.intersect(ui.clip_rect());
    if shown.is_positive() {
        solidcraft_ui_registry::publish_handle(&format!("dialog:{label}"), shown.center());
    }
}

/// Read-only text variants retain their original wrapping, allocation and scenario identities.
pub enum Text<'a> {
    Hint(&'a str),
    Note(&'a str),
    MonoNote(&'a str),
    Report(&'a str, bool),
    Error(&'a str),
    Heading(&'a str),
    Inline,
    Cell,
    NameCell(bool),
    SelectedNameCell,
    MonoCell,
    Value(&'a str, bool),
}

fn note_galley(ui: &Ui, text: &str) -> std::sync::Arc<egui::Galley> {
    let t = Tokens::get();
    ui.painter().layout(text.to_owned(), egui::FontId::proportional(t.font_small), t.form_dim, dimension(ui.available_width()))
}

pub fn text(ui: &mut Ui, text: &str, style: Text<'_>) -> Response {
    text_impl(ui, text, style, None)
}

fn text_impl(ui: &mut Ui, text: &str, style: Text<'_>, hover_hint: Option<&str>) -> Response {
    if super::settings_search::querying(ui) {
        let heading = matches!(style, Text::Heading(_));
        if heading {
            let hit = super::settings_search::texts_hit(ui, &[text]);
            super::settings_search::set_heading_open(ui, hit);
            if hit {
                super::settings_search::reveal(ui);
            } else if super::settings_search::suppress_chrome(ui) {
                return super::settings_search::hidden_response(ui);
            }
        } else if super::settings_search::suppress_chrome(ui) && !super::settings_search::texts_hit(ui, &[text]) {
            return super::settings_search::hidden_response(ui);
        } else if super::settings_search::texts_hit(ui, &[text]) && !super::settings_search::shows_all(ui) {
            super::settings_search::reveal(ui);
        }
    }
    let t = Tokens::get();
    let fixed = matches!(style, Text::Cell | Text::NameCell(_) | Text::SelectedNameCell | Text::MonoCell | Text::Value(_, _));
    let ink = match style {
        Text::Hint(_) | Text::Note(_) | Text::MonoNote(_) => t.form_dim,
        Text::Inline if in_catalog(ui) => t.form_dim,
        Text::Report(_, true) | Text::Value(_, true) | Text::Error(_) => t.form_error,
        Text::SelectedNameCell => t.selected_ink,
        _ => t.form_ink,
    };
    let font = match style {
        Text::Inline if in_catalog(ui) => crate::kit::type_face::semibold(t.font_small),
        Text::NameCell(true) | Text::SelectedNameCell => crate::kit::type_face::semibold(t.font_label),
        Text::NameCell(false) => crate::kit::type_face::medium(t.font_label),
        Text::Heading(_) => crate::kit::type_face::semibold(t.font_button),
        Text::MonoCell | Text::Value(_, _) => egui::FontId::monospace(t.font_body),
        Text::MonoNote(_) => egui::FontId::monospace(t.font_small),
        Text::Hint(_) | Text::Error(_) => egui::FontId::proportional(t.font_body),
        Text::Note(_) => egui::FontId::proportional(t.font_small),
        _ => egui::FontId::proportional(t.font_label),
    };
    let response = if fixed {
        let (rect, response) = ui.allocate_exact_size(
            vec2(dimension(ui.available_width()), row_height()),
            if matches!(style, Text::Value(_, _)) { egui::Sense::hover() } else { egui::Sense::click() },
        );
        let tooltip = hover_hint.map(|hint| {
            if ui.painter().layout_no_wrap(text.to_owned(), font.clone(), ink).size().x > rect.width() {
                format!("{text}\n{hint}")
            } else {
                hint.to_owned()
            }
        });
        paint_text(ui, rect, text, font, ink, false);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), text));
        response.backing_on_hover_text(tooltip.as_deref().unwrap_or(text))
    } else if matches!(style, Text::Error(_)) {
        crate::kit::Container::show_backing(
            ui,
            egui::Frame::NONE
                .fill(t.form_error_fill)
                .corner_radius(t.control_radius)
                .inner_margin(egui::Margin::symmetric(t.form_inset as i8, t.space_6 as i8)),
            |ui| {
                ui.set_min_width(dimension(ui.available_width()));
                ui.add(egui::Label::new(RichText::new(text).font(font).color(ink)).wrap())
            },
        )
        .inner
    } else if matches!(style, Text::Hint(_) | Text::Note(_) | Text::MonoNote(_)) {
        let width = dimension(ui.available_width());
        let galley = if matches!(style, Text::Note(_)) { note_galley(ui, text) } else { ui.painter().layout(text.to_owned(), font, ink, width) };
        let padding = if matches!(style, Text::Note(_) | Text::MonoNote(_)) { t.space_4 } else { 0.0 };
        let (rect, response) = ui.allocate_exact_size(vec2(width, galley.size().y + padding * 2.0), egui::Sense::hover());
        ui.painter().galley(egui::pos2(rect.left(), rect.top() + padding), galley, ink);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), text));
        response
    } else {
        let label = egui::Label::new(RichText::new(text).font(font).color(ink));
        ui.add(if matches!(style, Text::Inline) { label.truncate() } else { label.wrap() })
    };
    let response = if matches!(style, Text::Hint(_)) { response.backing_on_hover_text(text) } else { response };
    let (kind, key) = match style {
        Text::Hint(key) => ("hint", key),
        Text::Note(key) | Text::MonoNote(key) => ("note", key),
        Text::Report(key, _) | Text::Value(key, _) | Text::Error(key) | Text::Heading(key) => ("report", key),
        Text::Inline => ("inline", text),
        Text::Cell | Text::MonoCell | Text::NameCell(_) | Text::SelectedNameCell => ("cell", text),
    };
    let id = format!("dialog:{kind}:{key}");
    solidcraft_ui_registry::publish_rect(&id, response.rect);
    if matches!(style, Text::Heading(_)) {
        super::settings_search::remember(ui, text, response.rect);
        super::settings_search::follow(ui, text, response.rect);
    }
    if !matches!(style, Text::Note(_) | Text::MonoNote(_)) {
        solidcraft_ui_registry::publish_handle(&id, response.rect.center());
    }
    response
}
