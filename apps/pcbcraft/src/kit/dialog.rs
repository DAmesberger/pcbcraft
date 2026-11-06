//! Docked command dialogs in the subject slot, and the destructive
//! confirmation.
//!
//! Slice K4 (docs/sidebar-ia.md, "Dialogs and reviews"). A dialog has a
//! 40 pt header, a body capped at `body_rows` that is its only scroll, and a
//! pinned 48 pt footer with Cancel and the primary, primary last. A refusal
//! shows once, as the last body row, and is scrolled into view once per
//! refusal; a disabled primary explains itself on hover; a destructive
//! choice is the primary's variant. Esc cancels and Enter gives the primary
//! only while no field has focus, so Esc in a field still reverts it.
//! Adapted from Solidcraft 4f3ac07a `crates/ui-egui/src/ui_kit/panel.rs`
//! (`Panel::feature`, `body_rows`, `header_actions`), `ui_kit/actions.rs`
//! (`footer_with_left`, `ActionLook`'s danger fill, `keyboard`),
//! `dialogs.rs` (`reveal_refusal`) and `ui_kit/confirm.rs` (the Danger
//! tone's disc), MIT option; see ATTRIBUTION.md.

use super::card::FOOTER_HEIGHT;
use super::severity::{Level, severity_glyph};
use egui::{Align, FontId, Layout, Rect, Response, RichText, Sense, Ui, UiBuilder, pos2, vec2};
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::{fonts, ui_kit};

/// A docked dialog's header height, in points.
pub const DIALOG_HEADER_HEIGHT: f32 = 40.0;

/// The largest `body_rows` a dialog honours (upstream `Panel::body_rows`).
const MAX_BODY_ROWS: f32 = 32.0;

/// How the primary action looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrimaryKind {
    #[default]
    Normal,
    /// Removes or replaces work ("Replace footprint", "Remove pin").
    Destructive,
}

/// A docked command dialog.
pub struct DialogPanel<'a> {
    pub id: egui::Id,
    pub icon: &'a str,
    pub title: &'a str,
    /// The header's kind chip ("New", "Review").
    pub kind: &'a str,
    /// Body height cap, in control rows (1–32).
    pub body_rows: f32,
    pub primary: &'a str,
    pub primary_kind: PrimaryKind,
    /// Why the primary is disabled; `None` enables it.
    pub primary_disabled: Option<&'a str>,
    /// The engine's refusal of the last submit, in shown wording.
    pub refusal: Option<&'a str>,
    /// True on the frame the dialog opens: the body scrolls to the top.
    pub reset_scroll: bool,
    /// One dim line in the footer's left slot, beside the actions ("Remove
    /// the track to apply"); `None` for none.
    pub status: Option<&'a str>,
}

/// What the dialog's footer did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogAction {
    Cancel,
    Primary,
}

/// The body's height: `body_rows` control rows plus the body padding, but
/// never more than `room` (the slot height left after header and footer).
pub fn body_height(body_rows: f32, control_height: f32, padding: f32, room: f32) -> f32 {
    let rows = if body_rows.is_finite() {
        body_rows.clamp(1.0, MAX_BODY_ROWS)
    } else {
        MAX_BODY_ROWS
    };
    (rows * control_height + padding).min(room.max(0.0))
}

/// Whether a refusal is scrolled into view this frame: once per refusal,
/// so the user may scroll away from it (upstream `reveal_refusal`).
pub fn reveal_refusal(shown: Option<&str>, refusal: &str) -> bool {
    shown != Some(refusal)
}

impl<'a> DialogPanel<'a> {
    pub fn new(id: egui::Id, icon: &'a str, title: &'a str, primary: &'a str) -> Self {
        Self {
            id,
            icon,
            title,
            kind: "",
            body_rows: 12.0,
            primary,
            primary_kind: PrimaryKind::Normal,
            primary_disabled: None,
            refusal: None,
            reset_scroll: false,
            status: None,
        }
    }

    /// Draw the dialog in the slot `ui` offers: header, `body` in its
    /// bounded scroll with the refusal as its last row, then the footer
    /// pinned to the slot's bottom. The header's close is Cancel.
    pub fn show(&self, ui: &mut Ui, body: impl FnOnce(&mut Ui)) -> Option<DialogAction> {
        let t = Tokens::get();
        // Keys belong to a field focused when the last frame ended (Esc
        // reverts it, Enter commits it) before the dialog. egui drops focus
        // on Esc before this frame runs, so the last frame's state decides.
        let typing_id = self.id.with("typing");
        let typing = ui.data(|d| d.get_temp::<bool>(typing_id)).unwrap_or(false);
        let slot = ui.available_rect_before_wrap();
        let closed = header(ui, self.icon, self.title, self.kind);
        let padding = t.space_8 * 2.0;
        let room = if slot.bottom().is_finite() {
            slot.bottom() - ui.cursor().top() - FOOTER_HEIGHT
        } else {
            f32::INFINITY
        };
        let height = body_height(self.body_rows, t.control_height, padding, room);
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt(self.id.with("body"))
            .min_scrolled_height(0.0)
            .max_height(height)
            .auto_shrink([false, true]);
        if self.reset_scroll {
            scroll = scroll.vertical_scroll_offset(0.0);
        }
        let shown_id = self.id.with("refusal_shown");
        scroll.show(ui, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(t.space_12 as i8, t.space_8 as i8))
                .show(ui, |ui| {
                    super::form(ui, |ui| {
                        body(ui);
                        match self.refusal {
                            Some(refusal) => {
                                let row = super::refusal(ui, refusal);
                                let shown = ui.data(|d| d.get_temp::<String>(shown_id));
                                if reveal_refusal(shown.as_deref(), refusal) {
                                    ui.scroll_to_rect(row.rect, None);
                                    ui.data_mut(|d| d.insert_temp(shown_id, refusal.to_owned()));
                                }
                            }
                            None => ui.data_mut(|d| d.remove::<String>(shown_id)),
                        }
                    });
                });
        });
        // Pin the footer to the slot's bottom when the slot is bounded.
        if slot.bottom().is_finite() {
            let gap = slot.bottom() - FOOTER_HEIGHT - ui.cursor().top();
            if gap > 0.0 {
                ui.add_space(gap);
            }
        }
        let mut action = footer(
            ui,
            self.primary,
            self.primary_kind,
            self.primary_disabled,
            self.status,
        );
        if action.is_none() && closed {
            action = Some(DialogAction::Cancel);
        }
        if action.is_none() && !typing && ui.is_enabled() {
            action = keyboard(ui.ctx(), self.primary_disabled.is_none());
        }
        let focused = ui.ctx().memory(|m| m.focused().is_some());
        ui.data_mut(|d| d.insert_temp(typing_id, focused));
        action
    }
}

/// Esc cancels and Enter gives an enabled primary, consuming the key
/// (upstream `actions::keyboard`).
pub(crate) fn keyboard(ctx: &egui::Context, primary: bool) -> Option<DialogAction> {
    ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
            Some(DialogAction::Cancel)
        } else if primary && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
            Some(DialogAction::Primary)
        } else {
            None
        }
    })
}

/// The 40 pt dialog header: icon, title cut to one line, kind chip and a
/// close button. Returns whether close was clicked.
fn header(ui: &mut Ui, icon: &str, title: &str, kind: &str) -> bool {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), DIALOG_HEADER_HEIGHT),
        Sense::hover(),
    );
    let close_rect = Rect::from_center_size(
        pos2(
            (rect.right() - t.space_8 - t.form_close_size / 2.0).max(rect.left()),
            rect.center().y,
        ),
        vec2(t.form_close_size, t.form_close_size),
    );
    let close = ui
        .interact(close_rect, ui.scope_id().with("dialog_close"), Sense::click())
        .on_hover_text("Close")
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    close.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close"));
    if close.hovered() {
        ui.painter().rect_filled(close_rect, t.radius_sm, t.hover);
    }
    let h = t.form_close_size * 0.18;
    let c = close_rect.center();
    let stroke = egui::Stroke::new(1.4, t.form_dim);
    ui.painter()
        .line_segment([c + vec2(-h, -h), c + vec2(h, h)], stroke);
    ui.painter()
        .line_segment([c + vec2(-h, h), c + vec2(h, -h)], stroke);
    let icon_center = pos2(
        rect.left() + t.space_12 + t.form_icon_size / 2.0,
        rect.center().y,
    );
    super::icon(ui.painter(), icon_center, t.form_icon_size, icon, 1.0);
    let mut name_right = close_rect.left() - t.space_8;
    if !kind.is_empty() {
        let galley = ui.painter().layout_no_wrap(
            kind.to_owned(),
            fonts::semibold(t.font_small),
            t.selected_ink,
        );
        let chip = Rect::from_center_size(
            pos2(
                name_right - (galley.size().x + t.space_16) / 2.0,
                rect.center().y,
            ),
            vec2(galley.size().x + t.space_16, t.space_16 + t.space_4),
        );
        ui.painter().rect_filled(chip, t.radius_xl, t.selected_bg);
        ui.painter()
            .galley(chip.center() - galley.size() / 2.0, galley, t.selected_ink);
        name_right = chip.left() - t.space_8;
    }
    let left = icon_center.x + t.form_icon_size / 2.0 + t.space_8;
    let title_rect = Rect::from_min_max(
        pos2(left, rect.top()),
        pos2(name_right.max(left), rect.bottom()),
    );
    super::text::line(
        ui,
        ui.scope_id().with("dialog_title"),
        title_rect,
        title,
        "",
        fonts::semibold(t.font_heading),
        t.form_ink,
    );
    t.secondary_hline(ui.painter(), rect.x_range(), rect.bottom(), t.dialog_rule);
    close.clicked()
}

/// The pinned 48 pt footer band: fill and top rule, then the primary at the
/// right, Cancel before it and `status` in the left slot. A disabled
/// primary says why on hover (upstream `footer_with_left`).
pub(super) fn footer(
    ui: &mut Ui,
    primary: &str,
    kind: PrimaryKind,
    disabled: Option<&str>,
    status: Option<&str>,
) -> Option<DialogAction> {
    let t = Tokens::get();
    let (band, _) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), FOOTER_HEIGHT),
        Sense::hover(),
    );
    ui.painter().rect_filled(band, 0.0, t.dialog_footer);
    t.secondary_hline(ui.painter(), band.x_range(), band.top(), t.dialog_rule);
    let inner = band.shrink2(vec2(t.space_12, t.space_10));
    let mut action = None;
    let mut actions_left = inner.right();
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("dialog_footer")
            .max_rect(inner)
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = t.space_6;
            let button = primary_button(ui, primary, kind, disabled);
            if button.clicked() {
                action = Some(DialogAction::Primary);
            }
            actions_left = button.rect.left();
            let cancel = ui_kit::button(ui, "Cancel", ui_kit::Button::Enabled(true));
            if cancel.clicked() {
                action = Some(DialogAction::Cancel);
            }
            actions_left = actions_left.min(cancel.rect.left());
        },
    );
    if let Some(status) = status.filter(|s| !s.is_empty()) {
        let rect = Rect::from_min_max(
            inner.min,
            pos2((actions_left - t.space_8).max(inner.left()), inner.bottom()),
        );
        super::text::line(
            ui,
            ui.scope_id().with("dialog_status"),
            rect,
            status,
            "",
            FontId::proportional(t.font_small),
            t.form_dim,
        );
    }
    action
}

/// The primary: accent fill, or the error fill for a destructive choice
/// (upstream `ActionLook` with `danger`). Without an action to take it is
/// a plain disabled button whose hover gives `disabled`.
fn primary_button(ui: &mut Ui, label: &str, kind: PrimaryKind, disabled: Option<&str>) -> Response {
    let t = Tokens::get();
    match (disabled, kind) {
        (Some(reason), _) => super::primary(ui, label, false).on_disabled_hover_text(reason),
        (None, PrimaryKind::Normal) => super::primary(ui, label, true),
        (None, PrimaryKind::Destructive) => {
            let response = ui.add(
                egui::Button::new(
                    RichText::new(label)
                        .size(t.font_label)
                        .color(t.on_accent)
                        .strong(),
                )
                .fill(t.form_error)
                .stroke(egui::Stroke::NONE)
                .corner_radius(t.control_radius)
                .min_size(vec2(0.0, t.form_button_height)),
            );
            response.on_hover_cursor(egui::CursorIcon::PointingHand)
        }
    }
}

/// A destructive confirmation window: the danger disc, `text` and a footer
/// with Cancel and the destructive `proceed`. Returns `Some(true)` to
/// proceed, `Some(false)` to cancel (Cancel, close or Esc), `None` while it
/// stays open. Enter never proceeds.
pub fn confirm(
    ctx: &egui::Context,
    id: egui::Id,
    title: &str,
    text: &str,
    proceed: &str,
) -> Option<bool> {
    let mut answer = None;
    let closed = ui_kit::Window::new(id, "constraint", title)
        .show_body(ctx, |ui| answer = confirm_body(ui, text, proceed));
    if closed {
        answer = Some(false);
    }
    answer
}

/// The confirmation's content: disc and message, then the actions.
fn confirm_body(ui: &mut Ui, text: &str, proceed: &str) -> Option<bool> {
    let t = Tokens::get();
    let mut answer = None;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = t.space_12;
        let diameter = t.control_height + t.space_8;
        let (disc, _) = ui.allocate_exact_size(vec2(diameter, diameter), Sense::hover());
        ui.painter()
            .circle_filled(disc.center(), diameter / 2.0, t.form_error_fill);
        severity_glyph(ui.painter(), disc.center(), diameter * 0.6, Level::Error);
        ui.add(
            egui::Label::new(RichText::new(text).size(t.font_label).color(t.form_ink))
                .wrap()
                .selectable(false),
        );
    });
    super::actions(ui, |ui| {
        if primary_button(ui, proceed, PrimaryKind::Destructive, None).clicked() {
            answer = Some(true);
        }
        if ui_kit::button(ui, "Cancel", ui_kit::Button::Enabled(true)).clicked() {
            answer = Some(false);
        }
    });
    answer
}

#[cfg(test)]
#[path = "dialog_tests.rs"]
mod tests;
