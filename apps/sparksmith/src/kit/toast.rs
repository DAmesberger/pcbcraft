//! Transient feedback: the toast stack for command results and gesture
//! refusals, the viewport pills for persistent modes and the status chip
//! for background work.
//!
//! Slice K3 (docs/sidebar-ia.md, "Feedback"): toasts replace the canvas
//! status chip. Adapted from Solidcraft 4f3ac07a
//! `crates/ui-egui/src/viewport.rs` (`transient_messages`,
//! `paint_toast_with`, `STATUS_TOAST_MS`) and `ui_kit/viewport_status.rs`
//! (`viewport_status_row`), MIT option; see ATTRIBUTION.md.

use super::severity::{Level, severity_glyph};
use egui::{Rect, RichText, Ui, vec2};
use sparksmith_ui::frame::Side;
use sparksmith_ui::theme::Tokens;
use sparksmith_ui::ui_kit::type_face;
use sparksmith_ui::ui_kit::{self, Button, Container, Surface};
use std::collections::VecDeque;

/// How long a toast stays, in seconds (upstream `STATUS_TOAST_MS = 6000`).
pub const TOAST_SECONDS: f64 = 6.0;
/// The most toasts shown at once: a new toast replaces the shown one,
/// which stays in Messages, so quick actions don't pile up on the canvas.
pub const MAX_TOASTS: usize = 1;
/// How long a warning or error toast holds its place against a less
/// severe toast, in seconds, so it can be read; the newer toast then goes
/// to Messages only.
pub const HOLD_SECONDS: f64 = 3.0;
/// The most text rows of one toast.
pub const MAX_TOAST_ROWS: usize = 3;
/// The widest a toast grows, as upstream's centred toasts (440 pt).
const MAX_TOAST_WIDTH: f32 = 440.0;
/// The most toasts an editor can post in one frame.
#[allow(dead_code, reason = "kit contract (docs/sidebar-ia.md); no caller yet")]
const MAX_POSTED: usize = 16;

/// An action a toast offers.
#[allow(dead_code, reason = "kit contract (docs/sidebar-ia.md); no caller yet")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToastAction {
    Undo,
    /// Locate what the message names, or open the list it summarises.
    Show,
    /// Open Messages at this entry.
    Details,
}

impl ToastAction {
    fn label(self) -> &'static str {
        match self {
            Self::Undo => "Undo",
            Self::Show => "Show",
            Self::Details => "Details",
        }
    }
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
    /// Show `text` at `now`, replacing the shown toast; a repeat of the
    /// newest toast counts ×n. A warning or error younger than
    /// [`HOLD_SECONDS`] keeps its place against a less severe toast.
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
            newest.actions = actions;
            return;
        }
        // `Level` orders most severe first.
        if self.items.back().is_some_and(|newest| {
            newest.level <= Level::Warning && level > newest.level && now - newest.at < HOLD_SECONDS
        }) {
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

    /// Drop the newest toast when it says `text`, so the one that follows
    /// replaces it (a held ⌘Z's undos).
    pub fn remove_newest(&mut self, text: &str) {
        if self.items.back().is_some_and(|toast| toast.text == text) {
            self.items.pop_back();
        }
    }

    /// Drop toasts older than [`TOAST_SECONDS`].
    pub fn expire(&mut self, now: f64) {
        self.items.retain(|toast| now - toast.at < TOAST_SECONDS);
    }

    #[allow(dead_code, reason = "kit contract (docs/sidebar-ia.md); no caller yet")]
    pub fn items(&self) -> impl Iterator<Item = &Toast> {
        self.items.iter()
    }

    /// The toast at `index` (oldest first), as a [`ToastEvent`] names it.
    #[allow(dead_code, reason = "kit contract (docs/sidebar-ia.md); no caller yet")]
    pub fn get(&self, index: usize) -> Option<&Toast> {
        self.items.get(index)
    }

    #[allow(dead_code, reason = "kit contract (docs/sidebar-ia.md); no caller yet")]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Take `action` off every toast, such as Undo once the step it would
    /// undo is no longer the last one.
    pub fn withdraw(&mut self, action: ToastAction) {
        for toast in &mut self.items {
            toast.actions.retain(|a| *a != action);
        }
    }

    /// Dismiss error toasts: a later successful action or a workspace
    /// change makes them stale. Messages keeps them.
    pub fn dismiss_errors(&mut self) {
        self.items.retain(|toast| toast.level != Level::Error);
    }

    /// Draw the stack bottom-centre of `canvas` (clear of the bottom-left
    /// hint line), oldest on top.
    pub fn show(&mut self, ctx: &egui::Context, canvas: Rect, now: f64) -> Option<ToastEvent> {
        self.expire(now);
        let first_due = self
            .items
            .iter()
            .map(|toast| TOAST_SECONDS - (now - toast.at))
            .fold(f64::INFINITY, f64::min);
        if first_due.is_finite() {
            // One frame when the next is due to go, not a frame per tick.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                first_due.clamp(0.0, TOAST_SECONDS) + 0.01,
            ));
        }
        let t = Tokens::get();
        let width = MAX_TOAST_WIDTH.min(canvas.width() - 2.0 * t.space_8);
        if self.items.is_empty() || !canvas.is_finite() || width < t.form_card_width * 0.5 {
            return None;
        }
        let at = egui::pos2(canvas.center().x, canvas.bottom() - t.space_8);
        let host = Container::new(ctx, Surface::Toast);
        let frame = host
            .frame(ctx, t.panel, Side::Left)
            .inner_margin(toast_margin());
        let mut event = None;
        host.area(egui::Id::unique("toasts"))
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(at)
            .constrain_to(canvas)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = t.space_8;
                for (index, toast) in self.items.iter().enumerate() {
                    let shown = frame.show(ui, |ui| {
                        ui.set_max_width(width - frame.total_margin().sum().x);
                        toast_row(ui, index, toast)
                    });
                    if let Some(e) = shown.inner {
                        event = Some(e);
                    }
                }
            });
        event
    }
}

/// The room between a toast's box and its text.
fn toast_margin() -> egui::Margin {
    let t = Tokens::get();
    egui::Margin::symmetric(t.space_10 as i8, t.space_6 as i8)
}

/// The text a toast shows: its message and a repeat count.
pub fn toast_text(toast: &Toast) -> String {
    if toast.count > 1 {
        format!("{} ×{}", toast.text, toast.count)
    } else {
        toast.text.clone()
    }
}

fn toast_row(ui: &mut Ui, index: usize, toast: &Toast) -> Option<ToastEvent> {
    let t = Tokens::get();
    let mut event = None;
    ui.horizontal_top(|ui| {
        let (glyph, _) =
            ui.allocate_exact_size(vec2(t.font_label, t.font_label), egui::Sense::hover());
        severity_glyph(ui.painter(), glyph.center(), t.font_label, toast.level);
        let text = toast_text(toast);
        let mut job = egui::text::LayoutJob::simple(
            text.clone(),
            type_face::medium(t.font_label),
            t.text,
            ui.available_width().max(0.0),
        );
        job.wrap.max_rows = MAX_TOAST_ROWS;
        let body = ui
            .add(egui::Label::new(job).sense(egui::Sense::click()))
            .on_hover_text(format!("{text}\nClick for Messages"));
        if body.clicked() {
            event = Some(ToastEvent::OpenMessages);
        }
    });
    if !toast.actions.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(t.font_label + ui.spacing().item_spacing.x);
            for action in &toast.actions {
                if ui_kit::button(ui, action.label(), Button::Small).clicked() {
                    event = Some(ToastEvent::Action {
                        toast: index,
                        action: *action,
                    });
                }
            }
        });
    }
    event
}

/// A toast an editor posted without access to the app: a command result or
/// a gesture refusal it found itself. The app drains them each frame into
/// the toast stack and Messages.
#[derive(Clone, Debug, PartialEq)]
pub struct Posted {
    pub level: Level,
    pub text: String,
    pub actions: Vec<ToastAction>,
}

fn posted_id() -> egui::Id {
    egui::Id::unique("sparksmith-posted-toasts")
}

/// Post a toast from an editor ("Assigned 2 pads", Undo). It shows from
/// this frame and goes to Messages; at most a few per frame are kept.
#[allow(dead_code, reason = "kit contract (docs/sidebar-ia.md); no caller yet")]
pub fn post(ctx: &egui::Context, level: Level, text: impl Into<String>, actions: Vec<ToastAction>) {
    let text = text.into();
    ctx.data_mut(|d| {
        let posted = d.get_temp_mut_or_default::<Vec<Posted>>(posted_id());
        if posted.len() < MAX_POSTED {
            posted.push(Posted {
                level,
                text,
                actions,
            });
        }
    });
    ctx.request_repaint();
}

/// The toasts posted since the last call.
pub(crate) fn take_posted(ctx: &egui::Context) -> Vec<Posted> {
    ctx.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Vec<Posted>>(posted_id())))
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
    let ctx = ui.ctx().clone();
    let frame = Container::new(&ctx, Surface::Toast)
        .frame(&ctx, t.toolbar, Side::Left)
        .corner_radius(t.radius_lg)
        .inner_margin(egui::Margin::symmetric(t.space_8 as i8, t.space_2 as i8));
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(level) = level {
                let (glyph, _) =
                    ui.allocate_exact_size(vec2(t.font_label, t.font_label), egui::Sense::hover());
                severity_glyph(ui.painter(), glyph.center(), t.font_small, level);
            }
            // The text truncates into what is left after the ×, so a narrow
            // pill keeps its ×.
            let close = t.control_height + ui.spacing().item_spacing.x;
            let room = (ui.available_width() - close).max(0.0);
            out.clicked = ui
                .allocate_ui(vec2(room, t.control_height), |ui| {
                    ui.set_max_width(room);
                    ui.add(
                        egui::Button::new(RichText::new(text).size(t.font_label).color(t.text))
                            .frame_when_inactive(false)
                            .truncate(),
                    )
                })
                .inner
                .on_hover_text(text)
                .clicked();
            out.closed = ui_kit::button(ui, "×", Button::Small)
                .on_hover_text("End")
                .clicked();
        });
    });
    out
}

/// A small viewport chip for background work the view shows the result
/// of: a spinner while it runs (`level` `None`), else the severity glyph.
/// It names the work; it has no action, and the view stays usable.
pub fn status_chip(ui: &mut Ui, level: Option<Level>, text: &str) {
    let t = Tokens::get();
    let ctx = ui.ctx().clone();
    let frame = Container::new(&ctx, Surface::Toast)
        .frame(&ctx, t.toolbar, Side::Left)
        .corner_radius(t.radius_lg)
        .inner_margin(egui::Margin::symmetric(t.space_8 as i8, t.space_2 as i8));
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            match level {
                Some(level) => {
                    let (glyph, _) = ui.allocate_exact_size(
                        vec2(t.font_label, t.font_label),
                        egui::Sense::hover(),
                    );
                    severity_glyph(ui.painter(), glyph.center(), t.font_small, level);
                }
                None => {
                    ui.add(egui::Spinner::new().size(t.font_small));
                }
            }
            ui.add(
                egui::Label::new(RichText::new(text).size(t.font_small).color(t.text_dim))
                    .truncate(),
            )
            .on_hover_text(text);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_count_up_and_a_new_toast_replaces_the_shown_one() {
        let mut toasts = Toasts::default();
        toasts.push(Level::Error, "Locked", vec![], 1.0);
        toasts.push(Level::Error, "Locked", vec![ToastAction::Details], 2.0);
        assert_eq!(toasts.items().count(), 1);
        let first = toasts.get(0).cloned();
        assert_eq!(first.as_ref().map(|t| t.count), Some(2));
        assert_eq!(
            first.as_ref().map(|t| t.at),
            Some(2.0),
            "a repeat restarts the clock"
        );
        assert_eq!(first.as_ref().map(toast_text).as_deref(), Some("Locked ×2"));
        toasts.push(Level::Error, "Out of bounds", vec![], 2.5);
        let texts: Vec<_> = toasts.items().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["Out of bounds"], "another text replaces it");
        for i in 0..5 {
            let at = 6.0 + f64::from(i);
            toasts.push(
                Level::Ok,
                format!("Routed {i}"),
                vec![ToastAction::Undo],
                at,
            );
        }
        let shown: Vec<_> = toasts.items().cloned().collect();
        assert_eq!(shown.len(), 1, "one toast at a time");
        assert_eq!(shown.first().map(|t| t.text.as_str()), Some("Routed 4"));
        assert_eq!(
            shown.first().map(|t| t.actions.clone()),
            Some(vec![ToastAction::Undo]),
            "the newest keeps its Undo"
        );
    }

    /// A warning or error stays readable for [`HOLD_SECONDS`] against
    /// less severe toasts; one as severe or more replaces it at once.
    #[test]
    fn a_fresh_error_holds_against_info() {
        let texts =
            |toasts: &Toasts| -> Vec<String> { toasts.items().map(|t| t.text.clone()).collect() };
        let mut toasts = Toasts::default();
        toasts.push(Level::Error, "Locked", vec![], 10.0);
        toasts.push(Level::Ok, "Routed", vec![ToastAction::Undo], 10.5);
        toasts.push(Level::Warning, "Close to R3", vec![], 11.0);
        assert_eq!(texts(&toasts), ["Locked"]);
        toasts.push(Level::Ok, "Routed", vec![], 10.0 + HOLD_SECONDS);
        assert_eq!(texts(&toasts), ["Routed"], "read by now");
        toasts.push(Level::Warning, "Close to R3", vec![], 14.0);
        toasts.push(Level::Error, "Crossing N2", vec![], 14.5);
        assert_eq!(texts(&toasts), ["Crossing N2"], "as severe or more");
    }

    #[test]
    fn toasts_expire_and_undo_and_errors_can_be_withdrawn() {
        let mut toasts = Toasts::default();
        toasts.push(Level::Error, "Locked", vec![ToastAction::Details], 0.0);
        toasts.dismiss_errors();
        assert!(toasts.is_empty());
        let actions = vec![ToastAction::Undo, ToastAction::Show];
        toasts.push(Level::Ok, "Placed R1", actions, 0.0);
        toasts.withdraw(ToastAction::Undo);
        assert_eq!(
            toasts.get(0).map(|t| t.actions.clone()),
            Some(vec![ToastAction::Show])
        );
        toasts.dismiss_errors();
        assert_eq!(toasts.items().count(), 1);
        toasts.expire(TOAST_SECONDS - 0.1);
        assert!(!toasts.is_empty());
        toasts.expire(TOAST_SECONDS);
        assert!(toasts.is_empty());
    }

    #[test]
    fn editors_post_toasts_through_the_context() {
        let ctx = egui::Context::default();
        post(&ctx, Level::Ok, "Assigned 2 pads", vec![ToastAction::Undo]);
        for _ in 0..40 {
            post(&ctx, Level::Info, "again", vec![]);
        }
        let posted = take_posted(&ctx);
        assert_eq!(posted.len(), MAX_POSTED, "bounded per frame");
        assert_eq!(
            posted.first().map(|p| p.text.as_str()),
            Some("Assigned 2 pads")
        );
        assert!(take_posted(&ctx).is_empty(), "drained once");
    }

    /// The stack draws bottom-centre of the canvas, inside it, and a click
    /// on an action reports that toast and action.
    #[test]
    fn the_stack_draws_inside_the_canvas() {
        let ctx = egui::Context::default();
        sparksmith_ui::fonts::setup_bundled_fonts(&ctx);
        let mut toasts = Toasts::default();
        toasts.push(
            Level::Ok,
            "Placed R1 from the library",
            vec![ToastAction::Undo],
            0.0,
        );
        let canvas = Rect::from_min_size(egui::pos2(100.0, 50.0), vec2(600.0, 500.0));
        let mut drawn = None;
        for _ in 0..3 {
            ctx.run_ui(Default::default(), |ui| {
                toasts.show(ui.ctx(), canvas, 1.0);
                drawn = ui.ctx().memory(|m| m.area_rect(egui::Id::unique("toasts")));
            })
            .textures_delta
            .clear();
        }
        let drawn = drawn.unwrap_or(Rect::NOTHING);
        assert!(drawn.is_positive(), "{drawn:?}");
        assert!(canvas.contains_rect(drawn), "{drawn:?} in {canvas:?}");
        assert!((drawn.center().x - canvas.center().x).abs() < 1.0);
        assert!(drawn.bottom() > canvas.center().y, "bottom half");
    }
}
