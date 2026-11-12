//! Fields elements for dense cards; see `docs/ui-kit.md` for tokens and usage.
use super::{
    layout::{label, publish},
    metrics::*,
};
use crate::tokens::Tokens;
use egui::{Response, Ui, vec2};

/// One alignment policy for every kit text input. Align the galley itself rather than
/// approximating font metrics with integer margins (Plex Mono and Sans differ).
pub(super) fn text_input(edit: egui::TextEdit<'_>) -> egui::TextEdit<'_> {
    let t = Tokens::get();
    edit.align(egui::Align2::LEFT_CENTER).margin(vec2(t.form_inset, 0.0)).min_size(vec2(0.0, t.control_height))
}

/// The host's expression language, as the kit's value fields use it. The kit draws and edits the
/// text; it never parses expressions itself. The host installs one per context with
/// [`install_expressions`]. Without one, a field's label drag changes nothing (no text is a
/// literal), an editor offers no completion or stepping, and a length field edits plain
/// millimetres: scale 1.0 and suffix "mm", shown with `decimals` places and read back as a
/// plain number.
pub trait Expressions: Send + Sync {
    /// Whether the trimmed `text` is a plain literal: a number with an optional unit ("20 mm",
    /// "-1.5 in"), not a parameter or a formula. Only a literal can be changed by a label drag.
    fn is_literal(&self, text: &str) -> bool;
    /// Whether `unit`, the text after a literal's number as written (" mm", "in", ""), is a unit
    /// of the language. An empty unit is a unit-less number.
    fn is_unit(&self, unit: &str) -> bool;
    /// Offer completion and stepping for this frame under the editor `response` over `text`.
    /// Returns true when either changed the text; that frame's Enter then accepts the change
    /// instead of submitting.
    fn complete(&self, ui: &mut Ui, response: &Response, text: &mut String) -> bool;
    /// The display unit for the design unit name `units`: units per millimetre and the unit's
    /// name, as a length field's suffix shows it.
    fn unit_scale(&self, units: &str) -> (f64, &'static str);
    /// The millimetre length `mm` as a number in the display unit `unit`, without the unit,
    /// with `decimals` places (the language may show more where the unit needs them).
    fn length_number(&self, mm: f64, unit: &str, decimals: u8) -> String;
    /// The length typed as `text` in millimetres: a bare number is in the display unit (`scale`
    /// units per millimetre), an explicit unit is honoured. None when `text` is not a length.
    fn parse_length_mm(&self, text: &str, scale: f64) -> Option<f64>;
}

/// A plain number typed into a field, with a decimal point or a decimal comma (#1347): "2.5"
/// and "2,5" are both 2.5, as a numeric keypad types either depending on the locale. Spaces are
/// ignored and "−" is a minus, as in egui's own parser. A comma beside a point ("1.234,5") or a
/// second comma is not guessed at: the text is not a number.
pub fn parse_number(text: &str) -> Option<f64> {
    let t: String = text.chars().filter(|c| !c.is_whitespace()).map(|c| if c == '\u{2212}' { '-' } else { c }).collect();
    let t = if !t.contains('.') && t.matches(',').count() == 1 { t.replacen(',', ".", 1) } else { t };
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The whole numbers a count field may hold, and how its text evaluates (parameters and
/// expressions included). `eval` returns None for text that is not a value.
pub struct Count<'a> {
    pub min: i64,
    pub max: i64,
    pub eval: &'a dyn Fn(&str) -> Option<f64>,
}

/// A value a label drag can change: a number with an optional unit ("20 mm", "1,5 in", "45°").
#[derive(Clone, Debug, PartialEq)]
pub struct Literal {
    pub number: f64,
    /// The text after the number, as written (" mm").
    pub unit: String,
    pub comma: bool,
}

/// An ongoing label drag: the value and pointer x it is measured from, and the speed in use.
#[derive(Clone)]
struct Scrub {
    anchor: f64,
    origin: f32,
    step: f64,
}

/// Supplied entry in a searchable dropdown. Handles are optional stable application aliases.
pub struct SearchOption<'a> {
    pub value: &'a str,
    pub label: &'a str,
    pub handle: Option<&'a str>,
}

struct SearchMenu<'a> {
    custom: Option<&'a str>,
    pending: Option<&'a str>,
    icon: Option<&'static str>,
}

/// One row of a `search_picker`. The value, label and tip are searched; `tip` also shows on hover,
/// and a change of `group` between rows draws a heading.
pub struct SearchChoice<'a> {
    pub value: &'a str,
    pub label: &'a str,
    pub tip: &'a str,
    pub group: Option<&'a str>,
}

/// What a `search_picker` shows.
pub struct SearchPicker<'a> {
    /// ComboBox id salt; it also keys the search text, so one per picker in the window.
    pub id: &'a str,
    /// Scenario names: the control `{handles}`, its box `{handles}:search`, its custom row
    /// `{handles}:custom` and its rows `{handles}:{value}`.
    pub handles: &'a str,
    /// The text on the closed control, and the value of the row to highlight.
    pub current: &'a str,
    pub current_value: &'a str,
    /// The search box's empty-text hint.
    pub hint: &'a str,
    pub choices: &'a [SearchChoice<'a>],
    /// Offer what was typed as a value of its own when it is not a listed one.
    pub custom: bool,
    /// A dim line under the list: still listing, or why it could not.
    pub note: Option<&'a str>,
    /// With two or more groups, each folds under its header. Before anything is typed, only the
    /// current value's group and this one are open (every group, when neither names one); a group
    /// with a match opens once something is typed; a fold made by hand stays until the popup closes.
    pub open_group: Option<&'a str>,
    pub enabled: bool,
}

/// A `search_picker` choice: a listed value, or one typed into the box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Picked {
    Listed(String),
    Custom(String),
}

enum SearchRow<'a> {
    /// A lone group's dim heading.
    Heading(&'a str),
    /// One of several groups: its header, with how many of its choices are listed, folded or open.
    Group {
        name: &'a str,
        count: usize,
        open: bool,
    },
    Choice(&'a SearchChoice<'a>),
}

/// Selection target state; filled targets retain their reserved trailing space.
pub struct Selection {
    pub active: bool,
    pub filled: bool,
}

/// Stable-ID inline editor for tables. Completion is owned by the caller so Enter can commit
/// a cell without submitting its panel. Tokens: control_height/fill, form_inset, font_label, text/error.
pub fn cell_editor(ui: &mut Ui, id: egui::Id, value: &mut String, hint: &str, invalid: bool) -> Response {
    bounded_cell_editor(ui, id, value, hint, invalid, usize::MAX)
}

/// Stable-ID cell/name editor with a caller-supplied character cap. Tokens: cell_editor tokens.
pub fn bounded_cell_editor(ui: &mut Ui, id: egui::Id, value: &mut String, hint: &str, invalid: bool, limit: usize) -> Response {
    cell_editor_frame(ui, id, value, hint, invalid, limit, true)
}

fn cell_editor_frame(ui: &mut Ui, id: egui::Id, value: &mut String, hint: &str, invalid: bool, limit: usize, frame: bool) -> Response {
    let t = Tokens::get();
    let editor = text_input(egui::TextEdit::singleline(value))
        .char_limit(limit)
        .id(id)
        .hint_text(solidcraft_ui_text::tr_label(hint))
        .font(if frame { egui::FontId::monospace(t.font_label) } else { egui::FontId::proportional(t.font_label) })
        .background_color(if invalid { t.form_error_fill } else { t.input_fill() })
        .text_color(if invalid { t.form_error } else { t.form_ink })
        .clip_text(true);
    let editor = if invalid {
        editor.frame(
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(t.form_inset as i8, 0))
                .fill(t.form_error_fill)
                .stroke(egui::Stroke::new(1.0, t.form_error))
                .corner_radius(t.control_radius),
        )
    } else if frame {
        editor
    } else {
        editor.frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(t.space_8 as i8, 0)))
    };
    let mut response = ui.add_sized(vec2(dimension(ui.available_width()), row_height()), editor);
    super::text_menu::menu(ui, &mut response, value, super::text_menu::Field::LINE.limited(limit));
    if frame {
        publish(ui, &response, egui::Role::TextInput);
    } else {
        super::layout::publish_geometry(ui, &response);
    }
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::Role::TextInput, ui.is_enabled(), solidcraft_ui_text::tr_label(label(ui)));
        info.current_text_value = Some(value.clone());
        info
    });
    crate::tokens::TabAccent::number_focus(ui, &response);
    t.input_bevel(ui, &response);
    response
}

/// Behaviour of the one kit slider. Appearance is always owned by `slider`.
/// Values/ranges stay in caller units; formatting never rounds saved values during drawing.
pub struct Slider {
    pub step: Option<f64>,
    pub format: fn(f64) -> String,
    pub show_value: bool,
    pub integer: bool,
    pub workspace_accent: bool,
    /// Blur's off endpoint uses a muted hollow thumb; positive values use solid accent.
    pub off_endpoint: bool,
}

impl Default for Slider {
    fn default() -> Self {
        Self { step: None, format: |v| format!("{v:.2}"), show_value: true, integer: false, workspace_accent: false, off_endpoint: false }
    }
}
