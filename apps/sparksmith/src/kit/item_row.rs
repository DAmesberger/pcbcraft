//! The one row of a Properties list (Dimensions, Board parameters, Named
//! edges, a length group's members, Connections): what the item is, its
//! value and its end action on one line.
//!
//! Anatomy, left to right, in one band of `control_height + space_4`
//! (taller only when the name wraps to its second line):
//!
//! - **Name.** The item in a designer's words. Several parts read as one
//!   relation with a dim arrow between them ("Front face → bottom edge").
//!   It takes the width the other cells leave: one line in the label font,
//!   else two lines in the small font (the row keeps its height), and only
//!   then ends in "…", with the full name on its hover.
//! - **Badge.** An optional status after the name (a lost or inexact
//!   reference), a small pill in its severity's ink.
//! - **Value.** One aligned column ([`value_column`]): a compact field
//!   ([`value_field`]) whose driven value reads `fx width / 2 = 20 mm`
//!   while it is not being edited, or read-only text ([`value_text`]), or
//!   any control the caller draws.
//! - **Action.** A small icon button at the row's end (delete, remove),
//!   drawn while the row is hovered or selected or the button has the
//!   keyboard. It is always in the tab order and the accessibility tree,
//!   under a name that says what it acts on ("Delete dimension Origin →
//!   left edge").
//!
//! The row is on the selection's background when selected and on the
//! hover background under the pointer; a selectable row is a button that
//! selects its item (click, or Enter with the keyboard).
//!
//! Adapted from Solidcraft 1f6e28a `crates/ui-egui/src/ui_kit/table.rs`
//! (`Table::row_content`: the selected/hover row band and its selectable
//! button role; `bin_button`: the icon-only row delete),
//! `ui_kit/layout.rs` (`list_row`: a picked item with its remove action
//! at the row's end, hover lighting the item in the view), `actions.rs`
//! (`icon_button_presentation`: hover fill, focus ink, button role and
//! label) and `name_list.rs` (the name takes the width the other cells
//! leave and is cut only when that is too narrow), MIT option; see
//! ATTRIBUTION.md. Upstream keeps these crate-private, so the geometry is
//! reproduced on the public tokens, fonts and icons.
use super::severity::{self, Level};
use egui::text::{LayoutJob, TextFormat};
use egui::{Align, FontId, Layout, Rect, Response, Sense, Ui, UiBuilder, pos2, vec2};
use sparksmith_ui::theme::Tokens;
use sparksmith_ui::{icons, ui_kit};

/// Rows a name wraps to before it is cut.
const NAME_MAX_ROWS: usize = 2;
/// The most of a row the value column takes.
const VALUE_SHARE: f32 = 0.42;
/// The arrow between the parts of a name.
pub const ARROW: &str = " \u{2192} ";

/// What a row shows and offers. `name` parts are joined by a dim arrow.
pub struct ItemRow<'a> {
    pub name: &'a [&'a str],
    /// On the selection's background.
    pub selected: bool,
    /// A click (or Enter) on the row selects its item.
    pub selectable: bool,
    /// A status after the name, such as (Warning, "Inexact").
    pub badge: Option<(Level, &'a str)>,
    /// The icon button at the row's end.
    pub action: Option<RowAction<'a>>,
    /// The value column's width ([`value_column`]); 0 for no value.
    pub value_width: f32,
}

/// A row's end action: an icon, its accessible name and its hover help
/// (the name when empty).
#[derive(Clone, Copy)]
pub struct RowAction<'a> {
    pub icon: &'a str,
    pub label: &'a str,
    pub tip: &'a str,
}

impl<'a> RowAction<'a> {
    /// A delete or remove action named `label` ("Delete dimension …").
    #[allow(
        dead_code,
        reason = "Dimensions and Board parameters moved to kit::list; the remaining item_row lists move in wave 2"
    )]
    pub fn delete(label: &'a str) -> Self {
        Self {
            icon: "delete",
            label,
            tip: "",
        }
    }
}

impl<'a> ItemRow<'a> {
    pub fn new(name: &'a [&'a str], value_width: f32) -> Self {
        Self {
            name,
            selected: false,
            selectable: false,
            badge: None,
            action: None,
            value_width,
        }
    }
}

/// What a row did this frame.
#[allow(dead_code, reason = "the lists still on item_row read only some of it")]
pub struct ItemRowResponse {
    /// The row itself (not its value or action) was clicked.
    pub clicked: bool,
    /// The pointer is anywhere on the row, its value and action included.
    pub hovered: bool,
    /// The end action was clicked.
    pub action: bool,
    /// The row's band, for scrolling it into view.
    pub response: Response,
}

/// The name as one line of text: "Front face → bottom edge".
pub fn name_text(parts: &[&str]) -> String {
    parts.join(ARROW)
}

/// The width of the value column that shows every one of `values` (each
/// as its field shows it at rest), at most 42% of a row; at least a short
/// length's.
pub fn value_column<S: AsRef<str>>(ui: &Ui, values: impl IntoIterator<Item = S>) -> f32 {
    let t = Tokens::get();
    let font = FontId::monospace(t.font_label);
    let widest = values
        .into_iter()
        .map(|v| {
            ui.painter()
                .layout_no_wrap(v.as_ref().to_owned(), font.clone(), t.form_ink)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max);
    (widest + 2.0 * t.form_inset + t.space_4)
        .ceil()
        .max(t.control_height * 2.5)
}

/// The name in the label font on one line; where that does not fit, in
/// the small font on up to two lines, so the row keeps its height; past
/// that cut with an ellipsis.
fn name_galley(ui: &Ui, parts: &[&str], width: f32) -> std::sync::Arc<egui::Galley> {
    let t = Tokens::get();
    let layout = |size: f32, rows: usize| {
        let font = FontId::proportional(size);
        let mut job = LayoutJob::default();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                job.append(ARROW, 0.0, TextFormat::simple(font.clone(), t.form_dim));
            }
            job.append(part, 0.0, TextFormat::simple(font.clone(), t.form_ink));
        }
        job.wrap.max_width = width.max(1.0);
        job.wrap.max_rows = rows;
        job.wrap.overflow_character = Some(super::text::ELLIPSIS);
        ui.painter().layout_job(job)
    };
    let one = layout(t.font_label, 1);
    if one.elided {
        layout(t.font_small, NAME_MAX_ROWS)
    } else {
        one
    }
}

/// One list row: the name, its badge, the value drawn by `value` in the
/// value column, and the end action. Returns what the row did and
/// `value`'s result.
pub fn item_row<R>(
    ui: &mut Ui,
    row: &ItemRow<'_>,
    value: impl FnOnce(&mut Ui) -> R,
) -> (ItemRowResponse, R) {
    let t = Tokens::get();
    let width = ui.available_width().max(0.0);
    let action_size = if row.action.is_some() {
        t.control_height - t.space_6
    } else {
        0.0
    };
    let action_gap = if row.action.is_some() { t.space_2 } else { 0.0 };
    let value_width = row.value_width.clamp(0.0, width * VALUE_SHARE);
    let value_gap = if value_width > 0.0 { t.space_8 } else { 0.0 };
    let inset = t.space_6;
    let badge = row.badge.map(|(level, text)| {
        let galley = ui.painter().layout_no_wrap(
            text.to_owned(),
            FontId::proportional(t.font_badge),
            severity::ink(level),
        );
        (level, galley)
    });
    let badge_width = badge
        .as_ref()
        .map_or(0.0, |(_, g)| g.size().x + t.space_8 + t.space_6);
    let name_width =
        (width - inset - value_width - value_gap - action_size - action_gap - badge_width).max(1.0);
    let galley = name_galley(ui, row.name, name_width);
    let cut = galley.elided;
    let height = (galley.size().y + t.space_4).max(t.control_height + t.space_4);
    let sense = if row.selectable {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
    let hovered = ui.is_enabled() && ui.rect_contains_pointer(rect);
    let band = rect.expand2(vec2(t.space_4, 0.0));
    if row.selected {
        ui.painter()
            .rect_filled(band, t.control_radius, t.selected_bg);
    } else if hovered {
        ui.painter().rect_filled(band, t.control_radius, t.hover);
    }
    if row.selectable && response.has_focus() {
        ui.painter().rect_stroke(
            band,
            t.control_radius,
            egui::Stroke::new(1.0, t.form_primary),
            egui::StrokeKind::Inside,
        );
    }
    let name_left = rect.left() + inset;
    let name_size = galley.size();
    ui.painter().galley(
        pos2(name_left, rect.center().y - name_size.y / 2.0),
        galley,
        t.form_ink,
    );
    let full = name_text(row.name);
    if let Some((level, galley)) = badge {
        let size = galley.size() + vec2(t.space_8, t.space_2);
        let pill = Rect::from_min_size(
            pos2(
                name_left + name_size.x + t.space_6,
                rect.center().y - size.y / 2.0,
            ),
            size,
        );
        let ink = severity::ink(level);
        ui.painter()
            .rect_filled(pill, t.radius_xl, ink.gamma_multiply(0.18));
        ui.painter()
            .galley(pill.center() - galley.size() / 2.0, galley, ink);
    }
    let label = match row.badge {
        Some((_, text)) => format!("{full} · {text}"),
        None => full.clone(),
    };
    let role = if row.selectable {
        egui::Role::Button
    } else {
        egui::Role::Label
    };
    response.widget_info(|| egui::WidgetInfo::selected(role, true, row.selected, &label));
    if cut {
        // The full name on the name's own hover, so it never stacks with
        // the value's or the action's.
        let name_rect = Rect::from_min_size(
            pos2(name_left, rect.center().y - name_size.y / 2.0),
            name_size,
        );
        ui.interact(name_rect, response.id.with("item-name"), Sense::hover())
            .on_hover_text(&full);
    }
    let value_rect = Rect::from_min_size(
        pos2(
            rect.right() - action_size - action_gap - value_width,
            rect.center().y - t.control_height / 2.0,
        ),
        vec2(value_width, t.control_height),
    );
    // A field in the value column is named by the row.
    let key = super::form::row_label_key();
    ui.data_mut(|d| d.insert_temp(key, response.id));
    let inner = ui
        .scope_builder(
            UiBuilder::new()
                .id_salt("item-value")
                .max_rect(value_rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(value_rect.expand(1.0)));
                ui.spacing_mut().text_edit_width = value_width;
                value(ui)
            },
        )
        .inner;
    ui.data_mut(|d| d.remove::<egui::Id>(key));
    let mut action = false;
    if let Some(a) = row.action {
        let at = Rect::from_center_size(
            pos2(rect.right() - action_size / 2.0, rect.center().y),
            vec2(action_size, action_size),
        );
        let button = ui.interact(at, response.id.with("item-action"), Sense::click());
        let focused = button.has_focus();
        if hovered || row.selected || focused {
            if button.hovered() {
                ui.painter()
                    .rect_filled(at, t.control_radius, t.form_error_fill);
            }
            let lit = button.hovered() || focused;
            icons::paint_icon(
                ui.painter(),
                at.shrink(t.form_icon_inset),
                a.icon,
                if lit { 1.0 } else { 0.6 },
            );
        }
        if focused {
            ui.painter().rect_stroke(
                at,
                t.control_radius,
                egui::Stroke::new(1.0, t.form_primary),
                egui::StrokeKind::Inside,
            );
        }
        button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, a.label));
        let button = button.on_hover_cursor(egui::CursorIcon::PointingHand);
        let button = if a.tip.is_empty() {
            button.on_hover_text(a.label)
        } else {
            button.on_hover_text(format!("{}\n{}", a.label, a.tip))
        };
        action = button.clicked();
    }
    (
        ItemRowResponse {
            clicked: response.clicked(),
            hovered,
            action,
            response,
        },
        inner,
    )
}

/// The compact field of a row's value column. While not being edited, a
/// `driven` value reads `fx width / 2 = 20 mm` (`text` is the expression,
/// `driven` its result), or `fx 20 mm` where that does not fit, with the
/// whole on hover; plain text that does not fit ends in "…". `invalid`
/// marks text that is not stored.
#[allow(
    dead_code,
    reason = "Dimensions and Board parameters moved to kit::list; the remaining item_row lists move in wave 2"
)]
pub fn value_field(
    ui: &mut Ui,
    id: &str,
    text: &mut String,
    invalid: bool,
    driven: Option<&str>,
) -> Response {
    let t = Tokens::get();
    let key = ui.scope_id().with(("item-value", id));
    let response = ui_kit::cell_editor(ui, key, text, "", invalid);
    let response = super::form::named_by_row(ui, response);
    if invalid {
        return super::not_stored(response);
    }
    if response.has_focus() || text.is_empty() || !ui.is_enabled() {
        return response;
    }
    let inner = response.rect.shrink2(vec2(t.form_inset, 1.0));
    let mono = FontId::monospace(t.font_label);
    let small = FontId::proportional(t.font_small);
    let fx = || {
        let mut format =
            TextFormat::simple(egui::FontId::proportional(t.font_small), t.form_primary);
        format.italics = true;
        format
    };
    let full = match driven {
        Some(result) => {
            let mut job = LayoutJob::default();
            job.append("fx", 0.0, fx());
            job.append(
                text,
                t.space_4,
                TextFormat::simple(mono.clone(), t.form_ink),
            );
            job.append(
                &format!(" = {result}"),
                0.0,
                TextFormat::simple(small, t.form_dim),
            );
            job
        }
        None => LayoutJob::simple_singleline(text.clone(), mono.clone(), t.form_ink),
    };
    let whole = full.text.clone();
    let measured = ui.painter().layout_job(full.clone());
    let fits = measured.size().x <= inner.width();
    if fits && driven.is_none() {
        return response;
    }
    let compact = driven
        .map(|result| {
            let mut job = LayoutJob::default();
            job.append("fx", 0.0, fx());
            job.append(result, t.space_4, TextFormat::simple(mono, t.form_ink));
            ui.painter().layout_job(job)
        })
        .filter(|g| g.size().x <= inner.width());
    let galley = if fits {
        measured
    } else if let Some(compact) = compact {
        compact
    } else {
        let mut job = full;
        job.wrap.max_width = inner.width().max(1.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some(super::text::ELLIPSIS);
        ui.painter().layout_job(job)
    };
    ui.painter().rect_filled(inner, 0.0, t.input_fill());
    ui.painter().galley(
        pos2(inner.left(), inner.center().y - galley.size().y / 2.0),
        galley,
        t.form_ink,
    );
    if fits {
        response
    } else {
        response.on_hover_text(whole)
    }
}

/// Read-only text in a row's value column ("Reference", "2 parts"), dim,
/// cut with its full text on hover.
pub fn value_text(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::get();
    let rect = ui.max_rect();
    let id = ui.scope_id().with(("item-text", text));
    super::text::line(
        ui,
        id,
        rect.shrink2(vec2(t.form_inset, 0.0)),
        text,
        "",
        FontId::proportional(t.font_label),
        t.form_dim,
    )
}
