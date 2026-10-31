//! Transient feedback: the toast stack for command results and gesture
//! refusals, and the viewport pills for persistent modes.
//!
//! Contract from slice S0 (docs/sidebar-ia.md, "Toasts, Messages, pills");
//! slice K3 owns the final look and replaces the canvas status chip with it.
//! Adapted from Solidcraft 4f3ac07a `crates/ui-egui/src/viewport.rs`
//! (`transient_messages`, `paint_toast_with`, `STATUS_TOAST_MS`) and
//! `ui_kit/viewport_status.rs` (`viewport_status_row`), MIT option; see
//! ATTRIBUTION.md.
#![allow(dead_code, reason = "S0 contract; slice K3 uses it")]

use super::severity::{Level, severity_glyph};
use egui::{Rect, RichText, Ui, vec2};
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::ui_kit::{Container, Surface};
use std::collections::VecDeque;

/// How long a toast stays, in seconds (upstream `STATUS_TOAST_MS = 6000`).
pub const TOAST_SECONDS: f64 = 6.0;
/// The most toasts shown at once; older ones go to Messages only.
pub const MAX_TOASTS: usize = 3;
/// The most text rows of one toast.
pub const MAX_TOAST_ROWS: usize = 3;

/// An action a toast offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToastAction {
    Undo,
    /// Locate what the message names, or open the list it summarises.
    Show,
    /// Open Messages at this entry.
    Details,
}

/// One toast. A repeat of the newest toast counts up instead of stacking.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub level: Level,
    pub text: String,
    pub actions: Vec<ToastAction>,
    /// When it was shown or last repeated, in seconds of app time.
    pub at: f64,
    pub count: usize,
}

/// What a toast's click did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastEvent {
    Action {
        toast: usize,
        action: ToastAction,
    },
    /// The toast body: open Messages.
    OpenMessages,
}

/// The toast stack.
#[derive(Default)]
pub struct Toasts {
    items: VecDeque<Toast>,
}

impl Toasts {
    /// Show `text` at `now`; a repeat of the newest toast counts ×n.
    pub fn push(
        &mut self,
        level: Level,
        text: impl Into<String>,
        actions: Vec<ToastAction>,
        now: f64,
    ) {
        let text = text.into();
        if let Some(newest) = self.items.back_mut()
            && newest.level == level
            && newest.text == text
        {
            newest.count = newest.count.saturating_add(1);
            newest.at = now;
            return;
        }
        self.items.push_back(Toast {
            level,
            text,
            actions,
            at: now,
            count: 1,
        });
        while self.items.len() > MAX_TOASTS {
            self.items.pop_front();
        }
    }

    /// Drop toasts older than [`TOAST_SECONDS`].
    pub fn expire(&mut self, now: f64) {
        self.items.retain(|toast| now - toast.at < TOAST_SECONDS);
    }

    pub fn items(&self) -> impl Iterator<Item = &Toast> {
        self.items.iter()
    }

    /// Draw the stack bottom-centre of `canvas` (clear of the bottom-left
    /// hint line).
    pub fn show(&mut self, ctx: &egui::Context, canvas: Rect, now: f64) -> Option<ToastEvent> {
        self.expire(now);
        if self.items.is_empty() {
            return None;
        }
        let t = Tokens::get();
        let width = t.form_card_width.min(canvas.width());
        let at = egui::pos2(
            canvas.center().x - width / 2.0,
            canvas.bottom() - t.space_24,
        );
        let host = Container::new(ctx, Surface::Toast);
        let mut event = None;
        host.area(egui::Id::new("toasts"))
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(at)
            .show(ctx, |ui| {
                ui.set_width(width);
                for (index, toast) in self.items.iter().enumerate() {
                    if let Some(e) = toast_row(ui, index, toast) {
                        event = Some(e);
                    }
                }
            });
        event
    }
}

fn toast_row(ui: &mut Ui, index: usize, toast: &Toast) -> Option<ToastEvent> {
    let t = Tokens::get();
    let mut event = None;
    ui.horizontal(|ui| {
        let (glyph, _) = ui.allocate_exact_size(
            vec2(t.control_height, t.control_height),
            egui::Sense::hover(),
        );
        severity_glyph(ui.painter(), glyph.center(), t.font_label, toast.level);
        let text = if toast.count > 1 {
            format!("{} ×{}", toast.text, toast.count)
        } else {
            toast.text.clone()
        };
        let body = ui.add(
            egui::Button::new(RichText::new(text).size(t.font_label).color(t.text))
                .frame_when_inactive(false)
                .wrap(),
        );
        if body.clicked() {
            event = Some(ToastEvent::OpenMessages);
        }
        for action in &toast.actions {
            let label = match action {
                ToastAction::Undo => "Undo",
                ToastAction::Show => "Show",
                ToastAction::Details => "Details",
            };
            if ui.small_button(label).clicked() {
                event = Some(ToastEvent::Action {
                    toast: index,
                    action: *action,
                });
            }
        }
    });
    event
}

/// What a pill's click did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PillResponse {
    pub clicked: bool,
    /// The × ends the mode.
    pub closed: bool,
}

/// A viewport pill for a persistent mode ("Routing N22 ×", "Showing:
/// Clearance R3 ↔ R6 ×").
pub fn pill(ui: &mut Ui, level: Option<Level>, text: &str) -> PillResponse {
    let t = Tokens::get();
    let mut out = PillResponse::default();
    ui.horizontal(|ui| {
        if let Some(level) = level {
            let (glyph, _) =
                ui.allocate_exact_size(vec2(t.font_label, t.font_label), egui::Sense::hover());
            severity_glyph(ui.painter(), glyph.center(), t.font_small, level);
        }
        out.clicked = ui
            .add(egui::Button::new(RichText::new(text).size(t.font_label)).truncate())
            .clicked();
        out.closed = ui.small_button("×").on_hover_text("End").clicked();
    });
    out
}
