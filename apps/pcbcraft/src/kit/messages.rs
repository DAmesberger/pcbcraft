//! The Messages window: the history of every toast and refusal with the
//! full engine text, newest first, repeats folded ×n.
//!
//! Slice K3 (docs/sidebar-ia.md, "Feedback"): it opens from File ›
//! Messages… and from a toast. Adapted from Solidcraft 4f3ac07a
//! `crates/ui-egui/src/messages.rs` (`KEEP`, `MAX_CHARS`, the window and its
//! Copy all / Clear), MIT option; see ATTRIBUTION.md.

use super::severity::{Level, severity_glyph};
use egui::{RichText, vec2};
use pcbcraft_ui::theme::Tokens;
use pcbcraft_ui::ui_kit;
use std::collections::VecDeque;

/// Entries kept; older ones are dropped.
pub const KEEP: usize = 200;
/// Characters kept of one entry's text.
pub const MAX_CHARS: usize = 4000;

/// One history entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub level: Level,
    /// The full text, including the raw engine wording, at most
    /// [`MAX_CHARS`] characters.
    pub text: String,
    pub count: usize,
    /// App time of the latest repeat, in seconds.
    pub at: f64,
}

/// What the window's buttons did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MessagesResponse {
    pub closed: bool,
    pub copy_all: bool,
    pub cleared: bool,
}

/// The message history, newest first.
#[derive(Default)]
pub struct Messages {
    entries: VecDeque<Message>,
}

impl Messages {
    /// Record `text`; a repeat of the newest entry counts ×n. A text longer
    /// than [`MAX_CHARS`] is cut, ending in "…".
    pub fn push(&mut self, level: Level, text: &str, now: f64) {
        let text: String = if text.chars().count() > MAX_CHARS {
            let mut cut: String = text.chars().take(MAX_CHARS - 1).collect();
            cut.push('…');
            cut
        } else {
            text.to_owned()
        };
        if let Some(newest) = self.entries.front_mut()
            && newest.level == level
            && newest.text == text
        {
            newest.count = newest.count.saturating_add(1);
            newest.at = now;
            return;
        }
        self.entries.push_front(Message {
            level,
            text,
            count: 1,
            at: now,
        });
        self.entries.truncate(KEEP);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[allow(dead_code, reason = "kit contract (docs/sidebar-ia.md); no caller yet")]
    pub fn entries(&self) -> impl Iterator<Item = &Message> {
        self.entries.iter()
    }

    #[allow(dead_code, reason = "kit contract (docs/sidebar-ia.md); no caller yet")]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[allow(dead_code, reason = "kit contract (docs/sidebar-ia.md); no caller yet")]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry as plain text, newest first, for Copy all: errors and
    /// warnings say so, and repeats show ×n.
    pub fn copy_all(&self) -> String {
        self.entries.iter().map(line).collect::<Vec<_>>().join("\n")
    }

    /// The window, while `open`. The body is the window's one bounded
    /// scroll.
    pub fn window(&mut self, ctx: &egui::Context) -> MessagesResponse {
        let t = Tokens::get();
        let now = ctx.input(|i| i.time);
        let mut out = MessagesResponse::default();
        let any = !self.entries.is_empty();
        out.closed = ui_kit::Window::new(egui::Id::unique("messages"), "list", "Messages")
            .show_body(ctx, |ui| {
                super::actions(ui, |ui| {
                    out.copy_all =
                        ui_kit::button(ui, "Copy all", ui_kit::Button::Enabled(any)).clicked();
                    out.cleared =
                        ui_kit::button(ui, "Clear", ui_kit::Button::Enabled(any)).clicked();
                });
                if !any {
                    super::status(ui, "No messages yet", super::Tone::Dim);
                    return;
                }
                for message in &self.entries {
                    ui.horizontal_top(|ui| {
                        let (glyph, _) = ui.allocate_exact_size(
                            vec2(t.font_label, t.font_label),
                            egui::Sense::hover(),
                        );
                        severity_glyph(ui.painter(), glyph.center(), t.font_label, message.level);
                        ui.add_sized(
                            vec2(t.space_24 * 2.5, t.font_label),
                            egui::Label::new(
                                RichText::new(ago(now, message.at))
                                    .size(t.font_small)
                                    .color(t.text_dim),
                            ),
                        );
                        let text = if message.count > 1 {
                            format!("{} ×{}", message.text, message.count)
                        } else {
                            message.text.clone()
                        };
                        ui.add(
                            egui::Label::new(RichText::new(text).size(t.font_label).color(t.text))
                                .wrap()
                                .selectable(true),
                        );
                    });
                }
            });
        if out.cleared {
            self.clear();
        }
        if out.copy_all {
            ctx.copy_text(self.copy_all());
        }
        // The ages tick on while it is open.
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        out
    }
}

/// One entry as a line of Copy all.
fn line(message: &Message) -> String {
    let prefix = match message.level {
        Level::Error => "error: ",
        Level::Warning => "warning: ",
        _ => "",
    };
    let repeats = if message.count > 1 {
        format!(" (×{})", message.count)
    } else {
        String::new()
    };
    format!("{prefix}{}{repeats}", message.text)
}

/// How long ago `at` was, at `now` (seconds of app time).
fn ago(now: f64, at: f64) -> String {
    let s = (now - at).max(0.0).floor();
    if s < 60.0 {
        format!("{s:.0} s ago")
    } else if s < 3600.0 {
        format!("{:.0} min ago", (s / 60.0).floor())
    } else {
        format!("{:.0} h ago", (s / 3600.0).floor())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_fold_newest_first_and_the_oldest_go_past_the_limit() {
        let mut m = Messages::default();
        m.push(Level::Ok, "Saved", 1.0);
        m.push(Level::Ok, "Saved", 2.0);
        m.push(Level::Error, "Track 68 is locked", 3.0);
        let got: Vec<_> = m
            .entries()
            .map(|e| (e.text.as_str(), e.level, e.count))
            .collect();
        assert_eq!(
            got,
            [
                ("Track 68 is locked", Level::Error, 1),
                ("Saved", Level::Ok, 2)
            ]
        );
        assert_eq!(m.copy_all(), "error: Track 68 is locked\nSaved (×2)");
        for i in 0..KEEP + 5 {
            m.push(Level::Info, &format!("message {i}"), 4.0);
        }
        assert_eq!(m.len(), KEEP);
        assert_eq!(
            m.entries().last().map(|e| e.text.as_str()),
            Some("message 5"),
            "the oldest went first"
        );
        m.clear();
        assert!(m.is_empty());
    }

    #[test]
    fn a_very_long_message_is_cut_and_says_so() {
        let mut m = Messages::default();
        m.push(Level::Error, &"x".repeat(10_000), 0.0);
        let text = m
            .entries()
            .next()
            .map(|e| e.text.clone())
            .unwrap_or_default();
        assert_eq!(text.chars().count(), MAX_CHARS);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn ages_read_in_seconds_minutes_and_hours() {
        assert_eq!(ago(10.0, 4.0), "6 s ago");
        assert_eq!(ago(200.0, 0.0), "3 min ago");
        assert_eq!(ago(7300.0, 0.0), "2 h ago");
        assert_eq!(ago(0.0, 5.0), "0 s ago");
    }

    /// The window draws headless, and Clear empties the history.
    #[test]
    fn the_window_draws() {
        let ctx = egui::Context::default();
        pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
        let mut m = Messages::default();
        m.push(
            Level::Warning,
            "The project changed meanwhile; try again",
            0.0,
        );
        let mut out = MessagesResponse::default();
        ctx.run_ui(Default::default(), |ui| out = m.window(ui.ctx()))
            .textures_delta
            .clear();
        assert_eq!(out, MessagesResponse::default());
        assert_eq!(m.len(), 1);
    }
}
